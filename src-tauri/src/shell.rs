//! Platform shell: tray / menu bar (spec §10.10), close-to-tray, autostart and the updater.

use crate::{accounts::Account, commands::CmdResult, launch::recents::Recent, AppState};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Mutex,
};
use tauri::{
    menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Listener, Manager, Window,
};

const TRAY: &str = "main";
const MAX_ITEMS: usize = 10;
/// Autostart passes this, so "Start minimized" only applies to login starts.
pub const AUTOSTART_ARG: &str = "--autostart";
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const UPDATE_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// Close-to-tray hint: none → due (window hidden to the tray, a toast there is lost) →
/// shown (on the next show; once per run even if the seen flag can't be saved).
static HINT: AtomicU8 = AtomicU8::new(HINT_NONE);
const HINT_NONE: u8 = 0;
const HINT_DUE: u8 = 1;
const HINT_SHOWN: u8 = 2;
static PENDING: Mutex<Option<tauri_plugin_updater::Update>> = Mutex::new(None);

/// Submenu entries as (menu id, label). Empty = submenu disabled.
#[derive(Debug, PartialEq)]
pub struct Model {
    pub recents: Vec<(String, String)>,
    pub favorites: Vec<(String, String)>,
}

/// Windows menus treat `&` as a mnemonic marker.
fn text(s: &str) -> String {
    if cfg!(windows) {
        s.replace('&', "&&")
    } else {
        s.to_string()
    }
}

/// Recents newest first; favorites = pinned accounts in list order ("Account N" when
/// `hide`: privacy mode, the native menu can't be blurred). Nothing while locked.
pub fn model(locked: bool, hide: bool, recents: &[Recent], accounts: &[Account]) -> Model {
    if locked {
        return Model {
            recents: vec![],
            favorites: vec![],
        };
    }
    let mut pinned: Vec<&Account> = accounts.iter().filter(|a| a.pinned).collect();
    pinned.sort_by_key(|a| a.order);
    let label = |a: &Account| {
        [
            a.alias.as_deref().unwrap_or(""),
            &a.display_name,
            &a.username,
        ]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("?")
        .to_string()
    };
    Model {
        recents: recents
            .iter()
            .take(MAX_ITEMS)
            .map(|r| {
                let name = if r.name.is_empty() {
                    r.place_id.to_string()
                } else {
                    r.name.clone()
                };
                (format!("r:{}", r.place_id), text(&name))
            })
            .collect(),
        favorites: pinned
            .into_iter()
            .take(MAX_ITEMS)
            .enumerate()
            .map(|(i, a)| {
                let l = if hide {
                    format!("Account {}", i + 1)
                } else {
                    text(&label(a))
                };
                (format!("f:{}", a.id), l)
            })
            .collect(),
    }
}

fn flag(app: &AppHandle, ptr: &str) -> bool {
    let st = app.state::<AppState>();
    let s = st.settings.lock().unwrap_or_else(|e| e.into_inner());
    s.pointer(ptr) == Some(&Value::Bool(true))
}

/// One-shot "an update is being installed" flag: the relaunched app shows its window even
/// when this session began with `--autostart`.
const JUST_INSTALLED: &str = "/updates/justInstalled";

/// With it: the install ran by itself while RMI was opening, so the restart is that start.
const AT_START: &str = "/updates/installedAtStart";

fn just_installed(app: &AppHandle, on: bool, at_start: bool) {
    let st = app.state::<AppState>();
    let patch = json!({ "updates": { "justInstalled": on, "installedAtStart": on && at_start } });
    if let Err(e) = crate::commands::persist(&st, &patch) {
        log::warn!("update restart flag not saved: {e}");
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let st = app.state::<AppState>();
    let recents = crate::launch::recents::list(&crate::launch::launcher::library(&st), MAX_ITEMS);
    let hide =
        crate::settings::names_hidden(&st.settings.lock().unwrap_or_else(|e| e.into_inner()));
    let (locked, m) = {
        let g = st.vault.lock().unwrap_or_else(|e| e.into_inner());
        let accounts = g.as_ref().map(|v| v.payload().accounts.as_slice());
        (
            g.is_none(),
            model(g.is_none(), hide, &recents, accounts.unwrap_or(&[])),
        )
    };
    let sub = |title: &str, items: &[(String, String)]| {
        items
            .iter()
            .fold(
                SubmenuBuilder::new(app, title).enabled(!items.is_empty()),
                |b, (id, l)| b.text(id, l),
            )
            .build()
    };
    MenuBuilder::new(app)
        .text("open", "Open Roblox Multi Instance")
        .item(
            &MenuItemBuilder::with_id("lock", "Lock vault")
                .enabled(!locked)
                .build(app)?,
        )
        .separator()
        .item(&sub("Recent destinations", &m.recents)?)
        .item(&sub("Favorite accounts", &m.favorites)?)
        .separator()
        .text("quit", "Quit")
        .build()
}

fn refresh(app: &AppHandle) {
    // one rebuild at a time: each reads current state, so the last one to run is fresh
    static ONE: Mutex<()> = Mutex::new(());
    let _g = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(t) = app.tray_by_id(TRAY) else {
        return;
    };
    if let Err(e) = build_menu(app).and_then(|m| t.set_menu(Some(m))) {
        log::warn!("tray menu rebuild failed: {e}");
    }
}

/// Unhide, unminimize and focus the main window; shows the pending close-to-tray hint.
pub fn show_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    for r in [w.unminimize(), w.show(), w.set_focus()] {
        if let Err(e) = r {
            log::warn!("showing the window failed: {e}");
        }
    }
    // Marked seen only once actually shown (quitting while hidden keeps it due next time).
    if HINT
        .compare_exchange(HINT_DUE, HINT_SHOWN, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let _ = app.emit_to("main", "tray://hint", ());
        // Settings file write: off the calling (often main) thread.
        let h = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let st = h.state::<AppState>();
            match crate::commands::persist(&st, &json!({ "onboarding": { "trayToastSeen": true } }))
            {
                Ok(_) => crate::commands::state_changed(&h, &["settings"]),
                Err(e) => log::warn!("tray hint flag not saved: {e}"),
            }
        });
    }
}

/// Every exit path (tray Quit, last window closed, Cmd+Q, updater install): stop automation
/// and the watcher, lock (zeroize) the vault. Idempotent.
pub async fn shutdown(app: &AppHandle) {
    pause(app).await;
    crate::vault::lock(app);
}

/// Stop automation (capped) and the watcher. Returns whether automation was running.
async fn pause(app: &AppHandle) -> bool {
    let st = app.state::<AppState>();
    // a remote session gets its bye and lets go of held keys before the process ends
    st.remote.stop_and_wait().await;
    let s = st.automation.status().await;
    if tokio::time::timeout(QUIT_WAIT, st.automation.stop())
        .await
        .is_err()
    {
        log::warn!("automation did not stop in time; exiting anyway");
    }
    st.watcher.stop_wait(QUIT_WAIT).await;
    s.ws_running || s.api_running
}

/// Undo `pause` after a failed update install.
async fn resume(app: &AppHandle, automation_was_running: bool) {
    crate::watcher::reconcile(app);
    if automation_was_running {
        // Same short retry as at startup: the port may still be held for a moment.
        let st = app.state::<AppState>();
        let automation = &st.automation;
        let mut r = automation.start().await;
        for _ in 1..3 {
            if r.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            r = automation.start().await;
        }
        if let Err(e) = r {
            log::error!("automation restart failed: {e}");
        }
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    let play = |payload: Value| {
        if let Err(e) = app.emit_to("main", "tray://play", payload) {
            log::warn!("emit tray://play failed: {e}");
        }
    };
    match id {
        "open" => show_main(app),
        "lock" => {
            let a = app.clone();
            tauri::async_runtime::spawn(async move { crate::vault::lock(&a) });
        }
        // RunEvent::Exit runs `shutdown`
        "quit" => app.exit(0),
        _ => {
            if let Some(p) = id.strip_prefix("r:").and_then(|p| p.parse::<u64>().ok()) {
                play(json!({ "placeId": p }));
            } else if let Some(a) = id.strip_prefix("f:") {
                play(json!({ "accountId": a }));
            }
        }
    }
}

fn create(app: &AppHandle) -> tauri::Result<()> {
    // macOS: black template image, tinted by the menu bar. Elsewhere: white.
    let mac = cfg!(target_os = "macos");
    let icon: &[u8] = if mac {
        include_bytes!("../icons/tray-dark.png")
    } else {
        include_bytes!("../icons/tray.png")
    };
    TrayIconBuilder::with_id(TRAY)
        .icon(tauri::image::Image::from_bytes(icon)?)
        .icon_as_template(mac)
        .tooltip("RMI")
        .menu(&build_menu(app)?)
        .show_menu_on_left_click(mac)
        .on_menu_event(|app, e| on_menu(app, e.id().as_ref()))
        .on_tray_icon_event(|t, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                show_main(t.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Settings saved: apply `tray.enabled`; push the keys in `patch` that touch the OS.
/// Err: start-on-login could not be changed (the setting is put back to the real state).
pub fn on_settings(app: &AppHandle, patch: &Value) -> CmdResult<()> {
    sync_tray(app);
    if patch.pointer("/ui/showUsernames").is_some() {
        let h = app.clone();
        tauri::async_runtime::spawn_blocking(move || refresh(&h)); // favorites' labels
    }
    // E2E instance: never touch the real OS login item or the update server.
    if crate::paths::override_dir().is_some() {
        return Ok(());
    }
    if patch.pointer("/startup/launchOnLogin").is_some() {
        use tauri_plugin_autostart::ManagerExt;
        let al = app.autolaunch();
        let r = if flag(app, "/startup/launchOnLogin") {
            al.enable()
        } else {
            al.disable()
        };
        if let Err(e) = r {
            log::error!("start on login could not be changed: {e}");
            let actual = al.is_enabled().unwrap_or(false);
            let fix = json!({ "startup": { "launchOnLogin": actual } });
            if let Err(e2) = crate::commands::persist(&app.state::<AppState>(), &fix) {
                log::error!("launchOnLogin not reset: {e2}");
            }
            return Err(crate::commands::AUTOSTART_FAILED.into()); // details logged above
        }
    }
    if patch.pointer("/updates/auto") == Some(&Value::Bool(true)) {
        let h = app.clone();
        tauri::async_runtime::spawn(async move { quiet_check(&h).await });
    }
    Ok(())
}

fn sync_tray(app: &AppHandle) {
    match (flag(app, "/tray/enabled"), app.tray_by_id(TRAY)) {
        (true, None) => {
            if let Err(e) = create(app) {
                log::error!("tray icon failed: {e}");
            }
        }
        (false, Some(_)) => {
            app.remove_tray_by_id(TRAY);
        }
        _ => {}
    }
}

/// The OS owns "start on login" (Task Manager, Login Items): mirror it into the setting.
fn read_autostart(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let Ok(on) = app.autolaunch().is_enabled() else {
        return;
    };
    if on && app.state::<AppState>().portable {
        // rewrite the entry: one made before `--portable` was passed lacks it
        if let Err(e) = app.autolaunch().enable() {
            log::warn!("start on login entry not refreshed: {e}");
        }
    }
    if flag(app, "/startup/launchOnLogin") != on {
        let st = app.state::<AppState>();
        let patch = json!({ "startup": { "launchOnLogin": on } });
        match crate::commands::persist(&st, &patch) {
            Ok(_) => crate::commands::state_changed(app, &["settings"]),
            Err(e) => log::warn!("start-on-login state not saved: {e}"),
        }
    }
}

/// `startup.accounts`: comma-separated names, each an alias, display name or username (any
/// case). Friend-paused accounts are left out: an automated launch of one is refused, and
/// that would fail the whole batch. So are accounts whose client runs already (`live`
/// tracker ids): the restart after an update keeps `--autostart`, and launching those again
/// would close and reopen them.
fn login_ids(names: &str, accounts: &[Account], live: &[String]) -> Vec<String> {
    let want: Vec<String> = names
        .split(',')
        .map(|n| n.trim().to_lowercase())
        .filter(|n| !n.is_empty())
        .collect();
    accounts
        .iter()
        .filter(|a| !a.friend_paused && !live.contains(&a.tracker_id))
        .filter(|a| {
            [
                a.alias.as_deref().unwrap_or(""),
                &a.display_name,
                &a.username,
            ]
            .iter()
            .any(|s| want.contains(&s.to_lowercase()))
        })
        .map(|a| a.id.clone())
        .collect()
}

/// Login start: launch the `startup.accounts` accounts into their saved targets, once the
/// vault is open and roblox.com answers (after a crash the PC is back before its network).
fn login_launch(app: &AppHandle) {
    let names = {
        let st = app.state::<AppState>();
        let s = st.settings.lock().unwrap_or_else(|e| e.into_inner());
        let n = s.pointer("/startup/accounts").and_then(Value::as_str);
        n.unwrap_or_default().to_owned()
    };
    if names.trim().is_empty() {
        return;
    }
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        // An update found at start installs first: its restart does the launching. Not
        // counted below (a download, or Linux's admin prompt, can take a while).
        while !START_CHECKED.load(Ordering::SeqCst) || update_installing() {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        // ponytail: one batch, no retry of an account that fails; 5 s polls for 10 min
        for _ in 0..120 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if !crate::automation::relaunch::online().await {
                continue;
            }
            let h2 = h.clone();
            // Tracker ids with a client up; `None` while locked (wait for the unlock) or when
            // the process list could not be read (ask again: unknown is not "none run").
            let live = tauri::async_runtime::spawn_blocking(move || {
                let st = h2.state::<AppState>();
                let trackers: Vec<String> = {
                    let g = st.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let all = g.as_ref()?.payload().accounts.iter();
                    all.map(|a| a.tracker_id.clone()).collect()
                };
                let clients = st.platform.tracked_clients(&trackers)?;
                Some(clients.into_iter().map(|(_, t)| t).collect::<Vec<String>>())
            })
            .await
            .ok()
            .flatten();
            let Some(live) = live else { continue };
            // no await from here to the launch: a friend pause can't land in between
            let ids = {
                let st = h.state::<AppState>();
                let g = st.vault.lock().unwrap_or_else(|e| e.into_inner());
                g.as_ref()
                    .map(|v| login_ids(&names, &v.payload().accounts, &live))
            };
            let Some(ids) = ids else { continue }; // locked: wait for the unlock
            if ids.is_empty() {
                return log::info!("login start: nothing to launch (no match, or running already)");
            }
            return match crate::launch::launcher::start_batch(
                &h,
                ids,
                crate::launch::resolve::Bar::default(),
                false,
                false,
            ) {
                Ok(b) => log::info!("login start: launch {b}"),
                Err(e) => log::warn!("login start: launch failed: {e}"),
            };
        }
        log::warn!("login start: accounts not launched (vault locked or offline for 10 min)");
    });
}

/// Main window close: hide to the tray when there is one.
pub fn on_close(w: &Window, api: &tauri::CloseRequestApi) {
    let app = w.app_handle();
    sync_tray(app); // a tray that failed to build earlier gets one more try
    if app.tray_by_id(TRAY).is_none() {
        if flag(app, "/tray/enabled") {
            log::warn!("no tray icon, so closing the window quits");
        }
        return;
    }
    api.prevent_close();
    if let Err(e) = w.hide() {
        log::warn!("hide failed: {e}");
    }
    if !flag(app, "/onboarding/trayToastSeen") {
        let _ = HINT.compare_exchange(HINT_NONE, HINT_DUE, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// Setup: tray, autostart, menu refresh listeners, start minimized, quiet update checks.
pub fn init(app: &AppHandle) {
    sync_tray(app);
    let isolated = crate::paths::override_dir().is_some(); // E2E: no login item, no updater
    if !isolated {
        read_autostart(app);
    }
    for ev in ["accounts://changed", "recents://changed", "vault://locked"] {
        let h = app.clone();
        // off the emitter's thread: emitters may hold locks build_menu takes
        app.listen_any(ev, move |_| {
            let h = h.clone();
            tauri::async_runtime::spawn_blocking(move || refresh(&h));
        });
    }
    // The page asks for the window when a tray play has nothing to launch, so its toast is seen.
    let h = app.clone();
    app.listen_any("tray://show", move |_| show_main(&h));
    // The window is created hidden (no flash at login); show it unless starting in the tray.
    // The relaunch after an update keeps argv, so a login session would come back hidden.
    let updated = flag(app, JUST_INSTALLED);
    // That update installed itself while RMI was starting: this restart is still the start.
    let still_starting = flag(app, AT_START);
    if updated {
        just_installed(app, false, false);
    }
    let autostart = std::env::args().any(|a| a == AUTOSTART_ARG);
    // Not after an update the user installed mid-session: accounts they closed stay closed.
    let login = autostart && (!updated || still_starting);
    let minimized = !updated && autostart && flag(app, "/startup/minimized");
    if !minimized || app.tray_by_id(TRAY).is_none() {
        show_main(app);
        if minimized {
            if let Some(Err(e)) = app.get_webview_window("main").map(|w| w.minimize()) {
                log::warn!("start minimized failed: {e}");
            }
        }
    }
    if isolated {
        return;
    }
    if login {
        login_launch(app);
    }
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut at_start = true;
        loop {
            if flag(&h, "/updates/auto") {
                quiet_check(&h).await;
                // Only when RMI opens, like a game launcher: a session in use is never
                // restarted under the user. A later find waits for the next open (or Settings).
                let found = PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_some();
                // `!updated`: an install that left the version where it was must not loop.
                if at_start && found && !updated && flag(&h, "/updates/install") {
                    log::info!("update found at start: installing");
                    // returns only when it failed: carry on with this version
                    if let Err(e) = install(&h, true).await {
                        log::info!("update not installed at start: {e}");
                    }
                }
            }
            at_start = false;
            START_CHECKED.store(true, Ordering::SeqCst);
            tokio::time::sleep(UPDATE_EVERY).await;
        }
    });
}

/// The check at start is over and no install took the app down: a login start may launch.
static START_CHECKED: AtomicBool = AtomicBool::new(false);

/// Background check: failures are logged (info), never shown.
async fn quiet_check(app: &AppHandle) {
    if let Err(e) = check(app).await {
        log::info!("update check failed: {e}");
    }
}

/// The updater runs the installer, which updates the installed copy, never a portable exe.
const PORTABLE: &str =
    "Portable mode can't update itself. Download the new version and replace the exe.";

fn portable(app: &AppHandle) -> CmdResult<()> {
    if app.state::<AppState>().portable {
        return Err(PORTABLE.into());
    }
    Ok(())
}

async fn check(app: &AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;
    portable(app)?;
    // The plugin sets no timeout: a stalled connection would hang the check, or hold the
    // install slot (and block Automation start) for the whole session. Idle limits, not a
    // total one, so a slow but moving download still finishes. The found Update keeps them.
    let u = app
        .updater_builder()
        .configure_client(|b| {
            b.connect_timeout(std::time::Duration::from_secs(15))
                .read_timeout(std::time::Duration::from_secs(30))
        })
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let v = u.as_ref().map(|u| u.version.clone());
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = u;
    crate::commands::state_changed(app, &["update"]); // found or gone
    Ok(v)
}

/// Version of an available update. `force` checks now; otherwise the last check's result.
#[tauri::command]
pub async fn update_check(app: AppHandle, force: bool) -> CmdResult<Option<String>> {
    if !force {
        let g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        return Ok(g.as_ref().map(|u| u.version.clone())); // portable: never set
    }
    portable(&app)?;
    check(&app).await.map_err(|e| {
        log::info!("update check failed: {e}");
        "Couldn't check for updates.".into()
    })
}

pub const UPDATE_BUSY: &str = "UPDATE_BUSY";
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// An update is downloading or installing (background work must not be restarted: Windows
/// exits inside `install`).
pub fn update_installing() -> bool {
    INSTALLING.load(Ordering::SeqCst)
}

/// The in-flight install slot; dropping it (failure) frees it for a retry.
struct Installing;
impl Installing {
    fn take() -> Option<Self> {
        (!INSTALLING.swap(true, Ordering::SeqCst)).then_some(Installing)
    }
}
impl Drop for Installing {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

/// Download, install and restart into the update found by the last check.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> CmdResult<()> {
    install(&app, false).await
}

/// `update_install`; with `at_start`, RMI running it by itself as it opens
/// (`updates.install`). Returns only on failure.
async fn install(app: &AppHandle, at_start: bool) -> CmdResult<()> {
    let app = app.clone();
    // One install at a time, whatever the UI thinks (it may have remounted mid-download).
    portable(&app)?;
    let _busy = Installing::take().ok_or(UPDATE_BUSY)?;
    // cloned: a failed attempt leaves it pending for a retry
    let u = PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or("No update to install.")?;
    let fail = |e: tauri_plugin_updater::Error| {
        let m = format!("The update could not be installed: {e}");
        log::warn!("{m}");
        m
    };
    let bytes = u.download(|_, _| {}, || {}).await.map_err(fail)?;
    // Windows exits inside `install` (no RunEvent::Exit): stop background work first, but keep
    // the vault open so a failure still shows in Settings. Elsewhere restart() -> Exit -> shutdown.
    let was_running = cfg!(windows) && pause(&app).await;
    // Nobody asked for this restart: a launch begun since RMI opened is not cut off.
    // ponytail: a batch starting in the instant after this check still is.
    if at_start && crate::launch::launcher::busy() {
        if cfg!(windows) {
            resume(&app, was_running).await;
        }
        return Err("a launch is running; the update waits for the next start".into());
    }
    // Settings write and installer run block: off the async workers.
    let a = app.clone();
    let done = tauri::async_runtime::spawn_blocking(move || {
        // Set before install: Windows relaunches from inside it.
        just_installed(&a, true, at_start);
        let r = u.install(bytes);
        if r.is_err() {
            just_installed(&a, false, false);
        }
        r
    })
    .await
    .map_err(|e| format!("internal error: {e}"))
    .and_then(|r| r.map_err(fail));
    if let Err(m) = done {
        if cfg!(windows) {
            resume(&app, was_running).await;
        }
        return Err(m);
    }
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent(id: u64, name: &str) -> Recent {
        Recent {
            place_id: id,
            name: name.into(),
            thumbnail: None,
            last_played_at: String::new(),
        }
    }

    fn acct(id: &str, pinned: bool, order: i64, alias: Option<&str>, user: &str) -> Account {
        Account {
            id: id.into(),
            pinned,
            order,
            alias: alias.map(Into::into),
            username: user.into(),
            ..Default::default()
        }
    }

    #[test]
    fn login_ids_match_any_name_and_skip_friend_paused() {
        let mut paused = acct("3", false, 0, None, "Carl");
        paused.friend_paused = true;
        let mut shown = acct("4", false, 0, None, "user4");
        shown.display_name = "Dee".into();
        let all = [
            acct("1", false, 0, Some("Main"), "alice"),
            acct("2", false, 0, None, "Bob"),
            paused,
            shown,
        ];
        assert_eq!(
            login_ids(" main, BOB ,carl,dee,,nobody", &all, &[]),
            ["1", "2", "4"]
        );
        assert!(login_ids("", &all, &[]).is_empty());
        // a client that runs already is not launched again (the restart after an update)
        let mut up = acct("5", false, 0, None, "Eve");
        up.tracker_id = "t5".into();
        let all = [acct("1", false, 0, Some("Main"), "alice"), up];
        assert_eq!(login_ids("main, eve", &all, &["t5".into()]), ["1"]);
    }

    #[test]
    fn one_install_at_a_time() {
        let a = Installing::take().expect("free");
        assert!(Installing::take().is_none(), "second install refused");
        drop(a); // the first one failed
        assert!(Installing::take().is_some(), "retry allowed");
    }

    #[test]
    fn locked_is_empty() {
        let m = model(
            true,
            false,
            &[recent(1, "A")],
            &[acct("a", true, 0, None, "u")],
        );
        assert!(m.recents.is_empty() && m.favorites.is_empty());
    }

    #[test]
    fn builds_items() {
        let rs: Vec<Recent> = (1..=12).map(|i| recent(i, "")).collect();
        let accts = [
            acct("b", true, 2, None, "bob"),
            acct("x", false, 0, None, "nope"),
            acct("a", true, 1, Some("Al"), "al1"),
        ];
        let m = model(false, false, &rs, &accts);
        assert_eq!(m.recents.len(), MAX_ITEMS);
        assert_eq!(m.recents[0], ("r:1".into(), "1".into()));
        assert_eq!(
            m.favorites,
            vec![("f:a".into(), "Al".into()), ("f:b".into(), "bob".into())]
        );
        // privacy mode: no names in the native menu
        let m = model(false, true, &rs, &accts);
        assert_eq!(
            m.favorites,
            vec![
                ("f:a".into(), "Account 1".into()),
                ("f:b".into(), "Account 2".into())
            ]
        );
    }

    #[test]
    fn escapes_ampersand_on_windows() {
        let m = model(false, false, &[recent(5, "Cops & Robbers")], &[]);
        let want = if cfg!(windows) {
            "Cops && Robbers"
        } else {
            "Cops & Robbers"
        };
        assert_eq!(m.recents[0].1, want);
    }
}
