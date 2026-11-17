//! Run here: a running account moves from one of the owner's paired computers to this one.
//! The other computer closes its client for good (auto-relaunch stays off it), then the UI
//! launches it here with the normal Play. Accounts match by Roblox user id; no cookie or
//! secret crosses the network.

use crate::AppState;
use serde::Serialize;
use tauri::Manager;

/// What a hand-over needs from the app (vault, process list, auto-relaunch). Blocking calls.
pub trait Local: Send + Sync {
    /// Roblox user ids with a client open here. `None` while the vault is locked.
    fn running(&self) -> Option<Vec<u64>>;
    /// Close these accounts' clients for good. Returns the ids with no client left here
    /// (ids that were not running count as released). `None` while the vault is locked.
    fn release(&self, ids: &[u64]) -> Option<Vec<u64>>;
}

/// The real `Local`.
pub struct AppLocal(pub tauri::AppHandle);

impl Local for AppLocal {
    fn running(&self) -> Option<Vec<u64>> {
        running_ids(self.0.state::<AppState>().inner())
    }
    fn release(&self, ids: &[u64]) -> Option<Vec<u64>> {
        release(self.0.state::<AppState>().inner(), ids)
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Elsewhere {
    pub user_id: u64,
    pub device: String,
    pub ip: String,
}

/// Saved accounts with a user id: (user id, username, tracker id). `None` while locked.
fn saved(st: &AppState) -> Option<Vec<(u64, String, String)>> {
    let g = crate::commands::guard(&st.vault);
    let all = g.as_ref()?.payload().accounts.iter();
    Some(
        all.filter_map(|a| Some((a.user_id?, a.username.clone(), a.tracker_id.clone())))
            .collect(),
    )
}

/// The ids of `accs` with a live client (one process scan).
fn live(st: &AppState, accs: &[(u64, String, String)]) -> Vec<u64> {
    let trackers: Vec<String> = accs.iter().map(|a| a.2.clone()).collect();
    let live = st.platform.tracked_clients(&trackers);
    accs.iter()
        .filter(|a| live.iter().any(|(_, t)| *t == a.2))
        .map(|a| a.0)
        .collect()
}

/// Roblox user ids with a client here. `None` while locked. Blocking.
pub(crate) fn running_ids(st: &AppState) -> Option<Vec<u64>> {
    let accs = saved(st)?;
    Some(live(st, &accs))
}

/// `Local::release`. Blocking.
pub(crate) fn release(st: &AppState, ids: &[u64]) -> Option<Vec<u64>> {
    let accs: Vec<_> = saved(st)?
        .into_iter()
        .filter(|a| ids.contains(&a.0))
        .collect();
    for (_, user, _) in &accs {
        // Before the close: auto-relaunch must not read this exit as a crash and bring the
        // client back here (the other computer runs it now).
        st.automation
            .relaunch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .quiet(user);
    }
    // All at once: a close can take 3 s, and the asking computer waits `HANDOVER_WAIT` in all.
    std::thread::scope(|s| {
        for (_, user, tracker) in &accs {
            s.spawn(move || {
                if let Err(e) = st.platform.close_tracked(tracker) {
                    log::warn!("remote: could not close {user}: {e}");
                }
                log::info!("remote: handed {user} over");
            });
        }
    });
    let still = live(st, &accs);
    Some(ids.iter().filter(|i| !still.contains(i)).copied().collect())
}

/// Every account a paired computer runs. Empty while locked.
#[tauri::command]
pub fn remote_elsewhere(state: tauri::State<'_, AppState>) -> Vec<Elsewhere> {
    if crate::commands::guard(&state.vault).is_none() {
        return Vec::new();
    }
    state.remote.elsewhere()
}

/// The paired computers running `user_ids` close them; the ids now free to launch here.
/// Up to `HANDOVER_WAIT`.
#[tauri::command]
pub async fn remote_run_here(
    app: tauri::AppHandle,
    user_ids: Vec<u64>,
) -> Result<Vec<u64>, String> {
    let r = app.state::<AppState>().remote.clone();
    let a = &app;
    r.run_here(&user_ids, |ip, name, ids| {
        super::viewer::hand_over(a, ip, name, ids)
    })
    .await
}
