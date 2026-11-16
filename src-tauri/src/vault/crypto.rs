use super::{Result, VaultError};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{rand_core::RngCore, Aead, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    pub const DEFAULT: Self = Self {
        m_kib: 262144,
        t: 3,
        p: 1,
    };
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

pub fn seal_with(key: &[u8; 32], nonce: &[u8; 24], pt: &[u8], aad: &[u8]) -> Vec<u8> {
    XChaCha20Poly1305::new(key.into())
        .encrypt(XNonce::from_slice(nonce), Payload { msg: pt, aad })
        .expect("xchacha20poly1305 encrypt cannot fail for in-memory buffers")
}

pub fn seal(key: &[u8; 32], pt: &[u8], aad: &[u8]) -> ([u8; 24], Vec<u8>) {
    let nonce = random::<24>();
    (nonce, seal_with(key, &nonce, pt, aad))
}

/// Any failure (wrong key, tampered ct or aad) is `Corrupt`; callers remap to `WrongPassword`.
pub fn open(key: &[u8; 32], nonce: &[u8; 24], ct: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| VaultError::Corrupt)
}

/// Params come from a file header, so cap them: a crafted file must not OOM or hang us.
pub fn derive_kek(pw: &str, salt: &[u8; 16], p: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    if p.m_kib > 2 * KdfParams::DEFAULT.m_kib || p.t > 8 || p.p > 4 {
        return Err(VaultError::Corrupt);
    }
    let params = Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|_| VaultError::Corrupt)?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pw.as_bytes(), salt, &mut out[..])
        .map_err(|_| VaultError::Corrupt)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const TEST_KDF: KdfParams = KdfParams {
        m_kib: 64,
        t: 1,
        p: 1,
    };

    #[test]
    fn seal_open_round_trip() {
        let k = random::<32>();
        let (n, ct) = seal(&k, b"hello", b"aad");
        assert_eq!(&open(&k, &n, &ct, b"aad").unwrap()[..], b"hello");
    }

    #[test]
    fn tamper_fails() {
        let k = random::<32>();
        let (n, mut ct) = seal(&k, b"hello", b"aad");
        assert!(open(&k, &n, &ct, b"aaX").is_err());
        assert!(open(&random::<32>(), &n, &ct, b"aad").is_err());
        ct[0] ^= 1;
        assert!(open(&k, &n, &ct, b"aad").is_err());
    }

    #[test]
    fn kek_deterministic() {
        let s = [7u8; 16];
        let a = derive_kek("password1", &s, TEST_KDF).unwrap();
        assert_eq!(*a, *derive_kek("password1", &s, TEST_KDF).unwrap());
        assert_ne!(*a, *derive_kek("password2", &s, TEST_KDF).unwrap());
        assert_ne!(*a, *derive_kek("password1", &[8u8; 16], TEST_KDF).unwrap());
    }

    #[test]
    fn hostile_kdf_params_rejected() {
        let k = |m_kib, t, p| KdfParams { m_kib, t, p };
        let m = 2 * KdfParams::DEFAULT.m_kib;
        for bad in [k(m + 1, 1, 1), k(64, 9, 1), k(64, 1, 5), k(u32::MAX, 1, 1)] {
            assert!(matches!(
                derive_kek("x", &[0; 16], bad),
                Err(VaultError::Corrupt)
            ));
        }
        assert!(derive_kek("x", &[0; 16], k(64, 8, 4)).is_ok());
    }

    #[test]
    fn wrong_nonce_and_invalid_argon_params_rejected() {
        let k = random::<32>();
        let (mut n, ct) = seal(&k, b"hello", b"aad");
        n[0] ^= 1;
        assert!(open(&k, &n, &ct, b"aad").is_err());
        let bad = |m_kib, t, p| KdfParams { m_kib, t, p };
        // below Argon2's minimums (memory >= 8 KiB per lane, t >= 1, p >= 1)
        for b in [bad(0, 1, 1), bad(64, 0, 1), bad(64, 1, 0)] {
            assert!(matches!(
                derive_kek("x", &[0; 16], b),
                Err(VaultError::Corrupt)
            ));
        }
    }
}
