//! Signed notes (C2SP signed-note) and checkpoints (C2SP tlog-checkpoint).
//!
//! A note is a text, a blank line and signature lines `— <key name> <base64(key hash || sig)>`.
//! A verifier key is `<name>+<key hash in 8 hex digits>+<base64(0x01 || Ed25519 public key)>`,
//! where the key hash is the first 4 bytes of sha256(name + "\n" + 0x01 || public key).

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

const ALG_ED25519: u8 = 0x01;
const SIG_PREFIX: &str = "\u{2014} ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteError(pub String);

impl fmt::Display for NoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid note: {}", self.0)
    }
}

impl std::error::Error for NoteError {}

#[derive(Debug, Clone)]
pub struct Verifier {
    pub name: String,
    key_hash: u32,
    key: VerifyingKey,
}

impl Verifier {
    pub fn parse(vkey: &str) -> Result<Verifier, NoteError> {
        let err = |m: &str| NoteError(format!("verifier key: {m}"));
        let mut parts = vkey.splitn(3, '+');
        let (Some(name), Some(hash), Some(key)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(err("expected `<name>+<hash>+<key>`"));
        };
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(err("bad name"));
        }
        let raw = STANDARD.decode(key).map_err(|_| err("key is not base64"))?;
        let [alg, public @ ..] = raw.as_slice() else {
            return Err(err("empty key"));
        };
        if *alg != ALG_ED25519 {
            return Err(err("unknown algorithm"));
        }
        let public: [u8; 32] = public.try_into().map_err(|_| err("key must be 32 bytes"))?;
        let key = VerifyingKey::from_bytes(&public).map_err(|_| err("not an Ed25519 key"))?;
        let key_hash = key_hash(name, &raw);
        if hash.len() != 8 || u32::from_str_radix(hash, 16).ok() != Some(key_hash) {
            return Err(err("key hash does not match"));
        }
        Ok(Verifier {
            name: name.to_string(),
            key_hash,
            key,
        })
    }
}

/// The text of a note signed by `verifier`. Signatures by other keys are ignored.
pub fn open(signed: &str, verifier: &Verifier) -> Result<String, NoteError> {
    let split = signed
        .rfind("\n\n")
        .ok_or_else(|| NoteError("missing signature block".into()))?;
    let text = &signed[..split + 1];
    if text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(NoteError("text has control characters".into()));
    }
    let block = signed[split + 2..]
        .strip_suffix('\n')
        .ok_or_else(|| NoteError("signature block must end with a newline".into()))?;
    for line in block.split('\n') {
        let sig = line
            .strip_prefix(SIG_PREFIX)
            .ok_or_else(|| NoteError(format!("bad signature line `{line}`")))?;
        let (name, b64) = sig
            .split_once(' ')
            .ok_or_else(|| NoteError(format!("bad signature line `{line}`")))?;
        if name != verifier.name {
            continue;
        }
        let Ok(blob) = STANDARD.decode(b64) else {
            continue;
        };
        let Some((hash, sig)) = blob.split_first_chunk::<4>() else {
            continue;
        };
        if u32::from_be_bytes(*hash) != verifier.key_hash {
            continue;
        }
        let Ok(sig) = Signature::from_slice(sig) else {
            continue;
        };
        if verifier.key.verify_strict(text.as_bytes(), &sig).is_ok() {
            return Ok(text.to_string());
        }
    }
    Err(NoteError(format!(
        "no valid signature by `{}`",
        verifier.name
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub origin: String,
    pub size: u64,
    pub root: [u8; 32],
}

impl Checkpoint {
    /// The text of a checkpoint: origin, tree size and base64 root on three lines. Lines after them
    /// are extensions and are ignored.
    pub fn parse(text: &str) -> Result<Checkpoint, NoteError> {
        let err = || NoteError("bad checkpoint".into());
        let body = text.strip_suffix('\n').ok_or_else(err)?;
        let mut lines = body.split('\n');
        let (Some(origin), Some(size), Some(root)) = (lines.next(), lines.next(), lines.next())
        else {
            return Err(err());
        };
        if origin.is_empty() || size.is_empty() || (size.len() > 1 && size.starts_with('0')) {
            return Err(err());
        }
        let size = size.parse().map_err(|_| err())?;
        let root = STANDARD
            .decode(root)
            .ok()
            .and_then(|r| <[u8; 32]>::try_from(r).ok())
            .ok_or_else(err)?;
        Ok(Checkpoint {
            origin: origin.to_string(),
            size,
            root,
        })
    }

    pub fn to_text(&self) -> String {
        format!(
            "{}\n{}\n{}\n",
            self.origin,
            self.size,
            STANDARD.encode(self.root)
        )
    }
}

fn key_hash(name: &str, alg_and_key: &[u8]) -> u32 {
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.update(b"\n");
    h.update(alg_and_key);
    let d = h.finalize();
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}
