//! The only IPC boundary. Nothing here returns a secret unless its name says so.

use crate::automation::Status;
use crate::{
    platform::{Capabilities, Guard},
    settings, AppState,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{Manager, State};

pub(crate) type CmdResult<T> = Result<T, String>;

#[derive(Serialize)]
pub struct MonitorInfo {
    /// Device name (`\\.\DISPLAY1`), stable across calls; "x,y" when the OS gives none.
    id: String,
    label: String,
    work: crate::accounts::Rect,
    rect: crate::accounts::Rect,
    primary: bool,
    scale: f64,
}

fn monitors(app: &tauri::AppHandle) -> CmdResult<Vec<MonitorInfo>> {
    let id = |m: &tauri::Monitor| {
        m.name()
            .cloned()
            .unwrap_or_else(|| format!("{},{}", m.position().x, m.position().y))
    };
    let r =
        |p: &tauri::PhysicalPosition<i32>, s: &tauri::PhysicalSize<u32>| crate::accounts::Rect {
            x: p.x,
            y: p.y,
            w: s.width as i32,
            h: s.height as i32,
        };
    let primary = app.primary_monitor().ok().flatten().map(|m| id(&m));
    let all = app.available_monitors().map_err(|e| e.to_string())?;
    Ok(all
        .iter()
        .enumerate()
        .map(|(i, m)| MonitorInfo {
            id: id(m),
            label: format!("Monitor {}", i + 1),
            work: r(&m.work_area().position, &m.work_area().size),
            rect: r(m.position(), m.size()),
            primary: primary == Some(id(m)),
            scale: m.scale_factor(),
        })
        .collect())
}

#[tauri::command]
pub fn monitors_list(app: tauri::AppHandle) -> CmdResult<Vec<MonitorInfo>> {
    monitors(&app)
}

/// Missing args fall back to the saved `/gridArrange` default, then to today's 3-wide grid
/// on the primary monitor. An unknown (unplugged) monitor also means the primary.
#[tauri::command]
pub async fn grid_arrange(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    monitor: Option<String>,
    order: Option<Vec<String>>,
    layout: Option<crate::grid::Layout>,
) -> CmdResult<usize> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let saved = state.settings.lock().unwrap()["gridArrange"].clone();
        let monitor = monitor.or_else(|| saved["monitor"].as_str().map(String::from));
        let wanted: Vec<String> = order
            .or_else(|| serde_json::from_value(saved["order"].clone()).ok())
            .unwrap_or_default();
        let layout: crate::grid::Layout = layout
            .or_else(|| serde_json::from_value(saved["layout"].clone()).ok())
            .unwrap_or_default();
        #[cfg(windows)]
        let work = match monitor {
            Some(id) => monitors(&app)?
                .into_iter()
                .find(|m| m.id == id)
                .map(|m| m.work),
            None => None,
        };
        // macOS moves windows in points: the chosen (else primary) monitor's work area
        // divided by its scale.
        #[cfg(target_os = "macos")]
        let work = {
            let all = monitors(&app)?;
            let m = monitor
                .and_then(|id| all.iter().find(|m| m.id == id))
                .or_else(|| all.iter().find(|m| m.primary))
                .or(all.first())
                .ok_or("No monitor found.")?;
            let p = |v: i32| (f64::from(v) / m.scale).round() as i32;
            crate::accounts::Rect {
                x: p(m.work.x),
                y: p(m.work.y),
                w: p(m.work.w),
                h: p(m.work.h),
            }
        };
        let (ids, order, keep) = grid_order(&state, &wanted);
        // Focus `main: ""` (from `grid_suggest`): the first account of the order.
        let main = match &layout {
            crate::grid::Layout::Focus { main, .. } if main.is_empty() => Some(0),
            crate::grid::Layout::Focus { main, .. } => ids.iter().position(|id| id == main),
            _ => None,
        };
        tauri::async_runtime::spawn_blocking(move || {
            #[cfg(windows)]
            return crate::platform::windows::arrange_grid(&order, keep, work, &layout, main);
            #[cfg(target_os = "macos")]
            return crate::platform::macos::arrange_grid(&order, keep, work, &layout, main);
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (app, state, monitor, order, layout);
        Err("Grid Arrange is not available on this system.".into())
    }
}

/// Account ids and (tracker id, window title) in grid slot order: `wanted` first, then
/// account-list order (group, pinned first, manual order). Window i of the running ones
/// takes slot i; wanted accounts that aren't running are skipped, not left as holes.
/// `keep`: ranks below it get tiled (the wanted accounts; all when `wanted` is empty), so
/// unpicked accounts' windows stay put. Locked vault: empty, windows go by pid.
#[cfg(any(windows, target_os = "macos"))]
fn grid_order(state: &AppState, wanted: &[String]) -> (Vec<String>, Vec<(String, String)>, usize) {
    let s = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let (ids, order): (Vec<String>, Vec<(String, String)>) = guard(&state.vault)
        .as_ref()
        .map_or_else(Default::default, |v| {
            let p = v.payload();
            let group = |n: &str| p.groups.iter().position(|g| g == n).unwrap_or(usize::MAX);
            let pos = |id: &str| wanted.iter().position(|o| o == id).unwrap_or(usize::MAX);
            let mut a: Vec<_> = p.accounts.iter().collect();
            a.sort_by_key(|a| (pos(&a.id), group(&a.group), !a.pinned, a.order));
            a.iter()
                .map(|a| {
                    let title = crate::settings::window_title(&s, &a.username);
                    (a.id.clone(), (a.tracker_id.clone(), title))
                })
                .unzip()
        });
    let keep = if wanted.is_empty() {
        ids.len()
    } else {
        ids.iter().filter(|id| wanted.contains(id)).count()
    };
    (ids, order, keep)
}

/// The 3 or 4 best layouts for `count` windows, best first.
#[tauri::command]
pub fn grid_suggest(count: usize) -> Vec<crate::grid::Suggestion> {
    crate::grid::suggest(count)
}

/// Big slot numbers (1..N, same order as `grid_arrange`) over each Roblox window for ~3 s,
/// without focus or cursor changes. `order` missing: the saved one. Returns windows marked;
/// 0 off Windows.
#[tauri::command]
pub async fn grid_identify(app: tauri::AppHandle, order: Option<Vec<String>>) -> usize {
    #[cfg(windows)]
    {
        // Off the main thread: vault lock + process command lines.
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<AppState>();
            let wanted: Vec<String> = order
                .or_else(|| {
                    let saved = state.settings.lock().unwrap()["gridArrange"]["order"].clone();
                    serde_json::from_value(saved).ok()
                })
                .unwrap_or_default();
            let (_, order, keep) = grid_order(&state, &wanted);
            crate::platform::windows::identify(&order, keep)
        })
        .await
        .unwrap_or(0)
    }
    #[cfg(target_os = "macos")]
    {
        let a = app.clone();
        let frames = tauri::async_runtime::spawn_blocking(move || {
            let state = a.state::<AppState>();
            let wanted: Vec<String> = order
                .or_else(|| {
                    let saved = state.settings.lock().unwrap()["gridArrange"]["order"].clone();
                    serde_json::from_value(saved).ok()
                })
                .unwrap_or_default();
            let (_, order, keep) = grid_order(&state, &wanted);
            crate::platform::macos::identify_frames(&order, keep)
        })
        .await
        .unwrap_or_default();
        identify_badges(&app, &frames);
        frames.len()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (app, order);
        0
    }
}

/// macOS Identify: a small numbered window (public/identify.html) centered on each Roblox
/// window (points) for 3 s, never focused.
#[cfg(target_os = "macos")]
fn identify_badges(app: &tauri::AppHandle, frames: &[[i32; 4]]) {
    const S: f64 = 140.0;
    for (i, [x, y, w, h]) in frames.iter().enumerate() {
        let label = format!("identify-{i}");
        if let Some(old) = app.get_webview_window(&label) {
            let _ = old.destroy();
        }
        let url = tauri::WebviewUrl::App(format!("identify.html?n={}", i + 1).into());
        let built = tauri::WebviewWindowBuilder::new(app, &label, url)
            .title("")
            .inner_size(S, S)
            .position(
                f64::from(*x) + (f64::from(*w) - S) / 2.0,
                f64::from(*y) + (f64::from(*h) - S) / 2.0,
            )
            .decorations(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .build();
        if let Ok(win) = built {
            let _ = win.set_ignore_cursor_events(true);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let _ = win.destroy();
            });
        }
    }
}

#[derive(Serialize, Default)]
pub struct GridWindows {
    /// Accounts with an open Roblox game window on this PC.
    accounts: Vec<String>,
    /// Windows not mapped to an account (grid places them last).
    unknown: usize,
}

/// Which accounts really have a game window open: empty off Windows.
#[tauri::command]
pub async fn grid_windows(app: tauri::AppHandle) -> GridWindows {
    #[cfg(any(windows, target_os = "macos"))]
    {
        #[cfg(target_os = "macos")]
        use crate::platform::macos::ordered_windows;
        #[cfg(windows)]
        use crate::platform::windows::ordered_windows;
        tauri::async_runtime::spawn_blocking(move || {
            let (ids, order, keep) = grid_order(&app.state::<AppState>(), &[]);
            let mut out = GridWindows::default();
            for (rank, ..) in ordered_windows(&order, keep) {
                match ids.get(rank) {
                    Some(id) => out.accounts.push(id.clone()),
                    None => out.unknown += 1,
                }
            }
            out
        })
        .await
        .unwrap_or_default()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = app;
        GridWindows::default()
    }
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> Value {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
pub async fn settings_set(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    patch: Value,
) -> CmdResult<Value> {
    let prev = patch
        .get("friend")
        .map(|_| state.settings.lock().unwrap().clone());
    let next = persist(&state, &patch)?;
    // Remote off, moved or address cleared: drop RMI's Funnel entry for the old port first, so
    // it can't expose whatever listens there later. A failure only logs; the save stands.
    if let Some(port) = prev.and_then(|p| crate::friend::tailscale::stale_port(&p, &next)) {
        let r = tauri::async_runtime::spawn_blocking(move || {
            crate::friend::tailscale::disable_target(port)
        })
        .await;
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("removing the friend Funnel entry failed: {}", e.message),
            Err(e) => log::warn!("removing the friend Funnel entry failed: {e}"),
        }
    }
    if patch.get("multi").is_some() {
        state_changed(&app, &["roblox"]); // multiActive / cookieLockActive moved
    }
    crate::watcher::reconcile(&app);
    if patch.get("homelab").is_some() {
        let h = app.clone();
        tauri::async_runtime::spawn_blocking(move || crate::homelab::sync(&h));
    }
    let os = crate::shell::on_settings(&app, &patch);
    if next.pointer("/automation/enabled") == Some(&Value::Bool(true)) {
        token_ready(&app).await;
    }
    // Ports / LAN / enabled may have changed: rebind or stop the automation servers.
    let r = state.automation.reconcile().await;
    state_changed(&app, &["autoStatus"]); // running or not, on whichever path

    // Friend enabled / port may have changed too.
    let fr = friend_sync(&app).await;
    // Remote access: listen, move or stop (and end a live session when switched off).
    state.remote.sync().await;
    state_changed(&app, &["remoteStatus"]);
    if let Err(e) = r {
        log::error!("automation restart failed: {e}");
        return Err(format!(
            "Settings saved, but the automation servers could not restart: {e}"
        ));
    }
    if let Err(e) = fr {
        log::error!("friend server restart failed: {e}");
        return Err(format!(
            "Settings saved, but the friend server could not start: {e}"
        ));
    }
    if let Err(e) = os {
        state_changed(&app, &["settings"]); // launchOnLogin was put back
        return Err(e);
    }
    // Current settings, not this save's snapshot: a faster later save may have landed meanwhile.
    let now = state.settings.lock().unwrap().clone();
    Ok(now)
}

/// Save a settings patch, then re-apply the multi-instance guards to match. Read-only mode
/// (file unreadable at startup, never overwritten): applies in memory only, for this session.
pub(crate) fn persist(state: &AppState, patch: &Value) -> CmdResult<Value> {
    let mut cur = state.settings.lock().unwrap();
    let next = settings::apply_patch(&cur, patch)?;
    if !state.settings_read_only {
        let path = state.paths.settings();
        settings::save(&path, &next).map_err(|e| {
            log::error!("saving {} failed: {e}", path.display());
            format!("Could not save settings: {e}")
        })?;
    }
    *cur = next.clone();
    // Still under the settings lock, so concurrent toggles apply in save order.
    sync_guards(state, &next);
    drop(cur);
    Ok(next)
}

fn want(s: &Value, p: &str) -> bool {
    s.pointer(p).and_then(Value::as_bool).unwrap_or(false)
}

/// Take or release the mutex / cookie lock so they match `/multi/*`. Failures are logged
/// and ignored (spec §6.4); unsupported platforms are silent. Lock order: settings, then
/// guard slots; callers hold the settings lock (`s` is its value).
pub(crate) fn sync_guards(state: &AppState, s: &Value) {
    let _ = sync(
        &state.multi_guard,
        want_multi(state, s),
        "multi-instance mutex",
        || state.platform.hold_multi(),
    );
    let _ = sync(
        &state.cookie_guard,
        want(s, "/multi/cookieLock"),
        "cookie lock",
        || state.platform.hold_cookie_lock(),
    );
}

/// Retry the multi-instance mutex if enabled but not held (Roblox may have closed since).
/// Called by `roblox_status` and, later, before every launch. `Err(AlreadyExists)` = a
/// running Roblox owns it ("Close Roblox once, then multi-client will work.").
pub fn ensure_multi(state: &AppState) -> std::io::Result<()> {
    let s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    // Cookie lock too: the file may only exist / be free now. Failures are logged inside.
    let _ = sync(
        &state.cookie_guard,
        want(&s, "/multi/cookieLock"),
        "cookie lock",
        || state.platform.hold_cookie_lock(),
    );
    sync(
        &state.multi_guard,
        want_multi(state, &s),
        "multi-instance mutex",
        || state.platform.hold_multi(),
    )
}

/// `/multi/enabled`, except a held mutex is kept while Roblox clients run: releasing it
/// closes them. Turned off with clients open, it is let go on a later sync (status poll,
/// launch) once they have all closed.
fn want_multi(state: &AppState, s: &Value) -> bool {
    want(s, "/multi/enabled")
        || (state
            .multi_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|g| g.alive())
            && state.platform.roblox_running())
}

fn sync(
    slot: &Mutex<Option<Guard>>,
    want: bool,
    what: &str,
    acquire: impl FnOnce() -> std::io::Result<Guard>,
) -> std::io::Result<()> {
    use std::io::ErrorKind::{AlreadyExists, Unsupported};
    let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
    if !want {
        if g.take().is_some() {
            log::info!("{what} released");
        }
        return Ok(());
    }
    if g.as_ref().is_some_and(|x| x.alive()) {
        return Ok(());
    }
    if g.take().is_some() {
        log::warn!("{what} lost (holder gone); taking it again");
    }
    match acquire() {
        Ok(x) => {
            log::info!("{what} held");
            *g = Some(x);
            Ok(())
        }
        Err(e) if e.kind() == Unsupported => Ok(()),
        Err(e) => {
            // AlreadyExists repeats on every status poll while Roblox runs: keep it quiet.
            if e.kind() == AlreadyExists {
                log::debug!("{what} not held: {e}");
            } else {
                log::warn!("{what} not held: {e}");
            }
            Err(e)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RobloxStatus {
    installed: bool,
    bootstrapper: Option<String>,
    version_dir: Option<String>,
    running: bool,
    multi_active: bool,
    cookie_lock_active: bool,
    /// Multi enabled but a running Roblox already owns the mutex (§6.4 banner).
    multi_blocked_by_running_roblox: bool,
    /// `DFIntTaskSchedulerTargetFps` is off Roblox's FFlag allowlist: UI says "Roblox may ignore this".
    fps_flag_allowlisted: bool,
    /// The mutex is held inside RMI (holder process failed): quitting, updating or a crash
    /// of RMI releases it and closes the Roblox clients.
    multi_in_process: bool,
    /// Multi-instance is off in settings but still held until the running clients close.
    multi_release_pending: bool,
}

#[tauri::command]
pub async fn roblox_status(app: tauri::AppHandle) -> CmdResult<RobloxStatus> {
    use tauri::Manager;
    // Blocking: ensure_multi may copy and start the holder exe; keep it off async workers.
    tauri::async_runtime::spawn_blocking(move || roblox_status_now(&app.state::<AppState>()))
        .await
        .map_err(|e| format!("internal error: {e}"))
}

fn roblox_status_now(state: &AppState) -> RobloxStatus {
    let held = |m: &Mutex<Option<Guard>>| m.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    // Taken by someone else: only Roblox blocks multi-client; another RMI holding it is fine.
    let blocked = matches!(ensure_multi(state), Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists)
        && state.platform.roblox_running();
    let install = state.platform.roblox_install();
    install_changed(install.as_ref()); // the UI has this one now
                                       // Own statements: a guard temporary inside the struct literal would live to its end.
    let in_process = state
        .multi_guard
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|g| g.in_process());
    let multi_on = want(
        &state.settings.lock().unwrap_or_else(|e| e.into_inner()),
        "/multi/enabled",
    );
    RobloxStatus {
        installed: install.is_some(),
        bootstrapper: install.as_ref().and_then(|i| i.bootstrapper.clone()),
        version_dir: install
            .and_then(|i| i.version_dir)
            .map(|d| d.display().to_string()),
        running: state.platform.roblox_running(),
        multi_active: held(&state.multi_guard),
        cookie_lock_active: held(&state.cookie_guard),
        multi_blocked_by_running_roblox: blocked,
        fps_flag_allowlisted: false,
        multi_in_process: in_process,
        multi_release_pending: held(&state.multi_guard) && !multi_on,
    }
}

/// Whether the Roblox install (version folder or bootstrapper) differs from the last one seen
/// here; true on the first call. A change means the UI's `["roblox"]` is stale.
pub(crate) fn install_changed(i: Option<&crate::platform::RobloxInstall>) -> bool {
    static LAST: Mutex<Option<Seen>> = Mutex::new(None);
    swap_seen(&mut LAST.lock().unwrap_or_else(|e| e.into_inner()), i)
}

type Seen = Option<(Option<PathBuf>, Option<String>)>;

fn swap_seen(last: &mut Option<Seen>, i: Option<&crate::platform::RobloxInstall>) -> bool {
    let now = Some(i.map(|i| (i.version_dir.clone(), i.bootstrapper.clone())));
    std::mem::replace(last, now.clone()) != now
}

/// Stored copy of the user's custom client settings (merged before every vanilla launch).
pub(crate) const CLIENT_SETTINGS_COPY: &str = "client-settings.json";

/// Validate `path` as a JSON object and store a copy in the data dir; `None` clears it.
#[tauri::command]
pub async fn client_settings_set(app: tauri::AppHandle, path: Option<String>) -> CmdResult<Value> {
    use tauri::Manager;
    let h = app.clone();
    let next = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let copy = state.paths.root.join(CLIENT_SETTINGS_COPY);
        let Some(p) = path.as_deref().map(str::trim).filter(|p| !p.is_empty()) else {
            // Save first: a failed save keeps the path and its copy.
            let saved = persist(&state, &json!({ "client": { "customSettingsPath": null } }))?;
            if let Err(e) = std::fs::remove_file(&copy) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("could not remove {}: {e}", copy.display());
                }
            }
            return Ok(saved);
        };
        let v = read_client_settings(std::path::Path::new(p))?;
        // Copy aside, save the setting, then swap in: a failed save keeps the old copy.
        let tmp = copy.with_extension("json.new");
        settings::save(&tmp, &v).map_err(|e| format!("Couldn't store a copy: {e}"))?;
        let saved = persist(&state, &json!({ "client": { "customSettingsPath": p } }));
        if saved.is_ok() {
            std::fs::rename(&tmp, &copy).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("Couldn't store a copy: {e}")
            })?;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
        saved
    })
    .await
    .map_err(|e| format!("internal error: {e}"))??;
    state_changed(&h, &["settings"]);
    Ok(next)
}

/// A client settings file: JSON object, at most 1 MB.
pub(crate) fn read_client_settings(p: &std::path::Path) -> CmdResult<Value> {
    let len = std::fs::metadata(p)
        .map_err(|e| format!("Couldn't read that file: {e}"))?
        .len();
    if len > 1 << 20 {
        return Err("That file is too large for client settings (over 1 MB).".into());
    }
    let b = std::fs::read(p).map_err(|e| format!("Couldn't read that file: {e}"))?;
    match serde_json::from_slice::<Value>(&b) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err("The file must hold a JSON object of client settings.".into()),
        Err(e) => Err(format!("That file isn't valid JSON: {e}")),
    }
}

#[tauri::command]
pub async fn multi_set(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<Value> {
    let next = persist(&state, &json!({ "multi": { "enabled": enabled } }))?;
    state_changed(&app, &["settings", "roblox"]);
    Ok(next)
}

#[tauri::command]
pub async fn cookie_lock_set(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<Value> {
    let next = persist(&state, &json!({ "multi": { "cookieLock": enabled } }))?;
    state_changed(&app, &["settings", "roblox"]);
    Ok(next)
}

#[tauri::command(async)]
pub fn watcher_status(state: State<'_, AppState>) -> crate::watcher::Status {
    let cfg = crate::watcher::rules::Cfg::from_settings(&state.settings.lock().unwrap());
    state.watcher.status(&cfg)
}

/// Close one Roblox client (RobloxPlayerBeta.exe pids only).
#[tauri::command]
pub async fn watcher_kill(state: State<'_, AppState>, pid: u32) -> CmdResult<()> {
    if !state.platform.capabilities().watcher {
        return Err("The client watcher is only available on Windows.".into());
    }
    tauri::async_runtime::spawn_blocking(move || crate::watcher::kill_client(pid))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn settings_defaults() -> Value {
    settings::defaults()
}

/// Opens the data or logs folder in the OS file manager.
#[tauri::command]
pub fn open_folder(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    kind: String,
) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = match kind.as_str() {
        "data" => &state.paths.root,
        "logs" => &state.paths.logs,
        "backups" => &state.paths.backups,
        _ => return Err("Unknown folder.".into()),
    };
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| format!("Could not open the folder: {e}"))
}

#[tauri::command]
pub async fn platform_capabilities(state: State<'_, AppState>) -> CmdResult<Capabilities> {
    Ok(state.platform.capabilities())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    data_dir: String,
    portable: bool,
    /// Settings file unreadable at startup: changes last only until the app quits.
    settings_read_only: bool,
    /// settings.json was invalid at startup: defaults in use, old file set aside (message).
    settings_recovered: Option<String>,
    /// Windows transparency effects on, so the acrylic window material shows (the UI sets
    /// `data-acrylic`; without it the window gets an opaque base).
    acrylic: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: state.paths.root.display().to_string(),
        portable: state.portable,
        settings_read_only: state.settings_read_only,
        settings_recovered: state.settings_recovered.clone(),
        #[cfg(windows)]
        acrylic: crate::platform::windows::transparency_enabled(),
        #[cfg(not(windows))]
        acrylic: false,
    }
}

#[tauri::command]
pub async fn automation_status(state: State<'_, AppState>) -> CmdResult<Status> {
    Ok(state.automation.status().await)
}

#[tauri::command]
pub async fn automation_start(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<Status> {
    if !state.automation.ctx.flag("/automation/enabled") {
        return Err(crate::automation::AUTOMATION_OFF.into()); // skips making the token file
    }
    if crate::shell::update_installing() {
        return Err(crate::shell::UPDATE_BUSY.into()); // the install paused it; it resumes on failure
    }
    token_ready(&app).await;
    state.automation.start().await
}

#[tauri::command]
pub async fn automation_stop(state: State<'_, AppState>) -> CmdResult<Status> {
    Ok(state.automation.stop().await)
}

/// Secret: the local API token, for the Settings copy button.
#[tauri::command]
pub async fn automation_token(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<String> {
    // Only once Automation is on: the token file is made at first enable (spec 5.3).
    if state.automation.ctx.flag("/automation/enabled") {
        token_ready(&app).await;
    }
    Ok(state.automation.token())
}

/// Secret: issues a new token, saves it (OS-wrapped) and drops connected control clients.
#[tauri::command]
pub async fn automation_token_regenerate(app: tauri::AppHandle) -> CmdResult<String> {
    log::info!("automation token regenerated");
    let h = app.clone();
    let (t, warn) = tauri::async_runtime::spawn_blocking(move || {
        let st = h.state::<AppState>();
        st.automation
            .regenerate_token(&st.paths.root, || st.os().map_err(|e| e.to_string()))
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Some(key) = warn {
        notice(&app, key);
    }
    Ok(t)
}

/// Load the saved API token or save the current one (spec 5.3). `Some(i18n key)` =
/// the saved one did not decode and got replaced. Blocking (file + keyring).
pub(crate) fn ensure_token(st: &AppState) -> Option<&'static str> {
    st.automation
        .ensure_token(&st.paths.root, || st.os().map_err(|e| e.to_string()))
}

/// `ensure_token` off the async workers, toasting a replacement.
/// A loaded saved token replaces the in-memory one: refresh `["autoToken"]` then (only on a
/// change, so the `automation_token` refetch this triggers does not loop).
async fn token_ready(app: &tauri::AppHandle) {
    let h = app.clone();
    let before = app.state::<AppState>().automation.token();
    let key = tauri::async_runtime::spawn_blocking(move || ensure_token(&h.state::<AppState>()))
        .await
        .ok()
        .flatten();
    if let Some(key) = key {
        notice(app, key);
    } else if app.state::<AppState>().automation.token() != before {
        state_changed(app, &["autoToken"]);
    }
}

/// Load (or save) the friend token and password, off the async workers. Once per session.
pub(crate) async fn friend_token(app: &tauri::AppHandle) {
    let h = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let st = h.state::<AppState>();
        st.friend
            .ensure_token(&st.paths.root, || st.os().map_err(|e| e.to_string()))
    })
    .await;
}

/// Friend server to match the settings (token loaded first while enabled).
pub(crate) async fn friend_sync(app: &tauri::AppHandle) -> Result<(), String> {
    let st = app.state::<AppState>();
    if st.friend.enabled() {
        friend_token(app).await;
    }
    let r = st.friend.sync().await;
    state_changed(app, &["friendStatus"]);
    r
}

#[tauri::command]
pub async fn friend_status(state: State<'_, AppState>) -> CmdResult<crate::friend::FriendStatus> {
    Ok(state.friend.status().await)
}

/// The login page link for the owner to send the friend (no secret in it).
#[tauri::command]
pub async fn friend_link(state: State<'_, AppState>) -> CmdResult<String> {
    state.friend.link()
}

/// Secret: the friend password, for the owner to view or copy.
#[tauri::command]
pub async fn friend_password(app: tauri::AppHandle) -> CmdResult<String> {
    friend_token(&app).await;
    app.state::<AppState>().friend.password()
}

/// New friend password and token (signs the friend out at once); returns the password.
#[tauri::command]
pub async fn friend_new_password(app: tauri::AppHandle) -> CmdResult<String> {
    let h = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let st = h.state::<AppState>();
        st.friend
            .new_password(&st.paths.root, || st.os().map_err(|e| e.to_string()))
    })
    .await
    .map_err(|e| e.to_string())
}

/// Latest automation notice not yet shown: the UI takes it on mount and on every
/// `automation://notice`, so one emitted before its listener attached is not lost.
static NOTICE: Mutex<Option<String>> = Mutex::new(None);

/// Toast an automation notice (`key` = i18n key) in the UI. Every notice means the token
/// was replaced, so the cached one is stale too.
pub(crate) fn notice(app: &tauri::AppHandle, key: &str) {
    use tauri::Emitter;
    *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(key.to_owned());
    if let Err(e) = app.emit("automation://notice", json!({ "key": key })) {
        log::warn!("emit automation://notice failed: {e}");
    }
    state_changed(app, &["autoToken"]);
}

/// The pending automation notice (i18n key), once.
#[tauri::command]
pub fn automation_notice_take() -> Option<String> {
    NOTICE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Hub accounts plus `relaunchStatus {pendingAt, lastRelaunch, count, stopped, launching}`.
#[tauri::command(async)]
pub fn automation_snapshot(state: State<'_, AppState>) -> Vec<Value> {
    let (now, now_ms) = (
        tokio::time::Instant::now(),
        chrono::Utc::now().timestamp_millis(),
    );
    let eng = state
        .automation
        .relaunch
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state
        .automation
        .ctx
        .hub
        .snapshot()
        .into_iter()
        .map(|a| {
            let st = eng.status(&a.account, now, now_ms);
            let mut v = serde_json::to_value(a).unwrap_or_default();
            v["relaunchStatus"] = st;
            v
        })
        .collect()
}

/// Per-account auto-relaunch config from the control panel (same store as WS `relaunch.set`).
#[tauri::command]
pub fn automation_relaunch_set(
    state: State<'_, AppState>,
    account: String,
    enabled: bool,
    delay_sec: u64,
    place_id: Option<u64>,
    job_id: Option<String>,
) -> CmdResult<()> {
    if delay_sec > 86_400 {
        return Err("Delay can be at most 86400 seconds.".into());
    }
    let job_id = job_id
        .map(|j| j.trim().to_owned())
        .filter(|j| !j.is_empty());
    state.automation.ctx.hub.set_relaunch(
        &account,
        &json!({ "enabled": enabled, "seconds": delay_sec, "placeId": place_id, "jobId": job_id }),
    )
}

#[tauri::command]
pub fn automation_send(state: State<'_, AppState>, account: String, text: String) -> CmdResult<()> {
    state.automation.ctx.hub.send_message(&account, &text)
}

/// Suggested file name: anything outside `[A-Za-z0-9_.-]` becomes `_`.
fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Opens a save dialog and writes the whole console as text. Ok(false) = cancelled.
#[tauri::command]
pub async fn automation_save_console(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    account: String,
) -> CmdResult<bool> {
    use tauri_plugin_dialog::DialogExt;
    let text = state
        .automation
        .ctx
        .hub
        .console_text(&account)
        .ok_or_else(|| format!("{account} has no console"))?;
    let Some(path) = app
        .dialog()
        .file()
        .set_file_name(format!("{}-console.txt", safe_name(&account)))
        .add_filter("Text", &["txt"])
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("Could not save the console: {e}"))?;
    Ok(true)
}

#[tauri::command]
pub fn automation_ui_event(
    state: State<'_, AppState>,
    account: String,
    name: String,
    value: Value,
) -> CmdResult<()> {
    state
        .automation
        .ctx
        .hub
        .fire_ui_event(&account, &name, value)
}

// ---- vault ----
// Every command that can hit disk or Argon2 is `async` (runs off the main thread). Argon2
// never runs while `state.vault` is locked: key material is copied out, the mutex is only
// taken again to swap the result in, so `vault_status` never waits behind a KDF.

use crate::vault::{
    self,
    format::Mode,
    oswrap::OsWrap,
    store::{self, Backup, CreateMode, Keys, Vault},
    VaultError,
};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    Arc, MutexGuard, PoisonError,
};
use zeroize::Zeroizing;

const MIN_PW: usize = 8;
// Error codes: the UI branches on these with `hasCode()` and shows catalog text
// (src/lib/ipc.ts CODE_TEXT). Never reword them; never match error wording in the UI.
pub(crate) const LOCKED: &str = "VAULT_LOCKED";
pub(crate) const WAS_LOCKED: &str = "VAULT_WAS_LOCKED";
const VAULT_BUSY: &str = "VAULT_BUSY";
const EXPORT_WRITE: &str = "Could not write the export file.";

pub(crate) type Slot = Mutex<Option<Vault>>;

/// Lock-generation snapshot: the one guard against landing work in a vault that was locked
/// (and maybe re-opened) meanwhile. `Gen::now` before slow work, `check`/`current` after it;
/// when writing, check under the vault guard (`vault::take_for_lock` bumps under that mutex).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Gen(u64);

impl Gen {
    pub(crate) fn now(gen: &AtomicU64) -> Self {
        Self(gen.load(Ordering::SeqCst))
    }

    /// No lock happened since this snapshot.
    pub(crate) fn current(self, gen: &AtomicU64) -> bool {
        gen.load(Ordering::SeqCst) == self.0
    }

    /// `current` as a command result: `Err(WAS_LOCKED)` after a lock.
    pub(crate) fn check(self, gen: &AtomicU64) -> CmdResult<()> {
        if self.current(gen) {
            Ok(())
        } else {
            Err(WAS_LOCKED.into())
        }
    }
}

/// Tell the UI that Rust-owned state behind these react-query roots changed (`state://changed`,
/// handled by src/app/useStateSync.ts), e.g. `state_changed(app, &["autoStatus"])`.
pub(crate) fn state_changed(app: &tauri::AppHandle, keys: &[&str]) {
    use tauri::Emitter;
    if let Err(e) = app.emit("state://changed", keys) {
        log::warn!("could not emit state://changed: {e}");
    }
}

/// Everything the vault commands touch, borrowed from `AppState` (or built in tests).
struct Ctx<'a> {
    slot: &'a Slot,
    gen: &'a AtomicU64,
    activity: &'a AtomicI64,
    vault: PathBuf,
    cache: PathBuf,
    backups: PathBuf,
    root: PathBuf,
}

impl Ctx<'_> {
    fn of(state: &AppState) -> Ctx<'_> {
        Ctx {
            slot: &state.vault,
            gen: &state.lock_gen,
            activity: &state.last_activity,
            vault: state.paths.vault(),
            cache: state.paths.dk_cache(),
            backups: state.paths.backups.clone(),
            root: state.paths.root.clone(),
        }
    }

    fn touch(&self) {
        self.activity.store(vault::mono_now(), Ordering::Relaxed);
    }

    fn gen_now(&self) -> Gen {
        Gen::now(self.gen)
    }
}

/// A panic elsewhere must not brick the vault: take the data even if the mutex is poisoned.
pub(crate) fn guard(slot: &Slot) -> MutexGuard<'_, Option<Vault>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// User-readable text; everything that isn't a plain wrong password is logged (no secrets).
pub(crate) fn verr(e: VaultError) -> String {
    match &e {
        VaultError::WrongPassword => {}
        VaultError::Io(io) => log::warn!("vault io: {io}"),
        VaultError::OsWrap(d) => log::warn!("vault os wrap: {d}"),
        VaultError::Corrupt => log::warn!("vault file failed to parse or decrypt"),
        VaultError::DeviceKey(m) => log::error!("device key: {m}"),
        VaultError::Refused(m) => log::info!("vault refused: {m}"),
    }
    e.to_string()
}

fn check_pw(pw: &str) -> CmdResult<()> {
    if pw.chars().count() < MIN_PW {
        return Err(format!("Password must be at least {MIN_PW} characters."));
    }
    Ok(())
}

/// Insert `v` unless a lock happened since `start` (lock wins; checked under the slot mutex,
/// the same mutex `vault::lock` bumps the generation under). `prep` runs on `v` just before it
/// goes in, so file changes it makes (portable re-wrap) never happen for a cancelled unlock.
/// Another unlock already landed: `v` is dropped (same file and key). Returns the guard so the
/// caller can finish (remember cache) before a lock can slip in.
fn insert_if_current<'a>(
    c: &Ctx<'a>,
    start: Gen,
    mut v: Vault,
    prep: impl FnOnce(&mut Vault) -> CmdResult<()>,
) -> CmdResult<MutexGuard<'a, Option<Vault>>> {
    let mut g = guard(c.slot);
    start.check(c.gen)?;
    if g.is_none() {
        prep(&mut v)?;
        *g = Some(v);
    }
    Ok(g)
}

/// Run `f` on the unlocked vault. Never call Argon2 inside `f`.
pub(crate) fn with_vault<T>(
    slot: &Slot,
    f: impl FnOnce(&mut Vault) -> CmdResult<T>,
) -> CmdResult<T> {
    f(guard(slot).as_mut().ok_or(LOCKED)?)
}

pub(crate) fn keys(slot: &Slot) -> CmdResult<Keys> {
    with_vault(slot, |v| Ok(v.keys()))
}

/// Password-mode re-auth (Argon2 outside the mutex). OS mode passes.
pub(crate) fn require_password(slot: &Slot, pw: Option<&str>) -> CmdResult<()> {
    let k = keys(slot)?;
    if k.mode() == Mode::Password {
        let ok = match pw {
            Some(p) => k.verify_password(p).map_err(verr)?,
            None => false,
        };
        if !ok {
            return Err(VaultError::WrongPassword.to_string());
        }
    }
    Ok(())
}

/// Absolute path whose folder exists and is outside the app data dir.
fn export_dest(root: &Path, path: &str) -> CmdResult<PathBuf> {
    let p = Path::new(path);
    let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else {
        return Err("Choose a full file path.".into());
    };
    if !p.is_absolute() {
        return Err("Choose a full file path.".into());
    }
    let dir = dir
        .canonicalize()
        .map_err(|_| "That folder does not exist.")?;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    if dir.starts_with(&root) {
        return Err("Choose a folder outside the app data folder.".into());
    }
    Ok(dir.join(name))
}

fn write_export(dest: &Path, bytes: &[u8]) -> CmdResult<()> {
    store::write_atomic(dest, bytes, Backup::Skip).map_err(|e| {
        verr(e);
        EXPORT_WRITE.to_string()
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    exists: bool,
    unlocked: bool,
    mode: Option<Mode>,
    remember_active: bool,
    /// Locked and the file header can't be read: offer backup recovery.
    damaged: bool,
}

/// No OS store / keyring access here: header read + cache prefix only.
#[tauri::command]
pub async fn vault_status(state: State<'_, AppState>) -> CmdResult<VaultStatus> {
    let path = state.paths.vault();
    let open_mode = guard(&state.vault).as_ref().map(|v| v.mode());
    let exists = Vault::exists(&path);
    // Only a header that reads but won't decode is damage; a read error may be transient.
    let (header_mode, unreadable) = match open_mode {
        Some(_) => (None, false),
        None => match std::fs::read(&path) {
            Ok(b) => match vault::format::decode(&b) {
                Ok((h, _, _)) => (Some(h.mode), false),
                Err(_) => (None, true),
            },
            Err(_) => (None, false),
        },
    };
    Ok(VaultStatus {
        exists,
        unlocked: open_mode.is_some(),
        mode: open_mode.or(header_mode),
        remember_active: vault::cache_active(&state.paths.dk_cache(), vault::now_unix()),
        damaged: exists && unreadable,
    })
}

#[tauri::command]
pub async fn vault_create(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mode: String,
    password: Option<String>,
) -> CmdResult<()> {
    // Single flight: `Vault::create` clears a 0-byte placeholder as a crash leftover, so a
    // second create during Argon2 would clobber the first one's file.
    static CREATING: AtomicBool = AtomicBool::new(false);
    struct Turn;
    impl Drop for Turn {
        fn drop(&mut self) {
            CREATING.store(false, Ordering::SeqCst);
        }
    }
    if CREATING.swap(true, Ordering::SeqCst) {
        return Err(VAULT_BUSY.into());
    }
    let _turn = Turn;
    let password = password.map(Zeroizing::new);
    let c = Ctx::of(&state);
    let start = c.gen_now();
    if Vault::exists(&c.vault) {
        return Err("A vault already exists.".into());
    }
    let mode = match (mode.as_str(), password) {
        ("os", _) => CreateMode::Os,
        ("password", Some(pw)) => {
            check_pw(&pw)?;
            CreateMode::Password(pw)
        }
        ("password", None) => return Err("Enter a password.".into()),
        _ => return Err("Unknown vault mode.".into()),
    };
    let v = Vault::create(&c.vault, mode, state.os().map_err(verr)?).map_err(|e| match e {
        VaultError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists => {
            "A vault already exists.".into()
        }
        e => verr(e),
    })?;
    log::info!("vault created");
    c.touch();
    // Lock wins: a Lock during Argon2 leaves the new vault created but locked. Not an error,
    // the vault exists; the gate refetch shows Unlock.
    if insert_if_current(&c, start, v, |_| Ok(())).is_err() {
        log::info!("vault created, then locked before it opened");
    }
    state_changed(&app, &["vault"]);
    crate::accounts::cmds::emit_changed(&app);
    Ok(())
}

/// Remember-device side effects only follow a typed password: a passwordless (startup or
/// cache) unlock never deletes the cache.
fn apply_remember(
    c: &Ctx,
    os: &dyn OsWrap,
    typed: bool,
    remember: bool,
    mode: Mode,
    dk: &[u8; 32],
) {
    if !typed {
        return;
    }
    if !remember {
        vault::cache_remove(&c.cache);
    } else if mode == Mode::Password {
        if let Err(e) = vault::cache_write(&c.cache, os, dk, vault::now_unix()) {
            log::warn!("remember-device cache not written: {}", verr(e));
        }
    }
}

fn unlock_impl(
    c: &Ctx,
    start: Gen,
    os: Arc<dyn OsWrap>,
    password: Option<&str>,
    remember: bool,
) -> CmdResult<()> {
    let open_keys = guard(c.slot).as_ref().map(|v| v.keys());
    if let Some(k) = open_keys {
        // Already unlocked: a typed password must still be right; honour remember.
        if let Some(pw) = password {
            if k.mode() == Mode::Password && !k.verify_password(pw).map_err(verr)? {
                return Err(VaultError::WrongPassword.to_string());
            }
            apply_remember(c, &*os, true, remember, k.mode(), k.dk());
        }
        return Ok(());
    }
    if !Vault::exists(&c.vault) {
        return Err("No vault yet.".into());
    }
    let v = match password {
        Some(pw) => Vault::open(&c.vault, Some(pw), os.clone()).map_err(verr)?,
        None => {
            let dk = vault::cache_read(&c.cache, &*os, vault::now_unix())
                .inspect_err(|&dead| {
                    // expired or garbled body (a good prefix kept `rememberActive` true): purge.
                    // A read or OS-unwrap failure may be transient: keep it.
                    if dead {
                        vault::cache_remove(&c.cache);
                    }
                })
                .ok();
            let cached = dk.and_then(|dk| {
                Vault::open_with_dk(&c.vault, dk, os.clone())
                    .inspect_err(|e| {
                        // A wrong (stale) key fails decryption as Corrupt; Io may be transient.
                        if matches!(e, VaultError::Corrupt) {
                            log::warn!("remember-device cache is stale ({e}); removing it");
                            vault::cache_remove(&c.cache);
                        } else {
                            log::warn!("remember-device unlock failed: {e}");
                        }
                    })
                    .ok()
            });
            match cached {
                Some(v) => v,
                None => Vault::open(&c.vault, None, os.clone()).map_err(|e| match e {
                    VaultError::WrongPassword => "Password required.".into(),
                    e => verr(e),
                })?,
            }
        }
    };
    // A portable file dropped in as the live vault: bind it to this machine too. Argon2 runs
    // here; the file is re-wrapped only once the unlock is sure to land (insert_if_current).
    let rewrap = match (v.mode(), password) {
        (Mode::Portable, Some(pw)) => {
            let k = v.keys();
            let h = k.rewrap(Some(pw)).map_err(verr)?;
            Some((k, h))
        }
        _ => None,
    };
    let g = insert_if_current(c, start, v, |v| {
        if let Some((k, h)) = rewrap {
            v.apply_rewrap(&k, h).map_err(verr)?;
            log::info!("portable vault re-wrapped to password mode");
        }
        Ok(())
    })?;
    c.touch();
    let v = g.as_ref().ok_or(LOCKED)?;
    apply_remember(c, &*os, password.is_some(), remember, v.mode(), v.dk());
    Ok(())
}

#[tauri::command]
pub async fn vault_unlock(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    password: Option<String>,
    remember: bool,
) -> CmdResult<()> {
    let password = password.map(Zeroizing::new);
    let c = Ctx::of(&state);
    let start = c.gen_now();
    if !Vault::exists(&c.vault) {
        return Err("No vault yet.".into()); // before os(): never mint a device key without a vault
    }
    let os = state.os().map_err(verr)?;
    unlock_impl(
        &c,
        start,
        os,
        password.as_deref().map(|s| s.as_str()),
        remember,
    )?;
    log::info!("vault unlocked");
    state_changed(&app, &["vault"]);
    crate::accounts::cmds::emit_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn vault_lock(app: tauri::AppHandle) -> CmdResult<()> {
    vault::lock(&app);
    Ok(())
}

fn change_password_impl(c: &Ctx, current: Option<&str>, new: Option<&str>) -> CmdResult<()> {
    if let Some(pw) = new {
        check_pw(pw)?;
    }
    require_password(c.slot, current)?;
    let k = keys(c.slot)?;
    let h = k.rewrap(new).map_err(verr)?;
    // Cache first: the DK never changes, so a cache left by a crash after the re-wrap would
    // still open the vault without the new password. A failed re-wrap just costs a re-type.
    vault::cache_remove(&c.cache);
    with_vault(c.slot, |v| v.apply_rewrap(&k, h).map_err(verr))?;
    c.touch();
    Ok(())
}

#[tauri::command]
pub async fn vault_change_password(
    state: State<'_, AppState>,
    current: Option<String>,
    new: Option<String>,
) -> CmdResult<()> {
    let (current, new) = (current.map(Zeroizing::new), new.map(Zeroizing::new));
    change_password_impl(
        &Ctx::of(&state),
        current.as_deref().map(|s| s.as_str()),
        new.as_deref().map(|s| s.as_str()),
    )?;
    log::info!("vault key re-wrapped");
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    name: String,
    /// UTC, ISO 8601 with `Z`.
    created_at: String,
    size: u64,
}

#[tauri::command]
pub async fn vault_backups(state: State<'_, AppState>) -> CmdResult<Vec<BackupInfo>> {
    let dir = match std::fs::read_dir(&state.paths.backups) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(verr(e.into())),
    };
    let mut out: Vec<(store::BackupKey, BackupInfo)> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let k = store::parse_backup_name(&name)?;
            let info = BackupInfo {
                created_at: k.0.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                size: e.metadata().ok()?.len(),
                name,
            };
            Some((k, info))
        })
        .collect();
    out.sort_by_key(|(k, _)| std::cmp::Reverse(*k));
    Ok(out.into_iter().map(|(_, i)| i).collect())
}

/// Unlocked: swap the backup's payload into the open vault (current header kept; the
/// backup must decrypt with the current DK). Locked: only when the current vault can't be
/// opened with `password`; the backup must open with it, the damaged file is kept as
/// `vault.rmiv.damaged-<ts>`, and the vault stays locked.
fn restore_impl(
    c: &Ctx,
    name: &str,
    password: Option<&str>,
    os: impl FnOnce() -> CmdResult<Arc<dyn OsWrap>>,
) -> CmdResult<()> {
    if store::parse_backup_name(name).is_none() {
        return Err("Unknown backup.".into());
    }
    let src = c.backups.join(name);
    if !src.is_file() {
        return Err("Unknown backup.".into());
    }
    if guard(c.slot).is_some() {
        with_vault(c.slot, |v| v.restore_from(&src).map_err(verr))?;
        c.touch();
        return Ok(());
    }
    let os = os()?;
    // Locked path: only a genuinely damaged file. A wrong password (or any other error)
    // must never let someone swap out a healthy vault.
    let cur_mode = std::fs::read(&c.vault)
        .ok()
        .and_then(|b| vault::format::decode(&b).ok().map(|(h, _, _)| h.mode));
    match Vault::open(&c.vault, password, os.clone()) {
        Ok(_) => return Err("The vault opens fine: unlock it, then restore from Settings.".into()),
        Err(VaultError::Corrupt) => {}
        Err(e) => return Err(verr(e)),
    }
    // Password/portable backups must decrypt with the entered password (Vault::open checks).
    let b = Vault::open(&src, password, os).map_err(verr)?;
    if b.mode() == Mode::Os && cur_mode != Some(Mode::Os) {
        return Err(
            "This backup opens without a password, so it can't replace this vault here. \
             Open the backups folder to recover it manually."
                .into(),
        );
    }
    drop(b);
    let bytes = std::fs::read(&src).map_err(|e| verr(e.into()))?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    // never overwrite an earlier damaged copy (same second): add -1, -2, ...
    let keep = (0..)
        .map(|n| match n {
            0 => format!("vault.rmiv.damaged-{stamp}"),
            n => format!("vault.rmiv.damaged-{stamp}-{n}"),
        })
        .map(|name| c.vault.with_file_name(name))
        .find(|p| !p.exists())
        .expect("unbounded range");
    if Vault::exists(&c.vault) {
        std::fs::copy(&c.vault, &keep).map_err(|e| {
            log::error!("could not keep damaged vault copy: {e}");
            "Could not keep a copy of the damaged vault, so nothing was changed.".to_string()
        })?;
    }
    store::write_atomic(&c.vault, &bytes, Backup::Skip).map_err(verr)?;
    vault::cache_remove(&c.cache);
    log::warn!(
        "damaged vault replaced from backup; old file kept as {}",
        keep.display()
    );
    Ok(())
}

#[tauri::command]
pub async fn vault_restore(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    name: String,
    password: Option<String>,
) -> CmdResult<()> {
    let password = password.map(Zeroizing::new);
    restore_impl(
        &Ctx::of(&state),
        &name,
        password.as_deref().map(|s| s.as_str()),
        || state.os().map_err(verr),
    )?;
    log::info!("vault restored from backup");
    crate::accounts::cmds::emit_changed(&app);
    Ok(())
}

fn export_encrypted_impl(
    c: &Ctx,
    os: &dyn OsWrap,
    path: &str,
    password: &str,
    current: Option<&str>,
) -> CmdResult<()> {
    check_pw(password)?;
    let dest = export_dest(&c.root, path)?;
    require_password(c.slot, current)?;
    let (h, dk) = store::portable_wrap(password, os).map_err(verr)?;
    let bytes = with_vault(c.slot, |v| Ok(v.sealed_copy(h, &dk)))?;
    write_export(&dest, &bytes)
}

#[tauri::command]
pub async fn vault_export_encrypted(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    password: String,
    current: Option<String>,
) -> CmdResult<()> {
    let password = Zeroizing::new(password);
    let current = current.map(Zeroizing::new);
    let c = Ctx::of(&state);
    keys(c.slot)?; // fail fast when locked (before touching the OS store)
    crate::accounts::cmds::os_presence(&app, &state).await?;
    export_encrypted_impl(
        &c,
        &*state.os().map_err(verr)?,
        &path,
        &password,
        current.as_deref().map(|s| s.as_str()),
    )?;
    log::info!("vault exported (encrypted)");
    Ok(())
}

fn export_plain_impl(c: &Ctx, path: &str, confirm: &str, password: Option<&str>) -> CmdResult<()> {
    vault::check_plain_confirm(confirm)?;
    let dest = export_dest(&c.root, path)?;
    require_password(c.slot, password)?;
    let json = with_vault(c.slot, |v| Ok(vault::plain_json(v)))?;
    write_export(&dest, &json)
}

/// Plaintext export: typed confirmation, plus the current password in password mode.
#[tauri::command]
pub async fn vault_export_plain(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    confirm: String,
    password: Option<String>,
) -> CmdResult<()> {
    let password = password.map(Zeroizing::new);
    vault::check_plain_confirm(&confirm)?; // before any OS prompt
    crate::accounts::cmds::os_presence(&app, &state).await?;
    export_plain_impl(
        &Ctx::of(&state),
        &path,
        &confirm,
        password.as_deref().map(|s| s.as_str()),
    )?;
    log::warn!("vault exported as plaintext");
    Ok(())
}

/// Replace the current payload with the one in another vault file (any mode). The current
/// file is backed up first (required); memory changes only after the write succeeds.
fn import_impl(c: &Ctx, os: Arc<dyn OsWrap>, path: &str, password: Option<&str>) -> CmdResult<()> {
    let start = c.gen_now();
    keys(c.slot)?; // fail fast when locked
    let src = Vault::open(Path::new(path), password, os).map_err(verr)?;
    with_vault(c.slot, |v| {
        start.check(c.gen)?; // locked (and maybe reopened) while reading the source
        v.replace_payload(src.payload().clone()).map_err(verr)
    })?;
    c.touch();
    Ok(())
}

#[tauri::command]
pub async fn vault_import_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
) -> CmdResult<()> {
    let password = password.map(Zeroizing::new);
    let c = Ctx::of(&state);
    keys(c.slot)?;
    import_impl(
        &c,
        state.os().map_err(verr)?,
        &path,
        password.as_deref().map(|s| s.as_str()),
    )?;
    log::info!("vault imported from file");
    crate::accounts::cmds::emit_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn vault_touch(state: State<'_, AppState>) {
    Ctx::of(&state).touch();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::oswrap::FakeOsWrap;
    use serde_json::json;

    #[test]
    fn sync_reacquires_dead_guard() {
        struct G(bool);
        impl crate::platform::Held for G {
            fn alive(&self) -> bool {
                self.0
            }
        }
        let slot: Mutex<Option<Guard>> = Mutex::new(Some(Box::new(G(true))));
        sync(&slot, true, "t", || panic!("alive: no re-acquire")).unwrap();
        *slot.lock().unwrap() = Some(Box::new(G(false)));
        let mut n = 0;
        sync(&slot, true, "t", || {
            n += 1;
            Ok(Box::new(G(true)) as Guard)
        })
        .unwrap();
        assert_eq!(n, 1);
        assert!(slot.lock().unwrap().as_ref().unwrap().alive());
    }

    #[test]
    fn save_name_is_sanitized() {
        assert_eq!(safe_name("Al_ice-1.x"), "Al_ice-1.x");
        assert_eq!(safe_name("a/b\\c:d*e\n"), "a_b_c_d_e_");
    }

    #[test]
    fn read_only_settings_apply_in_memory_and_never_touch_the_file() {
        let d = tempfile::tempdir().unwrap();
        let rc =
            crate::roblox::RobloxClient::new(crate::roblox::BaseUrls::all("http://127.0.0.1:9"));
        let mut st = crate::test_state(d.path(), rc);
        st.settings_read_only = true;
        let out = persist(&st, &json!({ "multi": { "cookieLock": false } })).unwrap();
        assert_eq!(out.pointer("/multi/cookieLock"), Some(&json!(false)));
        assert_eq!(*st.settings.lock().unwrap(), out);
        assert!(!st.paths.settings().exists());
    }

    #[test]
    fn status_reports_in_process_mutex_without_deadlock() {
        struct InProc;
        impl crate::platform::Held for InProc {
            fn in_process(&self) -> bool {
                true
            }
        }
        let d = tempfile::tempdir().unwrap();
        let rc =
            crate::roblox::RobloxClient::new(crate::roblox::BaseUrls::all("http://127.0.0.1:9"));
        let st = crate::test_state(d.path(), rc);
        *st.settings.lock().unwrap() = json!({ "multi": { "enabled": true } });
        *st.multi_guard.lock().unwrap() = Some(Box::new(InProc));
        let s = serde_json::to_value(roblox_status_now(&st)).unwrap();
        assert_eq!(
            (
                &s["multiActive"],
                &s["multiInProcess"],
                &s["multiReleasePending"]
            ),
            (&json!(true), &json!(true), &json!(false))
        );
        // turned off, no client running (Null platform): released at once
        *st.settings.lock().unwrap() = json!({ "multi": { "enabled": false } });
        let s = serde_json::to_value(roblox_status_now(&st)).unwrap();
        assert_eq!(s["multiActive"], json!(false));
    }

    #[test]
    fn install_change_is_reported_once() {
        use crate::platform::RobloxInstall;
        let a = RobloxInstall {
            version_dir: Some("v1".into()),
            bootstrapper: None,
        };
        let b = RobloxInstall {
            bootstrapper: Some("Bloxstrap".into()),
            ..a.clone()
        };
        let mut last = None;
        assert!(swap_seen(&mut last, Some(&a)));
        assert!(!swap_seen(&mut last, Some(&a)));
        assert!(swap_seen(&mut last, Some(&b)));
        assert!(!swap_seen(&mut last, Some(&b)));
        assert!(swap_seen(&mut last, None));
    }

    #[test]
    fn client_settings_file_must_be_a_json_object() {
        let d = tempfile::tempdir().unwrap();
        let w = |name: &str, body: &str| {
            let p = d.path().join(name);
            std::fs::write(&p, body).unwrap();
            p
        };
        assert_eq!(
            read_client_settings(&w("ok.json", r#"{"A":1}"#)).unwrap(),
            json!({"A": 1})
        );
        assert!(read_client_settings(&w("arr.json", "[1]")).is_err());
        assert!(read_client_settings(&w("bad.json", "{nope")).is_err());
        assert!(read_client_settings(&d.path().join("missing.json")).is_err());
    }

    struct T {
        _d: tempfile::TempDir,
        root: PathBuf,
        slot: Slot,
        gen: AtomicU64,
        activity: AtomicI64,
    }

    impl T {
        fn c(&self) -> Ctx<'_> {
            Ctx {
                slot: &self.slot,
                gen: &self.gen,
                activity: &self.activity,
                vault: self.root.join("vault.rmiv"),
                cache: self.root.join("dk.cache"),
                backups: self.root.join("backups"),
                root: self.root.clone(),
            }
        }
        fn path(&self) -> PathBuf {
            self.root.join("vault.rmiv")
        }
        fn cache(&self) -> PathBuf {
            self.root.join("dk.cache")
        }
        fn lock(&self) {
            vault::take_for_lock(&self.slot, &self.gen);
        }
        fn unlocked(&self) -> bool {
            self.slot.lock().unwrap().is_some()
        }
        fn backups(&self) -> Vec<String> {
            let mut v: Vec<_> = std::fs::read_dir(self.root.join("backups"))
                .map(|d| {
                    d.flatten()
                        .map(|e| e.file_name().into_string().unwrap())
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            v
        }
    }

    fn setup(pw: Option<&str>) -> T {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_owned();
        let mode = match pw {
            Some(p) => CreateMode::Password(Zeroizing::new(p.into())),
            None => CreateMode::Os,
        };
        Vault::create(&root.join("vault.rmiv"), mode, os()).unwrap();
        T {
            root,
            slot: Mutex::new(None),
            gen: AtomicU64::new(0),
            activity: AtomicI64::new(-1),
            _d: d,
        }
    }

    fn os() -> Arc<dyn OsWrap> {
        Arc::new(FakeOsWrap)
    }

    fn unlock(t: &T, pw: Option<&str>, remember: bool) -> CmdResult<()> {
        let c = t.c();
        unlock_impl(&c, c.gen_now(), os(), pw, remember)
    }

    fn set_marker(t: &T, m: &str) {
        with_vault(&t.slot, |v| {
            v.payload_mut().extra.insert("m".into(), json!(m));
            v.save().map_err(verr)
        })
        .unwrap();
    }

    fn marker(t: &T) -> serde_json::Value {
        with_vault(&t.slot, |v| Ok(v.payload().extra["m"].clone())).unwrap()
    }

    #[test]
    fn unlock_without_password_says_required() {
        let t = setup(Some("hunter22"));
        assert_eq!(unlock(&t, None, false).unwrap_err(), "Password required.");
        assert_eq!(
            unlock(&t, Some("nope-nope"), false).unwrap_err(),
            "WRONG_PASSWORD"
        );
        assert!(!t.unlocked());
        unlock(&t, Some("hunter22"), false).unwrap();
        assert!(t.unlocked());
        assert!(t.activity.load(Ordering::Relaxed) >= 0, "touched on unlock");
    }

    #[test]
    fn gen_guard_flags_any_lock_since_the_snapshot() {
        let a = AtomicU64::new(7);
        let g = Gen::now(&a);
        assert!(g.current(&a) && g.check(&a).is_ok());
        a.fetch_add(1, Ordering::SeqCst);
        assert!(!g.current(&a));
        assert_eq!(g.check(&a).unwrap_err(), WAS_LOCKED);
    }

    #[test]
    fn lock_during_unlock_wins() {
        let t = setup(Some("hunter22"));
        let c = t.c();
        let start = c.gen_now();
        t.lock(); // vault::lock() landed while Argon2 ran: lock wins
        let r = unlock_impl(&c, start, os(), Some("hunter22"), true);
        assert_eq!(r.unwrap_err(), WAS_LOCKED);
        assert!(!t.unlocked());
        assert!(
            !t.cache().exists(),
            "no remember cache from an aborted unlock"
        );
        unlock(&t, Some("hunter22"), false).unwrap(); // a fresh attempt works
        assert!(t.unlocked());
    }

    #[test]
    fn remember_survives_passwordless_unlocks() {
        let t = setup(Some("hunter22"));
        unlock(&t, Some("hunter22"), true).unwrap();
        assert!(vault::cache_active(&t.cache(), vault::now_unix()));
        for _ in 0..2 {
            t.lock();
            unlock(&t, None, false).unwrap(); // startup auto-attempt sends remember=false
            assert!(t.cache().exists(), "cache kept after a cache unlock");
        }
        // typed password with remember=false clears it
        t.lock();
        unlock(&t, Some("hunter22"), false).unwrap();
        assert!(!t.cache().exists());
        t.lock();
        assert_eq!(unlock(&t, None, false).unwrap_err(), "Password required.");
    }

    #[test]
    fn stale_cache_falls_back_and_is_removed() {
        let t = setup(Some("hunter22"));
        vault::cache_write(&t.cache(), &FakeOsWrap, &[9u8; 32], vault::now_unix()).unwrap();
        assert_eq!(unlock(&t, None, false).unwrap_err(), "Password required.");
        assert!(!t.cache().exists());
        // good prefix, unreadable body: removed too, so rememberActive stops lying
        let far = (vault::now_unix() + 999).to_le_bytes();
        std::fs::write(t.cache(), [&far[..], b"junk"].concat()).unwrap();
        assert!(vault::cache_active(&t.cache(), vault::now_unix()));
        assert_eq!(unlock(&t, None, false).unwrap_err(), "Password required.");
        assert!(!t.cache().exists());
    }

    #[test]
    fn cache_kept_when_os_unwrap_fails() {
        struct DownOs;
        impl OsWrap for DownOs {
            fn wrap(&self, _: &[u8]) -> vault::Result<Vec<u8>> {
                Err(VaultError::OsWrap("down".into()))
            }
            fn unwrap(&self, _: &[u8]) -> vault::Result<Zeroizing<Vec<u8>>> {
                Err(VaultError::OsWrap("down".into()))
            }
        }
        let t = setup(Some("hunter22"));
        vault::cache_write(&t.cache(), &FakeOsWrap, &[9u8; 32], vault::now_unix()).unwrap();
        let c = t.c();
        assert!(unlock_impl(&c, c.gen_now(), Arc::new(DownOs), None, false).is_err());
        assert!(
            t.cache().exists(),
            "a transient OS-store failure must not forget the device"
        );
    }

    #[test]
    fn already_unlocked_still_checks_password_and_remember() {
        let t = setup(Some("hunter22"));
        unlock(&t, Some("hunter22"), false).unwrap();
        assert_eq!(
            unlock(&t, Some("wrongpw1"), true).unwrap_err(),
            "WRONG_PASSWORD"
        );
        assert!(!t.cache().exists());
        unlock(&t, Some("hunter22"), true).unwrap();
        assert!(t.cache().exists(), "remember honoured while unlocked");
    }

    #[test]
    fn change_password_needs_current() {
        let t = setup(Some("oldpass1"));
        let c = t.c();
        assert_eq!(
            change_password_impl(&c, Some("oldpass1"), Some("newpass1")).unwrap_err(),
            LOCKED
        );
        unlock(&t, Some("oldpass1"), true).unwrap();
        for cur in [None, Some("wrongpw1")] {
            assert_eq!(
                change_password_impl(&c, cur, Some("newpass1")).unwrap_err(),
                "WRONG_PASSWORD"
            );
        }
        assert!(change_password_impl(&c, Some("oldpass1"), Some("short")).is_err());
        change_password_impl(&c, Some("oldpass1"), Some("newpass1")).unwrap();
        assert!(!t.cache().exists());
        assert!(Vault::open(&t.path(), Some("newpass1"), os()).is_ok());
    }

    #[test]
    fn stale_keys_refused() {
        let t = setup(Some("oldpass1"));
        unlock(&t, Some("oldpass1"), false).unwrap();
        let k = keys(&t.slot).unwrap();
        let h = k.rewrap(Some("first111")).unwrap();
        change_password_impl(&t.c(), Some("oldpass1"), Some("second11")).unwrap();
        let e = with_vault(&t.slot, |v| v.apply_rewrap(&k, h).map_err(verr)).unwrap_err();
        assert_eq!(e, "The vault changed, try again.");
        assert!(Vault::open(&t.path(), Some("second11"), os()).is_ok());
    }

    #[test]
    fn portable_live_vault_rewrapped_on_unlock() {
        let t = setup(None);
        let v = Vault::open(&t.path(), None, os()).unwrap();
        v.export_portable(&t.path(), "portable1").unwrap();
        unlock(&t, Some("portable1"), false).unwrap();
        assert_eq!(
            t.slot.lock().unwrap().as_ref().unwrap().mode(),
            Mode::Password
        );
        let o = Vault::open(&t.path(), Some("portable1"), os()).unwrap();
        assert_eq!(o.mode(), Mode::Password);
    }

    #[test]
    fn lock_during_portable_unlock_leaves_file_portable() {
        let t = setup(None);
        let v = Vault::open(&t.path(), None, os()).unwrap();
        v.export_portable(&t.path(), "portable1").unwrap();
        let c = t.c();
        let start = c.gen_now();
        t.lock();
        let r = unlock_impl(&c, start, os(), Some("portable1"), false);
        assert_eq!(r.unwrap_err(), WAS_LOCKED);
        assert!(!t.unlocked());
        let o = Vault::open(&t.path(), Some("portable1"), os()).unwrap();
        assert_eq!(
            o.mode(),
            Mode::Portable,
            "not converted by a cancelled unlock"
        );
    }

    #[test]
    fn export_dest_rules() {
        let t = setup(None);
        let inside = t.root.join("x.json");
        assert!(export_dest(&t.root, inside.to_str().unwrap()).is_err());
        assert!(export_dest(&t.root, "relative.json").is_err());
        let out = tempfile::tempdir().unwrap();
        let ok = out.path().join("x.json");
        assert!(export_dest(&t.root, ok.to_str().unwrap()).is_ok());
    }

    #[test]
    fn exports_need_current_password() {
        let t = setup(Some("hunter22"));
        unlock(&t, Some("hunter22"), false).unwrap();
        let out = tempfile::tempdir().unwrap();
        let enc = out.path().join("e.rmiv");
        let plain = out.path().join("p.json");
        let (e, p) = (enc.to_str().unwrap(), plain.to_str().unwrap());
        for cur in [None, Some("wrongpw1")] {
            assert_eq!(
                export_encrypted_impl(&t.c(), &FakeOsWrap, e, "exportpw", cur).unwrap_err(),
                "WRONG_PASSWORD"
            );
            assert_eq!(
                export_plain_impl(&t.c(), p, "export plaintext", cur).unwrap_err(),
                "WRONG_PASSWORD"
            );
        }
        assert_eq!(
            export_plain_impl(&t.c(), p, "export", Some("hunter22")).unwrap_err(),
            "Type \"export plaintext\" to confirm."
        );
        assert!(!enc.exists() && !plain.exists());
        export_encrypted_impl(&t.c(), &FakeOsWrap, e, "exportpw", Some("hunter22")).unwrap();
        export_plain_impl(&t.c(), p, "export plaintext", Some("hunter22")).unwrap();
        assert!(Vault::open(&enc, Some("exportpw"), os()).is_ok());
        assert!(plain.exists());
    }

    #[test]
    fn export_write_failure_is_plain_message() {
        let t = setup(None);
        unlock(&t, None, false).unwrap();
        let out = tempfile::tempdir().unwrap();
        let dir_as_dest = out.path().join("adir");
        std::fs::create_dir(&dir_as_dest).unwrap();
        assert_eq!(
            export_plain_impl(
                &t.c(),
                dir_as_dest.to_str().unwrap(),
                "export plaintext",
                None
            )
            .unwrap_err(),
            EXPORT_WRITE
        );
    }

    #[test]
    fn restore_unlocked_swaps_payload_keeps_header() {
        let t = setup(Some("oldpass1"));
        unlock(&t, Some("oldpass1"), false).unwrap();
        set_marker(&t, "one");
        // routine saves throttle backups; take one of the "one" state explicitly
        let bytes = std::fs::read(t.path()).unwrap();
        store::write_atomic(&t.path(), &bytes, Backup::Try).unwrap();
        set_marker(&t, "two");
        let names = t.backups();
        let one = names
            .iter()
            .find(|n| {
                let v = Vault::open(&t.root.join("backups").join(n), Some("oldpass1"), os());
                v.is_ok_and(|v| v.payload().extra.get("m") == Some(&json!("one")))
            })
            .unwrap()
            .clone();
        change_password_impl(&t.c(), Some("oldpass1"), Some("newpass1")).unwrap();
        restore_impl(&t.c(), &one, None, || Ok(os())).unwrap();
        assert!(t.unlocked(), "restore keeps the vault open");
        assert_eq!(marker(&t), json!("one"));
        let o = Vault::open(&t.path(), Some("newpass1"), os()).unwrap();
        assert_eq!(o.payload().extra["m"], json!("one"));
    }

    #[test]
    fn restore_rejects_bad_names_damaged_and_foreign_backups() {
        let t = setup(None);
        unlock(&t, None, false).unwrap();
        set_marker(&t, "x");
        for bad in [
            "../vault.rmiv",
            "vault-x.rmiv",
            "vault-20260101-000000.rmiv",
        ] {
            assert_eq!(
                restore_impl(&t.c(), bad, None, || Ok(os())).unwrap_err(),
                "Unknown backup."
            );
        }
        let dir = t.root.join("backups");
        std::fs::write(dir.join("vault-20260101-000001.rmiv"), b"garbage").unwrap();
        // a vault with a different DK
        let other = tempfile::tempdir().unwrap();
        let op = other.path().join("vault.rmiv");
        Vault::create(&op, CreateMode::Os, os()).unwrap();
        std::fs::copy(&op, dir.join("vault-20260101-000002.rmiv")).unwrap();
        let before = std::fs::read(t.path()).unwrap();
        for n in ["vault-20260101-000001.rmiv", "vault-20260101-000002.rmiv"] {
            let e = restore_impl(&t.c(), n, None, || Ok(os())).unwrap_err();
            assert!(e.contains("current vault key"), "{e}");
        }
        assert_eq!(std::fs::read(t.path()).unwrap(), before);
        assert_eq!(marker(&t), json!("x"));
    }

    #[test]
    fn restore_locked_only_for_damaged_vault() {
        let t = setup(Some("hunter22"));
        unlock(&t, Some("hunter22"), false).unwrap();
        set_marker(&t, "good");
        set_marker(&t, "later");
        let name = t.backups().pop().unwrap();
        t.lock();
        // vault still fine: refuse
        assert!(restore_impl(&t.c(), &name, Some("hunter22"), || Ok(os()))
            .unwrap_err()
            .contains("opens fine"));
        // damage it
        std::fs::write(t.path(), b"RMIVAULT\0\0\0\0v001junk").unwrap();
        assert!(restore_impl(&t.c(), &name, Some("wrongpw1"), || Ok(os())).is_err());
        restore_impl(&t.c(), &name, Some("hunter22"), || Ok(os())).unwrap();
        assert!(!t.unlocked(), "stays locked");
        let kept: Vec<_> = std::fs::read_dir(&t.root)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("vault.rmiv.damaged-")
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert!(Vault::open(&t.path(), Some("hunter22"), os()).is_ok());
    }

    #[test]
    fn locked_restore_refuses_healthy_vault_and_os_backups() {
        let t = setup(Some("hunter22"));
        // an OS-mode vault file dropped into backups/
        let other = tempfile::tempdir().unwrap();
        let op = other.path().join("vault.rmiv");
        Vault::create(&op, CreateMode::Os, os()).unwrap();
        std::fs::create_dir_all(t.root.join("backups")).unwrap();
        let name = "vault-20260101-000003.rmiv";
        std::fs::copy(&op, t.root.join("backups").join(name)).unwrap();
        let before = std::fs::read(t.path()).unwrap();
        // healthy password vault, wrong or no password: refused
        assert_eq!(
            restore_impl(&t.c(), name, Some("wrongpw1"), || Ok(os())).unwrap_err(),
            "WRONG_PASSWORD"
        );
        assert_eq!(
            restore_impl(&t.c(), name, None, || Ok(os())).unwrap_err(),
            "WRONG_PASSWORD"
        );
        // right password: still refused, it opens fine
        assert!(restore_impl(&t.c(), name, Some("hunter22"), || Ok(os()))
            .unwrap_err()
            .contains("opens fine"));
        assert_eq!(std::fs::read(t.path()).unwrap(), before);
        // damaged vault of unknown mode: an OS-mode backup is still refused
        std::fs::write(t.path(), b"RMIVAULT\0\0\0\0v001junk").unwrap();
        assert!(restore_impl(&t.c(), name, None, || Ok(os()))
            .unwrap_err()
            .contains("opens without a password"));
        assert_eq!(
            std::fs::read(t.path()).unwrap(),
            b"RMIVAULT\0\0\0\0v001junk"
        );
    }

    #[test]
    fn locked_restore_os_vault_damaged_payload() {
        let t = setup(None);
        unlock(&t, None, false).unwrap();
        set_marker(&t, "a");
        set_marker(&t, "b");
        let name = t.backups().pop().unwrap();
        t.lock();
        // corrupt the payload, keep the (OS-mode) header readable
        let mut bytes = std::fs::read(t.path()).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(t.path(), &bytes).unwrap();
        restore_impl(&t.c(), &name, None, || Ok(os())).unwrap();
        assert!(Vault::open(&t.path(), None, os()).is_ok());
    }

    /// Rename onto the vault fails (file held without FILE_SHARE_DELETE) after a good backup.
    #[cfg(windows)]
    #[test]
    fn import_rolls_back_when_save_fails() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = setup(None);
        unlock(&t, None, false).unwrap();
        set_marker(&t, "mine");
        let other = tempfile::tempdir().unwrap();
        let op = other.path().join("vault.rmiv");
        let mut ov = Vault::create(&op, CreateMode::Os, os()).unwrap();
        ov.payload_mut().extra.insert("m".into(), json!("theirs"));
        ov.save().unwrap();
        let before = std::fs::read(t.path()).unwrap();
        let n_backups = t.backups().len();
        let hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ only: copy works, rename-over fails
            .open(t.path())
            .unwrap();
        assert!(import_impl(&t.c(), os(), op.to_str().unwrap(), None).is_err());
        drop(hold);
        assert_eq!(marker(&t), json!("mine"));
        assert_eq!(std::fs::read(t.path()).unwrap(), before);
        assert_eq!(t.backups().len(), n_backups + 1, "backup was taken first");
        let tmps = std::fs::read_dir(&t.root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(tmps, 0);
    }

    #[test]
    fn import_rolls_back_when_backup_fails() {
        let t = setup(None);
        unlock(&t, None, false).unwrap();
        set_marker(&t, "mine");
        let other = tempfile::tempdir().unwrap();
        let op = other.path().join("vault.rmiv");
        let mut ov = Vault::create(&op, CreateMode::Os, os()).unwrap();
        ov.payload_mut().extra.insert("m".into(), json!("theirs"));
        ov.save().unwrap();
        // make backups impossible: a file where the folder should be
        std::fs::remove_dir_all(t.root.join("backups")).unwrap();
        std::fs::write(t.root.join("backups"), b"x").unwrap();
        let before = std::fs::read(t.path()).unwrap();
        let e = import_impl(&t.c(), os(), op.to_str().unwrap(), None).unwrap_err();
        assert!(e.contains("back up"), "{e}");
        assert_eq!(marker(&t), json!("mine"));
        assert_eq!(std::fs::read(t.path()).unwrap(), before);
        // with backups working it goes through
        std::fs::remove_file(t.root.join("backups")).unwrap();
        import_impl(&t.c(), os(), op.to_str().unwrap(), None).unwrap();
        assert_eq!(marker(&t), json!("theirs"));
    }
}

/// Folder holding the running exe: what the Windows Security exclusion covers.
fn install_dir() -> CmdResult<String> {
    let exe = std::env::current_exe().map_err(|e| format!("Could not find the RMI folder: {e}"))?;
    let dir = exe.parent().ok_or("Could not find the RMI folder.")?;
    let dir = dir.display().to_string();
    // The path goes inside a double-quoted PowerShell argument.
    if dir.contains('"') {
        return Err("The RMI folder path has a quote in it.".into());
    }
    Ok(dir)
}

/// Double every char PowerShell reads as a single quote (ASCII and U+2018..U+201B).
#[cfg_attr(not(windows), allow(dead_code))]
fn ps_quote(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let q = matches!(c, '\'' | '\u{2018}'..='\u{201B}');
            std::iter::repeat_n(c, if q { 2 } else { 1 })
        })
        .collect()
}

/// The elevated command for this folder (single quotes doubled for PowerShell).
#[cfg_attr(not(windows), allow(dead_code))]
fn defender_cmd_for(dir: &str) -> String {
    format!("Add-MpPreference -ExclusionPath '{}'", ps_quote(dir))
}

/// Exit code of the outer script when the UAC prompt was declined (Start-Process threw).
#[cfg_attr(not(windows), allow(dead_code))]
const UAC_DECLINED: i32 = 2;

/// Outer script: run the command elevated and pass its exit code back.
#[cfg_attr(not(windows), allow(dead_code))]
fn defender_script(dir: &str) -> String {
    // Inside the -ArgumentList string every single quote is doubled once more.
    let inner = ps_quote(&defender_cmd_for(dir));
    format!(
        "try {{ $p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile -Command {inner}' }} catch {{ exit {UAC_DECLINED} }}; exit $p.ExitCode"
    )
}

/// The folder the Windows Security exclusion would cover, for the tour's "Show the command".
#[tauri::command]
pub fn defender_exclude_cmd() -> CmdResult<String> {
    install_dir()
}

/// macOS: may RMI move Roblox windows (Accessibility)? `prompt` also shows the system dialog
/// that lists RMI in System Settings. Elsewhere always true.
#[tauri::command]
pub fn ax_trusted(prompt: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::platform::macos::ax::trusted(prompt)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = prompt;
        true
    }
}

/// Windows only: adds the RMI folder to Windows Security exclusions via one UAC prompt.
#[tauri::command]
pub async fn defender_exclude() -> CmdResult<String> {
    #[cfg(not(windows))]
    {
        Err("Windows Security exclusions are Windows only.".into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let dir = install_dir()?;
        let script = defender_script(&dir);
        let status = tauri::async_runtime::spawn_blocking(move || {
            std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
        })
        .await
        .map_err(|e| format!("Could not run PowerShell: {e}"))?
        .map_err(|e| format!("Could not run PowerShell: {e}"))?;
        match status.code() {
            Some(0) => Ok(dir),
            Some(UAC_DECLINED) => Err("Admin prompt was cancelled.".into()),
            _ => Err("Windows Security did not accept the change.".into()),
        }
    }
}

#[cfg(test)]
mod defender_tests {
    use super::*;

    #[test]
    fn quotes_are_doubled() {
        assert_eq!(
            defender_cmd_for("/a'b"),
            "Add-MpPreference -ExclusionPath '/a''b'"
        );
        assert!(defender_script("/a'b")
            .contains("-Command Add-MpPreference -ExclusionPath ''/a''''b'''"));
        assert_eq!(
            defender_cmd_for("/a\u{2019}b"),
            "Add-MpPreference -ExclusionPath '/a\u{2019}\u{2019}b'"
        );
    }
}
