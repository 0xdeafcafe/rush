//! serve's SSH host key: an ed25519 key in remote-ssh-host-key, made on
//! first use, written as Go's ssh.MarshalPrivateKey writes it (OpenSSH's
//! own format), and its fingerprint, which pairing shows so the first
//! connection can be checked. After Go's internal/remote/sshd.go
//! (sshHostKey :38, SSHFingerprint :61). serve's SSH server itself
//! (serveSSH :71 and the rest of sshd.go) isn't ported.

use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use sha2::{Digest, Sha256};

use crate::config::{dir, rand_bytes, write_private};

fn key_path() -> PathBuf {
    dir().join("remote-ssh-host-key")
}

fn put_string(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    out.extend_from_slice(b);
}

fn get_string<'a>(b: &mut &'a [u8]) -> Option<&'a [u8]> {
    if b.len() < 4 {
        return None;
    }
    let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    if b.len() < 4 + n {
        return None;
    }
    let s = &b[4..4 + n];
    *b = &b[4 + n..];
    Some(s)
}

/// An ed25519 public key as SSH sends it.
fn pub_wire(public: &[u8]) -> Vec<u8> {
    let mut w = Vec::new();
    put_string(&mut w, b"ssh-ed25519");
    put_string(&mut w, public);
    w
}

/// The key made from seed, as OpenSSH's PEM.
pub fn marshal(seed: &[u8; 32], comment: &str) -> String {
    let kp = ring::signature::Ed25519KeyPair::from_seed_unchecked(seed).expect("a 32-byte seed");
    use ring::signature::KeyPair;
    let public = kp.public_key().as_ref().to_vec();
    let check = rand_bytes(4);
    let mut private = Vec::new();
    private.extend_from_slice(&check);
    private.extend_from_slice(&check);
    put_string(&mut private, b"ssh-ed25519");
    put_string(&mut private, &public);
    let mut both = seed.to_vec();
    both.extend_from_slice(&public);
    put_string(&mut private, &both);
    put_string(&mut private, comment.as_bytes());
    let mut pad = 1u8;
    while private.len() % 8 != 0 {
        private.push(pad);
        pad += 1;
    }
    let mut b = b"openssh-key-v1\0".to_vec();
    put_string(&mut b, b"none");
    put_string(&mut b, b"none");
    put_string(&mut b, b"");
    b.extend_from_slice(&1u32.to_be_bytes());
    put_string(&mut b, &pub_wire(&public));
    put_string(&mut b, &private);
    let enc = STANDARD.encode(&b);
    let mut out = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for c in enc.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(c).unwrap_or_default());
        out.push('\n');
    }
    out.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    out
}

/// The public key of an OpenSSH PEM, as SSH sends it.
pub fn public_of(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    let b = STANDARD.decode(body.trim()).ok()?;
    let mut r = b.strip_prefix(b"openssh-key-v1\0")?;
    get_string(&mut r)?;
    get_string(&mut r)?;
    get_string(&mut r)?;
    if r.len() < 4 {
        return None;
    }
    r = &r[4..];
    get_string(&mut r).map(<[u8]>::to_vec)
}

/// The host key's public half, made on first use.
fn host_public() -> Option<Vec<u8>> {
    let path = key_path();
    if let Ok(pem) = std::fs::read_to_string(&path) {
        return public_of(&pem);
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&rand_bytes(32));
    let pem = marshal(&seed, "rush serve");
    write_private(&path, pem.as_bytes()).ok()?;
    public_of(&pem)
}

/// A key's fingerprint, as ssh-keygen -l says it: SHA256:….
pub fn fingerprint_of(wire: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(wire)))
}

/// serve's SSH host key's fingerprint, for checking the first connection.
pub fn fingerprint() -> String {
    host_public().map_or_else(String::new, |w| fingerprint_of(&w))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key Go's ssh.MarshalPrivateKey wrote reads, and gives the
    /// fingerprint ssh-keygen gives it.
    #[test]
    fn reads_a_key_and_its_fingerprint() {
        let seed = [7u8; 32];
        let pem = marshal(&seed, "rush serve");
        let w = public_of(&pem).unwrap();
        let mut r = &w[..];
        assert_eq!(get_string(&mut r).unwrap(), b"ssh-ed25519");
        assert_eq!(get_string(&mut r).unwrap().len(), 32);
        assert!(fingerprint_of(&w).starts_with("SHA256:"));
        assert_eq!(fingerprint_of(&w).len(), 7 + 43);
        // OpenSSH reads the file, and says the same fingerprint.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("key");
        write_private(&p, pem.as_bytes()).unwrap();
        if let Ok(out) = std::process::Command::new("/usr/bin/ssh-keygen")
            .args(["-l", "-f"])
            .arg(&p)
            .output()
        {
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(said.contains(&fingerprint_of(&w)), "{said}");
            assert!(said.contains("rush serve (ED25519)"), "{said}");
        }
    }
}
