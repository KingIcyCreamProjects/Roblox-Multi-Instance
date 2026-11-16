//! OS secret store wrap: DPAPI CurrentUser on Windows, keyring device key elsewhere.

use super::{Result, VaultError};
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

pub trait OsWrap: Send + Sync {
    fn wrap(&self, data: &[u8]) -> Result<Vec<u8>>;
    fn unwrap(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>>;
}

#[cfg(windows)]
pub fn system(_data_dir: &Path) -> Result<Arc<dyn OsWrap>> {
    Ok(Arc::new(Dpapi))
}

#[cfg(not(windows))]
pub fn system(data_dir: &Path) -> Result<Arc<dyn OsWrap>> {
    Ok(Arc::new(DeviceKey::load(data_dir, &Keyring)?))
}

#[cfg(windows)]
pub struct Dpapi;

#[cfg(windows)]
impl OsWrap for Dpapi {
    fn wrap(&self, data: &[u8]) -> Result<Vec<u8>> {
        dpapi(data, true).map(|z| z.to_vec())
    }
    fn unwrap(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        dpapi(blob, false)
    }
}

#[cfg(windows)]
fn dpapi(data: &[u8], protect: bool) -> Result<Zeroizing<Vec<u8>>> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: input points at `data` for the call; DPAPI allocates `out` with LocalAlloc,
    // which we copy, wipe and free exactly once.
    unsafe {
        let r = if protect {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        r.map_err(|e| VaultError::OsWrap(e.to_string()))?;
        let v =
            Zeroizing::new(std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec());
        std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
        Ok(v)
    }
}

/// Where the device key lives. `Ok(None)` = no entry yet.
pub trait KeyStore {
    fn get(&self) -> std::result::Result<Option<Zeroizing<String>>, String>;
    fn set(&self, v: &str) -> std::result::Result<(), String>;
}

#[cfg(not(windows))]
struct Keyring;

#[cfg(not(windows))]
impl Keyring {
    fn entry() -> std::result::Result<keyring::Entry, String> {
        keyring::Entry::new("com.kingicycreamprojects.robloxmultiinstance", "device-key")
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(windows))]
impl KeyStore for Keyring {
    fn get(&self) -> std::result::Result<Option<Zeroizing<String>>, String> {
        match Self::entry()?.get_password() {
            Ok(s) => Ok(Some(Zeroizing::new(s))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set(&self, v: &str) -> std::result::Result<(), String> {
        Self::entry()?.set_password(v).map_err(|e| e.to_string())
    }
}

/// Random 32-byte device key (keyring, or `device.key` fallback), used with XChaCha.
/// Used on mac/linux; platform-neutral so its logic is tested everywhere.
pub struct DeviceKey(Zeroizing<[u8; 32]>);

impl DeviceKey {
    /// `device.key` wins if present. A new key is only minted while no vault exists,
    /// so a flaky keyring can never strand an existing vault behind a fresh key.
    pub fn load(data_dir: &Path, ks: &dyn KeyStore) -> Result<Self> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let parse = |s: &str| -> Option<Self> {
            let v = Zeroizing::new(STANDARD.decode(s.trim()).ok()?);
            Some(Self(Zeroizing::new(v.as_slice().try_into().ok()?)))
        };
        let file = data_dir.join("device.key");
        match std::fs::read_to_string(&file) {
            Ok(s) => {
                let s = Zeroizing::new(s);
                return parse(&s).ok_or(VaultError::DeviceKey("The device.key file is damaged."));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let got = ks.get();
        if let Ok(Some(s)) = &got {
            return parse(s).ok_or(VaultError::DeviceKey(
                "The device key in the OS keyring is damaged.",
            ));
        }
        // A portable export (moved from another computer) needs no old key: mint one for
        // its re-wrap on unlock.
        let vault = data_dir.join("vault.rmiv");
        let portable = std::fs::read(&vault).is_ok_and(|b| {
            super::format::decode(&b).is_ok_and(|(h, _, _)| h.mode == super::format::Mode::Portable)
        });
        if vault.is_file() && !portable {
            return Err(VaultError::DeviceKey(if got.is_err() {
                "The OS keyring is unavailable, so the vault can't be opened. Unlock the keyring and try again."
            } else {
                "This device's vault key is missing from the OS keyring. Import an exported vault instead."
            }));
        }
        let k = Self(Zeroizing::new(super::crypto::random::<32>()));
        let b64 = Zeroizing::new(STANDARD.encode(*k.0));
        let stored = match got {
            Ok(_) => ks.set(&b64),
            Err(e) => Err(e),
        };
        if let Err(why) = stored {
            log::warn!("OS keyring unavailable ({why}); using device.key file in data dir");
            // tmp (0600) + fsync + rename: never a truncated key file
            super::store::write_atomic(&file, b64.as_bytes(), super::store::Backup::Skip)?;
        }
        Ok(k)
    }
}

impl OsWrap for DeviceKey {
    fn wrap(&self, data: &[u8]) -> Result<Vec<u8>> {
        let (n, ct) = super::crypto::seal(&self.0, data, b"rmi-device");
        Ok([&n[..], &ct].concat())
    }
    fn unwrap(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if blob.len() < 24 {
            return Err(VaultError::Corrupt);
        }
        super::crypto::open(
            &self.0,
            blob[..24].try_into().unwrap(),
            &blob[24..],
            b"rmi-device",
        )
    }
}

/// Test stand-in: XOR with a constant.
#[cfg(test)]
pub struct FakeOsWrap;

#[cfg(test)]
impl OsWrap for FakeOsWrap {
    fn wrap(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.iter().map(|b| b ^ 0x5A).collect())
    }
    fn unwrap(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        Ok(Zeroizing::new(blob.iter().map(|b| b ^ 0x5A).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// `None` in `val` = no entry; `fail` = keyring down.
    struct FakeKs {
        val: RefCell<Option<String>>,
        fail: bool,
    }

    impl FakeKs {
        fn new(fail: bool) -> Self {
            Self {
                val: RefCell::new(None),
                fail,
            }
        }
    }

    impl KeyStore for FakeKs {
        fn get(&self) -> std::result::Result<Option<Zeroizing<String>>, String> {
            if self.fail {
                return Err("down".into());
            }
            Ok(self.val.borrow().clone().map(Zeroizing::new))
        }
        fn set(&self, v: &str) -> std::result::Result<(), String> {
            if self.fail {
                return Err("down".into());
            }
            *self.val.borrow_mut() = Some(v.into());
            Ok(())
        }
    }

    fn rt(a: &DeviceKey, b: &DeviceKey) -> bool {
        b.unwrap(&a.wrap(b"x").unwrap()).is_ok()
    }

    #[test]
    fn keyring_key_is_stable() {
        let d = tempfile::tempdir().unwrap();
        let ks = FakeKs::new(false);
        let a = DeviceKey::load(d.path(), &ks).unwrap();
        assert!(ks.val.borrow().is_some() && !d.path().join("device.key").exists());
        std::fs::write(d.path().join("vault.rmiv"), b"v").unwrap();
        assert!(rt(&a, &DeviceKey::load(d.path(), &ks).unwrap()));
    }

    #[test]
    fn flaky_keyring_never_strands_vault() {
        let d = tempfile::tempdir().unwrap();
        // first run: keyring down, falls back to device.key
        let a = DeviceKey::load(d.path(), &FakeKs::new(true)).unwrap();
        assert!(d.path().join("device.key").exists());
        std::fs::write(d.path().join("vault.rmiv"), b"v").unwrap();
        // later the keyring works but has no entry: device.key still wins
        let ks = FakeKs::new(false);
        assert!(rt(&a, &DeviceKey::load(d.path(), &ks).unwrap()));
        assert!(ks.val.borrow().is_none());
    }

    #[test]
    fn no_new_key_while_vault_exists() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("vault.rmiv"), b"v").unwrap();
        for fail in [false, true] {
            let ks = FakeKs::new(fail);
            assert!(matches!(
                DeviceKey::load(d.path(), &ks),
                Err(VaultError::DeviceKey(_))
            ));
            assert!(ks.val.borrow().is_none());
            assert!(!d.path().join("device.key").exists());
        }
    }

    /// A portable export dropped in as the vault (moving from another computer) doesn't use
    /// the device key, so a fresh key may be minted for its re-wrap.
    #[test]
    fn new_key_for_portable_vault() {
        let d = tempfile::tempdir().unwrap();
        let h = br#"{"version":1,"mode":"portable","kdf":null,"salt":null,"nonce":"","wrap":{"os":"","pw_nonce":null}}"#;
        std::fs::write(
            d.path().join("vault.rmiv"),
            super::super::format::encode(h, b"ct"),
        )
        .unwrap();
        let ks = FakeKs::new(false);
        assert!(DeviceKey::load(d.path(), &ks).is_ok());
        assert!(ks.val.borrow().is_some());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip() {
        let blob = Dpapi.wrap(b"device secret").unwrap();
        assert_ne!(&blob[..], b"device secret");
        assert_eq!(&Dpapi.unwrap(&blob).unwrap()[..], b"device secret");
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(Dpapi.unwrap(&bad).is_err());
    }

    #[test]
    fn wrapped_blob_opens_only_with_its_own_key() {
        let a = DeviceKey(Zeroizing::new(super::super::crypto::random::<32>()));
        let other = DeviceKey(Zeroizing::new(super::super::crypto::random::<32>()));
        let blob = a.wrap(b"x").unwrap();
        assert!(other.unwrap(&blob).is_err());
        assert!(matches!(a.unwrap(&[0u8; 23]), Err(VaultError::Corrupt)));
    }

    #[test]
    fn damaged_key_file_is_reported_not_replaced() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("device.key"), "not base64!!").unwrap();
        let ks = FakeKs::new(false);
        assert!(matches!(
            DeviceKey::load(d.path(), &ks),
            Err(VaultError::DeviceKey(m)) if m.contains("damaged")
        ));
        assert!(ks.val.borrow().is_none());
    }

    #[test]
    fn damaged_keyring_entry_is_reported() {
        let d = tempfile::tempdir().unwrap();
        let ks = FakeKs::new(false);
        // base64 of "short": decodes, but not 32 bytes
        *ks.val.borrow_mut() = Some("c2hvcnQ=".into());
        assert!(matches!(
            DeviceKey::load(d.path(), &ks),
            Err(VaultError::DeviceKey(m)) if m.contains("OS keyring")
        ));
        assert!(!d.path().join("device.key").exists());
    }

    #[test]
    fn vault_without_device_key_says_why() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("vault.rmiv"), b"v").unwrap();
        assert!(matches!(
            DeviceKey::load(d.path(), &FakeKs::new(true)),
            Err(VaultError::DeviceKey(m)) if m.contains("keyring is unavailable")
        ));
        assert!(matches!(
            DeviceKey::load(d.path(), &FakeKs::new(false)),
            Err(VaultError::DeviceKey(m)) if m.contains("missing from the OS keyring")
        ));
    }
}
