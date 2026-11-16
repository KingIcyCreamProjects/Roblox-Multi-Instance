//! HomeLab export: `homelab.json` next to `settings.json` lists the vault accounts and the
//! Roblox clients RMI tracks for each, for the owner's local HomeLab agent. No secrets,
//! cookies or tracker ids. Written while unlocked (unlock, account changes, every 15 s);
//! `updatedAt` refreshes at least every 60 s so the agent can tell RMI is alive. Locked:
//! the last file stays. `homelab.export` off: the file is deleted.

use crate::AppState;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Listener, Manager};

const FILE: &str = "homelab.json";
const ALIVE: Duration = Duration::from_secs(60);

/// Last written `accounts` JSON and when. Held for a whole sync: one writer at a time.
static LAST: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// `(userId, username, displayName, tracker id)` per account, plus tracked
/// `(pid, tracker id)` clients, as the exported `accounts` array.
fn accounts_json(accts: &[(u64, String, String, String)], clients: &[(u32, String)]) -> Value {
    accts
        .iter()
        .map(|(id, user, display, tracker)| {
            let pids: Vec<u32> = clients
                .iter()
                .filter(|(_, t)| t == tracker)
                .map(|(p, _)| *p)
                .collect();
            json!({ "userId": id, "username": user, "displayName": display, "pids": pids })
        })
        .collect()
}

fn doc(accounts: Value, updated_at: &str) -> Value {
    json!({ "version": 1, "updatedAt": updated_at, "accounts": accounts })
}

/// Write unless the accounts are unchanged and the last write is under 60 s old.
fn due(last: Option<&(String, Instant)>, accounts: &str, now: Instant) -> bool {
    last.is_none_or(|(prev, at)| prev != accounts || now.duration_since(*at) >= ALIVE)
}

pub fn sync(app: &AppHandle) {
    let st = app.state::<AppState>();
    let path = st.paths.root.join(FILE);
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let on = st
        .settings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pointer("/homelab/export")
        != Some(&Value::Bool(false));
    if !on {
        if last.take().is_some() || path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("could not delete {FILE}: {e}");
                }
            }
        }
        return;
    }
    let accts: Vec<(u64, String, String, String)> = match st
        .vault
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        Some(v) => v
            .payload()
            .accounts
            .iter()
            .filter_map(|a| {
                let id = a.user_id?;
                Some((
                    id,
                    a.username.clone(),
                    a.display_name.clone(),
                    a.tracker_id.clone(),
                ))
            })
            .collect(),
        None => return, // locked: keep the last file
    };
    let trackers: Vec<String> = accts.iter().map(|a| a.3.clone()).collect();
    let clients = if trackers.is_empty() {
        vec![]
    } else {
        st.platform.tracked_clients(&trackers)
    };
    let accounts = accounts_json(&accts, &clients);
    let key = accounts.to_string();
    let now = Instant::now();
    if !due(last.as_ref(), &key, now) {
        return;
    }
    let body = doc(accounts, &crate::accounts::now_iso()).to_string();
    match crate::vault::store::write_atomic(
        &path,
        body.as_bytes(),
        crate::vault::store::Backup::Skip,
    ) {
        Ok(()) => *last = Some((key, now)),
        Err(e) => log::warn!("could not write {FILE}: {e}"),
    }
}

/// Every 15 s, and on account list changes (unlock emits that too).
pub fn spawn(app: AppHandle) {
    let h = app.clone();
    app.listen_any("accounts://changed", move |_| {
        let h = h.clone();
        // off the emitter's thread: emitters may hold the vault lock
        tauri::async_runtime::spawn_blocking(move || sync(&h));
    });
    std::thread::spawn(move || loop {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sync(&app))).is_err() {
            log::error!("homelab export panicked; continuing");
        }
        std::thread::sleep(Duration::from_secs(15));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_exact_shape() {
        let accts = vec![
            (123, "x".into(), "y".into(), "t1".into()),
            (9, "a".into(), "b".into(), "t2".into()),
        ];
        let clients = vec![(4567, "t1".into()), (99, "other".into())];
        let d = doc(accounts_json(&accts, &clients), "2026-11-09T00:00:00Z");
        assert_eq!(
            d,
            json!({"version":1,"updatedAt":"2026-11-09T00:00:00Z","accounts":[
                {"userId":123,"username":"x","displayName":"y","pids":[4567]},
                {"userId":9,"username":"a","displayName":"b","pids":[]}
            ]})
        );
    }

    #[test]
    fn skips_unchanged_until_alive_interval() {
        let t = Instant::now();
        assert!(due(None, "[]", t));
        let last = ("[]".to_owned(), t);
        assert!(!due(Some(&last), "[]", t + Duration::from_secs(59)));
        assert!(due(Some(&last), "[]", t + ALIVE));
        assert!(due(Some(&last), "[1]", t + Duration::from_secs(1)));
    }
}
