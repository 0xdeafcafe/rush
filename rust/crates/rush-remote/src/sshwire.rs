//! SSH's transport, server side, for serve's SSH (sshd.rs): what Go gets
//! from golang.org/x/crypto/ssh's NewServerConn, which sshd.go's sshConn
//! (internal/remote/sshd.go:136) is built on. RFC 4253 with what today's
//! clients (OpenSSH, Termius, Blink) offer: curve25519-sha256 key
//! exchange (RFC 8731), an ed25519 host key (RFC 8709),
//! chacha20-poly1305@openssh.com and aes-gcm@openssh.com (RFC 5647),
//! strict key exchange (OpenSSH's kex-strict, against Terrapin), re-keying
//! when the client asks, and server-sig-algs (RFC 8308) so an RSA key signs
//! with SHA-2. Everything cryptographic is ring's.
//!
//! One thread reads (Transport); any thread sends through Out, which holds
//! back all but the key exchange's own messages while keys change.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey, chacha20_poly1305_openssh as cc};
use ring::agreement;
use ring::signature::{self, Ed25519KeyPair};
use sha2::{Digest, Sha256};

use crate::config::rand_bytes;
use crate::sshkey::{get_string, put_string};

pub const MSG_DISCONNECT: u8 = 1;
pub const MSG_IGNORE: u8 = 2;
pub const MSG_UNIMPLEMENTED: u8 = 3;
pub const MSG_DEBUG: u8 = 4;
pub const MSG_SERVICE_REQUEST: u8 = 5;
pub const MSG_SERVICE_ACCEPT: u8 = 6;
pub const MSG_EXT_INFO: u8 = 7;
pub const MSG_KEXINIT: u8 = 20;
pub const MSG_NEWKEYS: u8 = 21;
pub const MSG_KEX_ECDH_INIT: u8 = 30;
pub const MSG_KEX_ECDH_REPLY: u8 = 31;

/// What serve says it is.
pub const SERVER_VERSION: &str = "SSH-2.0-rush";

const KEX: &[&str] = &["curve25519-sha256", "curve25519-sha256@libssh.org"];
const HOST_KEY: &str = "ssh-ed25519";
const CIPHERS: &[&str] = &[
    "chacha20-poly1305@openssh.com",
    "aes256-gcm@openssh.com",
    "aes128-gcm@openssh.com",
];
/// Only for clients that insist on a MAC list in common: AEAD ciphers
/// carry their own, so whatever's picked here is never used.
const MACS: &[&str] = &["hmac-sha2-256-etm@openssh.com", "hmac-sha2-256"];
const STRICT_C: &str = "kex-strict-c-v00@openssh.com";
const STRICT_S: &str = "kex-strict-s-v00@openssh.com";

/// The client key types serve takes, as server-sig-algs lists them.
pub const SIG_ALGS: &str =
    "ssh-ed25519,ecdsa-sha2-nistp256,ecdsa-sha2-nistp384,rsa-sha2-256,rsa-sha2-512,ssh-rsa";

/// The largest packet read: OpenSSH's own limit.
const MAX_PACKET: usize = 256 * 1024;

/// A message being written.
#[derive(Default)]
pub struct Msg(pub Vec<u8>);

impl Msg {
    pub fn new(t: u8) -> Msg {
        Msg(vec![t])
    }
    pub fn byte(mut self, b: u8) -> Msg {
        self.0.push(b);
        self
    }
    pub fn bool(self, b: bool) -> Msg {
        self.byte(u8::from(b))
    }
    pub fn u32(mut self, n: u32) -> Msg {
        self.0.extend_from_slice(&n.to_be_bytes());
        self
    }
    pub fn bytes(mut self, b: &[u8]) -> Msg {
        put_string(&mut self.0, b);
        self
    }
    pub fn str(self, s: &str) -> Msg {
        self.bytes(s.as_bytes())
    }
}

/// b, an unsigned big-endian number, as an mpint.
fn put_mpint(out: &mut Vec<u8>, b: &[u8]) {
    let b = &b[b.iter().take_while(|x| **x == 0).count()..];
    if b.first().is_some_and(|x| x & 0x80 != 0) {
        out.extend_from_slice(&(b.len() as u32 + 1).to_be_bytes());
        out.push(0);
    } else {
        out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    }
    out.extend_from_slice(b);
}

/// A message being read.
pub struct Rd<'a>(pub &'a [u8]);

impl<'a> Rd<'a> {
    pub fn byte(&mut self) -> Option<u8> {
        let (b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(*b)
    }
    pub fn bool(&mut self) -> Option<bool> {
        self.byte().map(|b| b != 0)
    }
    pub fn u32(&mut self) -> Option<u32> {
        let b = self.0.get(..4)?;
        let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        self.0 = &self.0[4..];
        Some(n)
    }
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        get_string(&mut self.0)
    }
    pub fn str(&mut self) -> Option<String> {
        self.bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
    fn names(&mut self) -> Option<Vec<String>> {
        let s = self.str()?;
        Some(
            s.split(',')
                .filter(|n| !n.is_empty())
                .map(String::from)
                .collect(),
        )
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("ssh: {what}"))
}

/// The cipher one direction uses.
enum Cipher {
    None,
    Chacha {
        seal: Box<cc::SealingKey>,
        open: Box<cc::OpeningKey>,
    },
    Gcm {
        key: Box<LessSafeKey>,
        iv: [u8; 12],
    },
}

impl Cipher {
    fn new(name: &str, key: &[u8], iv: &[u8]) -> Cipher {
        match name {
            "chacha20-poly1305@openssh.com" => {
                let k: &[u8; cc::KEY_LEN] = key.try_into().expect("64 bytes of key");
                Cipher::Chacha {
                    seal: Box::new(cc::SealingKey::new(k)),
                    open: Box::new(cc::OpeningKey::new(k)),
                }
            }
            _ => {
                let alg = if name == "aes256-gcm@openssh.com" {
                    &aead::AES_256_GCM
                } else {
                    &aead::AES_128_GCM
                };
                let key = LessSafeKey::new(UnboundKey::new(alg, key).expect("a key of its length"));
                Cipher::Gcm {
                    key: Box::new(key),
                    iv: iv.try_into().expect("12 bytes of iv"),
                }
            }
        }
    }

    fn key_len(name: &str) -> (usize, usize) {
        match name {
            "chacha20-poly1305@openssh.com" => (cc::KEY_LEN, 0),
            "aes256-gcm@openssh.com" => (32, 12),
            _ => (16, 12),
        }
    }

    /// The block a packet's length is a multiple of, and whether the
    /// length itself counts toward it.
    fn block(&self) -> (usize, bool) {
        match self {
            Cipher::None => (8, true),
            Cipher::Chacha { .. } => (8, false),
            Cipher::Gcm { .. } => (16, false),
        }
    }

    /// GCM's nonce: the fixed part and a counter that grows per packet.
    fn gcm_next(iv: &mut [u8; 12]) -> Nonce {
        let n = Nonce::assume_unique_for_key(*iv);
        let mut c = u64::from_be_bytes(iv[4..].try_into().unwrap_or_default());
        c = c.wrapping_add(1);
        iv[4..].copy_from_slice(&c.to_be_bytes());
        n
    }
}

/// One direction's cipher and packet count.
struct Dir {
    cipher: Cipher,
    seq: u32,
}

impl Dir {
    fn plain() -> Dir {
        Dir {
            cipher: Cipher::None,
            seq: 0,
        }
    }

    /// payload as a packet on the wire.
    fn seal(&mut self, payload: &[u8]) -> Vec<u8> {
        let (block, len_counts) = self.cipher.block();
        let fixed = if len_counts { 5 } else { 1 };
        let mut pad = block - (fixed + payload.len()) % block;
        if pad < 4 {
            pad += block;
        }
        let len = 1 + payload.len() + pad;
        let mut p = Vec::with_capacity(4 + len + 16);
        p.extend_from_slice(&(len as u32).to_be_bytes());
        p.push(pad as u8);
        p.extend_from_slice(payload);
        p.extend_from_slice(&rand_bytes(pad));
        match &mut self.cipher {
            Cipher::None => {}
            Cipher::Chacha { seal, .. } => {
                let mut tag = [0u8; cc::TAG_LEN];
                seal.seal_in_place(self.seq, &mut p, &mut tag);
                p.extend_from_slice(&tag);
            }
            Cipher::Gcm { key, iv } => {
                let nonce = Cipher::gcm_next(iv);
                let (head, body) = p.split_at_mut(4);
                let tag = key
                    .seal_in_place_separate_tag(nonce, Aad::from(&*head), body)
                    .expect("a packet within GCM's limit");
                p.extend_from_slice(tag.as_ref());
            }
        }
        self.seq = self.seq.wrapping_add(1);
        p
    }

    /// The next packet's payload, read from r.
    fn open(&mut self, r: &mut impl Read) -> io::Result<Vec<u8>> {
        let mut head = [0u8; 4];
        r.read_exact(&mut head)?;
        let len = match &self.cipher {
            Cipher::Chacha { open, .. } => open.decrypt_packet_length(self.seq, head),
            _ => head,
        };
        let len = u32::from_be_bytes(len) as usize;
        if !(5..=MAX_PACKET).contains(&len) {
            return Err(bad("packet too large"));
        }
        let tag = if matches!(self.cipher, Cipher::None) {
            0
        } else {
            16
        };
        let mut p = vec![0u8; 4 + len + tag];
        p[..4].copy_from_slice(&head);
        r.read_exact(&mut p[4..])?;
        let plain: &[u8] = match &mut self.cipher {
            Cipher::None => &p[4..],
            Cipher::Chacha { open, .. } => {
                let mut t = [0u8; cc::TAG_LEN];
                t.copy_from_slice(&p[4 + len..]);
                let body = &mut p[..4 + len];
                open.open_in_place(self.seq, body, &t)
                    .map_err(|_| bad("message authentication failed"))?
            }
            Cipher::Gcm { key, iv } => {
                let nonce = Cipher::gcm_next(iv);
                let (head, body) = p.split_at_mut(4);
                key.open_in_place(nonce, Aad::from(&*head), body)
                    .map_err(|_| bad("message authentication failed"))?
            }
        };
        self.seq = self.seq.wrapping_add(1);
        let pad = *plain.first().ok_or_else(|| bad("empty packet"))? as usize;
        if pad + 1 > plain.len() {
            return Err(bad("bad padding"));
        }
        Ok(plain[1..plain.len() - pad].to_vec())
    }
}

/// The sending half: the socket, its cipher, and whether keys are
/// changing (when only the key exchange's own messages go).
pub struct Out {
    st: Mutex<OutState>,
    cv: Condvar,
}

struct OutState {
    w: TcpStream,
    dir: Dir,
    kexing: bool,
    closed: bool,
}

impl Out {
    fn lock(&self) -> MutexGuard<'_, OutState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sends m once keys aren't changing.
    pub fn send(&self, m: Msg) -> io::Result<()> {
        let mut st = self.lock();
        while st.kexing && !st.closed {
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        Self::write(&mut st, &m.0)
    }

    fn write(st: &mut OutState, payload: &[u8]) -> io::Result<()> {
        if st.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let p = st.dir.seal(payload);
        let r = st.w.write_all(&p);
        if r.is_err() {
            st.closed = true;
        }
        r
    }

    /// Ends the connection: what's waiting to send gives up.
    pub fn close(&self) {
        let mut st = self.lock();
        st.closed = true;
        let _ = st.w.shutdown(std::net::Shutdown::Both);
        drop(st);
        self.cv.notify_all();
    }

    /// Says why the connection ends, and ends it.
    pub fn disconnect(&self, reason: u32, why: &str) {
        {
            let mut st = self.lock();
            let m = Msg::new(MSG_DISCONNECT).u32(reason).str(why).str("");
            let _ = Self::write(&mut st, &m.0);
        }
        self.close();
    }
}

/// What the key exchange picked.
struct Algs {
    c2s: String,
    s2c: String,
}

/// The receiving half and the key exchange: one thread reads.
pub struct Transport {
    r: BufReader<TcpStream>,
    dir: Dir,
    pub out: Arc<Out>,
    seed: [u8; 32],
    host_pub: Vec<u8>,
    pub client_version: String,
    pub session_id: Vec<u8>,
    strict: bool,
    ext_info: bool,
}

impl Transport {
    /// The handshake on a new connection: versions, then keys. seed and
    /// host_pub are the host key's.
    pub fn accept(c: TcpStream, seed: [u8; 32], host_pub: Vec<u8>) -> io::Result<Transport> {
        let mut w = c.try_clone()?;
        w.write_all(format!("{SERVER_VERSION}\r\n").as_bytes())?;
        let mut r = BufReader::new(c);
        let client_version = read_version(&mut r)?;
        let out = Arc::new(Out {
            st: Mutex::new(OutState {
                w,
                dir: Dir::plain(),
                kexing: true,
                closed: false,
            }),
            cv: Condvar::new(),
        });
        let mut t = Transport {
            r,
            dir: Dir::plain(),
            out,
            seed,
            host_pub,
            client_version,
            session_id: Vec::new(),
            strict: false,
            ext_info: false,
        };
        let ours = t.send_kexinit()?;
        let theirs = t.dir.open(&mut t.r)?;
        if theirs.first() != Some(&MSG_KEXINIT) {
            return Err(bad("the first message isn't a key exchange"));
        }
        t.exchange(&theirs, &ours)?;
        Ok(t)
    }

    /// Our KEXINIT, sent, with the send side held for the exchange.
    fn send_kexinit(&mut self) -> io::Result<Vec<u8>> {
        let mut kex = KEX.join(",");
        if self.session_id.is_empty() {
            kex.push(',');
            kex.push_str(STRICT_S);
        }
        let ciphers = CIPHERS.join(",");
        let macs = MACS.join(",");
        let m = Msg::new(MSG_KEXINIT);
        let mut m = Msg(
            [m.0, rand_bytes(16)].concat(), // cookie
        )
        .str(&kex)
        .str(HOST_KEY)
        .str(&ciphers)
        .str(&ciphers)
        .str(&macs)
        .str(&macs)
        .str("none")
        .str("none")
        .str("")
        .str("")
        .bool(false)
        .u32(0);
        let payload = std::mem::take(&mut m.0);
        let mut st = self.out.lock();
        st.kexing = true;
        Out::write(&mut st, &payload)?;
        Ok(payload)
    }

    /// One key exchange, from the client's KEXINIT (theirs) and ours.
    fn exchange(&mut self, theirs: &[u8], ours: &[u8]) -> io::Result<()> {
        let first = self.session_id.is_empty();
        let mut rd = Rd(theirs.get(17..).ok_or_else(|| bad("short KEXINIT"))?);
        let lists: Vec<Vec<String>> = (0..10)
            .map(|_| rd.names())
            .collect::<Option<_>>()
            .ok_or_else(|| bad("short KEXINIT"))?;
        let follows = rd.bool().unwrap_or(false);
        let pick = |i: usize, ours: &[&str]| -> Option<String> {
            lists[i]
                .iter()
                .find(|c| ours.contains(&c.as_str()))
                .cloned()
        };
        let kex = pick(0, KEX).ok_or_else(|| bad("no common key exchange"))?;
        pick(1, &[HOST_KEY]).ok_or_else(|| bad("no common host key"))?;
        let algs = Algs {
            c2s: pick(2, CIPHERS).ok_or_else(|| bad("no common cipher"))?,
            s2c: pick(3, CIPHERS).ok_or_else(|| bad("no common cipher"))?,
        };
        if !lists[6].iter().any(|c| c == "none") || !lists[7].iter().any(|c| c == "none") {
            return Err(bad("no common compression"));
        }
        if first {
            self.strict = lists[0].iter().any(|c| c == STRICT_C);
            self.ext_info = lists[0].iter().any(|c| c == "ext-info-c");
        }
        // A wrong guess at the exchange is a packet to drop.
        let mut skip = follows
            && (lists[0].first() != Some(&kex)
                || lists[1].first().map(String::as_str) != Some(HOST_KEY));

        let init = loop {
            let p = self.dir.open(&mut self.r)?;
            if skip {
                skip = false;
                continue;
            }
            match p.first() {
                Some(&MSG_KEX_ECDH_INIT) => break p,
                Some(&(MSG_IGNORE | MSG_DEBUG)) if !(first && self.strict) => {}
                _ => return Err(bad("unexpected message during key exchange")),
            }
        };
        let q_c = Rd(&init[1..])
            .bytes()
            .ok_or_else(|| bad("short ECDH init"))?
            .to_vec();
        if q_c.len() != 32 {
            return Err(bad("bad curve25519 key"));
        }
        let rng = ring::rand::SystemRandom::new();
        let eph = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
            .map_err(|_| bad("no randomness"))?;
        let q_s = eph
            .compute_public_key()
            .map_err(|_| bad("no randomness"))?
            .as_ref()
            .to_vec();
        let k = agreement::agree_ephemeral(
            eph,
            &agreement::UnparsedPublicKey::new(&agreement::X25519, &q_c),
            |s| s.to_vec(),
        )
        .map_err(|_| bad("bad curve25519 key"))?;
        let mut k_mp = Vec::new();
        put_mpint(&mut k_mp, &k);

        let mut h = Sha256::new();
        for s in [
            self.client_version.as_bytes(),
            SERVER_VERSION.as_bytes(),
            theirs,
            ours,
            &self.host_pub,
            &q_c,
            &q_s,
        ] {
            let mut b = Vec::new();
            put_string(&mut b, s);
            h.update(&b);
        }
        h.update(&k_mp);
        let h = h.finalize().to_vec();
        if first {
            self.session_id = h.clone();
        }
        let kp =
            Ed25519KeyPair::from_seed_unchecked(&self.seed).map_err(|_| bad("bad host key"))?;
        let sig = Msg::default().str(HOST_KEY).bytes(kp.sign(&h).as_ref()).0;
        let reply = Msg::new(MSG_KEX_ECDH_REPLY)
            .bytes(&self.host_pub)
            .bytes(&q_s)
            .bytes(&sig);

        let derive = |letter: u8, n: usize| -> Vec<u8> {
            let mut out = Sha256::new()
                .chain_update(&k_mp)
                .chain_update(&h)
                .chain_update([letter])
                .chain_update(&self.session_id)
                .finalize()
                .to_vec();
            while out.len() < n {
                let more = Sha256::new()
                    .chain_update(&k_mp)
                    .chain_update(&h)
                    .chain_update(&out)
                    .finalize();
                out.extend_from_slice(&more);
            }
            out.truncate(n);
            out
        };
        let (kl, il) = Cipher::key_len(&algs.s2c);
        let send = Cipher::new(&algs.s2c, &derive(b'D', kl), &derive(b'B', il));
        let (kl, il) = Cipher::key_len(&algs.c2s);
        let recv = Cipher::new(&algs.c2s, &derive(b'C', kl), &derive(b'A', il));

        {
            let mut st = self.out.lock();
            Out::write(&mut st, &reply.0)?;
            Out::write(&mut st, &[MSG_NEWKEYS])?;
            st.dir.cipher = send;
            if self.strict {
                st.dir.seq = 0;
            }
            if first && self.ext_info {
                let m = Msg::new(MSG_EXT_INFO)
                    .u32(1)
                    .str("server-sig-algs")
                    .str(SIG_ALGS);
                Out::write(&mut st, &m.0)?;
            }
            st.kexing = false;
        }
        self.out.cv.notify_all();

        loop {
            let p = self.dir.open(&mut self.r)?;
            match p.first() {
                Some(&MSG_NEWKEYS) => break,
                Some(&(MSG_IGNORE | MSG_DEBUG)) if !(first && self.strict) => {}
                _ => return Err(bad("unexpected message during key exchange")),
            }
        }
        self.dir.cipher = recv;
        if self.strict {
            self.dir.seq = 0;
        }
        Ok(())
    }

    /// The next message for the layers above: the transport's own
    /// (re-keying, ignore, debug) are handled here.
    pub fn read(&mut self) -> io::Result<Vec<u8>> {
        loop {
            let p = self.dir.open(&mut self.r)?;
            match p.first() {
                Some(&MSG_KEXINIT) => {
                    let ours = self.send_kexinit()?;
                    self.exchange(&p, &ours)?;
                }
                Some(&(MSG_IGNORE | MSG_DEBUG | MSG_UNIMPLEMENTED)) => {}
                Some(&MSG_DISCONNECT) => return Err(io::ErrorKind::ConnectionAborted.into()),
                Some(_) => return Ok(p),
                None => return Err(bad("empty message")),
            }
        }
    }
}

/// The client's version line; lines before it are skipped.
fn read_version(r: &mut BufReader<TcpStream>) -> io::Result<String> {
    for _ in 0..32 {
        let mut line = Vec::new();
        r.by_ref().take(256).read_until(b'\n', &mut line)?;
        if line.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let s = String::from_utf8_lossy(&line)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if s.starts_with("SSH-2.0-") || s.starts_with("SSH-1.99-") {
            return Ok(s);
        }
    }
    Err(bad("no version line"))
}

/// Whether sig signs msg with the client key blob, as algo.
pub fn verify(algo: &str, blob: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let mut k = Rd(blob);
    let Some(ktype) = k.str() else {
        return false;
    };
    let mut s = Rd(sig);
    let (Some(salgo), Some(sbody)) = (s.str(), s.bytes()) else {
        return false;
    };
    if salgo != algo {
        return false;
    }
    match (ktype.as_str(), algo) {
        ("ssh-ed25519", "ssh-ed25519") => k.bytes().is_some_and(|pk| {
            signature::UnparsedPublicKey::new(&signature::ED25519, pk)
                .verify(msg, sbody)
                .is_ok()
        }),
        ("ssh-rsa", "rsa-sha2-256" | "rsa-sha2-512" | "ssh-rsa") => {
            let (Some(e), Some(n)) = (k.bytes(), k.bytes()) else {
                return false;
            };
            let strip = |b: &'_ [u8]| -> Vec<u8> {
                b[b.iter().take_while(|x| **x == 0).count()..].to_vec()
            };
            let params = match algo {
                "rsa-sha2-256" => &signature::RSA_PKCS1_2048_8192_SHA256,
                "rsa-sha2-512" => &signature::RSA_PKCS1_2048_8192_SHA512,
                _ => &signature::RSA_PKCS1_2048_8192_SHA1_FOR_LEGACY_USE_ONLY,
            };
            signature::RsaPublicKeyComponents {
                n: strip(n),
                e: strip(e),
            }
            .verify(params, msg, sbody)
            .is_ok()
        }
        (
            t @ ("ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384"),
            a @ ("ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384"),
        ) if t == a => {
            let (Some(_curve), Some(q)) = (k.bytes(), k.bytes()) else {
                return false;
            };
            let (alg, n): (&signature::EcdsaVerificationAlgorithm, usize) =
                if t == "ecdsa-sha2-nistp256" {
                    (&signature::ECDSA_P256_SHA256_FIXED, 32)
                } else {
                    (&signature::ECDSA_P384_SHA384_FIXED, 48)
                };
            let mut rs = Rd(sbody);
            let (Some(r), Some(s)) = (rs.bytes(), rs.bytes()) else {
                return false;
            };
            let mut fixed = Vec::with_capacity(2 * n);
            for x in [r, s] {
                let x = &x[x.iter().take_while(|b| **b == 0).count()..];
                if x.len() > n {
                    return false;
                }
                fixed.extend(std::iter::repeat_n(0, n - x.len()));
                fixed.extend_from_slice(x);
            }
            signature::UnparsedPublicKey::new(alg, q)
                .verify(msg, &fixed)
                .is_ok()
        }
        _ => false,
    }
}

/// Whether algo is a key type serve takes, for a key of type ktype.
pub fn takes(algo: &str, blob: &[u8]) -> bool {
    let ktype = Rd(blob).str().unwrap_or_default();
    SIG_ALGS.split(',').any(|a| a == algo)
        && match algo {
            "rsa-sha2-256" | "rsa-sha2-512" => ktype == "ssh-rsa",
            a => ktype == a,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mpints as RFC 4251 §5 writes them.
    #[test]
    fn mpints() {
        for (n, want) in [
            (&[0u8; 0][..], &[0u8, 0, 0, 0][..]),
            (&[0, 0, 0x7f], &[0, 0, 0, 1, 0x7f]),
            (&[0x80], &[0, 0, 0, 2, 0, 0x80]),
            (&[0x09, 0xa3], &[0, 0, 0, 2, 0x09, 0xa3]),
        ] {
            let mut b = Vec::new();
            put_mpint(&mut b, n);
            assert_eq!(b, want, "{n:?}");
        }
    }

    /// A packet sealed one way opens the other, for each cipher, and a
    /// tampered one doesn't.
    #[test]
    fn packets_round_trip() {
        for name in CIPHERS {
            let (kl, il) = Cipher::key_len(name);
            let (k, iv) = (rand_bytes(kl), rand_bytes(il));
            let mut a = Dir {
                cipher: Cipher::new(name, &k, &iv),
                seq: 3,
            };
            let mut b = Dir {
                cipher: Cipher::new(name, &k, &iv),
                seq: 3,
            };
            for payload in [&b"hello"[..], &[7u8; 1000][..]] {
                let p = a.seal(payload);
                assert_eq!(b.open(&mut &p[..]).unwrap(), payload, "{name}");
            }
            let mut p = a.seal(b"x");
            let at = p.len() - 20;
            p[at] ^= 1;
            assert!(b.open(&mut &p[..]).is_err(), "{name}");
        }
        let mut a = Dir::plain();
        let p = a.seal(b"plain");
        assert_eq!((p.len() - 4) % 8, 4);
        assert_eq!(Dir::plain().open(&mut &p[..]).unwrap(), b"plain");
    }

    /// An ed25519 signature checks, and not over other words.
    #[test]
    fn verifies_ed25519() {
        let kp = Ed25519KeyPair::from_seed_unchecked(&[9u8; 32]).unwrap();
        use ring::signature::KeyPair;
        let blob = Msg::default()
            .str("ssh-ed25519")
            .bytes(kp.public_key().as_ref())
            .0;
        let sig = Msg::default()
            .str("ssh-ed25519")
            .bytes(kp.sign(b"msg").as_ref())
            .0;
        assert!(verify("ssh-ed25519", &blob, b"msg", &sig));
        assert!(!verify("ssh-ed25519", &blob, b"other", &sig));
        assert!(takes("ssh-ed25519", &blob));
        assert!(!takes("rsa-sha2-256", &blob));
    }
}
