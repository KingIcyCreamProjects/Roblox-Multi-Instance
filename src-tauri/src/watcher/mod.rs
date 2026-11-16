//! Client watcher (spec §13). `capabilities().watcher` gates it. The process/window calls
//! are per OS in `win`; Linux has processes and logs but no window access, so its title and
//! rect features do nothing. Pure logic lives in `rules`/`logparse`.

pub mod logparse;
pub mod rules;
mod win;

pub use win::kill_client;
#[cfg(windows)]
pub use win::spawn_retitle;
pub(crate) use win::start_ms;

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

#[cfg(windows)]
pub const PROCESS_NAME: &str = "RobloxPlayerBeta.exe";
/// Sober's Roblox process shows up under its main thread's name. Other programs' "Main"
/// processes carry no tracker id, so they are never judged or closed (`is_game_client`).
#[cfg(target_os = "linux")]
pub const PROCESS_NAME: &str = "Main";
#[cfg(target_os = "macos")]
pub const PROCESS_NAME: &str = crate::platform::macos::PLAYER;

/// Maps a roblox-player tracker id to the owning account.
pub trait AccountLookup: Send + Sync {
    /// `(account_id, username)`
    fn by_tracker(&self, tracker: &str) -> Option<(String, String)>;
}

/// Tracker id from a client command line: `browsertrackerid:<n>` (inside the
/// roblox-player URI), `--browsertrackerid <n>` / `=<n>`, or `-b <n>`.
pub fn tracker_from_cmdline(cmd: &str) -> Option<String> {
    use regex::Regex;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(?:browsertrackerid[:=\s]\s*|(?:^|\s)-b\s+)(\d+)").unwrap()
    });
    re.captures(cmd).map(|c| c[1].to_string())
}

/// A game client carries a tracker id or launch args (a bare roblox-player URI only with
/// `launchmode:play`); the desktop/tray app is also a RobloxPlayerBeta.exe but has
/// neither, and must never be judged or closed.
pub fn is_game_client(cmd: &str) -> bool {
    let lc = cmd.to_ascii_lowercase();
    tracker_from_cmdline(cmd).is_some()
        || (lc.contains("roblox-player:") && lc.contains("launchmode:play"))
        || cmd
            .split_whitespace()
            .any(|a| matches!(a, "-j" | "--play" | "--joinurl"))
}

/// Launched to the home screen (empty Play bar, `launchmode:app` + a tracker).
pub fn is_home_launch(cmd: &str) -> bool {
    // No lowercase copy: the command line holds the auth ticket.
    cmd.as_bytes()
        .windows(14)
        .any(|w| w.eq_ignore_ascii_case(b"launchmode:app"))
}

/// Only real game logs: `..._Player_XXXXX_last.log`, never the CrashHandler's.
pub fn is_player_log(name: &str) -> bool {
    name.ends_with(".log") && name.contains("_Player_") && !name.contains("CrashHandler")
}

/// A log created slightly before (clock skew) or up to this long after the client started.
const LOG_BEFORE_MS: i64 = 5_000;
const LOG_AFTER_MS: i64 = 120_000;

/// Log for a client: the player log whose creation time (ms) is nearest the client's
/// start, preferring one created at/after it, skipping logs another client already owns.
/// `started_ms`: precise process creation time, so same-second launches stay apart.
pub fn pick_log<'a>(
    files: &'a [(PathBuf, i64)],
    started_ms: i64,
    claimed: &HashSet<PathBuf>,
) -> Option<&'a Path> {
    files
        .iter()
        .filter(|(p, c)| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_player_log)
                && !claimed.contains(p)
                && (started_ms - LOG_BEFORE_MS..=started_ms + LOG_AFTER_MS).contains(c)
        })
        .min_by_key(|(_, c)| ((*c < started_ms), (c - started_ms).abs()))
        .map(|(p, _)| p.as_path())
}

/// A log created (ms) at/after the client's start (secs) is its own; an earlier one may be
/// a foreign log (an exited client's), so keep re-picking and do not judge on it.
pub fn log_settled(created_ms: i64, started_s: i64) -> bool {
    created_ms >= started_s * 1000
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub pid: u32,
    pub account_id: Option<String>,
    pub username: Option<String>,
    /// Private (committed) memory, not working set.
    pub memory_mb: u64,
    pub title: Option<String>,
    pub started_at: i64,
    /// False for the Roblox desktop/tray app: listed, never closed.
    pub game_client: bool,
    pub last_server_event: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleStatus {
    pub id: &'static str,
    pub enabled: bool,
    pub last_fired_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub clients: Vec<ClientInfo>,
    pub rules: Vec<RuleStatus>,
}

/// Broadcast on every successful close (auto-relaunch); the UI gets `state://changed` instead.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub account_id: Option<String>,
    pub pid: u32,
    pub rule: &'static str,
    pub at: i64,
    /// Closes of this account in the last 10 minutes, including this one.
    pub count: u32,
}

#[derive(Default)]
struct Shared {
    clients: Vec<ClientInfo>,
    last_fired: HashMap<&'static str, i64>,
    /// account id -> recent close times (cooldown).
    fires: HashMap<String, Vec<i64>>,
    /// account id -> recent auto reconnect times (rate limit).
    rejoins: HashMap<String, Vec<i64>>,
}

pub struct Watcher {
    shared: Arc<Mutex<Shared>>,
    tx: tokio::sync::broadcast::Sender<Action>,
    running: AtomicBool,
    /// Bumped on every start/stop; a scanner whose generation is stale does nothing more.
    epoch: Arc<AtomicU64>,
    /// Wakes the current run only (each start gets its own), so a stop can never hand its
    /// permit to the next run while the old one sleeps a full interval.
    wake: Mutex<Arc<tokio::sync::Notify>>,
    /// Clients (pid, start time) already running when the app started.
    existing: OnceLock<HashSet<(u32, i64)>>,
    /// Serializes `reconcile` (read setting + start/stop) against quick toggles.
    reconciling: Mutex<()>,
    /// The current run, so an exit can wait for its final rect flush (`stop_wait`).
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl Default for Watcher {
    fn default() -> Self {
        Self {
            shared: Default::default(),
            tx: tokio::sync::broadcast::channel(32).0,
            running: AtomicBool::new(false),
            epoch: Default::default(),
            wake: Default::default(),
            existing: OnceLock::new(),
            reconciling: Mutex::new(()),
            task: Mutex::new(None),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Watcher {
    /// Auto-relaunch (M8.3) subscribes here: a watcher close counts as a disconnect.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Action> {
        self.tx.subscribe()
    }

    pub fn status(&self, cfg: &rules::Cfg) -> Status {
        let sh = lock(&self.shared);
        Status {
            running: self.running.load(Ordering::Relaxed),
            clients: sh.clients.clone(),
            rules: rules::Rule::ALL
                .iter()
                .map(|r| RuleStatus {
                    id: r.id(),
                    enabled: cfg.enabled(*r),
                    last_fired_at: sh.last_fired.get(r.id()).copied(),
                })
                .collect(),
        }
    }

    fn start(&self, app: tauri::AppHandle) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let my = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let wake = Arc::new(tokio::sync::Notify::new());
        *lock(&self.wake) = wake.clone();
        let t = tauri::async_runtime::spawn(win::run(app, self.epoch.clone(), my, wake));
        *lock(&self.task) = Some(t);
    }

    /// `stop`, then wait (at most `max`) for the run's final flush: on exit the vault is
    /// locked next, and a flush after that can't save the window rects.
    pub(crate) async fn stop_wait(&self, max: std::time::Duration) {
        self.stop();
        let t = lock(&self.task).take();
        if let Some(t) = t {
            if tokio::time::timeout(max, t).await.is_err() {
                log::warn!("watcher: final flush did not finish in time");
            }
        }
    }

    /// Close history (`fires`, `last_fired`) is kept on purpose: a stop/start must not reset
    /// the cooldown that stops a close/relaunch loop.
    pub(crate) fn stop(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            self.epoch.fetch_add(1, Ordering::SeqCst);
            lock(&self.wake).notify_one();
            lock(&self.shared).clients.clear();
        }
    }

    /// Run `my` died on its own: mark stopped unless a newer start or stop already happened.
    fn ended(&self, my: u64) {
        let next = self
            .epoch
            .compare_exchange(my, my + 1, Ordering::SeqCst, Ordering::SeqCst);
        if next.is_ok() {
            self.running.store(false, Ordering::SeqCst);
            lock(&self.shared).clients.clear();
        }
    }
}

/// Remember the clients already running now, once per app run (spec: "ignore clients
/// already running when Roblox Multi Instance starts").
pub fn init(app: &tauri::AppHandle) {
    use tauri::Manager;
    let st = app.state::<crate::AppState>();
    if st.platform.capabilities().watcher {
        let _ = st.watcher.existing.set(win::running_clients());
    }
}

/// Start/stop the scan task to match `/watcher/enabled` (and the platform capability),
/// and make a running scan pick up new settings now. Call at startup and after every
/// settings save.
pub fn reconcile(app: &tauri::AppHandle) {
    use tauri::Manager;
    let st = app.state::<crate::AppState>();
    let _g = lock(&st.watcher.reconciling);
    let want = st.platform.capabilities().watcher
        && lock(&st.settings)
            .pointer("/watcher/enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
    if want {
        st.watcher.start(app.clone());
        lock(&st.watcher.wake).notify_one();
    } else {
        st.watcher.stop();
    }
    crate::commands::state_changed(app, &["watcherStatus"]);
}

/// Vault-backed lookup: `accounts[].trackerId` -> `accounts[].id` / `username`.
/// Locked vault = no mapping (clients show as unknown).
pub struct VaultLookup(pub tauri::AppHandle);

impl VaultLookup {
    pub fn unlocked(&self) -> bool {
        use tauri::Manager;
        lock(&self.0.state::<crate::AppState>().vault).is_some()
    }
}

impl AccountLookup for VaultLookup {
    fn by_tracker(&self, tracker: &str) -> Option<(String, String)> {
        use tauri::Manager;
        let st = self.0.state::<crate::AppState>();
        let g = lock(&st.vault);
        g.as_ref()?
            .payload()
            .accounts
            .iter()
            .find(|a| a.tracker_id == tracker)
            .map(|a| (a.id.clone(), a.username.clone()))
    }
}

/// Store a client's last window rect (left, top, right, bottom) on the account. Blocking
/// vault IO: call off the async runtime. Locked vault: ignored.
fn persist_rect(app: &tauri::AppHandle, account_id: &str, rect: [i32; 4]) {
    use crate::accounts::{cmds, Rect};
    use tauri::Manager;
    let [l, t, r, b] = rect;
    let rect = Rect {
        x: l,
        y: t,
        w: r - l,
        h: b - t,
    };
    let st = app.state::<crate::AppState>();
    let start = crate::commands::Gen::now(&st.lock_gen);
    // Err skips the save (locked, unknown account, unchanged).
    let saved = cmds::mutate(&st, start, |p| {
        match p.accounts.iter_mut().find(|a| a.id == account_id) {
            Some(a) if rect_differs(a.window_rect, rect) => {
                a.window_rect = Some(rect);
                Ok(())
            }
            _ => Err(String::new()),
        }
    });
    if saved.is_ok() {
        cmds::emit_changed(app);
    }
}

fn rect_differs(old: Option<crate::accounts::Rect>, new: crate::accounts::Rect) -> bool {
    old != Some(new)
}

/// Auto reconnect of one account whose disconnected client the watcher just closed.
struct RejoinReq {
    id: String,
    name: String,
    tracker: String,
    last_join: Option<(u64, String)>,
    /// `last_join` was a plain public join of the start place (see `Track::plain_join`).
    plain: bool,
    start_place: Option<u64>,
    same_server: bool,
    delay_s: u64,
}

/// Bar (and `use_bar_for_all`) of an auto reconnect. `target`: the account/group saved
/// target exists, `Some(true)` = it is a private server link.
/// A plain public join with no private target: the last place (+ jobId on the first try).
/// Else (private/reserved server, teleport, private target) the saved target, else the
/// start place on any server.
fn reconnect_bar(
    last_join: Option<(u64, String)>,
    plain: bool,
    start_place: Option<u64>,
    target: Option<bool>,
    same_server: bool,
) -> (crate::launch::resolve::Bar, bool) {
    use crate::launch::resolve::{Bar, Mode};
    match (
        last_join.filter(|_| plain && target != Some(true)),
        start_place,
    ) {
        (Some((place, job)), _) => (
            Bar {
                destination: place.to_string(),
                server: if same_server { job } else { String::new() },
                mode: if same_server {
                    Mode::Specific
                } else {
                    Mode::Any
                },
                ..Default::default()
            },
            true,
        ),
        (None, Some(place)) if target.is_none() => (
            Bar {
                destination: place.to_string(),
                server: String::new(),
                mode: Mode::Any,
                ..Default::default()
            },
            true,
        ),
        _ => (Bar::default(), false),
    }
}

/// Account `id`'s saved target for `reconnect_bar` (`Some(true)` = private link); outer
/// `None` = vault locked.
fn saved_target(st: &crate::AppState, id: &str) -> Option<Option<bool>> {
    let g = lock(&st.vault);
    let p = g.as_ref()?.payload();
    Some(
        p.accounts
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| crate::accounts::target_for(p, a))
            .map(|t| t.private_link.is_some()),
    )
}

/// A friend disconnected account `id` (vault locked = false).
fn friend_paused(st: &crate::AppState, id: &str) -> bool {
    lock(&st.vault)
        .as_ref()
        .is_some_and(|v| crate::accounts::friend_paused(v.payload(), id))
}

/// A reconnect has somewhere to go: a bar from the log, or the saved target. Checked
/// before the close, so a client is never closed for a launch that can't happen.
fn has_destination(
    last_join: Option<(u64, String)>,
    plain: bool,
    start_place: Option<u64>,
    target: Option<bool>,
) -> bool {
    target.is_some() || reconnect_bar(last_join, plain, start_place, target, false).1
}

/// Wait out the delay (and for the network, up to 10 min), then relaunch through the
/// launcher (TURN serializes it with every other batch); target per `reconnect_bar`.
/// Stands down if the account already has a client or an RMI launch. Every give-up or
/// failed launch hands the account back to auto-relaunch (the watcher quieted it).
async fn rejoin(app: tauri::AppHandle, r: RejoinReq) {
    use tauri::Manager;
    let name = r.name.clone();
    if !rejoin_go(&app, r).await {
        app.state::<crate::AppState>()
            .automation
            .relaunch
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unquiet(&name);
    }
}

/// `rejoin`; true = a client runs or launched (no hand-back).
async fn rejoin_go(app: &tauri::AppHandle, r: RejoinReq) -> bool {
    use std::time::Duration;
    use tauri::{Emitter, Manager};
    tokio::time::sleep(Duration::from_secs(r.delay_s)).await;
    // watcher.waitForGame: no launch while a fullscreen app (a game) is in front; every
    // check below runs fresh once it is gone.
    let wait = || {
        let on = {
            let st = app.state::<crate::AppState>();
            let s = lock(&st.settings);
            let b = |p| s.pointer(p) == Some(&serde_json::Value::Bool(true));
            b("/watcher/waitForGame") && b("/watcher/reconnect") && b("/watcher/enabled")
        };
        on && other_app_front().is_some_and(|(h, _)| win::fullscreen(h))
    };
    if wait() {
        log::info!(
            "watcher: a fullscreen app is in front; holding reconnect of {}",
            r.name
        );
        while wait() {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
    let mut tries = 0;
    while !crate::automation::relaunch::online().await {
        tries += 1;
        if tries > 60 {
            log::warn!("watcher: still offline; not reconnecting {}", r.name);
            return false;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    let st = app.state::<crate::AppState>();
    // Turned off during the wait (the watcher itself too: its stop does not end this task).
    let on = {
        let s = lock(&st.settings);
        let b = |p| s.pointer(p) == Some(&serde_json::Value::Bool(true));
        b("/watcher/enabled") && b("/watcher/reconnect")
    };
    if !on {
        return false;
    }
    if friend_paused(&st, &r.id) {
        log::info!("watcher: {} is friend-paused; not reconnecting", r.name);
        return false;
    }
    let Some(target) = saved_target(&st, &r.id) else {
        log::info!("watcher: vault locked; not reconnecting {}", r.name);
        return false;
    };
    // The saved target may have been cleared during the delay.
    if !has_destination(r.last_join.clone(), r.plain, r.start_place, target) {
        log::info!("watcher: not reconnecting {}: no destination known", r.name);
        return false;
    }
    let (bar, all) = reconnect_bar(r.last_join, r.plain, r.start_place, target, r.same_server);
    // Foreground before the launch: guard it only when it is not Roblox or RMI.
    let prev = other_app_front();
    let (a, tracker) = (app.clone(), r.tracker.clone());
    let running = tauri::async_runtime::spawn_blocking(move || {
        !a.state::<crate::AppState>()
            .platform
            .tracked_pids(&tracker)
            .is_empty()
    })
    .await
    .unwrap_or(false);
    if running {
        log::info!("watcher: {} already has a client; not reconnecting", r.name);
        return true;
    }
    // A queued / validating RMI launch has no process yet. Not its grace: that may belong to
    // the launch whose client was just closed.
    if st.automation.own_running(&r.id) {
        log::info!("watcher: {} has an RMI launch; not reconnecting", r.name);
        return true;
    }
    // A pause can land while the close ran: start_batch would clear it.
    if friend_paused(&st, &r.id) {
        log::info!("watcher: {} is friend-paused; not reconnecting", r.name);
        return false;
    }
    let batch = match crate::launch::launcher::start_batch(app, vec![r.id], bar, all, false) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("watcher: reconnect of {} failed: {e}", r.name);
            return false;
        }
    };
    let _ = app.emit(
        "watcher://reconnect",
        serde_json::json!({ "username": r.name }),
    );
    if let Some((prev, _)) = prev {
        let b = batch.clone();
        let _ =
            tauri::async_runtime::spawn_blocking(move || keep_focus(prev, &r.tracker, &b)).await;
    }
    // Follow the batch to its end (at most 10 min): no launch = hand back.
    for _ in 0..600 {
        match crate::launch::launcher::batch_status(&batch) {
            Some(v) if v["done"] == true => return v["launched"].as_u64().unwrap_or(0) > 0,
            None => return true, // evicted: unknown, don't risk a second launch
            _ => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
    true
}

/// The foreground window and its pid, unless it is RMI or a Roblox client.
fn other_app_front() -> Option<(isize, u32)> {
    win::foreground().filter(|(_, pid)| {
        *pid != std::process::id() && {
            let mut s = sysinfo::System::new();
            let p = sysinfo::Pid::from_u32(*pid);
            s.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[p]), true);
            !s.process(p)
                .is_some_and(|p| p.name().eq_ignore_ascii_case(PROCESS_NAME))
        }
    })
}

/// Until the batch ends (+30 s, at most 10 min): if the new client takes the foreground
/// from `prev`, hand it back. Stops when the user moves to any other window, and after
/// the first hand-back, or the fifth when `prev` is fullscreen (a game: the client may
/// grab again while it loads; more than that is the user wanting the client).
// ponytail: a 50 ms poll, not a WinEvent hook (no message loop to run). Best effort:
// Windows' foreground lock can refuse SetForegroundWindow to a background process, so the
// fallback minimizes the new client (its restore rect, restoreWindowRect and Grid
// Arrange's SW_RESTORE stay intact). A bootstrapper window first is not covered.
fn keep_focus(prev: isize, tracker: &str, batch: &str) {
    use std::time::{Duration, Instant};
    let hard = Instant::now() + Duration::from_secs(600);
    let (mut end, mut gave, mut n) = (hard, 0u32, 0u32);
    let game = win::fullscreen(prev);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
        n += 1;
        if end == hard
            && n % 10 == 0
            && crate::launch::launcher::batch_status(batch).is_none_or(|v| v["done"] == true)
        {
            end = Instant::now() + Duration::from_secs(30);
        }
        match win::foreground() {
            Some((fg, _)) if fg == prev => {}
            Some((fg, pid)) if win::is_tracked(pid, tracker) => {
                win::refocus(prev, fg);
                if gave == 0 {
                    log::info!("watcher: gave the foreground back after reconnect");
                }
                gave += 1;
                if !game || gave >= 5 {
                    return;
                }
            }
            Some(_) => return,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejoin_skips_friend_paused() {
        let d = tempfile::tempdir().unwrap();
        let st = crate::test_state(
            d.path(),
            crate::roblox::RobloxClient::new(crate::roblox::BaseUrls::prod()),
        );
        let acct = serde_json::from_value(serde_json::json!({"id": "A"})).unwrap();
        let mut g = lock(&st.vault);
        g.as_mut().unwrap().payload_mut().accounts.push(acct);
        drop(g);
        assert!(!friend_paused(&st, "A"));
        lock(&st.vault).as_mut().unwrap().payload_mut().accounts[0].friend_paused = true;
        assert!(friend_paused(&st, "A"));
        assert!(!friend_paused(&st, "nope"));
    }

    #[test]
    fn rect_only_if_changed() {
        use crate::accounts::Rect;
        let r = Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        };
        assert!(rect_differs(None, r));
        assert!(!rect_differs(Some(r), r));
        assert!(rect_differs(Some(Rect { x: 0, ..r }), r));
    }

    #[test]
    fn reconnect_bar_decisions() {
        use crate::launch::resolve::Mode;
        let j = || Some((5u64, "job".to_string()));
        let b = |lj, plain, sp, t, same| {
            let (b, all) = reconnect_bar(lj, plain, sp, t, same);
            (b.destination, b.server, b.mode, all)
        };
        let s = |x: &str| x.to_string();
        // plain public join: last place, jobId on the first try only
        assert_eq!(
            b(j(), true, Some(5), None, true),
            (s("5"), s("job"), Mode::Specific, true)
        );
        assert_eq!(
            b(j(), true, Some(5), Some(false), false),
            (s("5"), s(""), Mode::Any, true)
        );
        // private target wins over a plain join
        assert_eq!(
            b(j(), true, Some(5), Some(true), true),
            (s(""), s(""), Mode::default(), false)
        );
        // private server / teleport: saved target, else the start place on any server
        assert_eq!(
            b(j(), false, Some(1), Some(false), true),
            (s(""), s(""), Mode::default(), false)
        );
        assert_eq!(
            b(j(), false, Some(1), None, true),
            (s("1"), s(""), Mode::Any, true)
        );
        assert_eq!(
            b(None, true, None, None, true),
            (s(""), s(""), Mode::default(), false)
        );
    }

    #[test]
    fn tracker_variants() {
        let t = |s: &str| tracker_from_cmdline(s);
        assert_eq!(
            t("RobloxPlayerBeta.exe roblox-player:1+launchmode:play+gameinfo:abc+browsertrackerid:123456789+robloxLocale:en_us"),
            Some("123456789".into())
        );
        assert_eq!(t("x.exe --browsertrackerid 42 --app"), Some("42".into()));
        assert_eq!(t("x.exe --browsertrackerid=77"), Some("77".into()));
        assert_eq!(
            t("x.exe --app -t tok -j url -b 555 --launchtime=1"),
            Some("555".into())
        );
        assert_eq!(t("BROWSERTRACKERID:9"), Some("9".into()));
    }

    #[test]
    fn game_vs_tray_client() {
        assert!(is_game_client(
            "x.exe --app -t tok -j https://x -b 5 --launchtime=1"
        ));
        assert!(is_game_client("x.exe roblox-player:1+launchmode:play"));
        assert!(!is_game_client("x.exe roblox-player:1+launchmode:app"));
        assert!(is_game_client(
            "x.exe roblox-player:1+launchmode:app+browsertrackerid:7"
        ));
        assert!(is_home_launch(
            "x.exe roblox-player:1+launchmode:app+browsertrackerid:7"
        ));
        assert!(!is_home_launch(
            "x.exe roblox-player:1+launchmode:play+browsertrackerid:7"
        ));
        assert!(is_game_client("x.exe --play -t tok"));
        assert!(!is_game_client(
            "C:/Roblox/Versions/v/RobloxPlayerBeta.exe --app"
        ));
        assert!(!is_game_client(""));
        // the tray app's own log must not look like a server event either
        let line = "2026-10-20T06:23:08.324Z,0.324703,5b84,6,Info [FLog::Systray] Systray initialized in sync mode";
        assert_eq!(logparse::LogRules::embedded().classify(line), None);
    }

    #[test]
    fn tracker_absent() {
        assert_eq!(tracker_from_cmdline("x.exe --app -b notnum"), None);
        assert_eq!(tracker_from_cmdline("x.exe --sub-b 12"), None);
        assert_eq!(tracker_from_cmdline(""), None);
    }

    #[test]
    fn player_log_names() {
        assert!(is_player_log(
            "0.744.0.7441080_20261024T070146Z_Player_3E7C8_last.log"
        ));
        assert!(!is_player_log(
            "0.744.0.7441080_20261024T070146Z_Player_BC909_CrashHandler_last.log"
        ));
        assert!(!is_player_log("RobloxPlayerInstaller_E5283.log"));
        assert!(!is_player_log("cacert.pem"));
    }

    fn f(n: &str, c: i64) -> (PathBuf, i64) {
        (PathBuf::from(format!("x_Player_{n}_last.log")), c)
    }

    #[test]
    fn pick_log_nearest_player_log() {
        let none = HashSet::new();
        let crash = (PathBuf::from("x_Player_C_CrashHandler_last.log"), 1_000_000);
        let files = vec![
            crash,
            f("old", 900_000),
            f("a", 1_010_000),
            f("b", 1_002_000),
        ];
        assert_eq!(
            pick_log(&files, 1_000_000, &none),
            Some(files[3].0.as_path())
        );
        // crash handler alone is never picked
        assert_eq!(pick_log(&files[..1], 1_000_000, &none), None);
        // too old
        assert_eq!(pick_log(&files[1..2], 1_000_000, &none), None);
    }

    #[test]
    fn pick_log_prefers_after_start_and_skips_claimed() {
        let files = vec![f("before", 997_000), f("after", 1_004_000)];
        let none = HashSet::new();
        // 3 s before vs 4 s after: after wins because created >= started is preferred
        assert_eq!(
            pick_log(&files, 1_000_000, &none),
            Some(files[1].0.as_path())
        );
        let claimed = HashSet::from([files[1].0.clone()]);
        assert_eq!(
            pick_log(&files, 1_000_000, &claimed),
            Some(files[0].0.as_path())
        );
        // ms precision: 400 ms apart still ordered
        let t = vec![f("p", 1_000_700), f("q", 1_000_300)];
        assert_eq!(pick_log(&t, 1_000_000, &none), Some(t[1].0.as_path()));
    }

    #[test]
    fn pick_log_same_second_batch_keeps_each_own_log() {
        // Two clients started 500 ms apart within one second, each log 50 ms after start.
        let files = vec![f("b", 1_000_650), f("a", 1_000_150)];
        let none = HashSet::new();
        assert_eq!(
            pick_log(&files, 1_000_100, &none),
            Some(files[1].0.as_path())
        );
        assert_eq!(
            pick_log(&files, 1_000_600, &none),
            Some(files[0].0.as_path())
        );
    }

    #[test]
    fn log_settled_only_at_or_after_start() {
        assert!(log_settled(1_000_000, 1_000));
        assert!(log_settled(1_000_001, 1_000));
        assert!(!log_settled(999_999, 1_000));
    }

    #[test]
    fn destination_from_target_or_bar() {
        assert!(
            has_destination(None, false, None, Some(true)),
            "saved target"
        );
        assert!(has_destination(None, false, None, Some(false)));
        assert!(has_destination(None, true, Some(1), None), "start place");
        assert!(!has_destination(None, true, None, None));
        assert!(
            !has_destination(Some((5, "j".into())), false, None, None),
            "private join, no target"
        );
    }

    #[test]
    fn status_lists_rules_with_last_fired() {
        let w = Watcher::default();
        lock(&w.shared).last_fired.insert("lowMemory", 123);
        let cfg =
            rules::Cfg::from_settings(&serde_json::json!({"watcher": {"closeLowMemory": true}}));
        let s = w.status(&cfg);
        assert!(!s.running);
        assert_eq!(s.rules.len(), rules::Rule::ALL.len());
        let low = s.rules.iter().find(|r| r.id == "lowMemory").unwrap();
        assert!(low.enabled);
        assert_eq!(low.last_fired_at, Some(123));
        let no = s.rules.iter().find(|r| r.id == "noServer").unwrap();
        assert!(!no.enabled && no.last_fired_at.is_none());
    }

    #[test]
    fn stale_run_cannot_end_newer_one() {
        let w = Watcher::default();
        w.running.store(true, Ordering::SeqCst);
        w.epoch.store(5, Ordering::SeqCst);
        w.ended(4);
        assert!(w.running.load(Ordering::SeqCst), "run 4 is stale");
        w.ended(5);
        assert!(!w.running.load(Ordering::SeqCst));
        assert_eq!(w.epoch.load(Ordering::SeqCst), 6);
        // stop bumps the epoch only when running
        w.running.store(true, Ordering::SeqCst);
        w.stop();
        assert_eq!(w.epoch.load(Ordering::SeqCst), 7);
        w.stop();
        assert_eq!(w.epoch.load(Ordering::SeqCst), 7, "already stopped");
    }

    #[test]
    fn pick_log_window_edges_inclusive() {
        let none = HashSet::new();
        let start = 1_000_000;
        let at = |c: i64| vec![f("x", c)];
        // [start - 5 s, start + 120 s], both ends included
        assert!(pick_log(&at(start - LOG_BEFORE_MS), start, &none).is_some());
        assert!(pick_log(&at(start - LOG_BEFORE_MS - 1), start, &none).is_none());
        assert!(pick_log(&at(start + LOG_AFTER_MS), start, &none).is_some());
        assert!(pick_log(&at(start + LOG_AFTER_MS + 1), start, &none).is_none());
    }

    #[test]
    fn tracker_bare_flag_at_start() {
        assert_eq!(tracker_from_cmdline("-b 7 --app"), Some("7".into()));
        assert_eq!(tracker_from_cmdline("-b"), None);
    }
}
