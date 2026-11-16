pub mod accounts;
mod antiafk;
pub mod automation;
mod commands;
pub mod discover;
pub mod friend;
pub mod grid;
mod homelab;
mod homelab_get;
pub mod import;
pub mod launch;
pub mod logging;
pub mod paths;
pub mod platform;
pub mod remote;
pub mod roblox;
pub mod settings;
pub mod shell;
pub mod tools;
pub mod vault;
pub mod watcher;

use std::sync::{
    atomic::{AtomicI64, AtomicU64},
    Arc, Mutex,
};
use tauri::{Emitter, Manager};
use vault::{oswrap::OsWrap, store::Vault};

pub struct AppState {
    pub paths: paths::AppPaths,
    pub portable: bool,
    pub settings: Arc<Mutex<serde_json::Value>>,
    /// Settings file existed but was unreadable at startup: never overwrite it.
    pub settings_read_only: bool,
    /// settings.json was invalid at startup and replaced with defaults (why, where it went).
    pub settings_recovered: Option<String>,
    pub platform: Box<dyn platform::Platform>,
    pub automation: Arc<automation::Automation>,
    /// Friend remote server (loopback; Tailscale Funnel exposes it).
    pub friend: Arc<friend::Friend>,
    /// Remote access host (Tailscale address only, off by default).
    pub remote: Arc<remote::Remote>,
    /// Held while `/multi/enabled` (Windows `ROBLOX_singletonMutex`).
    pub multi_guard: Mutex<Option<platform::Guard>>,
    /// Held while `/multi/cookieLock` (exclusive `RobloxCookies.dat` handle).
    pub cookie_guard: Mutex<Option<platform::Guard>>,
    /// `None` = locked. Dropping the `Vault` zeroizes DK and payload.
    pub vault: Mutex<Option<Vault>>,
    /// Unix secs of the last `vault_touch`; drives auto-lock.
    pub last_activity: AtomicI64,
    /// Bumped by every `vault::lock`; inserts after slow work check it hasn't moved.
    pub lock_gen: AtomicU64,
    pub watcher: watcher::Watcher,
    pub accounts: accounts::Runtime,
    pub roblox: Arc<roblox::RobloxClient>,
    os_wrap: Mutex<Option<Arc<dyn OsWrap>>>,
}

impl AppState {
    /// OS wrap, created on first use (keyring access can prompt on mac/linux).
    pub fn os(&self) -> vault::Result<Arc<dyn OsWrap>> {
        let mut g = self
            .os_wrap
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.is_none() {
            *g = Some(vault::oswrap::system(&self.paths.root)?);
        }
        Ok(g.clone().unwrap())
    }
}

#[cfg(test)]
impl AppState {
    /// Unlocked-able state on `root` with a fake platform and every Roblox host at `url`.
    pub(crate) fn for_test(
        root: &std::path::Path,
        platform: Box<dyn platform::Platform>,
        url: &str,
        settings: serde_json::Value,
    ) -> Self {
        let settings = Arc::new(Mutex::new(settings));
        Self {
            paths: paths::AppPaths::at(root.to_path_buf()),
            portable: false,
            automation: Arc::new(automation::Automation::new(
                settings.clone(),
                Arc::new(automation::NullBackend),
            )),
            friend: Arc::new(friend::Friend::new(
                settings.clone(),
                Arc::new(automation::NullBackend),
            )),
            remote: Arc::new(remote::Remote::new(
                settings.clone(),
                root,
                Arc::new(remote::desk::Null),
            )),
            settings,
            settings_read_only: false,
            settings_recovered: None,
            platform,
            multi_guard: Mutex::new(None),
            cookie_guard: Mutex::new(None),
            vault: Mutex::new(None),
            last_activity: AtomicI64::new(0),
            lock_gen: AtomicU64::new(0),
            watcher: Default::default(),
            accounts: accounts::Runtime::default(),
            roblox: Arc::new(roblox::RobloxClient::new(roblox::BaseUrls::all(url))),
            os_wrap: Mutex::new(None),
        }
    }
}

/// One redacted, timestamped crash.log line (panic text can embed secrets).
fn crash_line(msg: &str) -> String {
    logging::redact(&format!(
        "{} panic: {msg}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
    ))
}

/// Append panics to `<data>/logs/crash.log`.
fn install_panic_hook(logs: std::path::PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let line = crash_line(&info.to_string());
        eprint!("{line}");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(logs.join("crash.log"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Mutex holder process (spawned by `hold_multi`): no window, tray, plugins or logger.
    #[cfg(windows)]
    if let Some(main_exe) = platform::windows::holder_arg(std::env::args()) {
        std::process::exit(platform::windows::run_holder(&main_exe));
    }
    // WebKitGTK's DMABUF renderer draws garbage on some GPUs (seen on a Radeon HD 8490).
    // Set before any thread or the webview starts.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let portable = std::env::args().any(|a| a == "--portable");
    // E2E (debug only): own data dir, and no single-instance hand-off to a running RMI or
    // shared window-state file, so a test run never touches the user's instance.
    let isolated = paths::override_dir();
    let paths = match &isolated {
        Some(d) => paths::AppPaths::at(d.clone()),
        None => paths::AppPaths::resolve(portable),
    };
    install_panic_hook(paths.logs.clone());
    let loaded = settings::load(&paths.settings());
    // The logger only exists once the builder runs, so replay startup problems in `.setup()`.
    let mut startup: Vec<String> = paths.problems.clone();
    startup.extend(loaded.recovered.clone());
    let start_automation = ["/automation/enabled", "/ws/startOnLaunch"]
        .iter()
        .all(|p| loaded.value.pointer(p) == Some(&serde_json::Value::Bool(true)));
    let settings = Arc::new(Mutex::new(loaded.value));
    // The backend needs the AppHandle; it is handed over in `.setup()`.
    let backend = Arc::new(automation::backend::AppBackend::<tauri::AppHandle>::default());
    let automation = Arc::new(automation::Automation::new(
        settings.clone(),
        backend.clone(),
    ));
    let friend = Arc::new(friend::Friend::new(settings.clone(), backend.clone()));
    let remote = Arc::new(remote::Remote::new(
        settings.clone(),
        &paths.root,
        remote::desk::system(),
    ));

    let mut builder = tauri::Builder::default();
    if isolated.is_none() {
        // Second launch: surface this one. Its args are dropped on purpose: RMI registers no
        // URI scheme or file type, and `--portable` / `--autostart` only mean something at start.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            shell::show_main(app)
        }));
    }
    builder = builder.plugin(logging::plugin(paths.logs.clone()));
    if isolated.is_none() {
        builder = builder.plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::DECORATIONS
                        // hidden-in-tray must not persist into the next start
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                // placed fresh each session (top centre of the primary monitor)
                .with_denylist(&[remote::INDICATOR])
                .build(),
        );
    }
    let result = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            // a portable login start must stay portable (else it opens the AppData data dir)
            Some(if portable {
                vec![shell::AUTOSTART_ARG, "--portable"]
            } else {
                vec![shell::AUTOSTART_ARG]
            }),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState {
            paths,
            portable,
            settings,
            settings_read_only: loaded.read_only,
            settings_recovered: loaded.recovered,
            platform: platform::current(),
            automation: automation.clone(),
            friend: friend.clone(),
            remote: remote.clone(),
            multi_guard: Mutex::new(None),
            cookie_guard: Mutex::new(None),
            vault: Mutex::new(None),
            last_activity: AtomicI64::new(vault::mono_now()),
            lock_gen: AtomicU64::new(0),
            watcher: Default::default(),
            accounts: accounts::Runtime::default(),
            roblox: Arc::new(roblox::RobloxClient::new(roblox::BaseUrls::prod())),
            os_wrap: Mutex::new(None),
        })
        .on_window_event(|w, e| match e {
            tauri::WindowEvent::Destroyed => discover::cancel_streams(|l| l == w.label()),
            // Focusing any app window (sign-in and site windows too, which can't call
            // vault_touch) counts as activity for the idle auto-lock.
            tauri::WindowEvent::Focused(true) => tauri::Manager::state::<AppState>(w)
                .last_activity
                .store(vault::mono_now(), std::sync::atomic::Ordering::Relaxed),
            tauri::WindowEvent::CloseRequested { api, .. } if w.label() == "main" => {
                shell::on_close(w, api)
            }
            // closing the "being controlled" bar is the same as its Stop button; the window
            // stays (hidden once the session ends) and is shown again for the next one
            tauri::WindowEvent::CloseRequested { api, .. } if w.label() == remote::INDICATOR => {
                api.prevent_close();
                tauri::Manager::state::<AppState>(w).remote.stop_session()
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings_get,
            commands::settings_set,
            commands::settings_defaults,
            commands::open_folder,
            commands::defender_exclude,
            commands::defender_exclude_cmd,
            commands::ax_trusted,
            commands::platform_capabilities,
            commands::grid_arrange,
            commands::monitors_list,
            commands::grid_suggest,
            commands::grid_identify,
            commands::grid_windows,
            commands::app_info,
            commands::automation_status,
            commands::automation_start,
            commands::automation_stop,
            commands::automation_token,
            commands::automation_token_regenerate,
            accounts::cmds::automation_token_copy,
            commands::automation_notice_take,
            commands::friend_status,
            commands::friend_link,
            commands::friend_password,
            accounts::cmds::friend_password_copy,
            commands::friend_new_password,
            friend::tailscale::friend_ts_status,
            friend::tailscale::friend_ts_enable,
            friend::tailscale::friend_ts_disable,
            remote::remote_status,
            remote::remote_pair_code,
            remote::remote_unpair,
            remote::remote_stop,
            remote::viewer::remote_peers,
            remote::viewer::remote_pair,
            remote::viewer::remote_connect,
            remote::viewer::remote_forget,
            homelab_get::homelab_latest,
            homelab_get::homelab_download,
            commands::automation_snapshot,
            commands::automation_send,
            commands::automation_ui_event,
            commands::automation_save_console,
            commands::automation_relaunch_set,
            commands::roblox_status,
            commands::multi_set,
            commands::cookie_lock_set,
            launch::launcher::launch_start,
            launch::launcher::launch_cancel,
            launch::launcher::launch_status,
            launch::launcher::destination_resolve,
            launch::launcher::recents_list,
            launch::launcher::recents_clear,
            discover::servers_stream,
            discover::servers_stop,
            discover::servers_find_player,
            discover::servers_regions,
            discover::servers_reserve_join,
            discover::games_discover,
            discover::games_search,
            discover::game_details,
            discover::universe_of,
            discover::universe_places,
            discover::favorites_list,
            discover::favorites_add,
            discover::favorites_update,
            discover::favorites_remove,
            discover::favorites_reorder,
            commands::client_settings_set,
            commands::vault_status,
            commands::vault_create,
            commands::vault_unlock,
            commands::vault_lock,
            commands::vault_change_password,
            commands::vault_backups,
            commands::vault_restore,
            commands::vault_export_encrypted,
            commands::vault_export_plain,
            commands::vault_import_file,
            commands::vault_touch,
            commands::watcher_status,
            commands::watcher_kill,
            accounts::cmds::accounts_list,
            accounts::cmds::accounts_add_cookies,
            accounts::cmds::accounts_add_pending,
            accounts::cmds::accounts_update,
            accounts::cmds::accounts_remove,
            accounts::cmds::accounts_undo_remove,
            accounts::cmds::accounts_reorder,
            accounts::cmds::groups_reorder,
            accounts::cmds::group_rename,
            accounts::cmds::group_target_set,
            accounts::cmds::account_copy,
            accounts::cmds::account_reveal,
            accounts::cmds::reauth,
            accounts::cmds::signin_start,
            accounts::cmds::signin_kill_stray,
            accounts::cmds::accounts_validate,
            accounts::cmds::avatar_full,
            import::cmds::import_pick_folder,
            import::cmds::import_detect,
            import::cmds::import_preview,
            import::cmds::import_commit,
            tools::tool_pin_state,
            tools::tool_pin_unlock,
            tools::tool_display_name,
            tools::tool_password,
            tools::tool_email_get,
            tools::tool_email_set,
            tools::tool_privacy_get,
            tools::tool_privacy_set,
            tools::tool_signout_others,
            tools::tool_friend_request,
            tools::tool_block,
            tools::tool_blocked_list,
            tools::tool_unblock_all,
            tools::tool_robux,
            tools::tool_info,
            tools::tool_export_summary,
            tools::outfits_list,
            tools::outfit_wear,
            tools::site::account_open_site,
            shell::update_check,
            shell::update_install,
        ])
        .setup(move |app| {
            // First, before anything can start Roblox (relaunch engine, automation, watcher):
            // take the mutex / cookie lock if enabled.
            {
                let state = app.state::<AppState>();
                let s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
                commands::sync_guards(&state, &s);
            }
            vault::spawn_autolock(app.handle().clone());
            watcher::init(app.handle());
            watcher::reconcile(app.handle());
            #[cfg(windows)]
            watcher::spawn_retitle(app.handle().clone());
            let h = app.handle().clone();
            app.state::<AppState>()
                .roblox
                .set_ratelimit_hook(Arc::new(move |host, ms| {
                    use tauri::Emitter;
                    let _ = h.emit(
                        "roblox://ratelimited",
                        serde_json::json!({"host": host, "retryInMs": ms}),
                    );
                }));
            // headshot cache only (works for portable too); login dirs stay outside it
            let thumbs = app.state::<AppState>().paths.cache.join("thumbs");
            if let Err(e) = std::fs::create_dir_all(&thumbs)
                .map_err(|e| e.to_string())
                .and_then(|_| {
                    app.asset_protocol_scope()
                        .allow_directory(&thumbs, true)
                        .map_err(|e| e.to_string())
                })
            {
                log::error!("headshot folder not available to the UI: {e}");
            }
            accounts::sync::spawn(app.handle().clone());
            let h = app.handle().clone();
            std::thread::spawn(move || accounts::signin::clean_temp_dirs(&h));
            log::info!(
                "Roblox Multi Instance {} starting",
                env!("CARGO_PKG_VERSION")
            );
            for m in &startup {
                log::error!("{m}");
            }
            backend.set_host(app.handle().clone());
            automation::relaunch::spawn(app.handle().clone());
            antiafk::spawn(app.handle().clone());
            homelab::spawn(app.handle().clone());
            let hub = automation.ctx.hub.clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    for account in hub.next_changes().await {
                        let payload = serde_json::json!({ "account": account });
                        if let Err(e) = handle.emit("automation://changed", payload) {
                            log::warn!("emit automation://changed failed: {e}");
                        }
                    }
                    // App-level sync (survives a lock / page unmount).
                    commands::state_changed(&handle, &["snap"]);
                }
            });
            let changed = automation.status_changed.clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    changed.notified().await;
                    commands::state_changed(&handle, &["autoStatus"]);
                }
            });
            if let Some(mut secrets) = automation.ctx.hub.take_secret_events() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    while let Some(account) = secrets.recv().await {
                        let payload = serde_json::json!({ "account": account });
                        if let Err(e) = handle.emit("automation://secret", payload) {
                            log::warn!("emit automation://secret failed: {e}");
                        }
                    }
                });
            }
            if automation.ctx.flag("/automation/enabled") {
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // Saved token first (keyring may prompt), then the servers.
                    let h2 = h.clone();
                    let warn = tauri::async_runtime::spawn_blocking(move || {
                        commands::ensure_token(h2.state::<AppState>().inner())
                    })
                    .await
                    .ok()
                    .flatten();
                    if start_automation {
                        if let Err(e) = automation.start().await {
                            log::error!("automation start failed: {e}");
                        }
                        commands::state_changed(&h, &["autoStatus"]);
                    }
                    if let Some(key) = warn {
                        // kept until the UI takes it (automation_notice_take), however late it loads
                        commands::notice(&h, key);
                    }
                });
            }
            if friend.enabled() {
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = commands::friend_sync(&h).await {
                        log::error!("friend server start failed: {e}");
                    }
                });
            }
            let h = app.handle().clone();
            remote.set_hook(Arc::new(move || remote::on_change(&h)));
            if remote.enabled() {
                let r = remote.clone();
                tauri::async_runtime::spawn(async move { r.sync().await });
            }
            shell::init(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .map(|app| {
            app.run(|app, e| match e {
                // every exit path (tray Quit, last window closed, Cmd+Q) ends here
                tauri::RunEvent::Exit => tauri::async_runtime::block_on(shell::shutdown(app)),
                // macOS: Dock icon click with the window hidden in the menu bar
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => shell::show_main(app),
                _ => {}
            })
        });
    if let Err(e) = result {
        log::error!("error while running Roblox Multi Instance: {e}");
        eprintln!("error while running Roblox Multi Instance: {e}");
        std::process::exit(1);
    }
}

/// Unlocked OS-mode vault in `root`, Roblox calls to `roblox`.
#[cfg(test)]
pub(crate) fn test_state(root: &std::path::Path, roblox: roblox::RobloxClient) -> AppState {
    let v = Vault::create(
        &root.join("vault.rmiv"),
        vault::store::CreateMode::Os,
        Arc::new(vault::oswrap::FakeOsWrap),
    )
    .expect("test vault");
    let settings = Arc::new(Mutex::new(serde_json::json!({})));
    AppState {
        paths: paths::AppPaths::at(root.to_path_buf()),
        portable: false,
        settings: settings.clone(),
        friend: Arc::new(friend::Friend::new(
            settings.clone(),
            Arc::new(automation::NullBackend),
        )),
        remote: Arc::new(remote::Remote::new(
            settings.clone(),
            root,
            Arc::new(remote::desk::Null),
        )),
        automation: Arc::new(automation::Automation::new(
            settings,
            Arc::new(automation::NullBackend),
        )),
        settings_read_only: false,
        settings_recovered: None,
        // never the real platform: see platform::Null
        platform: Box::new(platform::Null),
        multi_guard: Mutex::new(None),
        cookie_guard: Mutex::new(None),
        vault: Mutex::new(Some(v)),
        last_activity: AtomicI64::new(0),
        lock_gen: AtomicU64::new(0),
        accounts: accounts::Runtime::default(),
        roblox: Arc::new(roblox),
        os_wrap: Mutex::new(None),
        watcher: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn crash_line_redacts() {
        let l = super::crash_line("boom Cookie: .ROBLOSECURITY=_|WARNING:-abc123 at x.rs:1");
        assert!(l.contains("panic: boom") && l.ends_with('\n'));
        assert!(!l.contains("abc123"), "{l}");
    }
}
