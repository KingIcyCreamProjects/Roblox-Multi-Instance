//! Encrypted vault: one `vault.rmiv` file holding accounts and their secrets (spec §5).

pub mod crypto;
pub mod format;
pub mod model;
pub mod oswrap;
pub mod store;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    // Codes, not text: the UI maps them (src/lib/ipc.ts CODE_TEXT) and branches on them.
    #[error("WRONG_PASSWORD")]
    WrongPassword,
    #[error("VAULT_DAMAGED")]
    Corrupt,
    #[error("Could not read or write the vault file.")]
    Io(#[from] std::io::Error),
    #[error("The system key store refused the request.")]
    OsWrap(String),
    /// mac/linux device key problems, user-readable.
    #[error("{0}")]
    DeviceKey(&'static str),
    /// An operation refused for a user-readable reason.
    #[error("{0}")]
    Refused(&'static str),
}

pub type Result<T> = std::result::Result<T, VaultError>;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use oswrap::OsWrap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use store::Vault;
use zeroize::{Zeroize, Zeroizing};

pub const PLAIN_CONFIRM: &str = "export plaintext";
pub const REMEMBER_SECS: i64 = 7 * 86400;

pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Seconds on a monotonic clock (0 = first call). `AppState::last_activity` and the auto-lock
/// use it, so setting the wall clock can't stretch or cut the idle timer. On Windows it keeps
/// counting through sleep (QueryPerformanceCounter).
pub fn mono_now() -> i64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs() as i64
}

/// Typed confirmation for the plaintext export.
pub fn check_plain_confirm(confirm: &str) -> std::result::Result<(), String> {
    if confirm != PLAIN_CONFIRM {
        return Err(format!("Type \"{PLAIN_CONFIRM}\" to confirm."));
    }
    Ok(())
}

/// Plaintext JSON of the payload (build under the vault lock, write after releasing it).
pub fn plain_json(v: &Vault) -> Zeroizing<Vec<u8>> {
    json_wiped(v.payload(), true)
}

/// JSON in one exactly sized buffer, wiped on drop. A growing `Vec` frees each outgrown
/// copy (partial plaintext with cookies) without zeroing it, so size it first.
pub(crate) fn json_wiped<T: Serialize>(v: &T, pretty: bool) -> Zeroizing<Vec<u8>> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 += b.len();
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let write = |w: &mut dyn std::io::Write| match pretty {
        true => serde_json::to_writer_pretty(w, v),
        false => serde_json::to_writer(w, v),
    };
    let mut n = Count(0);
    write(&mut n).expect("serializes");
    let mut out = Zeroizing::new(Vec::with_capacity(n.0));
    write(&mut *out).expect("serializes");
    out
}

#[derive(Serialize, Deserialize)]
struct DkCache {
    dk_b64: String,
    expires_unix: i64,
}

impl Drop for DkCache {
    fn drop(&mut self) {
        self.dk_b64.zeroize();
    }
}

/// Remember-device cache: `expires_unix (i64 LE) | os.wrap(json {dk_b64, expires_unix})`.
/// The plain prefix only lets status skip the OS store; the wrapped copy is authoritative.
pub fn cache_write(path: &Path, os: &dyn OsWrap, dk: &[u8; 32], now: i64) -> Result<()> {
    let c = DkCache {
        dk_b64: B64.encode(dk),
        expires_unix: now + REMEMBER_SECS,
    };
    let json = json_wiped(&c, false);
    let file = [&c.expires_unix.to_le_bytes()[..], &os.wrap(&json)?].concat();
    store::write_atomic(path, &file, store::Backup::Skip)
}

/// Cheap check (no OS store): cache file present with an unexpired prefix.
pub fn cache_active(path: &Path, now: i64) -> bool {
    use std::io::Read;
    let mut b = [0u8; 8];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok_and(|_| now < i64::from_le_bytes(b))
}

/// DK from the cache. `Err(true)`: the file is there but expired or garbled, delete it.
/// `Err(false)`: missing, unreadable or the OS store would not unwrap it (maybe transient), keep it.
pub fn cache_read(
    path: &Path,
    os: &dyn OsWrap,
    now: i64,
) -> std::result::Result<Zeroizing<[u8; 32]>, bool> {
    let file = std::fs::read(path).map_err(|_| false)?;
    let (prefix, body) = file.split_first_chunk::<8>().ok_or(true)?;
    if now >= i64::from_le_bytes(*prefix) {
        return Err(true);
    }
    let json = os.unwrap(body).map_err(|_| false)?;
    let c: DkCache = serde_json::from_slice(&json).map_err(|_| true)?;
    if now >= c.expires_unix {
        return Err(true);
    }
    let raw = Zeroizing::new(B64.decode(&c.dk_b64).map_err(|_| true)?);
    let mut dk = Zeroizing::new([0u8; 32]);
    (raw.len() == 32)
        .then(|| dk.copy_from_slice(&raw))
        .ok_or(true)?;
    Ok(dk)
}

/// Delete the remember cache; anything but NotFound is logged.
pub fn cache_remove(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("could not delete remember-device cache: {e}");
        }
    }
}

/// Take the vault out of `slot` and bump `gen` under the slot mutex. Lock wins: the bump happens
/// even when already locked, so an unlock or create in flight is cancelled and stays locked.
/// With nothing in flight the bump changes nothing (no events either: `lock` returns early).
pub fn take_for_lock(slot: &Mutex<Option<Vault>>, gen: &AtomicU64) -> Option<Vault> {
    let mut g = slot.lock().unwrap_or_else(PoisonError::into_inner);
    gen.fetch_add(1, Ordering::SeqCst);
    g.take()
}

/// Locks the vault (if open), purges an expired remember cache, emits `vault://locked`.
pub fn lock(app: &tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    let st = app.state::<crate::AppState>();
    // Record the lock first: anything polling a session window sees VAULT_WAS_LOCKED, not a
    // window that vanished ("cancelled").
    let v = take_for_lock(&st.vault, &st.lock_gen);
    // signed-in roblox.com webviews must not outlive the lock
    let closed = crate::accounts::signin::close_session_windows(app);
    if closed > 0 {
        log::info!("closed {closed} Roblox session windows on lock");
    }
    let Some(v) = v else {
        return;
    };
    v.lock();
    // running streams hold a copied cookie; stop them with the vault
    crate::discover::cancel_streams(|_| true);
    // and launch batches, so no account past the lock gets launched
    crate::launch::launcher::cancel_all();
    st.accounts.on_lock();
    let c = st.paths.dk_cache();
    if c.exists() && !cache_active(&c, now_unix()) {
        cache_remove(&c);
    }
    log::info!("vault locked");
    if let Err(e) = app.emit("vault://locked", ()) {
        log::warn!("could not emit vault://locked: {e}");
    }
    crate::accounts::cmds::emit_changed(app);
    // relaunch batches just got cancelled: the automation page's relaunch status is stale
    crate::commands::state_changed(app, &["snap"]);
}

/// Auto-lock decision. `last_activity`, `now` in `mono_now` secs; `mins` 0 = never on idle.
pub fn should_lock(
    now: i64,
    last_activity: i64,
    slept: bool,
    mins: i64,
    on_sleep: bool,
    ws_locked: bool,
) -> bool {
    let idle = mins > 0 && now - last_activity > mins * 60;
    idle || (on_sleep && (slept || ws_locked))
}

/// Auto reconnect or the friend remote is on: they keep clients going while the owner is
/// away and need the vault, so the idle auto-lock waits while RMI's clients run (the sleep /
/// OS-lock one does not). Automation alone is not enough: it is a broad "let programs control
/// RMI" switch, and keeping the vault open for it would cover far more than AFK play.
fn keeps_clients(s: &serde_json::Value) -> bool {
    let on = |p: &str| s.pointer(p).and_then(serde_json::Value::as_bool) == Some(true);
    (on("/watcher/enabled") && on("/watcher/reconnect")) || on("/friend/enabled")
}

/// A Roblox client launched by RMI (a saved account's tracker) is running. Blocking.
fn clients_running(st: &crate::AppState) -> bool {
    let trackers: Vec<String> = st
        .vault
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|v| {
            v.payload()
                .accounts
                .iter()
                .map(|a| a.tracker_id.clone())
                .collect()
        })
        .unwrap_or_default();
    // An unreadable process list is unknown, not "none run": they may still need the vault.
    !trackers.is_empty()
        && st
            .platform
            .tracked_clients(&trackers)
            .is_none_or(|c| !c.is_empty())
}

/// Every 5 s: lock on idle > `vault.autoLockMin` (0 = never), or, with `vault.lockOnSleep`,
/// on a > 30 s gap between ticks (system slept) or a locked workstation.
pub fn spawn_autolock(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || {
        let mut last = (mono_now(), now_unix());
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let tick = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (now, wall) = (mono_now(), now_unix());
                // Windows: the monotonic clock counts sleep and ignores clock changes. Elsewhere
                // it stops during suspend, so only the wall clock can see a sleep.
                // ponytail: mac/linux still read a forward clock jump as sleep; use a boot-time
                // clock there when those platforms ship.
                let slept = if cfg!(windows) {
                    now - last.0 > 30
                } else {
                    wall - last.1 > 30
                };
                // Sleep time the monotonic clock missed (mac/linux): counts as idle time too.
                let missed = if cfg!(windows) || !slept {
                    0
                } else {
                    ((wall - last.1) - (now - last.0)).max(0)
                };
                last = (now, wall);
                let st = app.state::<crate::AppState>();
                if st
                    .vault
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_none()
                {
                    return;
                }
                let (mins, on_sleep, away) = {
                    let s = st.settings.lock().unwrap_or_else(PoisonError::into_inner);
                    (
                        s.pointer("/vault/autoLockMin")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(15),
                        s.pointer("/vault/lockOnSleep")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true),
                        keeps_clients(&s),
                    )
                };
                let mut activity = st.last_activity.fetch_sub(missed, Ordering::Relaxed) - missed;
                if mins > 0 && away && clients_running(&st) {
                    // Away features at work: the idle timer starts over once they stop.
                    st.last_activity.store(now, Ordering::Relaxed);
                    activity = now;
                }
                // only query WTS when it can matter
                let ws = on_sleep && !slept && workstation_locked();
                if should_lock(now, activity, slept, mins, on_sleep, ws) {
                    lock(&app);
                }
            }));
            if tick.is_err() {
                log::error!("auto-lock tick panicked; continuing");
            }
        }
    });
}

/// Session lock state from WTS (Win10/11 flag semantics). UAC's secure desktop is not a lock.
#[cfg(windows)]
fn workstation_locked() -> bool {
    use std::sync::atomic::AtomicBool;
    use windows::core::PWSTR;
    use windows::Win32::System::RemoteDesktop::{
        WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSINFOEXW,
        WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK,
    };
    static LOGGED: AtomicBool = AtomicBool::new(false);
    let mut buf = PWSTR::null();
    let mut len = 0u32;
    // SAFETY: WTS allocates `buf` (len bytes); we read it only if large enough, then free once.
    unsafe {
        let r = WTSQuerySessionInformationW(
            Some(WTS_CURRENT_SERVER_HANDLE),
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &mut buf,
            &mut len,
        );
        if r.is_err() || buf.is_null() {
            if !LOGGED.swap(true, Ordering::Relaxed) {
                log::warn!("WTS session query failed ({r:?}); lock-on-OS-lock unavailable");
            }
            return false;
        }
        let locked = len as usize >= std::mem::size_of::<WTSINFOEXW>() && {
            let info = &*(buf.0 as *const WTSINFOEXW);
            info.Level == 1
                && info.Data.WTSInfoExLevel1.SessionFlags == WTS_SESSIONSTATE_LOCK as i32
        };
        WTSFreeMemory(buf.0 as _);
        locked
    }
}

// ponytail: mac/linux rely on the sleep-gap check; add screen-lock signals when those ship.
#[cfg(not(windows))]
fn workstation_locked() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use oswrap::FakeOsWrap;
    use std::sync::Arc;
    use store::CreateMode;

    #[test]
    fn plain_confirm_is_exact() {
        for bad in ["", "export", "Export plaintext", "export plaintext "] {
            assert!(check_plain_confirm(bad).is_err());
        }
        assert!(check_plain_confirm("export plaintext").is_ok());
        let d = tempfile::tempdir().unwrap();
        let mut v = Vault::create(
            &d.path().join("vault.rmiv"),
            CreateMode::Os,
            Arc::new(FakeOsWrap),
        )
        .unwrap();
        v.payload_mut().extra.insert("k".into(), "v".into());
        let j: serde_json::Value = serde_json::from_slice(&plain_json(&v)).unwrap();
        assert_eq!(j["k"], "v");
    }

    #[test]
    fn remember_cache_expiry() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.rmiv");
        let c = d.path().join("dk.cache");
        let os = Arc::new(FakeOsWrap);
        let v = Vault::create(
            &p,
            CreateMode::Password(Zeroizing::new("hunter22".into())),
            os.clone(),
        )
        .unwrap();
        let t0 = 1_000_000;
        cache_write(&c, &*os, v.dk(), t0).unwrap();
        assert!(cache_active(&c, t0 + REMEMBER_SECS - 1));
        assert!(!cache_active(&c, t0 + REMEMBER_SECS));
        let dk = cache_read(&c, &*os, t0 + REMEMBER_SECS - 1).expect("fresh cache");
        assert_eq!(*dk, *v.dk());
        assert!(Vault::open_with_dk(&p, dk, os.clone()).is_ok());
        // expired: no DK, so unlocking needs the password
        assert_eq!(
            cache_read(&c, &*os, t0 + REMEMBER_SECS + 1).err(),
            Some(true)
        );
        assert!(matches!(
            Vault::open(&p, None, os.clone()),
            Err(VaultError::WrongPassword)
        ));
        assert_eq!(
            cache_read(&d.path().join("missing"), &*os, t0).err(),
            Some(false)
        );
    }

    #[test]
    fn lock_bumps_generation_even_when_locked() {
        let d = tempfile::tempdir().unwrap();
        let v = Vault::create(
            &d.path().join("vault.rmiv"),
            CreateMode::Os,
            Arc::new(FakeOsWrap),
        )
        .unwrap();
        let slot = Mutex::new(Some(v));
        let gen = AtomicU64::new(0);
        assert!(take_for_lock(&slot, &gen).is_some());
        assert!(take_for_lock(&slot, &gen).is_none());
        assert_eq!(gen.load(Ordering::SeqCst), 2);
        assert!(slot.lock().unwrap().is_none());
    }

    #[test]
    fn should_lock_rules() {
        let t = 10_000;
        // idle
        assert!(should_lock(t, t - 15 * 60 - 1, false, 15, false, false));
        assert!(!should_lock(t, t - 15 * 60, false, 15, false, false));
        assert!(!should_lock(t, 0, false, 0, false, false), "0 = never");
        // sleep / OS lock only when lockOnSleep
        assert!(should_lock(t, t, true, 15, true, false));
        assert!(should_lock(t, t, false, 15, true, true));
        assert!(!should_lock(t, t, true, 15, false, true));
        assert!(!should_lock(t, t, false, 15, true, false));
    }

    #[test]
    fn idle_lock_waits_for_away_features() {
        use serde_json::json;
        assert!(!keeps_clients(&json!({})));
        assert!(
            !keeps_clients(&json!({"watcher": {"enabled": false, "reconnect": true}})),
            "reconnect needs the watcher"
        );
        assert!(keeps_clients(
            &json!({"watcher": {"enabled": true, "reconnect": true}})
        ));
        assert!(
            !keeps_clients(&json!({"automation": {"enabled": true}})),
            "Automation alone is too broad to keep the vault open"
        );
        assert!(keeps_clients(&json!({"friend": {"enabled": true}})));
    }

    #[test]
    fn clients_running_needs_a_saved_accounts_client() {
        use crate::platform::{Capabilities, Platform, RobloxInstall};
        struct Fake;
        impl Platform for Fake {
            fn capabilities(&self) -> Capabilities {
                Capabilities::default()
            }
            fn roblox_install(&self) -> Option<RobloxInstall> {
                None
            }
            fn open_uri(&self, _: &str) -> std::io::Result<()> {
                Ok(())
            }
            fn tracked_pids(&self, t: &str) -> Option<Vec<u32>> {
                Some(if t == "77" { vec![1] } else { vec![] })
            }
        }
        let d = tempfile::tempdir().unwrap();
        let mut st = crate::test_state(
            d.path(),
            crate::roblox::RobloxClient::new(crate::roblox::BaseUrls::prod()),
        );
        st.platform = Box::new(Fake);
        assert!(!clients_running(&st), "no saved accounts");
        let a = serde_json::from_value(serde_json::json!({"id": "A", "trackerId": "5"})).unwrap();
        st.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts
            .push(a);
        assert!(!clients_running(&st), "its client is not running");
        st.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[0]
            .tracker_id = "77".into();
        assert!(clients_running(&st));
        *st.vault.lock().unwrap() = None;
        assert!(!clients_running(&st), "locked");
    }

    #[cfg(windows)]
    #[test]
    fn interactive_session_is_not_locked() {
        assert!(!workstation_locked());
    }

    #[test]
    fn remember_cache_garbage_deleted_outage_kept() {
        use oswrap::OsWrap;
        struct Down;
        impl OsWrap for Down {
            fn wrap(&self, _: &[u8]) -> Result<Vec<u8>> {
                Err(VaultError::OsWrap("down".into()))
            }
            fn unwrap(&self, _: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
                Err(VaultError::OsWrap("down".into()))
            }
        }
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("dk.cache");
        let os = FakeOsWrap;
        let t0 = 1_000_000;
        assert!(!cache_active(&d.path().join("missing"), t0));
        // shorter than the 8-byte expiry prefix: garbled, delete
        std::fs::write(&c, [1u8, 2]).unwrap();
        assert_eq!(cache_read(&c, &os, t0).err(), Some(true));
        // wrong-length DK inside a valid, unexpired wrap: delete
        let bad = DkCache {
            dk_b64: B64.encode([1u8; 5]),
            expires_unix: t0 + REMEMBER_SECS,
        };
        let json = serde_json::to_vec(&bad).unwrap();
        let file = [
            &bad.expires_unix.to_le_bytes()[..],
            &os.wrap(&json).unwrap(),
        ]
        .concat();
        std::fs::write(&c, &file).unwrap();
        assert_eq!(cache_read(&c, &os, t0).err(), Some(true));
        // OS store refuses to unwrap (maybe transient): keep the file
        cache_write(&c, &os, &[9u8; 32], t0).unwrap();
        assert_eq!(cache_read(&c, &Down, t0).err(), Some(false));
        assert!(c.exists());
    }

    #[test]
    fn remember_cache_wrapped_copy_is_authoritative() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("dk.cache");
        let os = FakeOsWrap;
        let t0 = 1_000_000;
        // plain prefix says valid for a week, wrapped copy already expired at t0
        let stale = DkCache {
            dk_b64: B64.encode([9u8; 32]),
            expires_unix: t0,
        };
        let json = serde_json::to_vec(&stale).unwrap();
        let file = [
            &(t0 + REMEMBER_SECS).to_le_bytes()[..],
            &os.wrap(&json).unwrap(),
        ]
        .concat();
        std::fs::write(&c, &file).unwrap();
        assert!(cache_active(&c, t0));
        assert_eq!(cache_read(&c, &os, t0).err(), Some(true));
        // wrapped body is not JSON: garbled, delete
        let file = [
            &(t0 + REMEMBER_SECS).to_le_bytes()[..],
            &os.wrap(b"not json").unwrap(),
        ]
        .concat();
        std::fs::write(&c, &file).unwrap();
        assert_eq!(cache_read(&c, &os, t0).err(), Some(true));
    }
}
