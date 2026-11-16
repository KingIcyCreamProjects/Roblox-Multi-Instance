//! File layout: `MAGIC(16) | u32 LE header len | header JSON | ciphertext`.
//! The header JSON bytes are the payload AAD, so any header edit breaks decryption.

use super::{crypto::KdfParams, Result, VaultError};
use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8; 16] = b"RMIVAULT\0\0\0\0v001";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// DK wrapped by the OS store only.
    Os,
    /// DK wrapped by Argon2id KEK, then by the OS store.
    Password,
    /// Export: DK wrapped by Argon2id KEK only, opens on any machine.
    Portable,
}

/// Binary fields are standard base64.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WrappedKey {
    /// Wrapped DK blob. OS layer applied in `os`/`password` modes, absent in `portable`.
    pub os: String,
    pub pw_nonce: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Header {
    pub version: u32,
    pub mode: Mode,
    pub kdf: Option<KdfParams>,
    pub salt: Option<String>,
    pub nonce: String,
    pub wrap: WrappedKey,
}

pub fn encode(header_json: &[u8], ct: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + header_json.len() + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
    out.extend_from_slice(header_json);
    out.extend_from_slice(ct);
    out
}

/// Returns the parsed header, its exact bytes (the AAD) and the ciphertext.
pub fn decode(bytes: &[u8]) -> Result<(Header, &[u8], &[u8])> {
    if bytes.len() < 20 || &bytes[..16] != MAGIC {
        return Err(VaultError::Corrupt);
    }
    let len = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
    let hb = bytes.get(20..20 + len).ok_or(VaultError::Corrupt)?;
    let h: Header = serde_json::from_slice(hb).map_err(|_| VaultError::Corrupt)?;
    if h.version != 1 {
        return Err(VaultError::Corrupt);
    }
    Ok((h, hb, &bytes[20 + len..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            version: 1,
            mode: Mode::Os,
            kdf: None,
            salt: None,
            nonce: "AAAA".into(),
            wrap: WrappedKey {
                os: "AAAA".into(),
                pw_nonce: None,
            },
        }
    }

    #[test]
    fn magic() {
        assert_eq!(MAGIC.len(), 16);
        assert_eq!(MAGIC, b"RMIVAULT\0\0\0\0v001");
    }

    #[test]
    fn round_trip() {
        let hj = serde_json::to_vec(&header()).unwrap();
        let file = encode(&hj, b"ct");
        let (h, hb, ct) = decode(&file).unwrap();
        assert_eq!(h.mode, Mode::Os);
        assert_eq!(hb, &hj[..]);
        assert_eq!(ct, b"ct");
    }

    #[test]
    fn rejects_bad_files() {
        let hj = serde_json::to_vec(&header()).unwrap();
        let good = encode(&hj, b"ct");
        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert!(matches!(decode(&bad_magic), Err(VaultError::Corrupt)));
        assert!(matches!(decode(&good[..18]), Err(VaultError::Corrupt)));
        assert!(matches!(decode(&good[..25]), Err(VaultError::Corrupt)));
        let mut long = good.clone();
        long[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(decode(&long), Err(VaultError::Corrupt)));
        let mut v2 = header();
        v2.version = 2;
        let f = encode(&serde_json::to_vec(&v2).unwrap(), b"ct");
        assert!(matches!(decode(&f), Err(VaultError::Corrupt)));
    }

    #[test]
    fn garbage_or_empty_header_is_corrupt() {
        let mut junk = MAGIC.to_vec();
        junk.extend_from_slice(&2u32.to_le_bytes());
        junk.extend_from_slice(b"{x");
        assert!(matches!(decode(&junk), Err(VaultError::Corrupt)));
        assert!(matches!(
            decode(&encode(b"", b"ct")),
            Err(VaultError::Corrupt)
        ));
    }
}
