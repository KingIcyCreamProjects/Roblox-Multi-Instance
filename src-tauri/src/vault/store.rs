use super::crypto::{self, KdfParams};
use super::format::{self, Header, Mode, WrappedKey};
use super::model::VaultPayload;
use super::oswrap::OsWrap;
use super::{Result, VaultError};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{NaiveDate, NaiveDateTime};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zeroize::Zeroizing;

/// KDF cost for new wraps. Tests use cheap params; real files carry theirs in the header.
#[cfg(not(test))]
pub const NEW_KDF: KdfParams = KdfParams::DEFAULT;
#[cfg(test)]
pub const NEW_KDF: KdfParams = KdfParams {
    m_kib: 64,
    t: 1,
    p: 1,
};

const DK_AAD: &[u8] = b"rmi-dk";
const BACKUP_FMT: &str = "%Y%m%d-%H%M%S";

pub enum CreateMode {
    Os,
    Password(Zeroizing<String>),
}

pub struct Vault {
    path: PathBuf,
    /// Key material only; `nonce` is refreshed on every save.
    header: Header,
    dk: Zeroizing<[u8; 32]>,
    payload: VaultPayload,
    os: Arc<dyn OsWrap>,
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.payload.wipe();
    }
}

fn b64d(s: &str) -> Result<Zeroizing<Vec<u8>>> {
    B64.decode(s)
        .map(Zeroizing::new)
        .map_err(|_| VaultError::Corrupt)
}

fn arr<const N: usize>(v: &[u8]) -> Result<[u8; N]> {
    v.try_into().map_err(|_| VaultError::Corrupt)
}

/// Wrap DK for `mode`. Password/Portable need `pw`.
fn wrap_dk(dk: &[u8; 32], mode: Mode, pw: Option<&str>, os: &dyn OsWrap) -> Result<Header> {
    let (kdf, salt, wrap) = match mode {
        Mode::Os => (
            None,
            None,
            WrappedKey {
                os: B64.encode(os.wrap(dk)?),
                pw_nonce: None,
            },
        ),
        Mode::Password | Mode::Portable => {
            let pw = pw.ok_or(VaultError::WrongPassword)?;
            let salt = crypto::random::<16>();
            let kek = crypto::derive_kek(pw, &salt, NEW_KDF)?;
            let (n, ct) = crypto::seal(&kek, dk, DK_AAD);
            let blob = if mode == Mode::Password {
                os.wrap(&ct)?
            } else {
                ct
            };
            (
                Some(NEW_KDF),
                Some(B64.encode(salt)),
                WrappedKey {
                    os: B64.encode(blob),
                    pw_nonce: Some(B64.encode(n)),
                },
            )
        }
    };
    Ok(Header {
        version: 1,
        mode,
        kdf,
        salt,
        nonce: String::new(),
        wrap,
    })
}

fn unwrap_dk(h: &Header, pw: Option<&str>, os: &dyn OsWrap) -> Result<Zeroizing<[u8; 32]>> {
    let blob = b64d(&h.wrap.os)?;
    let dk = match h.mode {
        Mode::Os => os.unwrap(&blob)?,
        Mode::Password | Mode::Portable => {
            let pw = pw.ok_or(VaultError::WrongPassword)?;
            let inner = if h.mode == Mode::Password {
                os.unwrap(&blob)?
            } else {
                blob
            };
            let salt = arr(&b64d(h.salt.as_deref().ok_or(VaultError::Corrupt)?)?)?;
            let nonce = arr(&b64d(
                h.wrap.pw_nonce.as_deref().ok_or(VaultError::Corrupt)?,
            )?)?;
            let kek = crypto::derive_kek(pw, &salt, h.kdf.ok_or(VaultError::Corrupt)?)?;
            crypto::open(&kek, &nonce, &inner, DK_AAD).map_err(|_| VaultError::WrongPassword)?
        }
    };
    let mut out = Zeroizing::new([0u8; 32]);
    if dk.len() != 32 {
        return Err(VaultError::Corrupt);
    }
    out.copy_from_slice(&dk);
    Ok(out)
}

fn seal_file(mut header: Header, dk: &[u8; 32], payload: &VaultPayload) -> Vec<u8> {
    let nonce = crypto::random::<24>();
    header.nonce = B64.encode(nonce);
    let hj = serde_json::to_vec(&header).expect("header serializes");
    let pt = Zeroizing::new(serde_json::to_vec(payload).expect("payload serializes"));
    format::encode(&hj, &crypto::seal_with(dk, &nonce, &pt, &hj))
}

/// Read + decrypt; never writes.
fn load(
    path: &Path,
    os: Arc<dyn OsWrap>,
    get_dk: impl FnOnce(&Header, &dyn OsWrap) -> Result<Zeroizing<[u8; 32]>>,
) -> Result<Vault> {
    let bytes = fs::read(path)?;
    let (header, hb, ct) = format::decode(&bytes)?;
    let dk = get_dk(&header, &*os)?;
    let nonce = arr(&b64d(&header.nonce)?)?;
    let pt = crypto::open(&dk, &nonce, ct, hb)?;
    let payload = serde_json::from_slice(&pt).map_err(|_| VaultError::Corrupt)?;
    Ok(Vault {
        path: path.to_owned(),
        header,
        dk,
        payload,
        os,
    })
}

/// What `write_atomic` does with an existing target before replacing it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Backup {
    Skip,
    /// A failed backup is logged and the save goes on.
    Try,
    /// Routine save: like `Try`, but no new backup when the newest is < 5 min old.
    Throttled,
    /// Destructive replace (import, restore, password rewrap): no backup, no write.
    Require,
}

static TMP_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Unique tmp (0600 on unix) + fsync + rename (retried, Windows AV/indexers lock files)
/// + parent fsync on unix. The tmp file is removed on any failure.
pub fn write_atomic(path: &Path, bytes: &[u8], backup: Backup) -> Result<()> {
    use std::io::Write;
    sweep_tmp(path);
    let n = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{n}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let r = (|| -> Result<()> {
        let mut o = fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        let skip = backup == Backup::Skip
            || (backup == Backup::Throttled && recent_backup(path, BACKUP_EVERY_SECS));
        if !skip && path.is_file() {
            if let Err(e) = self::backup(path) {
                if backup == Backup::Require {
                    log::error!("vault backup failed, aborting replace: {e}");
                    return Err(VaultError::Refused(
                        "Could not back up the current vault, so nothing was changed.",
                    ));
                }
                // ponytail: routine saves keep going; the new data matters more.
                log::warn!("vault backup failed: {e}");
            }
        }
        let mut r = fs::rename(&tmp, path);
        for _ in 0..2 {
            if r.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            r = fs::rename(&tmp, path);
        }
        r?;
        #[cfg(unix)]
        if let Some(dir) = path.parent() {
            fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    })();
    if r.is_err() {
        if let Err(e) = fs::remove_file(&tmp) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("could not remove {}: {e}", tmp.display());
            }
        }
    }
    r
}

/// Remove `<name>.<pid>-<n>.tmp` siblings of `path` a crashed earlier run left behind (a
/// plaintext export tmp only when exporting to that same path again). Other pids only: ours
/// may be in flight; the app is single-instance.
fn sweep_tmp(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let pid = std::process::id().to_string();
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let stale = n
            .strip_prefix(&prefix)
            .and_then(|r| r.strip_suffix(".tmp"))
            .and_then(|r| r.split_once('-'))
            .is_some_and(|(p, k)| p != pid && p.parse::<u32>().is_ok() && k.parse::<u32>().is_ok());
        if stale {
            if let Err(err) = fs::remove_file(e.path()) {
                log::warn!("could not remove stale {n}: {err}");
            }
        }
    }
}

const BACKUP_EVERY_SECS: i64 = 300;

/// A backup newer than `secs` exists (by its UTC name stamp).
fn recent_backup(path: &Path, secs: i64) -> bool {
    let now = chrono::Utc::now().naive_utc();
    fs::read_dir(backup_dir(path)).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            parse_backup_name(&e.file_name().to_string_lossy())
                .is_some_and(|(t, _)| (0..secs).contains(&(now - t).num_seconds()))
        })
    })
}

pub fn backup_dir(vault_path: &Path) -> PathBuf {
    vault_path.with_file_name("backups")
}

/// Backup sort key: UTC time + same-second counter.
pub type BackupKey = (NaiveDateTime, u32);

/// `vault-YYYYMMDD-HHMMSS.rmiv` or `vault-YYYYMMDD-HHMMSS-N.rmiv` (UTC).
pub fn parse_backup_name(name: &str) -> Option<BackupKey> {
    let s = name.strip_prefix("vault-")?.strip_suffix(".rmiv")?;
    let t = NaiveDateTime::parse_from_str(s.get(..15)?, BACKUP_FMT).ok()?;
    let n = match s.get(15..)? {
        "" => 0,
        rest => {
            let d = rest.strip_prefix('-')?;
            if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            d.parse().ok()?
        }
    };
    Some((t, n))
}

/// Newest 10, plus the newest of each of the last 7 days (today included).
pub fn backups_to_keep(keys: &[BackupKey], today: NaiveDate) -> BTreeSet<BackupKey> {
    let mut s = keys.to_vec();
    s.sort_unstable_by(|a, b| b.cmp(a));
    let mut keep: BTreeSet<_> = s.iter().take(10).copied().collect();
    let mut days = BTreeSet::new();
    for k in &s {
        let age = (today - k.0.date()).num_days();
        if (0..7).contains(&age) && days.insert(k.0.date()) {
            keep.insert(*k);
        }
    }
    keep
}

fn backup(path: &Path) -> Result<()> {
    let dir = backup_dir(path);
    fs::create_dir_all(&dir)?;
    let now = chrono::Utc::now().naive_utc();
    let stamp = now.format(BACKUP_FMT).to_string();
    let mut n = 0;
    let (mut out, file) = loop {
        let name = match n {
            0 => format!("vault-{stamp}.rmiv"),
            _ => format!("vault-{stamp}-{n}.rmiv"),
        };
        let file = dir.join(name);
        let mut o = fs::OpenOptions::new();
        o.write(true).create_new(true);
        // Same 0600 as `write_atomic`: a backup is the vault too.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        match o.open(&file) {
            Ok(f) => break (f, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => n += 1,
            Err(e) => return Err(e.into()),
        }
    };
    let copied = fs::File::open(path)
        .and_then(|mut src| std::io::copy(&mut src, &mut out))
        .and_then(|_| out.sync_all());
    if let Err(e) = copied {
        // A partial file would pass as a recent backup and count toward pruning.
        drop(out);
        let _ = fs::remove_file(&file);
        return Err(e.into());
    }
    let names: Vec<(String, BackupKey)> = fs::read_dir(&dir)?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().into_string().ok()?;
            parse_backup_name(&n).map(|k| (n, k))
        })
        .collect();
    let keys: Vec<_> = names.iter().map(|(_, k)| *k).collect();
    let keep = backups_to_keep(&keys, now.date());
    for (n, k) in &names {
        if !keep.contains(k) {
            if let Err(e) = fs::remove_file(dir.join(n)) {
                log::warn!("could not prune backup {n}: {e}");
            }
        }
    }
    Ok(())
}

/// Key material copied out of an unlocked vault, so Argon2 can run without holding
/// the app's vault mutex. Re-wraps are applied back with [`Vault::apply_rewrap`].
pub struct Keys {
    header: Header,
    dk: Zeroizing<[u8; 32]>,
    os: Arc<dyn OsWrap>,
}

impl Keys {
    pub fn mode(&self) -> Mode {
        self.header.mode
    }

    pub fn dk(&self) -> &[u8; 32] {
        &self.dk
    }

    /// Re-auth check (Password mode only; others give `Ok(false)`). AEAD unwrap success
    /// is the comparison. Only a wrong password is `Ok(false)`; other failures are errors.
    pub fn verify_password(&self, pw: &str) -> Result<bool> {
        if self.header.mode != Mode::Password {
            return Ok(false);
        }
        match unwrap_dk(&self.header, Some(pw), &*self.os) {
            Ok(_) => Ok(true),
            Err(VaultError::WrongPassword) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// New header wrapping the same DK for `new` (`None` = OS mode). Runs Argon2.
    pub fn rewrap(&self, new: Option<&str>) -> Result<Header> {
        let mode = if new.is_some() {
            Mode::Password
        } else {
            Mode::Os
        };
        wrap_dk(&self.dk, mode, new, &*self.os)
    }
}

/// Fresh DK + password-only header for a portable export. Runs Argon2.
pub fn portable_wrap(pw: &str, os: &dyn OsWrap) -> Result<(Header, Zeroizing<[u8; 32]>)> {
    let dk = Zeroizing::new(crypto::random::<32>());
    Ok((wrap_dk(&dk, Mode::Portable, Some(pw), os)?, dk))
}

const STALE: &str = "The vault changed, try again.";
const RESTORE_KEY: &str =
    "This backup can't be opened with the current vault key. It is damaged or from an older vault.";

impl Vault {
    /// A non-empty file. 0 bytes is `create`'s placeholder after a crash before its rename (no
    /// vault, first run can create one again), unless backups exist: then it is a damaged vault,
    /// so Recovery shows instead of a new DK that no backup opens with.
    pub fn exists(path: &Path) -> bool {
        fs::metadata(path).is_ok_and(|m| {
            m.is_file()
                && (m.len() > 0
                    || fs::read_dir(backup_dir(path)).is_ok_and(|mut d| d.next().is_some()))
        })
    }

    /// Fails with `Io(AlreadyExists)` if a vault is already there (claimed with `create_new`).
    pub fn create(path: &Path, mode: CreateMode, os: Arc<dyn OsWrap>) -> Result<Vault> {
        let dk = Zeroizing::new(crypto::random::<32>());
        let header = match &mode {
            CreateMode::Os => wrap_dk(&dk, Mode::Os, None, &*os)?,
            CreateMode::Password(pw) => wrap_dk(&dk, Mode::Password, Some(pw), &*os)?,
        };
        if fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == 0) {
            // Crash leftover. Never a live create's placeholder: `vault_create` is single
            // flight (VAULT_BUSY).
            fs::remove_file(path)?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        // Backups left by an earlier vault whose file is gone: move them aside so this vault's
        // pruning never deletes them (its new key can't restore them; they stay importable).
        let old = backup_dir(path);
        let has_old = fs::read_dir(&old).is_ok_and(|rd| {
            rd.flatten()
                .any(|e| parse_backup_name(&e.file_name().to_string_lossy()).is_some())
        });
        if has_old {
            let stamp = chrono::Utc::now().format(BACKUP_FMT);
            let aside = old.with_file_name(format!("backups-before-{stamp}"));
            // A file held open inside (an antivirus scan, say) blocks the move for a moment on
            // Windows: retry over about a second, then refuse rather than risk those backups.
            let mut r = fs::rename(&old, &aside);
            for _ in 0..5 {
                if r.is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                r = fs::rename(&old, &aside);
            }
            if let Err(e) = r {
                log::warn!("moving the old vault backups aside failed: {e}");
                let _ = fs::remove_file(path);
                return Err(VaultError::Refused(
                    "A file in the backups folder is in use. Close it and try again.",
                ));
            }
            log::warn!("old vault backups moved to {}", aside.display());
        }
        let v = Vault {
            path: path.to_owned(),
            header,
            dk,
            payload: VaultPayload::default(),
            os,
        };
        if let Err(e) = write_atomic(path, &v.sealed(), Backup::Skip) {
            let _ = fs::remove_file(path);
            return Err(e);
        }
        Ok(v)
    }

    pub fn open(path: &Path, pw: Option<&str>, os: Arc<dyn OsWrap>) -> Result<Vault> {
        load(path, os, |h, os| unwrap_dk(h, pw, os))
    }

    /// Open with an already-known DK (remember-device cache).
    pub fn open_with_dk(
        path: &Path,
        dk: Zeroizing<[u8; 32]>,
        os: Arc<dyn OsWrap>,
    ) -> Result<Vault> {
        load(path, os, |_, _| Ok(dk))
    }

    pub fn mode(&self) -> Mode {
        self.header.mode
    }

    pub fn dk(&self) -> &[u8; 32] {
        &self.dk
    }

    pub fn keys(&self) -> Keys {
        Keys {
            header: self.header.clone(),
            dk: self.dk.clone(),
            os: self.os.clone(),
        }
    }

    pub fn payload(&self) -> &VaultPayload {
        &self.payload
    }

    pub fn payload_mut(&mut self) -> &mut VaultPayload {
        &mut self.payload
    }

    fn sealed(&self) -> Vec<u8> {
        seal_file(self.header.clone(), &self.dk, &self.payload)
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(&self.path, &self.sealed(), Backup::Throttled)
    }

    /// Save `p` as the payload (a backup of the current file is required first); the
    /// in-memory payload is swapped only once the file is written.
    pub fn replace_payload(&mut self, mut p: VaultPayload) -> Result<()> {
        let bytes = seal_file(self.header.clone(), &self.dk, &p);
        if let Err(e) = write_atomic(&self.path, &bytes, Backup::Require) {
            p.wipe();
            return Err(e);
        }
        std::mem::replace(&mut self.payload, p).wipe();
        Ok(())
    }

    /// Swap in a backup's payload, keeping the current header (password, mode). The backup
    /// must decrypt with the current DK.
    pub fn restore_from(&mut self, backup: &Path) -> Result<()> {
        let b =
            Vault::open_with_dk(backup, self.dk.clone(), self.os.clone()).map_err(|e| match e {
                VaultError::Corrupt => VaultError::Refused(RESTORE_KEY),
                e => e,
            })?;
        self.replace_payload(b.payload.clone())
    }

    /// Install a header from `keys.rewrap()` and save. Refuses if the vault changed since
    /// `keys` was taken (reopened, or another password change won the race).
    pub fn apply_rewrap(&mut self, keys: &Keys, header: Header) -> Result<()> {
        let same = *self.dk == *keys.dk
            && serde_json::to_vec(&self.header).ok() == serde_json::to_vec(&keys.header).ok();
        if !same {
            return Err(VaultError::Refused(STALE));
        }
        let old = std::mem::replace(&mut self.header, header);
        // always keep a copy under the old key: no backup, no rewrap
        if let Err(e) = write_atomic(&self.path, &self.sealed(), Backup::Require) {
            self.header = old;
            return Err(e);
        }
        Ok(())
    }

    /// Re-wrap DK only (spec §5.2). `None` switches to OS mode. Saves.
    pub fn change_password(&mut self, new: Option<&str>) -> Result<()> {
        let keys = self.keys();
        let h = keys.rewrap(new)?;
        self.apply_rewrap(&keys, h)
    }

    pub fn verify_password(&self, pw: &str) -> Result<bool> {
        self.keys().verify_password(pw)
    }

    /// File bytes of this payload under a `portable_wrap` header/DK (write them unlocked).
    pub fn sealed_copy(&self, header: Header, dk: &[u8; 32]) -> Vec<u8> {
        seal_file(header, dk, &self.payload)
    }

    /// Portable copy (fresh DK, password-only wrap) for moving to another machine.
    pub fn export_portable(&self, dest: &Path, pw: &str) -> Result<()> {
        let (h, dk) = portable_wrap(pw, &*self.os)?;
        write_atomic(dest, &self.sealed_copy(h, &dk), Backup::Skip)
    }

    /// Zeroize DK and payload (`Zeroizing` + `Drop` do the work).
    pub fn lock(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::oswrap::FakeOsWrap;
    use chrono::Timelike;
    use serde_json::json;

    struct NoOs;
    impl OsWrap for NoOs {
        fn wrap(&self, _: &[u8]) -> Result<Vec<u8>> {
            Err(VaultError::OsWrap("none".into()))
        }
        fn unwrap(&self, _: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
            Err(VaultError::OsWrap("none".into()))
        }
    }

    fn os() -> Arc<dyn OsWrap> {
        Arc::new(FakeOsWrap)
    }

    fn sample(v: &mut Vault) {
        v.payload_mut()
            .accounts
            .push(serde_json::from_value(json!({"id": "a1", "name": "alt"})).unwrap());
        v.payload_mut().secrets.insert(
            "a1".into(),
            serde_json::from_value(json!({"cookie": "_|WARNING:x"})).unwrap(),
        );
        v.payload_mut().extra.insert("future".into(), json!(42));
    }

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.rmiv");
        fs::create_dir_all(d.path().join("backups")).unwrap();
        (d, p)
    }

    #[test]
    fn round_trip_both_modes() {
        for mode in [
            CreateMode::Os,
            CreateMode::Password(Zeroizing::new("hunter22".into())),
        ] {
            let (_d, p) = tmp();
            let pw = matches!(mode, CreateMode::Password(_)).then_some("hunter22");
            let mut v = Vault::create(&p, mode, os()).unwrap();
            sample(&mut v);
            v.save().unwrap();
            let want = v.payload().clone();
            let o = Vault::open(&p, pw, os()).unwrap();
            assert!(o.payload() == &want);
            assert!(!p.with_file_name("vault.rmiv.tmp").exists());
        }
    }

    #[test]
    fn crashed_create_and_tmp_leftovers_recover() {
        let (d, p) = tmp();
        fs::write(&p, b"").unwrap(); // create crashed before its rename
        assert!(!Vault::exists(&p));
        // 0 bytes with backups around: damaged, not "no vault"
        fs::write(d.path().join("backups").join("b.rmiv"), b"x").unwrap();
        assert!(Vault::exists(&p));
        fs::remove_file(d.path().join("backups").join("b.rmiv")).unwrap();
        Vault::create(&p, CreateMode::Os, os()).unwrap();
        assert!(Vault::exists(&p));
        let stale = d.path().join("vault.rmiv.1-0.tmp");
        let ours = d
            .path()
            .join(format!("vault.rmiv.{}-99.tmp", std::process::id()));
        let other = d.path().join("other.1-0.tmp");
        for f in [&stale, &ours, &other] {
            fs::write(f, b"x").unwrap();
        }
        write_atomic(&p, b"data", Backup::Skip).unwrap();
        assert!(!stale.exists() && ours.exists() && other.exists());
    }

    #[test]
    fn wrong_password_leaves_file_alone() {
        let (_d, p) = tmp();
        Vault::create(
            &p,
            CreateMode::Password(Zeroizing::new("hunter22".into())),
            os(),
        )
        .unwrap();
        let before = fs::read(&p).unwrap();
        assert!(matches!(
            Vault::open(&p, Some("nope-nope"), os()),
            Err(VaultError::WrongPassword)
        ));
        assert!(matches!(
            Vault::open(&p, None, os()),
            Err(VaultError::WrongPassword)
        ));
        assert_eq!(fs::read(&p).unwrap(), before);
    }

    #[test]
    fn truncated_is_corrupt_and_untouched() {
        let (_d, p) = tmp();
        Vault::create(&p, CreateMode::Os, os()).unwrap();
        let full = fs::read(&p).unwrap();
        fs::write(&p, &full[..full.len() - 5]).unwrap();
        assert!(matches!(
            Vault::open(&p, None, os()),
            Err(VaultError::Corrupt)
        ));
        assert_eq!(fs::read(&p).unwrap(), &full[..full.len() - 5]);
    }

    #[test]
    fn change_password_keeps_dk() {
        let (_d, p) = tmp();
        let mut v = Vault::create(
            &p,
            CreateMode::Password(Zeroizing::new("oldpass1".into())),
            os(),
        )
        .unwrap();
        sample(&mut v);
        v.save().unwrap();
        let dk = *v.dk();
        assert!(v.verify_password("oldpass1").unwrap());
        v.change_password(Some("newpass1")).unwrap();
        assert!(v.verify_password("newpass1").unwrap() && !v.verify_password("oldpass1").unwrap());
        let o = Vault::open(&p, Some("newpass1"), os()).unwrap();
        assert_eq!(*o.dk(), dk);
        assert!(o.payload() == v.payload());
        assert!(matches!(
            Vault::open(&p, Some("oldpass1"), os()),
            Err(VaultError::WrongPassword)
        ));
        // switch to OS mode
        v.change_password(None).unwrap();
        let o = Vault::open(&p, None, os()).unwrap();
        assert_eq!((o.mode(), *o.dk()), (Mode::Os, dk));
    }

    #[test]
    fn password_mode_needs_os_layer() {
        let (_d, p) = tmp();
        Vault::create(
            &p,
            CreateMode::Password(Zeroizing::new("hunter22".into())),
            os(),
        )
        .unwrap();
        assert!(matches!(
            Vault::open(&p, Some("hunter22"), Arc::new(NoOs)),
            Err(VaultError::OsWrap(_))
        ));
    }

    #[test]
    fn portable_export_opens_without_os() {
        let (d, p) = tmp();
        let mut v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        sample(&mut v);
        let e = d.path().join("export.rmiv");
        v.export_portable(&e, "exportpw").unwrap();
        let o = Vault::open(&e, Some("exportpw"), Arc::new(NoOs)).unwrap();
        assert_eq!(o.mode(), Mode::Portable);
        assert!(o.payload() == v.payload());
        assert_ne!(o.dk(), v.dk());
        assert!(matches!(
            Vault::open(&e, Some("wrongpw1"), Arc::new(NoOs)),
            Err(VaultError::WrongPassword)
        ));
    }

    #[test]
    fn save_backs_up_previous_file() {
        let (d, p) = tmp();
        let v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        let first = fs::read(&p).unwrap();
        v.save().unwrap();
        let backups: Vec<_> = fs::read_dir(d.path().join("backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0]).unwrap(), first);
        assert!(parse_backup_name(backups[0].file_name().unwrap().to_str().unwrap()).is_some());
    }

    #[test]
    fn backup_pruning() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        let at = |d: u32, h: u32| {
            NaiveDate::from_ymd_opt(2026, 10, d)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .map(|t| (t, 0))
                .unwrap()
        };
        // 12 today (h 0..12), one each on 20th, 19th (in window), 2 on 17th, one on 10th (old).
        let mut s: Vec<_> = (0..12).map(|h| at(24, h)).collect();
        s.extend([
            at(20, 5),
            at(20, 1),
            at(18, 9),
            at(17, 3),
            at(17, 8),
            at(10, 1),
        ]);
        let keep = backups_to_keep(&s, today);
        let mut want: BTreeSet<_> = (2..12).map(|h| at(24, h)).collect(); // newest 10
        want.extend([at(20, 5), at(18, 9)]); // newest per day, 18th..24th
        assert_eq!(keep, want);
        let t = at(24, 1).0.with_second(3).unwrap().with_minute(2).unwrap();
        assert_eq!(
            parse_backup_name("vault-20261024-010203.rmiv"),
            Some((t, 0))
        );
        assert_eq!(
            parse_backup_name("vault-20261024-010203-7.rmiv"),
            Some((t, 7))
        );
        for bad in [
            "../vault-20261024-010203.rmiv",
            "vault-20261024-010203-.rmiv",
            "vault-20261024-010203-+1.rmiv",
            "vault-20261024-010203x.rmiv",
            "vault-x.rmiv",
        ] {
            assert!(parse_backup_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn same_second_backups_dont_collide() {
        let (d, p) = tmp();
        let v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        for _ in 0..3 {
            write_atomic(&p, &v.sealed(), Backup::Try).unwrap();
        }
        let names: Vec<String> = fs::read_dir(d.path().join("backups"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 3);
        // 3 saves within < 1 s: at least two share a second, so a counter suffix appears
        assert!(names.iter().any(|n| n.ends_with("-1.rmiv")), "{names:?}");
    }

    #[test]
    fn routine_saves_throttle_backups() {
        let (d, p) = tmp();
        let v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        for _ in 0..3 {
            v.save().unwrap();
        }
        assert_eq!(fs::read_dir(d.path().join("backups")).unwrap().count(), 1);
        assert!(recent_backup(&p, 300) && !recent_backup(&p, 0));
        // a future-stamped backup (clock went back) doesn't block new ones
        fs::remove_dir_all(d.path().join("backups")).unwrap();
        fs::create_dir(d.path().join("backups")).unwrap();
        fs::write(d.path().join("backups/vault-29991231-000000.rmiv"), b"x").unwrap();
        assert!(!recent_backup(&p, 300));
    }

    #[test]
    fn create_refuses_existing_vault() {
        let (_d, p) = tmp();
        Vault::create(&p, CreateMode::Os, os()).unwrap();
        let before = fs::read(&p).unwrap();
        let e = Vault::create(&p, CreateMode::Os, os()).err().unwrap();
        assert!(matches!(e, VaultError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists));
        assert_eq!(fs::read(&p).unwrap(), before);
    }

    #[test]
    fn create_moves_old_backups_aside() {
        let (d, p) = tmp();
        Vault::create(&p, CreateMode::Os, os()).unwrap();
        let old = d.path().join("backups").join("vault-20260101-000000.rmiv");
        fs::copy(&p, &old).unwrap();
        fs::remove_file(&p).unwrap();
        assert!(!Vault::exists(&p));
        Vault::create(&p, CreateMode::Os, os()).unwrap();
        assert!(!old.exists(), "never left where the new vault prunes");
        let aside: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|q| q.to_string_lossy().contains("backups-before-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].join("vault-20260101-000000.rmiv").is_file());
    }

    /// A backup held open: a clear refusal, the old backups untouched, no vault left.
    #[cfg(windows)]
    #[test]
    fn create_refuses_clearly_when_old_backups_are_in_use() {
        let (d, p) = tmp();
        Vault::create(&p, CreateMode::Os, os()).unwrap();
        let old = d.path().join("backups").join("vault-20260101-000000.rmiv");
        fs::copy(&p, &old).unwrap();
        fs::remove_file(&p).unwrap();
        let held = fs::File::open(&old).unwrap();
        let r = Vault::create(&p, CreateMode::Os, os());
        assert!(
            matches!(r, Err(VaultError::Refused(m)) if m.contains("in use")),
            "{:?}",
            r.err()
        );
        assert!(old.is_file() && !Vault::exists(&p));
        drop(held);
    }

    #[test]
    fn failed_write_removes_tmp() {
        let (d, _p) = tmp();
        let dest = d.path().join("adir");
        fs::create_dir(&dest).unwrap();
        assert!(write_atomic(&dest, b"plaintext", Backup::Skip).is_err());
        let tmps = fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(tmps, 0);
    }

    #[test]
    fn replace_payload_needs_backup() {
        let (d, p) = tmp();
        let mut v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        sample(&mut v);
        v.save().unwrap();
        fs::remove_dir_all(d.path().join("backups")).unwrap();
        fs::write(d.path().join("backups"), b"x").unwrap();
        let before = fs::read(&p).unwrap();
        let want = v.payload().clone();
        assert!(matches!(
            v.replace_payload(VaultPayload::default()),
            Err(VaultError::Refused(_))
        ));
        assert!(v.payload() == &want);
        assert_eq!(fs::read(&p).unwrap(), before);
        // routine saves still go through without a backup
        v.save().unwrap();
    }

    fn rewrite_header(p: &Path, f: impl FnOnce(&mut serde_json::Value)) {
        let bytes = fs::read(p).unwrap();
        let (_, hb, ct) = format::decode(&bytes).unwrap();
        let mut h: serde_json::Value = serde_json::from_slice(hb).unwrap();
        f(&mut h);
        fs::write(p, format::encode(&serde_json::to_vec(&h).unwrap(), ct)).unwrap();
    }

    #[test]
    fn header_tamper_is_corrupt() {
        let (_d, p) = tmp();
        Vault::create(&p, CreateMode::Os, os()).unwrap();
        // unknown field: header still parses, but AAD no longer matches
        rewrite_header(&p, |h| {
            h["x"] = json!(0);
        });
        assert!(matches!(
            Vault::open(&p, None, os()),
            Err(VaultError::Corrupt)
        ));
    }

    #[test]
    fn header_kdf_over_cap_is_corrupt() {
        let (_d, p) = tmp();
        let pw = Zeroizing::new(String::from("hunter22"));
        Vault::create(&p, CreateMode::Password(pw), os()).unwrap();
        rewrite_header(&p, |h| {
            h["kdf"]["m_kib"] = json!(2 * KdfParams::DEFAULT.m_kib + 1);
        });
        assert!(matches!(
            Vault::open(&p, Some("hunter22"), os()),
            Err(VaultError::Corrupt)
        ));
    }

    #[test]
    fn restore_refuses_foreign_backup_and_swaps_payload() {
        let (d, p) = tmp();
        let mut v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        // a backup from another vault has another DK
        let other = d.path().join("other.rmiv");
        Vault::create(&other, CreateMode::Os, os()).unwrap();
        let before = fs::read(&p).unwrap();
        assert!(matches!(
            v.restore_from(&other),
            Err(VaultError::Refused(m)) if m.starts_with("This backup")
        ));
        assert_eq!(fs::read(&p).unwrap(), before);
        // the empty-payload snapshot restores after content was added
        let snap = d.path().join("snap.rmiv");
        fs::copy(&p, &snap).unwrap();
        sample(&mut v);
        v.save().unwrap();
        v.restore_from(&snap).unwrap();
        assert!(v.payload().accounts.is_empty());
        assert!(Vault::open(&p, None, os())
            .unwrap()
            .payload()
            .accounts
            .is_empty());
    }

    #[test]
    fn stale_rewrap_is_refused() {
        let (_d, p) = tmp();
        let mut v = Vault::create(
            &p,
            CreateMode::Password(Zeroizing::new("oldpass1".into())),
            os(),
        )
        .unwrap();
        let keys = v.keys();
        let h = keys.rewrap(Some("newpass1")).unwrap();
        // the vault moves on first, so this rewrap is stale
        v.change_password(None).unwrap();
        assert!(matches!(
            v.apply_rewrap(&keys, h),
            Err(VaultError::Refused(m)) if m.starts_with("The vault changed")
        ));
        assert_eq!(Vault::open(&p, None, os()).unwrap().mode(), Mode::Os);
    }

    #[test]
    fn verify_password_only_in_password_mode() {
        let (_d, p) = tmp();
        let v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        assert!(!v.verify_password("anything").unwrap());
    }

    #[test]
    fn exists_needs_a_regular_file() {
        let (d, p) = tmp();
        assert!(!Vault::exists(&p));
        fs::create_dir(d.path().join("dir.rmiv")).unwrap();
        assert!(!Vault::exists(&d.path().join("dir.rmiv")));
    }

    #[test]
    fn password_header_missing_kdf_or_salt_is_corrupt() {
        let (_d, p) = tmp();
        Vault::create(
            &p,
            CreateMode::Password(Zeroizing::new("hunter22".into())),
            os(),
        )
        .unwrap();
        let full = fs::read(&p).unwrap();
        rewrite_header(&p, |h| {
            h["salt"] = json!(null);
        });
        assert!(matches!(
            Vault::open(&p, Some("hunter22"), os()),
            Err(VaultError::Corrupt)
        ));
        fs::write(&p, &full).unwrap();
        rewrite_header(&p, |h| {
            h["kdf"] = json!(null);
        });
        assert!(matches!(
            Vault::open(&p, Some("hunter22"), os()),
            Err(VaultError::Corrupt)
        ));
    }

    #[test]
    fn decrypted_non_object_payload_is_corrupt() {
        let (_d, p) = tmp();
        let v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        let (mut h, _, _) = format::decode(&fs::read(&p).unwrap()).unwrap();
        let nonce = crypto::random::<24>();
        h.nonce = B64.encode(nonce);
        let hj = serde_json::to_vec(&h).unwrap();
        let ct = crypto::seal_with(v.dk(), &nonce, b"[1]", &hj);
        fs::write(&p, format::encode(&hj, &ct)).unwrap();
        assert!(matches!(
            Vault::open(&p, None, os()),
            Err(VaultError::Corrupt)
        ));
    }

    #[test]
    fn backups_are_pruned_on_write() {
        let (d, p) = tmp();
        let v = Vault::create(&p, CreateMode::Os, os()).unwrap();
        let bytes = v.sealed();
        for _ in 0..12 {
            write_atomic(&p, &bytes, Backup::Try).unwrap();
        }
        assert_eq!(fs::read_dir(d.path().join("backups")).unwrap().count(), 10);
    }

    #[test]
    fn os_unwrap_of_wrong_length_is_corrupt() {
        struct ShortOs;
        impl OsWrap for ShortOs {
            fn wrap(&self, d: &[u8]) -> Result<Vec<u8>> {
                FakeOsWrap.wrap(d)
            }
            fn unwrap(&self, b: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
                let mut v = FakeOsWrap.unwrap(b)?;
                v.truncate(16);
                Ok(v)
            }
        }
        let (_d, p) = tmp();
        Vault::create(&p, CreateMode::Os, os()).unwrap();
        assert!(matches!(
            Vault::open(&p, None, Arc::new(ShortOs)),
            Err(VaultError::Corrupt)
        ));
    }

    #[test]
    fn wrong_length_salt_or_nonce_is_corrupt() {
        let (_d, p) = tmp();
        Vault::create(
            &p,
            CreateMode::Password(Zeroizing::new("hunter22".into())),
            os(),
        )
        .unwrap();
        let full = fs::read(&p).unwrap();
        rewrite_header(&p, |h| {
            h["salt"] = json!("AAAA");
        });
        assert!(matches!(
            Vault::open(&p, Some("hunter22"), os()),
            Err(VaultError::Corrupt)
        ));
        fs::write(&p, &full).unwrap();
        rewrite_header(&p, |h| {
            h["nonce"] = json!("AAAA");
        });
        assert!(matches!(
            Vault::open(&p, Some("hunter22"), os()),
            Err(VaultError::Corrupt)
        ));
    }

    #[test]
    fn failed_password_change_keeps_old_password() {
        let (d, p) = tmp();
        let mut v = Vault::create(
            &p,
            CreateMode::Password(Zeroizing::new("oldpass1".into())),
            os(),
        )
        .unwrap();
        let before = fs::read(&p).unwrap();
        // backups is a plain file: Require cannot back up, so nothing is replaced
        fs::remove_dir_all(d.path().join("backups")).unwrap();
        fs::write(d.path().join("backups"), b"x").unwrap();
        assert!(matches!(
            v.change_password(Some("newpass1")),
            Err(VaultError::Refused(_))
        ));
        assert_eq!(fs::read(&p).unwrap(), before);
        assert!(v.verify_password("oldpass1").unwrap());
        assert!(!v.verify_password("newpass1").unwrap());
    }
}
