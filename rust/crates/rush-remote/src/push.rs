//! Web Push notifications (RFC 8030, encrypted as RFC 8291, signed as RFC
//! 8292) to the browsers that asked for them. Its key and their
//! subscriptions live in remote-push.json, readable only by you, the key
//! as Go writes it (PEM, PKCS #8), so Go's and Rust's serve share it.
//!
//! After Go's internal/remote/push.go (Push :35, Subscription :47, Notice
//! :56, LoadPush :70, save :100, PublicKey :117, pushHosts :125,
//! pushHostOK :127, Add :143, Send :168, send :202, vapid :229, encrypt
//! :250, seal :273).

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{dir, rand_bytes, write_private};
use crate::expose::fields;

/// A browser's subscription, as PushSubscription.toJSON gives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subscription {
    pub endpoint: String,
    pub keys: Keys,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keys {
    pub p256dh: String,
    pub auth: String,
}

/// What one notification says, and where tapping it goes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub url: String,
    pub tag: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct PushFile {
    key: String,
    #[serde(rename = "subscriptions")]
    subs: Option<Vec<Subscription>>,
}

/// Sends a notice to one endpoint: its status, or why it couldn't.
pub type Sender =
    Box<dyn Fn(&str, &[(String, String)], Vec<u8>) -> Result<u16, String> + Send + Sync>;

/// The sender's key and the browsers subscribed.
pub struct Push {
    path: PathBuf,
    pkcs8: Vec<u8>,
    key: EcdsaKeyPair,
    /// Who the push services can write to about this sender.
    pub subject: String,
    subs: Mutex<Vec<Subscription>>,
    send: Sender,
}

/// The push services a subscription may name: sending only to them keeps
/// a subscription from aiming serve at anything else.
const PUSH_HOSTS: [&str; 4] = [
    "fcm.googleapis.com",
    ".push.apple.com",
    ".notify.windows.com",
    ".push.services.mozilla.com",
];

/// Whether endpoint is an https URL on one of the push services, with no
/// user or port.
pub fn push_host_ok(endpoint: &str) -> bool {
    let Some(rest) = endpoint.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.contains('@') || host.contains(':') || host.is_empty() {
        return false;
    }
    let h = host.to_ascii_lowercase();
    PUSH_HOSTS
        .iter()
        .any(|s| h == s.trim_start_matches('.') || (s.starts_with('.') && h.ends_with(s)))
}

fn pem_encode(der: &[u8]) -> String {
    let b = STANDARD.encode(der);
    let mut out = String::from("-----BEGIN PRIVATE KEY-----\n");
    for c in b.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(c).unwrap_or_default());
        out.push('\n');
    }
    out.push_str("-----END PRIVATE KEY-----\n");
    out
}

fn pem_decode(s: &str) -> Option<Vec<u8>> {
    let start = s.find("-----BEGIN ")?;
    let body = &s[start..];
    let body = &body[body.find('\n')? + 1..];
    let end = body.find("-----END ")?;
    let b64: String = body[..end].chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(b64).ok()
}

/// An ECDSA P-256 key's PKCS #8 as Go's x509.MarshalPKCS8PrivateKey writes
/// it, from ring's: the same document with the curve inside the private
/// key left out, which ring reads too.
fn generate_pkcs8() -> Result<Vec<u8>, String> {
    let rng = SystemRandom::new();
    EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
        .map(|d| d.as_ref().to_vec())
        .map_err(|_| "can't make a key".to_string())
}

impl Push {
    /// Reads the sender's key and subscriptions, making a key on first use.
    pub fn load(subject: &str) -> Result<Push, String> {
        Push::load_at(dir().join("remote-push.json"), subject, Box::new(http_send))
    }

    /// load with the file at path and sends by send (tests: a fake).
    pub fn load_at(path: PathBuf, subject: &str, send: Sender) -> Result<Push, String> {
        let f: PushFile = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| e.to_string())?,
            Err(_) => PushFile::default(),
        };
        let pkcs8 = match pem_decode(&f.key) {
            Some(d) => d,
            None => generate_pkcs8()?,
        };
        let key = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            &pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|e| format!("remote-push.json: not an ECDSA key: {e}"))?;
        let p = Push {
            path,
            pkcs8,
            key,
            subject: subject.into(),
            subs: Mutex::new(f.subs.unwrap_or_default()),
            send,
        };
        let subs = p.subs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        p.save(&subs)?;
        Ok(p)
    }

    fn save(&self, subs: &[Subscription]) -> Result<(), String> {
        let f = PushFile {
            key: pem_encode(&self.pkcs8),
            subs: (!subs.is_empty()).then(|| subs.to_vec()),
        };
        let b = serde_json::to_vec_pretty(&f).map_err(|e| e.to_string())?;
        write_private(&self.path, &b)
    }

    /// The sender's key as a browser's subscribe takes it.
    pub fn public_key(&self) -> String {
        B64.encode(self.key.public_key().as_ref())
    }

    /// Keeps a browser's subscription, replacing one to the same endpoint.
    pub fn add(&self, s: Subscription) -> Result<(), String> {
        if !push_host_ok(&s.endpoint) {
            return Err("not a push service rush sends to".into());
        }
        if B64.decode(&s.keys.p256dh).map(|k| k.len()) != Ok(65) {
            return Err("bad p256dh key".into());
        }
        if B64.decode(&s.keys.auth).map(|k| k.len()) != Ok(16) {
            return Err("bad auth secret".into());
        }
        let mut subs = self.subs.lock().unwrap_or_else(|e| e.into_inner());
        match subs.iter_mut().find(|x| x.endpoint == s.endpoint) {
            Some(x) => *x = s,
            None => subs.push(s),
        }
        self.save(&subs)
    }

    /// How many browsers are subscribed.
    pub fn subscribed(&self) -> usize {
        self.subs.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Sends n to every subscription, dropping those the push service says
    /// are gone. Delivery is the push service's best effort.
    pub fn send(&self, n: &Notice) {
        let body = serde_json::to_vec(n).unwrap_or_default();
        let subs = self.subs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (mut gone, mut failed) = (Vec::new(), Vec::new());
        for s in &subs {
            match self.send_one(s, &body) {
                Ok(404 | 410) => gone.push(s.endpoint.clone()),
                Ok(_) => {}
                Err(e) => failed.push(e),
            }
        }
        let mut f = json!({ "title": n.title, "body": n.body, "tag": n.tag, "devices": subs.len(), "gone": gone.len() });
        if !failed.is_empty() {
            f["errs"] = json!(failed);
        }
        rush_events::log("push-sent", fields(f));
        if gone.is_empty() {
            return;
        }
        let mut subs = self.subs.lock().unwrap_or_else(|e| e.into_inner());
        subs.retain(|s| !gone.contains(&s.endpoint));
        let _ = self.save(&subs);
    }

    fn send_one(&self, s: &Subscription, payload: &[u8]) -> Result<u16, String> {
        if !push_host_ok(&s.endpoint) {
            return Err("bad endpoint".into());
        }
        let body = encrypt(s, payload)?;
        let jwt = self.vapid(&s.endpoint)?;
        let h = vec![
            ("Content-Encoding".to_string(), "aes128gcm".to_string()),
            (
                "Content-Type".to_string(),
                "application/octet-stream".to_string(),
            ),
            ("TTL".to_string(), "86400".to_string()),
            ("Urgency".to_string(), "high".to_string()),
            (
                "Authorization".to_string(),
                format!("vapid t={jwt}, k={}", self.public_key()),
            ),
        ];
        (self.send)(&s.endpoint, &h, body)
    }

    /// The JWT (ES256) that says this sender is the one the subscription
    /// was made for.
    pub fn vapid(&self, endpoint: &str) -> Result<String, String> {
        let rest = endpoint.split_once("://").map_or("", |(_, r)| r);
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        let scheme = endpoint.split_once("://").map_or("", |(s, _)| s);
        let mut sub = self.subject.clone();
        if sub.is_empty() || !(sub.starts_with("https://") || sub.starts_with("mailto:")) {
            sub = "mailto:rush@localhost".into();
        }
        let head = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let exp = rush_state::Time::now().to_ms() / 1000 + 12 * 3600;
        let claims = serde_json::to_vec(
            &json!({ "aud": format!("{scheme}://{host}"), "exp": exp, "sub": sub }),
        )
        .map_err(|e| e.to_string())?;
        let unsigned = format!("{head}.{}", B64.encode(claims));
        let sig = self
            .key
            .sign(&SystemRandom::new(), unsigned.as_bytes())
            .map_err(|_| "can't sign".to_string())?;
        Ok(format!("{unsigned}.{}", B64.encode(sig.as_ref())))
    }
}

/// Sends a push over HTTPS, at most 20s.
fn http_send(url: &str, h: &[(String, String)], body: Vec<u8>) -> Result<u16, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.post(url);
    for (k, v) in h {
        req = req.header(k, v);
    }
    let r = req.send(&body[..]).map_err(|e| e.to_string())?;
    Ok(r.status().as_u16())
}

/// payload as RFC 8291's aes128gcm for subscription s, in one record.
pub fn encrypt(s: &Subscription, payload: &[u8]) -> Result<Vec<u8>, String> {
    use ring::agreement::{ECDH_P256, EphemeralPrivateKey, UnparsedPublicKey, agree_ephemeral};
    let ua_pub = B64.decode(&s.keys.p256dh).map_err(|e| e.to_string())?;
    let auth = B64.decode(&s.keys.auth).map_err(|e| e.to_string())?;
    let rng = SystemRandom::new();
    let as_key = EphemeralPrivateKey::generate(&ECDH_P256, &rng).map_err(|_| "can't make a key")?;
    let as_pub = as_key
        .compute_public_key()
        .map_err(|_| "can't make a key")?
        .as_ref()
        .to_vec();
    let salt = rand_bytes(16);
    let secret = agree_ephemeral(as_key, &UnparsedPublicKey::new(&ECDH_P256, &ua_pub), |k| {
        k.to_vec()
    })
    .map_err(|_| "bad p256dh key".to_string())?;
    seal(&secret, &as_pub, &ua_pub, &auth, &salt, payload)
}

/// HKDF-SHA256 of ikm with salt and info, n bytes of it.
fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], n: usize) -> Vec<u8> {
    use ring::hkdf::{HKDF_SHA256, KeyType, Salt};
    struct Len(usize);
    impl KeyType for Len {
        fn len(&self) -> usize {
            self.0
        }
    }
    let prk = Salt::new(HKDF_SHA256, salt).extract(ikm);
    let mut out = vec![0; n];
    let infos = [info];
    if let Ok(okm) = prk.expand(&infos, Len(n)) {
        let _ = okm.fill(&mut out);
    }
    out
}

/// The body for the ECDH secret between the sender's key as_pub and the
/// browser's ua_pub (seal; the secret worked out by the caller, as ring
/// keeps a key it made to itself).
pub fn seal(
    secret: &[u8],
    as_pub: &[u8],
    ua_pub: &[u8],
    auth: &[u8],
    salt: &[u8],
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    let mut info = b"WebPush: info\x00".to_vec();
    info.extend_from_slice(ua_pub);
    info.extend_from_slice(as_pub);
    let ikm = hkdf(auth, secret, &info, 32);
    let cek = hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\x00", 16);
    let nonce = hkdf(salt, &ikm, b"Content-Encoding: nonce\x00", 12);
    if payload.len() > 3000 {
        return Err(format!(
            "push payload of {} bytes is too big",
            payload.len()
        ));
    }
    let key = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).map_err(|_| "bad key")?);
    let mut rec = payload.to_vec();
    rec.push(2);
    let n = Nonce::try_assume_unique_for_key(&nonce).map_err(|_| "bad nonce")?;
    key.seal_in_place_append_tag(n, Aad::empty(), &mut rec)
        .map_err(|_| "can't seal")?;
    let mut out = Vec::with_capacity(16 + 4 + 1 + as_pub.len() + rec.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_pub.len() as u8);
    out.extend_from_slice(as_pub);
    out.extend_from_slice(&rec);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn d(s: &str) -> Vec<u8> {
        B64.decode(s).unwrap()
    }

    /// RFC 8291 appendix A: its keys, secret, salt and message make the
    /// body it gives.
    #[test]
    fn seal_rfc8291() {
        let got = seal(
            &d("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs"),
            &d("BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"),
            &d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"),
            &d("BTBZMqHH6r4Tts7J_aSIgg"),
            &d("DGv6ra1nlYgDCS1FRnbzlw"),
            b"When I grow up, I want to be a watermelon",
        )
        .unwrap();
        let want = d(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
        );
        assert_eq!(got, want);
    }

    #[test]
    fn push_hosts() {
        for (u, want) in [
            ("https://fcm.googleapis.com/fcm/send/x", true),
            ("https://web.push.apple.com/abc", true),
            ("https://updates.push.services.mozilla.com/x", true),
            ("http://fcm.googleapis.com/x", false),
            ("https://127.0.0.1/x", false),
            ("https://evil.com/.push.apple.com", false),
            ("https://x.push.apple.com.evil.com/", false),
            ("https://user@fcm.googleapis.com/", false),
            ("https://fcm.googleapis.com:8443/", false),
        ] {
            assert_eq!(push_host_ok(u), want, "{u}");
        }
    }

    /// A key Go wrote reads back, and the JWT it signs checks against its
    /// public key (server_test.go TestVapidSignature).
    #[test]
    fn vapid_signs_and_keeps_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-push.json");
        let p = Push::load_at(path.clone(), "", Box::new(|_, _, _| Ok(201))).unwrap();
        let jwt = p.vapid("https://fcm.googleapis.com/fcm/send/x").unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value = serde_json::from_slice(&d(parts[1])).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["sub"], "mailto:rush@localhost");
        let pubkey = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_FIXED,
            d(&p.public_key()),
        );
        pubkey
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &d(parts[2]),
            )
            .unwrap();
        // Read again, it's the same key.
        let again = Push::load_at(path, "", Box::new(|_, _, _| Ok(201))).unwrap();
        assert_eq!(again.public_key(), p.public_key());
        let f = std::fs::read_to_string(dir.path().join("remote-push.json")).unwrap();
        assert!(f.contains("-----BEGIN PRIVATE KEY-----"), "{f}");
    }

    /// The key Go's serve made reads, so a Rust serve keeps every phone's
    /// subscription (made by Go 1.27's MarshalPKCS8PrivateKey).
    #[test]
    fn reads_gos_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-push.json");
        let pem = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgdcSwSNJZiHTiVhUY\nxnKwHNG5LU8/IDdojODhwE6p65GhRANCAAQUO4CjVsK1HBOSMlz4719ChheK7spH\nOhhdyKuUpg8H/4B/lxaSxgNUO7HlZNylP9eQNA1JbU5fh/4lqDJX3YQh\n-----END PRIVATE KEY-----\n";
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({ "key": pem, "subscriptions": null })).unwrap(),
        )
        .unwrap();
        let p = Push::load_at(path, "", Box::new(|_, _, _| Ok(201))).unwrap();
        assert_eq!(p.pkcs8, pem_decode(pem).unwrap());
        assert_eq!(pem_encode(&p.pkcs8), pem);
    }

    /// A subscription is sent an encrypted body the browser can open, and
    /// one the push service says is gone is dropped.
    #[test]
    fn sends_and_drops_the_gone() {
        use ring::agreement::{ECDH_P256, EphemeralPrivateKey};
        let dir = tempfile::tempdir().unwrap();
        type Sent = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
        let seen: Sent = Arc::default();
        let s2 = seen.clone();
        let p = Push::load_at(
            dir.path().join("remote-push.json"),
            "",
            Box::new(move |url, h, body| {
                assert!(
                    h.iter()
                        .any(|(k, v)| k == "Content-Encoding" && v == "aes128gcm")
                );
                s2.lock().unwrap().push((url.into(), body));
                Ok(if url.ends_with("/gone") { 410 } else { 201 })
            }),
        )
        .unwrap();
        let rng = SystemRandom::new();
        let ua = EphemeralPrivateKey::generate(&ECDH_P256, &rng).unwrap();
        let ua_pub = ua.compute_public_key().unwrap().as_ref().to_vec();
        let auth = rand_bytes(16);
        let sub = |e: &str| Subscription {
            endpoint: format!("https://fcm.googleapis.com/{e}"),
            keys: Keys {
                p256dh: B64.encode(&ua_pub),
                auth: B64.encode(&auth),
            },
        };
        assert!(
            p.add(Subscription {
                endpoint: "https://evil.com/x".into(),
                ..sub("x")
            })
            .is_err()
        );
        p.add(sub("live")).unwrap();
        p.add(sub("gone")).unwrap();
        p.add(sub("live")).unwrap();
        assert_eq!(p.subscribed(), 2);
        p.send(&Notice {
            title: "t".into(),
            body: "b".into(),
            url: "/".into(),
            tag: "x".into(),
        });
        assert_eq!(p.subscribed(), 1);
        // The browser opens the first with its own key.
        let (_, body) = seen.lock().unwrap()[0].clone();
        let salt = &body[..16];
        let as_pub = &body[21..86];
        let secret = ring::agreement::agree_ephemeral(
            ua,
            &ring::agreement::UnparsedPublicKey::new(&ECDH_P256, as_pub),
            |k| k.to_vec(),
        )
        .unwrap();
        let mut info = b"WebPush: info\x00".to_vec();
        info.extend_from_slice(&ua_pub);
        info.extend_from_slice(as_pub);
        let ikm = hkdf(&auth, &secret, &info, 32);
        let cek = hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\x00", 16);
        let nonce = hkdf(salt, &ikm, b"Content-Encoding: nonce\x00", 12);
        use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
        let key = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).unwrap());
        let mut rec = body[86..].to_vec();
        let plain = key
            .open_in_place(
                Nonce::try_assume_unique_for_key(&nonce).unwrap(),
                Aad::empty(),
                &mut rec,
            )
            .unwrap();
        assert_eq!(
            plain,
            br#"{"title":"t","body":"b","url":"/","tag":"x"}"#
                .iter()
                .copied()
                .chain([2])
                .collect::<Vec<u8>>()
        );
    }
}
