//! Run here: a running account moves from one of the owner's paired computers to this one.
//! The other computer closes its client for good (auto-relaunch stays off it), then the UI
//! launches it here with the normal Play. Accounts match by Roblox user id; no cookie or
//! secret crosses the network.
//! Run on: the other way round. The paired computer launches it first (a refusal closes
//! nothing), then it closes everywhere else.

use crate::launch::launcher::Host;
use crate::AppState;
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::Manager;

/// What a hand-over needs from the app (vault, process list, auto-relaunch). Blocking calls.
pub trait Local: Send + Sync {
    /// Roblox user ids with a client open here. `None` while the vault is locked.
    fn running(&self) -> Option<Vec<u64>>;
    /// Close these accounts' clients for good. Returns the ids with no client left here
    /// (ids that were not running count as released). `None` while the vault is locked.
    fn release(&self, ids: &[u64]) -> Option<Vec<u64>>;
    /// Start launching these accounts here, each into its own destination (like Play; no
    /// destination opens Roblox's home screen). Returns the ids whose client started (ids
    /// not saved or signed in here are left out). `None` while the vault is locked.
    fn launch(&self, ids: &[u64]) -> Option<Vec<u64>>;
    /// Accounts can be launched here (Roblox or Sober installed).
    fn can_launch(&self) -> bool;
    /// Run on: no automated launch or relaunch of these accounts here while they move.
    /// Returns what it held (give it back with `unhold`, which needs no unlocked vault).
    fn hold(&self, _ids: &[u64]) -> Vec<Held> {
        Vec::new()
    }
    /// Automated launches and relaunches of these held accounts again.
    fn unhold(&self, _held: &[Held]) {}
}

/// A held account: (user id, account id, username).
pub type Held = (u64, String, String);

/// The real `Local`.
pub struct AppLocal(pub tauri::AppHandle);

impl Local for AppLocal {
    fn running(&self) -> Option<Vec<u64>> {
        running_ids(self.0.state::<AppState>().inner())
    }
    fn release(&self, ids: &[u64]) -> Option<Vec<u64>> {
        release(self.0.state::<AppState>().inner(), ids)
    }
    fn launch(&self, ids: &[u64]) -> Option<Vec<u64>> {
        launch_saved(&self.0, ids, LAUNCH_WAIT)
    }
    fn can_launch(&self) -> bool {
        can_launch(self.0.state::<AppState>().inner())
    }
    fn hold(&self, ids: &[u64]) -> Vec<Held> {
        hold(self.0.state::<AppState>().inner(), ids)
    }
    fn unhold(&self, held: &[Held]) {
        unhold(self.0.state::<AppState>().inner(), held)
    }
}

/// `Local::hold`. Blocking.
pub(super) fn hold(st: &AppState, ids: &[u64]) -> Vec<Held> {
    let Some(mut accs) = saved(st) else {
        return Vec::new();
    };
    accs.retain(|a| ids.contains(&a.0));
    // only clients here (an account handed away earlier must stay so after `unhold`); an
    // unreadable process list holds them all
    if let Some(here) = live(st, &accs) {
        accs.retain(|a| here.contains(&a.0));
    }
    let held: Vec<Held> = accs
        .into_iter()
        .map(|(uid, user, _, id)| (uid, id, user))
        .collect();
    let names: Vec<String> = held.iter().map(|h| h.1.clone()).collect();
    st.automation.hand_over(&names);
    let mut r = relaunch(st);
    for (_, _, user) in &held {
        r.quiet(user);
    }
    held
}

/// `Local::unhold`. Blocking.
pub(super) fn unhold(st: &AppState, held: &[Held]) {
    let names: Vec<String> = held.iter().map(|h| h.1.clone()).collect();
    st.automation.take_back(&names);
    let mut r = relaunch(st);
    for (_, _, user) in held {
        r.unquiet(user);
    }
}

fn relaunch(st: &AppState) -> std::sync::MutexGuard<'_, crate::automation::relaunch::Engine> {
    st.automation
        .relaunch
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Each account's latest `launch_saved`, by account id: a late-client watch for an earlier
/// one leaves the account alone once a newer launch of it started.
static LAUNCHES: std::sync::Mutex<Option<std::collections::HashMap<String, u64>>> =
    std::sync::Mutex::new(None);

/// The account's launch generation; `bump` starts a new one.
fn launch_gen(id: &str, bump: bool) -> u64 {
    let mut g = LAUNCHES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let n = g
        .get_or_insert_with(Default::default)
        .entry(id.into())
        .or_default();
    *n += u64::from(bump);
    *n
}

/// Roblox (or Sober, with pasta for more than one client) is there to launch. Blocking.
pub(crate) fn can_launch(st: &AppState) -> bool {
    let c = st.platform.capabilities();
    c.launch && c.multi_instance
}

/// How long `Local::launch` takes at most: `HANDOVER_WAIT * 3 / 8`, inside the asker's wait.
pub(super) const LAUNCH_WAIT: Duration =
    Duration::from_secs(super::HANDOVER_WAIT.as_secs() * 3 / 8);

/// `Local::launch`: only the ids with a new client here count (the other computer closes
/// those); unknown (still launching, or Windows took the launch and no player started) stays
/// where it runs. Blocking, at most `within` (inside the asker's wait).
pub(crate) fn launch_saved<H: Host>(h: &H, ids: &[u64], within: Duration) -> Option<Vec<u64>> {
    use crate::accounts::SessionState::{Expired, Pending};
    let st = h.st();
    // (user id, account id, tracker id) of the saved accounts asked for; a dead session would
    // only fail here, after the other computer let go
    let accs: Vec<(u64, String, String)> = {
        let g = crate::commands::guard(&st.vault);
        let p = g.as_ref()?.payload();
        let signed_in = |id: &str| p.secrets.get(id).is_some_and(|s| !s.cookie.is_empty());
        p.accounts
            .iter()
            .filter(|a| !matches!(a.session, Expired | Pending) && signed_in(&a.id))
            .filter_map(|a| Some((a.user_id?, a.id.clone(), a.tracker_id.clone())))
            .filter(|a| ids.contains(&a.0))
            .collect()
    };
    if accs.is_empty() || !can_launch(st) {
        return Some(Vec::new());
    }
    let bar = crate::launch::resolve::Bar {
        home: true,
        ..Default::default()
    };
    let accounts = accs.iter().map(|a| a.1.clone()).collect();
    let trackers: Vec<String> = accs.iter().map(|a| a.2.clone()).collect();
    let t0 = Instant::now();
    // Unreadable process list: no new client could be told apart, so none launches here.
    let Some(before) = st.platform.tracked_clients(&trackers) else {
        return Some(Vec::new());
    };
    // a newer launch of these: an earlier one's late-client watch leaves them alone
    let gens: Vec<u64> = accs.iter().map(|a| launch_gen(&a.1, true)).collect();
    // The owner asked for it on the other computer: a user's launch, so a pause ends.
    match crate::launch::launcher::start_batch(h, accounts, bar, false, true) {
        Ok(b) => {
            let up = crate::launch::launcher::wait_launched(&b, within * 2 / 3, within / 6);
            // A started launch is not a player: count only a client that was not there before.
            loop {
                // Unreadable: nothing counted yet (asked again until `within`).
                let now = st.platform.tracked_clients(&trackers).unwrap_or_default();
                let new = |a: &&(u64, String, String)| {
                    up.contains(&a.1)
                        && now
                            .iter()
                            .any(|c| c.1 == a.2 && !before.iter().any(|b| b.0 == c.0))
                };
                let ok: Vec<u64> = accs.iter().filter(new).map(|a| a.0).collect();
                if ok.len() >= up.len() || t0.elapsed() >= within {
                    let late: Vec<_> = accs
                        .iter()
                        .zip(&gens)
                        .filter(|(a, _)| up.contains(&a.1) && !ok.contains(&a.0))
                        .map(|(a, g)| (a.clone(), *g))
                        .collect();
                    close_late(h.clone(), late, before);
                    return Some(ids.iter().copied().filter(|i| ok.contains(i)).collect());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        Err(e) if e == crate::commands::LOCKED => None,
        Err(e) => {
            log::warn!("remote: could not start the sent accounts: {e}");
            Some(Vec::new())
        }
    }
}

/// How long a client of a sent account that was still launching when the answer went is
/// watched for (a Roblox update can hold a launch this long). Short in tests: no watch
/// thread outlives them.
const LATE: Duration = if cfg!(test) {
    Duration::from_millis(600)
} else {
    Duration::from_secs(120)
};

/// Run on, here the target: accounts past the point a launch can be cancelled, without a
/// client yet when the answer went, were not counted (they stay where they ran). A client of
/// theirs that shows up within `LATE` is closed again: an account never runs in two places.
/// A newer `launch_saved` of an account (a retried send) ends its watch.
/// ponytail: a plain Play of the same account here within `LATE` is closed too; have the
/// launcher bump `launch_gen` if that ever bites.
fn close_late<H: Host>(h: H, late: Vec<((u64, String, String), u64)>, before: Vec<(u32, String)>) {
    if late.is_empty() {
        return;
    }
    let run = move || {
        let st = h.st();
        let (mut left, t0, mut warned) = (late, Instant::now(), Vec::new());
        while t0.elapsed() < LATE {
            left.retain(|(a, g)| launch_gen(&a.1, false) == *g);
            if left.is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
            let trackers: Vec<String> = left.iter().map(|a| a.0 .2.clone()).collect();
            let Some(now) = st.platform.tracked_clients(&trackers) else {
                continue;
            };
            let (shown, wait): (Vec<_>, Vec<_>) = left.into_iter().partition(|(a, _)| {
                now.iter()
                    .any(|c| c.1 == a.2 && !before.iter().any(|b| b.0 == c.0))
            });
            left = wait;
            for ((uid, id, tracker), g) in shown {
                // another computer runs it: no automated launch brings it back here
                st.automation.hand_over(std::slice::from_ref(&id));
                if let Some(a) = saved(st).into_iter().flatten().find(|a| a.0 == uid) {
                    relaunch(st).quiet(&a.1);
                }
                // closed only once the process list shows it gone; else tried again
                let closed = matches!(st.platform.close_tracked(&tracker), Ok(n) if n > 0)
                    && st
                        .platform
                        .tracked_clients(std::slice::from_ref(&tracker))
                        .is_some_and(|c| c.is_empty());
                if closed {
                    log::info!("remote: closed a sent account that started too late");
                } else {
                    if !warned.contains(&uid) {
                        log::warn!("remote: could not close a late sent account; trying again");
                        warned.push(uid);
                    }
                    left.push(((uid, id, tracker), g));
                }
            }
        }
        if !left.is_empty() && !warned.is_empty() {
            log::warn!("remote: a late sent account may run here and where it was sent from");
        }
    };
    if let Err(e) = std::thread::Builder::new()
        .name("remote-late".into())
        .spawn(run)
    {
        log::warn!("remote: late launches not watched: {e}");
    }
}

/// What a send did: the ids launching on the target, and those of them still running where
/// they were (a close failed or could not be asked): two places until closed there.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub started: Vec<u64>,
    pub still: Vec<u64>,
    /// A computer whose older RMI runs some of the asked accounts: those were not sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub ip: String,
    pub device: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Elsewhere {
    pub user_id: u64,
    pub device: String,
    pub ip: String,
}

/// (user id, username, tracker id, account id).
type Saved = (u64, String, String, String);

/// Saved accounts with a user id. `None` while locked.
fn saved(st: &AppState) -> Option<Vec<Saved>> {
    let g = crate::commands::guard(&st.vault);
    let all = g.as_ref()?.payload().accounts.iter();
    let row = |a: &crate::accounts::Account| {
        Some((
            a.user_id?,
            a.username.clone(),
            a.tracker_id.clone(),
            a.id.clone(),
        ))
    };
    Some(all.filter_map(row).collect())
}

/// The ids of `accs` with a live client (one process scan). `None`: the process list could
/// not be read.
fn live(st: &AppState, accs: &[Saved]) -> Option<Vec<u64>> {
    let trackers: Vec<String> = accs.iter().map(|a| a.2.clone()).collect();
    let live = st.platform.tracked_clients(&trackers)?;
    Some(
        accs.iter()
            .filter(|a| live.iter().any(|(_, t)| *t == a.2))
            .map(|a| a.0)
            .collect(),
    )
}

/// Roblox user ids with a client here. `None` while locked or unknown. Blocking.
pub(crate) fn running_ids(st: &AppState) -> Option<Vec<u64>> {
    let accs = saved(st)?;
    live(st, &accs)
}

/// `Local::release`. Blocking.
pub(crate) fn release(st: &AppState, ids: &[u64]) -> Option<Vec<u64>> {
    let mut accs: Vec<_> = saved(st)?
        .into_iter()
        .filter(|a| ids.contains(&a.0))
        .collect();
    // An RMI launch of it here (queued or running) would start it again after the close:
    // not released, it stays here.
    let busy: Vec<u64> = accs
        .iter()
        .filter(|a| st.automation.own_running(&a.3))
        .map(|a| a.0)
        .collect();
    accs.retain(|a| !busy.contains(&a.0));
    let relaunch = || {
        st.automation
            .relaunch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    st.automation
        .hand_over(&accs.iter().map(|a| a.3.clone()).collect::<Vec<_>>());
    for (_, user, _, _) in &accs {
        // Before the close: auto-relaunch must not read this exit as a crash and bring the
        // client back here (the other computer runs it now).
        relaunch().quiet(user);
    }
    // All at once: a close can take 3 s, and the asking computer waits `HANDOVER_WAIT` in all.
    std::thread::scope(|s| {
        for (_, user, tracker, _) in &accs {
            s.spawn(move || {
                if let Err(e) = st.platform.close_tracked(tracker) {
                    log::warn!("remote: could not close {user}: {e}");
                }
            });
        }
    });
    // Unreadable process list: every one counts as a survivor, never as handed over.
    let still = live(st, &accs).unwrap_or_else(|| accs.iter().map(|a| a.0).collect());
    for (_, user, _, _) in accs.iter().filter(|a| !still.contains(&a.0)) {
        log::info!("remote: handed {user} over");
    }
    // Survivors still run here: watched by auto-relaunch again, like a failed helper close.
    for (_, user, _, id) in accs.iter().filter(|a| still.contains(&a.0)) {
        relaunch().unquiet(user);
        st.automation.take_back(std::slice::from_ref(id));
    }
    let kept = |i: &&u64| !still.contains(i) && !busy.contains(i);
    Some(ids.iter().filter(kept).copied().collect())
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
    let st = app.state::<AppState>();
    let gen = unlocked(&st)?;
    let a = &app;
    let ask = |ip, name, ids| async move {
        gen.check(&a.state::<AppState>().lock_gen)?;
        super::viewer::hand_over(a, ip, name, ids).await
    };
    let out = st.remote.clone().run_here(&user_ids, ask).await;
    // Locked meanwhile: the UI must not launch into a locked vault (and says so).
    gen.check(&st.lock_gen)?;
    out
}

/// A hand-over starts only while unlocked; a lock meanwhile wins (no further ask goes out).
fn unlocked(st: &AppState) -> Result<crate::commands::Gen, String> {
    let gen = crate::commands::Gen::now(&st.lock_gen);
    if crate::commands::guard(&st.vault).is_none() {
        return Err(crate::commands::LOCKED.into());
    }
    Ok(gen)
}

/// Paired computers that can take accounts right now (a report or answer within PEER_TTL
/// that said so). Empty while locked.
#[tauri::command]
pub fn remote_targets(state: tauri::State<'_, AppState>) -> Vec<Target> {
    if crate::commands::guard(&state.vault).is_none() {
        return Vec::new();
    }
    state.remote.targets()
}

/// Sends the accounts to the paired computer at `ip`: it launches them, then they are closed
/// wherever else they run (here included). Returns the user ids now launching there.
#[tauri::command]
pub async fn remote_send(
    app: tauri::AppHandle,
    user_ids: Vec<u64>,
    ip: String,
) -> Result<Sent, String> {
    let st = app.state::<AppState>();
    let gen = unlocked(&st)?;
    let a = &app;
    let lock_gen = &st.lock_gen;
    let ask = |ip, name, ids| async move {
        gen.check(lock_gen)?;
        super::viewer::launch_on(a, ip, name, ids).await
    };
    let pull = |ip, name, ids| async move {
        gen.check(lock_gen)?;
        super::viewer::hand_over(a, ip, name, ids).await
    };
    st.remote.clone().send(&user_ids, &ip, ask, pull).await
}
