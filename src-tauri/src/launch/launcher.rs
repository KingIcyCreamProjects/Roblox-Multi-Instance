//! Batch launcher (spec §6.3): per account validate → FPS → replace previous → ticket →
//! URI → open → optional window rect, staggered by `/launch/delayMs`, cancellable.

use super::clientsettings::write_fps;
use super::recents::{self, Recent};
use super::resolve::{
    cached_place, effective_bar, join_request, place_info, preview, Bar, Preview,
};
use super::uri::{build_launch_uri, build_place_launcher_url};
use crate::accounts::cmds::{expire_session, mutate};
use crate::accounts::{new_id, now_iso, SessionState};
use crate::commands::{ensure_multi, guard, CmdResult, Gen, LOCKED};
use crate::platform::RobloxInstall;
use crate::roblox::{
    api::{auth, users},
    RobloxError, Secret,
};
use crate::AppState;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    path::PathBuf,
    sync::{LazyLock, Mutex, PoisonError},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub batch_id: String,
    pub account_id: String,
    /// queued | validating | launching | launched | failed | cancelled
    pub state: &'static str,
    pub message: Option<String>,
    pub game_name: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub batch_id: String,
    pub launched: usize,
    pub failed: usize,
    pub cancelled: usize,
}

/// Why one account didn't launch.
#[derive(Debug, PartialEq)]
pub enum Fail {
    /// User-facing message.
    Error(String),
    Cancelled,
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail::Error(s)
    }
}

impl From<&str> for Fail {
    fn from(s: &str) -> Self {
        Fail::Error(s.into())
    }
}

/// Runs `launch` for each id in order, starts at least `delay` apart. `launch` returns the
/// game name or why it didn't launch; one failure never stops the batch. Once `launch` has
/// handed the URI to the OS it can no longer be cancelled.
pub async fn run_batch<F, Fut>(
    batch_id: &str,
    ids: Vec<String>,
    delay: Duration,
    cancel: &CancellationToken,
    emit: &(dyn Fn(Progress) + Send + Sync),
    launch: F,
) -> Done
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Option<String>, Fail>>,
{
    let p = |id: &str, state, message, game_name| Progress {
        batch_id: batch_id.into(),
        account_id: id.into(),
        state,
        message,
        game_name,
    };
    for id in &ids {
        emit(p(id, "queued", None, None));
    }
    let mut d = Done {
        batch_id: batch_id.into(),
        ..Default::default()
    };
    let mut next = tokio::time::Instant::now();
    for id in ids {
        if !cancel.is_cancelled() {
            cancel
                .run_until_cancelled(tokio::time::sleep_until(next))
                .await;
        }
        if cancel.is_cancelled() {
            d.cancelled += 1;
            emit(p(&id, "cancelled", None, None));
            continue;
        }
        next = tokio::time::Instant::now() + delay;
        match launch(id.clone()).await {
            Ok(game) => {
                d.launched += 1;
                emit(p(&id, "launched", None, game));
            }
            Err(Fail::Error(m)) => {
                d.failed += 1;
                emit(p(&id, "failed", Some(m), None));
            }
            Err(Fail::Cancelled) => {
                d.cancelled += 1;
                emit(p(&id, "cancelled", None, None));
            }
        }
    }
    d
}

/// Where to write ClientAppSettings.json: always on a vanilla install (so the custom file is
/// merged every launch), never under a bootstrapper (it owns it).
pub fn settings_dir(install: Option<&RobloxInstall>) -> Option<PathBuf> {
    install
        .filter(|i| i.bootstrapper.is_none())?
        .version_dir
        .clone()
}

/// FPS cap and graphics level (1-10) to start this account's client with; the account's own
/// beats the global one, `None` leaves Roblox's value.
pub fn start_caps(acc_fps: Option<u32>, acc_q: Option<u8>, s: &Value) -> (Option<u32>, Option<u8>) {
    let fps = flag(s, "/fps/unlock").then(|| num(s, "/fps/value", 60) as u32);
    let q = flag(s, "/graphics/set").then(|| num(s, "/graphics/level", 1) as u8);
    (acc_fps.or(fps), acc_q.or(q))
}

const PROGRESS: &str = "launch://progress";
const DONE: &str = "launch://done";
const EXPIRED: &str = "Session expired. Sign in again.";

static BATCHES: LazyLock<Mutex<HashMap<String, CancellationToken>>> =
    LazyLock::new(Default::default);
/// Accounts with a launch in progress, across batches.
static IN_FLIGHT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// A launch batch is running or waiting its turn.
pub fn busy() -> bool {
    !BATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty()
}

/// After a batch ends, how long its accounts stay "ours" for auto-relaunch (old client
/// exit, socket drop).
pub const OWN_GRACE: Duration = Duration::from_secs(30);

/// Error code: this account already has a launch running.
pub const LAUNCH_BUSY: &str = "LAUNCH_BUSY";

struct Flight(String);

impl Flight {
    fn take(id: &str) -> Option<Self> {
        let mut g = IN_FLIGHT.lock().unwrap_or_else(PoisonError::into_inner);
        g.insert(id.to_owned()).then(|| Flight(id.to_owned()))
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        IN_FLIGHT
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// What the launcher needs from the app: state + events. `AppHandle` in the app, a fake in tests.
pub trait Host: Clone + Send + Sync + 'static {
    fn st(&self) -> &AppState;
    fn emit_json(&self, event: &str, payload: Value);
    /// Fetch missing account headshots in the background.
    fn kick_headshots(&self) {}
}

impl Host for AppHandle {
    fn st(&self) -> &AppState {
        self.state::<AppState>().inner()
    }
    fn kick_headshots(&self) {
        crate::accounts::sync::kick_headshots(self);
    }
    fn emit_json(&self, event: &str, payload: Value) {
        if let Err(e) = Emitter::emit(self, event, payload) {
            log::warn!("could not emit {event}: {e}");
        }
    }
}

fn emit<H: Host>(h: &H, event: &str, p: impl Serialize) {
    let v = serde_json::to_value(p).unwrap_or_default();
    if event == PROGRESS || event == DONE {
        record(event == DONE, &v);
    }
    h.emit_json(event, v);
}

/// Last progress per batch (and its Done), for the API's `GET /v1/launches/{id}`.
static STATUS: LazyLock<Mutex<VecDeque<(String, Value)>>> = LazyLock::new(Default::default);
const STATUS_MAX: usize = 200;

fn record(done: bool, v: &Value) {
    let Some(id) = v["batchId"].as_str() else {
        return;
    };
    let mut g = STATUS.lock().unwrap_or_else(PoisonError::into_inner);
    let i = match g.iter().position(|(b, _)| b == id) {
        Some(i) => i,
        None => {
            if g.len() >= STATUS_MAX {
                g.pop_front();
            }
            g.push_back((
                id.to_owned(),
                serde_json::json!({ "launchId": id, "done": false }),
            ));
            g.len() - 1
        }
    };
    let e = &mut g[i].1;
    if done {
        e["done"] = true.into();
        for k in ["launched", "failed", "cancelled"] {
            e[k] = v[k].clone();
        }
    } else {
        for k in ["accountId", "state", "message", "gameName"] {
            e[k] = v[k].clone();
        }
        // Every account's latest state, not only the last one's.
        if let Some(a) = v["accountId"].as_str() {
            e["accounts"][a] = serde_json::json!({
                "state": v["state"], "message": v["message"], "gameName": v["gameName"]
            });
        }
    }
}

/// `{launchId, accountId, state, message, gameName, done, accounts: {id: {state, message,
/// gameName}}, launched?, failed?, cancelled?}`; the top-level account fields are the latest event.
pub fn batch_status(id: &str) -> Option<Value> {
    STATUS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(b, _)| b == id)
        .map(|(_, v)| v.clone())
}

fn num(s: &Value, p: &str, d: u64) -> u64 {
    s.pointer(p).and_then(Value::as_u64).unwrap_or(d)
}

fn flag(s: &Value, p: &str) -> bool {
    s.pointer(p).and_then(Value::as_bool).unwrap_or(false)
}

fn user_err(e: RobloxError) -> String {
    match e {
        RobloxError::Unauthorized => EXPIRED.into(),
        RobloxError::Status { code, body_snippet } => {
            log::debug!("Roblox HTTP {code}: {body_snippet}");
            RobloxError::Status {
                code,
                body_snippet: String::new(),
            }
            .to_string()
        }
        e => e.to_string(),
    }
}

pub(crate) fn library(st: &AppState) -> PathBuf {
    st.paths.root.join("library.json")
}

/// File I/O, vault saves and platform calls block: keep them off async workers.
async fn blocking<H: Host, T: Send + 'static>(
    h: &H,
    f: impl FnOnce(&AppState) -> T + Send + 'static,
) -> Result<T, String> {
    let h = h.clone();
    tauri::async_runtime::spawn_blocking(move || f(h.st()))
        .await
        .map_err(|e| format!("internal error: {e}"))
}

async fn cancellable<T>(c: &CancellationToken, f: impl Future<Output = T>) -> Result<T, Fail> {
    c.run_until_cancelled(f).await.ok_or(Fail::Cancelled)
}

/// The validated copy `client_settings_set` stored in the data dir, if a file is chosen.
fn custom_settings(s: &Value, st: &AppState) -> Option<Value> {
    s.pointer("/client/customSettingsPath")?.as_str()?;
    match std::fs::read(st.paths.root.join(crate::commands::CLIENT_SETTINGS_COPY))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("custom client settings unreadable, skipped: {e}");
            None
        }
    }
}

/// Saved session is dead: mark the account (unless its cookie was replaced meanwhile) and
/// fail with the sign-in hint.
async fn expire<H: Host>(h: &H, id: &str, cookie: &Secret) -> Fail {
    let (id2, ck) = (id.to_owned(), Secret::new(cookie.expose()));
    match blocking(h, move |st| expire_session(st, &id2, &ck)).await {
        Ok(Ok(true)) => h.emit_json("accounts://changed", Value::Null),
        Ok(Ok(false)) => {}
        Ok(Err(e)) | Err(e) => log::debug!("session state not saved: {e}"),
    }
    EXPIRED.into()
}

pub async fn launch_one<H: Host>(
    h: &H,
    cancel: &CancellationToken,
    batch_id: &str,
    id: &str,
    bar: &Bar,
    use_bar_for_all: bool,
) -> Result<Option<String>, Fail> {
    let _flight = Flight::take(id).ok_or(LAUNCH_BUSY)?;
    let st = h.st();
    let start = Gen::now(&st.lock_gen);
    let step = |state| {
        let p = Progress {
            batch_id: batch_id.into(),
            account_id: id.into(),
            state,
            message: None,
            game_name: None,
        };
        emit(h, PROGRESS, p);
    };
    let (acc, target, cookie) = {
        let g = guard(&st.vault);
        let p = g.as_ref().ok_or(LOCKED)?.payload();
        let a = p
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found.")?;
        let c = p
            .secrets
            .get(id)
            .filter(|s| !s.cookie.is_empty())
            .map(|s| Secret::new(s.cookie.as_str()))
            .ok_or(EXPIRED)?;
        (a.clone(), crate::accounts::target_for(p, a), c)
    };
    let s = st
        .settings
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let tracker = acc.tracker_id.clone();

    step("validating");
    match cancellable(cancel, users::authenticated(&st.roblox, &cookie)).await? {
        Ok(_) => {}
        Err(RobloxError::Unauthorized) => return Err(expire(h, id, &cookie).await),
        Err(e) => return Err(user_err(e).into()),
    }
    let bar = effective_bar(target.as_ref(), bar, use_bar_for_all);
    let max = num(&s, "/recents/max", 20) as usize;
    let pages = num(&s, "/launch/randomPages", 5) as u32;
    let recents = blocking(h, move |st| recents::list(&library(st), max)).await?;
    let (req, place) = cancellable(
        cancel,
        join_request(&st.roblox, &bar, &cookie, &recents, pages),
    )
    .await??;
    let ticket = match cancellable(cancel, auth::ticket(&st.roblox, &cookie)).await? {
        Ok(t) => Zeroizing::new(t),
        Err(RobloxError::Unauthorized) => return Err(expire(h, id, &cookie).await),
        Err(e) => return Err(user_err(e).into()),
    };
    // Commit point: past here cancel is ignored, so a closed previous client is always
    // replaced (cancel never leaves the account with no client). Only a lock still stops it.
    // A friend Disconnect after start_batch cleared the pause still stops this launch.
    let paused = || {
        guard(&st.vault)
            .as_ref()
            .is_some_and(|v| crate::accounts::friend_paused(v.payload(), id))
    };
    if cancel.is_cancelled() || paused() {
        return Err(Fail::Cancelled);
    }
    start.check(&st.lock_gen)?;

    step("launching");
    let install = blocking(h, |st| st.platform.roblox_install()).await?;
    if crate::commands::install_changed(install.as_ref()) {
        h.emit_json("state://changed", serde_json::json!(["roblox"])); // Roblox updated or bootstrapper changed
    }
    if flag(&s, "/launch/replacePrevious") {
        let t = tracker.clone();
        match blocking(h, move |st| st.platform.close_tracked(&t)).await? {
            Ok(0) => {}
            Ok(n) => log::info!("closed {n} previous client(s) of account {id}"),
            Err(e) => log::warn!("closing the previous client failed: {e}"),
        }
    }
    // After the close (the old client may have owned the mutex / cookie file). Logged inside;
    // without the mutex sequential launches still work. Idle tray players own the mutex:
    // closed first, or the holder never gets it.
    blocking(h, |st| {
        st.platform.close_idle_trays();
        drop(ensure_multi(st))
    })
    .await?;
    let dir = settings_dir(install.as_ref());
    let (fps, quality) = start_caps(acc.fps_cap, acc.graphics, &s);
    let s2 = s.clone();
    blocking(h, move |st| {
        // FPS goes through Roblox's own settings file now; None also clears our old flag.
        // After the close: an exiting client writes that file back. Per-account values in a
        // batch are best-effort: a client reads it at start, the next launch rewrites it (a
        // launch with no override restores the user's value from before RMI's first one).
        if let Some(dir) = dir {
            if let Err(e) = write_fps(&dir, None, custom_settings(&s2, st).as_ref()) {
                log::warn!("client settings not written: {e}");
            }
        }
        if let Err(e) = super::basicsettings::write(&st.paths.root, quality, fps) {
            log::warn!("graphics and FPS not written: {e}");
        }
    })
    .await?;
    let t = tracker.clone();
    let before = blocking(h, move |st| st.platform.tracked_pids(&t)).await?;
    // Locked meanwhile: nothing more opens with this vault's session.
    start.check(&st.lock_gen)?;
    if paused() {
        return Err(Fail::Cancelled);
    }
    let url = req.map(|r| build_place_launcher_url(&r, &tracker, &new_id()));
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let uri = Zeroizing::new(build_launch_uri(&ticket, ms, &tracker, url.as_deref()));
    drop(ticket);
    blocking(h, move |st| st.platform.open_uri(&uri))
        .await?
        .map_err(|e| format!("Couldn't start Roblox: {e}"))?;

    // Launched: nothing below can fail the account.
    let now = now_iso();
    let (id2, now2) = (id.to_owned(), now.clone());
    let saved = blocking(h, move |st| {
        // Routine save: Vault::save throttles backups (one per 5 min at most).
        mutate(st, start, |p| {
            if let Some(a) = p.accounts.iter_mut().find(|a| a.id == id2) {
                a.last_launched_at = Some(now2);
            }
            Ok(())
        })
    })
    .await;
    match saved {
        Ok(Ok(())) => h.emit_json("accounts://changed", Value::Null),
        Ok(Err(e)) | Err(e) => log::debug!("lastLaunchedAt not saved: {e}"),
    }
    if let Some(pid) = place {
        tauri::async_runtime::spawn(push_recent(h.clone(), pid, now, max));
    }
    // Window titled with the username, "Roblox" in privacy mode (Grid Arrange relies on the
    // tracker, not the title).
    let rect = acc
        .window_rect
        .filter(|_| flag(&s, "/launch/restoreWindowRect"))
        .map(|r| (r.x, r.y, r.w, r.h));
    let minimize = flag(&s, "/launch/startMinimized");
    let (h2, name) = (h.clone(), crate::settings::window_title(&s, &acc.username));
    let (id2, who) = (id.to_owned(), acc.username.clone());
    tauri::async_runtime::spawn_blocking(move || {
        match h2
            .st()
            .platform
            .setup_window(&tracker, &name, rect, &before, minimize)
        {
            Ok(()) => log::info!("window of {who} set up"),
            Err(e) => log::info!("window of {who} not set up: {e}"),
        }
        close_if_paused(h2.st(), &id2, &tracker);
    });
    Ok(place.and_then(cached_place).map(|p| p.name))
}

/// A friend Disconnect between the last pause check and the client's start closed nothing:
/// close the client now that its window is up (or setup gave up). Blocking.
fn close_if_paused(st: &AppState, id: &str, tracker: &str) {
    let paused = guard(&st.vault)
        .as_ref()
        .is_some_and(|v| crate::accounts::friend_paused(v.payload(), id));
    if paused {
        match st.platform.close_tracked(tracker) {
            Ok(n) => log::info!("friend paused during launch: closed {n} client(s) of {id}"),
            Err(e) => log::warn!("friend paused during launch; close failed: {e}"),
        }
    }
}

/// After "launched" is out: look the game up (cached 10 min) and record it.
async fn push_recent<H: Host>(h: H, pid: u64, now: String, max: usize) {
    let info = place_info(&h.st().roblox, pid).await.ok();
    let r = Recent {
        place_id: pid,
        name: info
            .as_ref()
            .map_or_else(|| format!("Place {pid}"), |i| i.name.clone()),
        thumbnail: info.and_then(|i| i.icon),
        last_played_at: now,
    };
    match blocking(&h, move |st| recents::push(&library(st), r, max)).await {
        Ok(Ok(())) => h.emit_json("recents://changed", Value::Null),
        Ok(Err(e)) => log::warn!("recent not saved: {e}"),
        Err(e) => log::warn!("recent not saved: {e}"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchId {
    batch_id: String,
}

#[tauri::command]
pub async fn launch_start(
    app: AppHandle,
    account_ids: Vec<String>,
    bar: Bar,
    use_bar_for_all: bool,
) -> CmdResult<BatchId> {
    start_batch(&app, account_ids, bar, use_bar_for_all, true).map(|batch_id| BatchId { batch_id })
}

/// Ends a batch on drop, so a panicking batch still does: leaves `BATCHES`, ends the
/// own-launch mark, emits `launch://done` (a batch with no result counts all as cancelled).
struct Own<H: Host> {
    h: H,
    batch: String,
    ids: Vec<String>,
    done: Option<Done>,
}

impl<H: Host> Drop for Own<H> {
    fn drop(&mut self) {
        BATCHES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.batch);
        let t = tokio::time::Instant::now() + OWN_GRACE;
        self.h.st().automation.own_end(&self.ids, t);
        let d = self.done.take().unwrap_or_else(|| {
            log::error!("launch batch ended without a result");
            Done {
                batch_id: self.batch.clone(),
                cancelled: self.ids.len(),
                ..Default::default()
            }
        });
        emit(&self.h, DONE, d);
    }
}

/// One batch runs at a time, whatever started it (UI, API, auto-relaunch): no interleaving.
/// Holds when the next batch may launch, so `/launch/delayMs` also spaces batches apart.
static TURN: LazyLock<tokio::sync::Mutex<Option<tokio::time::Instant>>> =
    LazyLock::new(Default::default);

/// Live batches with their latest per-account states, for the UI to resync after a reload.
#[tauri::command]
pub fn launch_status() -> Vec<Value> {
    let ids: Vec<String> = BATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .keys()
        .cloned()
        .collect();
    ids.iter().filter_map(|id| batch_status(id)).collect()
}

/// Lock: cancel every batch (accounts not yet past their commit point stop).
pub fn cancel_all() {
    for t in BATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
    {
        t.cancel();
    }
}

/// An automated launch of a friend-paused account.
pub const FRIEND_PAUSED: &str = "Your friend disconnected this account. Reconnect it first.";

/// Validate, then run the batch in the background. Returns the batch id. Also used by the
/// automation API and auto-relaunch. `clear_pause`: a user's launch (UI, friend Reconnect)
/// ends a friend pause; an automated one (API, relaunch, watcher) is refused instead, so a
/// friend Disconnect landing after its own pause check is never undone.
pub fn start_batch<H: Host>(
    h: &H,
    mut account_ids: Vec<String>,
    bar: Bar,
    use_bar_for_all: bool,
    clear_pause: bool,
) -> CmdResult<String> {
    let mut seen = HashSet::new();
    account_ids.retain(|i| seen.insert(i.clone()));
    if account_ids.is_empty() {
        return Err("Pick at least one account.".into());
    }
    let st = h.st();
    // Taken before the unlocked check: a lock landing before the batch is in `BATCHES`
    // (its cancel_all misses it) is caught right after the insert below.
    let gen = Gen::now(&st.lock_gen);
    if guard(&st.vault).is_none() {
        return Err(LOCKED.into());
    }
    let delay = {
        let s = st.settings.lock().unwrap_or_else(PoisonError::into_inner);
        Duration::from_millis(num(&s, "/launch/delayMs", 2500))
    };
    let paused = guard(&st.vault).as_ref().is_some_and(|v| {
        let p = v.payload();
        account_ids
            .iter()
            .any(|i| crate::accounts::friend_paused(p, i))
    });
    if paused && !clear_pause {
        return Err(FRIEND_PAUSED.into());
    }
    // Counted before the pause save: a second Reconnect during that save sees it running.
    st.automation.own_begin(&account_ids);
    if paused {
        let ids = account_ids.clone();
        let r = mutate(st, gen, move |p| {
            for a in p.accounts.iter_mut().filter(|a| ids.contains(&a.id)) {
                a.friend_paused = false;
            }
            Ok(())
        });
        match r {
            Ok(()) => h.emit_json("accounts://changed", Value::Null),
            Err(e) => log::warn!("could not clear friend pause: {e}"),
        }
    }
    let batch_id = new_id();
    let token = CancellationToken::new();
    BATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(batch_id.clone(), token.clone());
    if !gen.current(&st.lock_gen) {
        token.cancel(); // locked meanwhile: run_batch reports every account cancelled
    }
    // Known to `batch_status` / `launch_status` before the task runs (it may wait its turn).
    for id in &account_ids {
        record(
            false,
            &serde_json::json!({ "batchId": batch_id, "accountId": id, "state": "queued" }),
        );
    }
    let (a, b) = (h.clone(), batch_id.clone());
    // Made before the spawn: if the task never runs, dropping it still ends the own mark.
    let own = Own {
        h: a.clone(),
        batch: b.clone(),
        ids: account_ids.clone(),
        done: None,
    };
    tauri::async_runtime::spawn(async move {
        let mut own = own;
        // A cancel while waiting skips the wait; run_batch then cancels every account.
        let mut turn = token.run_until_cancelled(TURN.lock()).await;
        if let Some(&Some(t)) = turn.as_deref() {
            token.run_until_cancelled(tokio::time::sleep_until(t)).await;
        }
        let emit_p = |p: Progress| emit(&a, PROGRESS, p);
        let (ar, br, barr, tr) = (&a, &b, &bar, &token);
        let d = run_batch(
            &b,
            account_ids,
            delay,
            &token,
            &emit_p,
            move |id| async move { launch_one(ar, tr, br, &id, barr, use_bar_for_all).await },
        )
        .await;
        if let Some(t) = turn.as_deref_mut().filter(|_| d.launched > 0) {
            *t = Some(tokio::time::Instant::now() + delay);
        }
        log::info!(
            "launch batch done: {} launched, {} failed, {} cancelled",
            d.launched,
            d.failed,
            d.cancelled
        );
        own.done = Some(d);
    });
    Ok(batch_id)
}

#[tauri::command]
pub async fn launch_cancel(batch_id: String) -> CmdResult<()> {
    if let Some(t) = BATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&batch_id)
    {
        t.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn destination_resolve(state: State<'_, AppState>, input: String) -> CmdResult<Preview> {
    // Share links need a signed-in account; public calls never send a cookie.
    let cookie = guard(&state.vault).as_ref().and_then(|v| {
        let p = v.payload();
        p.accounts
            .iter()
            .filter(|a| a.session == SessionState::Ok)
            .find_map(|a| p.secrets.get(&a.id).filter(|s| !s.cookie.is_empty()))
            .map(|s| Secret::new(s.cookie.as_str()))
    });
    Ok(preview(&state.roblox, &input, cookie.as_ref()).await)
}

#[tauri::command]
pub async fn recents_list(app: AppHandle) -> CmdResult<Vec<Recent>> {
    blocking(&app, |st| {
        let max = num(
            &st.settings.lock().unwrap_or_else(PoisonError::into_inner),
            "/recents/max",
            20,
        );
        recents::list(&library(st), max as usize)
    })
    .await
}

#[tauri::command]
pub async fn recents_clear(app: AppHandle) -> CmdResult<()> {
    blocking(&app, |st| recents::clear(&library(st)))
        .await?
        .map_err(|e| format!("Could not clear recents: {e}"))?;
    app.emit_json("recents://changed", Value::Null);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Capabilities, Platform};
    use crate::vault::{
        oswrap::FakeOsWrap,
        store::{CreateMode, Vault},
    };
    use std::sync::{Arc, Mutex};
    use tokio::time::Instant;

    #[test]
    fn settings_dir_unless_bootstrapper_and_account_caps_win() {
        let vanilla = RobloxInstall {
            version_dir: Some(PathBuf::from("v")),
            bootstrapper: None,
        };
        let boot = RobloxInstall {
            version_dir: Some(PathBuf::from("v")),
            bootstrapper: Some("Bloxstrap".into()),
        };
        assert_eq!(settings_dir(Some(&boot)), None);
        assert_eq!(settings_dir(None), None);
        assert_eq!(settings_dir(Some(&vanilla)), Some(PathBuf::from("v")));
        let on = serde_json::json!({"fps": {"unlock": true, "value": 144}, "graphics": {"set": true, "level": 2}});
        let off = serde_json::json!({"fps": {"unlock": false, "value": 144}, "graphics": {"set": false, "level": 2}});
        assert_eq!(start_caps(None, None, &on), (Some(144), Some(2)));
        assert_eq!(start_caps(Some(60), Some(7), &on), (Some(60), Some(7)));
        assert_eq!(start_caps(Some(60), Some(7), &off), (Some(60), Some(7)));
        assert_eq!(start_caps(None, None, &off), (None, None));
    }

    // ---- run_batch ----

    type Log = Arc<Mutex<Vec<Progress>>>;
    type Launch = std::pin::Pin<Box<dyn Future<Output = Result<Option<String>, Fail>>>>;

    fn ids(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    fn states(log: &Log, id: &str) -> Vec<&'static str> {
        states_of(&log.lock().unwrap(), id)
    }

    fn states_of(l: &[Progress], id: &str) -> Vec<&'static str> {
        l.iter()
            .filter(|p| p.account_id == id)
            .map(|p| p.state)
            .collect()
    }

    async fn run(
        n: &[&str],
        cancel: &CancellationToken,
        launch: impl Fn(String) -> Launch,
    ) -> (Done, Log) {
        let log: Log = Arc::default();
        let l2 = log.clone();
        let emit = move |p: Progress| l2.lock().unwrap().push(p);
        let d = run_batch("b", ids(n), Duration::from_secs(2), cancel, &emit, launch).await;
        (d, log)
    }

    #[tokio::test(start_paused = true)]
    async fn staggers_in_order() {
        let t0 = Instant::now();
        let starts = Arc::new(Mutex::new(vec![]));
        let s2 = starts.clone();
        let (d, log) = run(&["a", "b", "c"], &CancellationToken::new(), move |id| {
            s2.lock().unwrap().push((id, t0.elapsed().as_secs()));
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(Some("Doors".into()))
            })
        })
        .await;
        let s = starts.lock().unwrap().clone();
        assert_eq!(s, vec![("a".into(), 0), ("b".into(), 2), ("c".into(), 4)]);
        assert_eq!((d.launched, d.failed, d.cancelled), (3, 0, 0));
        assert_eq!(states(&log, "a"), ["queued", "launched"]);
        let l = log.lock().unwrap();
        assert!(
            l[..3].iter().all(|p| p.state == "queued"),
            "all queued first"
        );
        assert_eq!(l.last().unwrap().game_name.as_deref(), Some("Doors"));
    }

    #[tokio::test(start_paused = true)]
    async fn one_failure_does_not_stop_batch() {
        let (d, log) = run(&["a", "b", "c"], &CancellationToken::new(), |id| {
            Box::pin(async move {
                if id == "b" {
                    Err(EXPIRED.into())
                } else {
                    Ok(None)
                }
            })
        })
        .await;
        assert_eq!((d.launched, d.failed, d.cancelled), (2, 1, 0));
        assert_eq!(states(&log, "b"), ["queued", "failed"]);
        let l = log.lock().unwrap();
        let f = l.iter().find(|p| p.state == "failed").unwrap();
        assert_eq!(f.message.as_deref(), Some(EXPIRED));
        assert_eq!(states_of(&l, "c"), ["queued", "launched"]);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_during_stagger_cancels_rest() {
        let c = CancellationToken::new();
        let c2 = c.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            c2.cancel();
        });
        let t0 = Instant::now();
        let (d, log) = run(&["a", "b", "c"], &c, |_| Box::pin(async { Ok(None) })).await;
        assert_eq!((d.launched, d.failed, d.cancelled), (1, 0, 2));
        assert_eq!(states(&log, "b"), ["queued", "cancelled"]);
        assert_eq!(states(&log, "c"), ["queued", "cancelled"]);
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "no waiting after cancel"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_mid_launch_counts_as_cancelled() {
        let c = CancellationToken::new();
        let c2 = c.clone();
        let (d, log) = run(&["a", "b"], &c, move |_| {
            c2.cancel();
            Box::pin(async { Err(Fail::Cancelled) })
        })
        .await;
        assert_eq!((d.launched, d.cancelled), (0, 2));
        assert_eq!(states(&log, "a"), ["queued", "cancelled"]);
    }

    // ---- launch_one with a fake platform, fake session/ticket server ----

    type Calls = Arc<Mutex<Vec<String>>>;

    struct FakePlat {
        calls: Calls,
        install: RobloxInstall,
    }

    impl Platform for FakePlat {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn roblox_install(&self) -> Option<RobloxInstall> {
            Some(self.install.clone())
        }
        fn open_uri(&self, uri: &str) -> std::io::Result<()> {
            assert!(
                uri.contains("gameinfo:TICKET") && uri.contains("browsertrackerid:123456789012")
            );
            self.calls.lock().unwrap().push("open".into());
            Ok(())
        }
        fn close_tracked(&self, t: &str) -> std::io::Result<usize> {
            assert_eq!(t, "123456789012");
            self.calls.lock().unwrap().push("close".into());
            Ok(1)
        }
        fn close_idle_trays(&self) {
            self.calls.lock().unwrap().push("trays".into());
        }
        fn hold_multi(&self) -> std::io::Result<crate::platform::Guard> {
            self.calls.lock().unwrap().push("hold".into());
            Err(std::io::ErrorKind::Unsupported.into())
        }
    }

    #[derive(Clone)]
    struct TestHost(Arc<AppState>, Calls);

    impl Host for TestHost {
        fn st(&self) -> &AppState {
            &self.0
        }
        fn emit_json(&self, event: &str, p: Value) {
            if event == "state://changed" {
                return; // install_changed is process-wide: order across tests varies
            }
            let s = p.get("state").and_then(Value::as_str).unwrap_or(event);
            self.1.lock().unwrap().push(s.to_owned());
        }
    }

    async fn server(calls: Calls) -> String {
        use axum::{http::HeaderMap, http::StatusCode, response::IntoResponse, routing::*, Json};
        let c1 = calls.clone();
        let app = axum::Router::new()
            .route(
                "/v1/users/authenticated",
                get(move |h: HeaderMap| async move {
                    let ok = h
                        .get("cookie")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|c| c.contains("good"));
                    c1.lock().unwrap().push("validate".into());
                    if ok {
                        Json(serde_json::json!({"id": 7, "name": "bob", "displayName": "Bob"}))
                            .into_response()
                    } else {
                        StatusCode::UNAUTHORIZED.into_response()
                    }
                }),
            )
            .route(
                "/v1/authentication-ticket",
                post(move || async move {
                    calls.lock().unwrap().push("ticket".into());
                    [("rbx-authentication-ticket", "TICKET")]
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        url
    }

    /// Unlocked vault with account `id` (cookie `cookie`), vanilla or bootstrapped install.
    async fn host(
        dir: &std::path::Path,
        id: &str,
        cookie: &str,
        settings: Value,
        bootstrapper: bool,
    ) -> TestHost {
        let calls: Calls = Arc::default();
        let url = server(calls.clone()).await;
        let plat = FakePlat {
            calls: calls.clone(),
            install: RobloxInstall {
                version_dir: Some(dir.join("ver")),
                bootstrapper: bootstrapper.then(|| "Bloxstrap".into()),
            },
        };
        let s = crate::settings::apply_patch(&crate::settings::defaults(), &settings).unwrap();
        let st = AppState::for_test(&dir.join("data"), Box::new(plat), &url, s);
        let mut v = Vault::create(&st.paths.vault(), CreateMode::Os, Arc::new(FakeOsWrap)).unwrap();
        v.payload_mut().accounts.push(
            serde_json::from_value(serde_json::json!({
                "id": id, "username": "bob", "trackerId": "123456789012", "session": "ok"
            }))
            .unwrap(),
        );
        v.payload_mut().secrets.insert(
            id.into(),
            serde_json::from_value(serde_json::json!({ "cookie": cookie })).unwrap(),
        );
        *st.vault.lock().unwrap() = Some(v);
        TestHost(Arc::new(st), calls)
    }

    fn bar(dest: &str) -> Bar {
        Bar {
            destination: dest.into(),
            ..Default::default()
        }
    }

    fn account(h: &TestHost, id: &str) -> crate::accounts::Account {
        let g = h.0.vault.lock().unwrap();
        let p = g.as_ref().unwrap().payload();
        p.accounts.iter().find(|a| a.id == id).unwrap().clone()
    }

    fn client_settings(d: &std::path::Path) -> std::path::PathBuf {
        d.join("ver")
            .join("ClientSettings")
            .join("ClientAppSettings.json")
    }

    #[tokio::test]
    async fn launch_one_steps_settings_and_recents() {
        let d = tempfile::tempdir().unwrap();
        let s = serde_json::json!({"client": {"customSettingsPath": "C:/picked.json"}});
        let h = host(d.path(), "L1", "good", s, false).await;
        // the stored copy (not the picked path) is merged
        std::fs::write(
            h.st().paths.root.join("client-settings.json"),
            r#"{"Custom":2}"#,
        )
        .unwrap();
        let cs = client_settings(d.path());
        std::fs::create_dir_all(cs.parent().unwrap()).unwrap();
        std::fs::write(&cs, r#"{"Keep":1,"DFIntTaskSchedulerTargetFps":60}"#).unwrap();

        let r = launch_one(&h, &CancellationToken::new(), "b", "L1", &bar("5"), false).await;
        assert_eq!(r, Ok(None));
        let calls = h.1.lock().unwrap().clone();
        let order: Vec<_> = calls
            .iter()
            .filter(|c| c.as_str() != "accounts://changed")
            .map(String::as_str)
            .collect();
        assert_eq!(
            order,
            [
                "validating",
                "validate",
                "ticket",
                "launching",
                "close",
                "trays",
                "hold",
                "open"
            ]
        );
        assert!(calls.contains(&"accounts://changed".to_string()));
        // no cap: our FPS key is removed, other keys kept, custom copy merged
        let v: Value = serde_json::from_slice(&std::fs::read(&cs).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"Keep": 1, "Custom": 2}));
        assert!(account(&h, "L1").last_launched_at.is_some());
        // recent is pushed after "launched" (detached)
        let lib = library(h.st());
        let mut rec = vec![];
        for _ in 0..200 {
            rec = recents::list(&lib, 20);
            if !rec.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(rec.first().map(|r| r.place_id), Some(5));
    }

    #[tokio::test]
    async fn expired_session_fails_and_marks_account() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "L2", "bad", serde_json::json!({}), false).await;
        let r = launch_one(&h, &CancellationToken::new(), "b", "L2", &bar("5"), false).await;
        assert_eq!(r, Err(Fail::Error(EXPIRED.into())));
        assert_eq!(account(&h, "L2").session, SessionState::Expired);
        assert!(!h.1.lock().unwrap().contains(&"open".to_string()));
    }

    #[tokio::test]
    async fn replace_off_and_bootstrapper_skip_close_and_settings() {
        let d = tempfile::tempdir().unwrap();
        let s = serde_json::json!({"launch": {"replacePrevious": false}, "fps": {"unlock": true}});
        let h = host(d.path(), "L3", "good", s, true).await;
        let r = launch_one(&h, &CancellationToken::new(), "b", "L3", &bar("5"), false).await;
        assert_eq!(r, Ok(None));
        let calls = h.1.lock().unwrap().clone();
        assert!(!calls.contains(&"close".to_string()), "{calls:?}");
        assert!(calls.contains(&"open".to_string()));
        assert!(
            !client_settings(d.path()).exists(),
            "bootstrapper owns client settings"
        );
    }

    #[tokio::test]
    async fn duplicate_in_flight_and_cancel_before_open() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "L4", "good", serde_json::json!({}), false).await;
        let held = Flight::take("L4").unwrap();
        let r = launch_one(&h, &CancellationToken::new(), "b", "L4", &bar("5"), false).await;
        assert_eq!(r, Err(Fail::Error(LAUNCH_BUSY.into())));
        drop(held);
        let c = CancellationToken::new();
        c.cancel();
        let r = launch_one(&h, &c, "b", "L4", &bar("5"), false).await;
        assert_eq!(r, Err(Fail::Cancelled));
        assert!(!h.1.lock().unwrap().contains(&"open".to_string()));
    }

    #[tokio::test]
    async fn status_keeps_every_account_and_a_dropped_batch_still_ends() {
        use serde_json::json as j;
        record(
            false,
            &j!({"batchId": "S1", "accountId": "a", "state": "failed", "message": "x"}),
        );
        record(
            false,
            &j!({"batchId": "S1", "accountId": "b", "state": "launched"}),
        );
        let v = batch_status("S1").unwrap();
        assert_eq!(v["accounts"]["a"]["state"], "failed");
        assert_eq!(v["accounts"]["a"]["message"], "x");
        assert_eq!(v["state"], "launched");

        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "L6", "good", serde_json::json!({}), false).await;
        BATCHES
            .lock()
            .unwrap()
            .insert("S2".into(), CancellationToken::new());
        record(
            false,
            &j!({"batchId": "S2", "accountId": "a", "state": "queued"}),
        );
        assert!(launch_status().iter().any(|v| v["launchId"] == "S2"));
        drop(Own {
            h: h.clone(),
            batch: "S2".into(),
            ids: ids(&["a", "b"]),
            done: None,
        });
        assert!(!launch_status().iter().any(|v| v["launchId"] == "S2"));
        let v = batch_status("S2").unwrap();
        assert_eq!(
            (v["done"].clone(), v["cancelled"].clone()),
            (j!(true), j!(2))
        );
    }

    #[test]
    fn start_caps_defaults_when_value_missing() {
        let s = serde_json::json!({"fps": {"unlock": true}, "graphics": {"set": true}});
        assert_eq!(start_caps(None, None, &s), (Some(60), Some(1)));
    }

    #[test]
    fn user_err_hides_status_body_and_maps_expiry() {
        assert_eq!(user_err(RobloxError::Unauthorized), EXPIRED);
        let e = user_err(RobloxError::Status {
            code: 500,
            body_snippet: "secret-body".into(),
        });
        assert!(!e.contains("secret-body"), "{e}");
        assert!(e.contains("500"), "{e}");
        assert_eq!(
            user_err(RobloxError::RateLimited),
            RobloxError::RateLimited.to_string()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pre_cancelled_batch_launches_nothing() {
        let c = CancellationToken::new();
        c.cancel();
        let (d, log) = run(&["a", "b"], &c, |_| Box::pin(async { Ok(None) })).await;
        assert_eq!((d.launched, d.cancelled), (0, 2));
        assert_eq!(states(&log, "a"), ["queued", "cancelled"]);
    }

    #[tokio::test]
    async fn launch_one_rejects_before_any_launch_step() {
        let d = tempfile::tempdir().unwrap();
        let c = CancellationToken::new();
        let h = host(d.path(), "L7", "good", serde_json::json!({}), false).await;
        assert_eq!(
            launch_one(&h, &c, "b", "nope", &bar("5"), false).await,
            Err(Fail::Error("Account not found.".into()))
        );
        let specific = Bar {
            destination: "5".into(),
            server: String::new(),
            mode: crate::launch::resolve::Mode::Specific,
            ..Default::default()
        };
        assert_eq!(
            launch_one(&h, &c, "b", "L7", &specific, false).await,
            Err(Fail::Error(
                "Enter a server JobId or private server link.".into()
            ))
        );
        let calls = h.1.lock().unwrap().clone();
        assert!(!calls.contains(&"ticket".to_string()), "{calls:?}");
        assert!(!calls.contains(&"open".to_string()), "{calls:?}");

        // no cookie stored: expired without a session check
        let d2 = tempfile::tempdir().unwrap();
        let h2 = host(d2.path(), "L8", "", serde_json::json!({}), false).await;
        assert_eq!(
            launch_one(&h2, &c, "b", "L8", &bar("5"), false).await,
            Err(Fail::Error(EXPIRED.into()))
        );
        assert!(!h2.1.lock().unwrap().contains(&"validate".to_string()));
    }

    #[tokio::test]
    async fn locked_vault_stops_launch_and_batch_start() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "L9", "good", serde_json::json!({}), false).await;
        *h.0.vault.lock().unwrap() = None;
        assert_eq!(
            launch_one(&h, &CancellationToken::new(), "b", "L9", &bar("5"), false).await,
            Err(Fail::Error(LOCKED.into()))
        );
        assert_eq!(
            start_batch(&h, vec!["L9".into()], bar("5"), false, true),
            Err(LOCKED.into())
        );
        assert_eq!(
            start_batch(&h, vec![], bar("5"), false, true),
            Err("Pick at least one account.".into())
        );
    }

    #[tokio::test]
    async fn custom_settings_needs_picked_path_and_stored_copy() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "L10", "good", serde_json::json!({}), false).await;
        let st = h.st();
        let copy = st.paths.root.join(crate::commands::CLIENT_SETTINGS_COPY);
        std::fs::write(&copy, r#"{"K":1}"#).unwrap();
        // copy stored but no path picked
        assert_eq!(custom_settings(&serde_json::json!({}), st), None);
        let picked = serde_json::json!({"client": {"customSettingsPath": "C:/x.json"}});
        assert_eq!(
            custom_settings(&picked, st),
            Some(serde_json::json!({"K": 1}))
        );
        std::fs::write(&copy, "[oops").unwrap();
        assert_eq!(
            custom_settings(&picked, st),
            None,
            "unreadable copy skipped"
        );
    }

    #[tokio::test]
    async fn stale_cookie_expiry_is_ignored() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "L11", "good", serde_json::json!({}), false).await;
        let r = expire(&h, "L11", &Secret::new("older-cookie")).await;
        assert_eq!(r, Fail::Error(EXPIRED.into()));
        assert_eq!(account(&h, "L11").session, SessionState::Ok);
        assert!(!h
            .1
            .lock()
            .unwrap()
            .contains(&"accounts://changed".to_string()));
    }

    /// `cancel_all` cancels every live batch in the process: tests asserting a real batch's
    /// outcome hold this so it never runs mid-batch.
    /// Tests that start a launching batch also pass `launch.delayMs` 0: the process-global TURN
    /// would otherwise make the next batch wait the real 2.5 s stagger.
    static LIVE_BATCHES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn cancel_all_cancels_live_batches() {
        let _g = LIVE_BATCHES.blocking_lock();
        let t = CancellationToken::new();
        BATCHES.lock().unwrap().insert("CA1".into(), t.clone());
        cancel_all();
        assert!(t.is_cancelled());
        BATCHES.lock().unwrap().remove("CA1");
    }

    #[tokio::test]
    async fn push_recent_falls_back_to_place_name() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "L12", "good", serde_json::json!({}), false).await;
        // the test server has no game details: the name falls back to the id
        push_recent(h.clone(), 9, "2026-01-01T00:00:00Z".into(), 20).await;
        let rec = recents::list(&library(h.st()), 20);
        assert_eq!(rec.len(), 1);
        assert_eq!((rec[0].place_id, rec[0].name.as_str()), (9, "Place 9"));
        assert!(h
            .1
            .lock()
            .unwrap()
            .contains(&"recents://changed".to_string()));
    }

    #[tokio::test]
    async fn start_batch_dedupes_account_ids() {
        let _g = LIVE_BATCHES.lock().await;
        let d = tempfile::tempdir().unwrap();
        let h = host(
            d.path(),
            "D1",
            "good",
            serde_json::json!({"launch": {"delayMs": 0}}),
            false,
        )
        .await;
        let id = start_batch(&h, vec!["D1".into(), "D1".into()], bar("5"), false, true).unwrap();
        let mut v = Value::Null;
        for _ in 0..500 {
            if let Some(s) = batch_status(&id).filter(|s| s["done"] == true) {
                v = s;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(v["launched"], 1, "{v}");
    }

    #[tokio::test]
    async fn start_batch_clears_friend_pause() {
        let d = tempfile::tempdir().unwrap();
        let h = host(
            d.path(),
            "FP1",
            "good",
            serde_json::json!({"launch": {"delayMs": 0}}),
            false,
        )
        .await;
        for a in
            h.0.vault
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .payload_mut()
                .accounts
                .iter_mut()
        {
            a.friend_paused = true;
        }
        assert!(account(&h, "FP1").friend_paused);
        // automated launch: refused, the pause stays, nothing counted as running
        assert_eq!(
            start_batch(&h, vec!["FP1".into()], bar("5"), false, false),
            Err(FRIEND_PAUSED.into())
        );
        assert!(account(&h, "FP1").friend_paused);
        assert!(!h.0.automation.own_running("FP1"));
        start_batch(&h, vec!["FP1".into()], bar("5"), false, true).unwrap();
        assert!(!account(&h, "FP1").friend_paused);
        assert!(h
            .1
            .lock()
            .unwrap()
            .contains(&"accounts://changed".to_string()));
    }

    #[tokio::test]
    async fn pause_after_open_closes_the_new_client() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "FP3", "good", serde_json::json!({}), false).await;
        close_if_paused(h.st(), "FP3", "123456789012");
        assert!(!h.1.lock().unwrap().contains(&"close".to_string()));
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[0]
            .friend_paused = true;
        close_if_paused(h.st(), "FP3", "123456789012");
        assert!(h.1.lock().unwrap().contains(&"close".to_string()));
    }

    #[tokio::test]
    async fn friend_pause_mid_launch_cancels_before_open() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), "FP2", "good", serde_json::json!({}), false).await;
        // Friend Disconnect lands after start_batch cleared the pause.
        {
            let mut g = h.0.vault.lock().unwrap();
            let p = g.as_mut().unwrap().payload_mut();
            p.accounts.iter_mut().for_each(|a| a.friend_paused = true);
        }
        let r = launch_one(&h, &CancellationToken::new(), "b", "FP2", &bar("5"), false).await;
        assert_eq!(r, Err(Fail::Cancelled));
        assert!(!h.1.lock().unwrap().contains(&"open".to_string()));
    }
}
