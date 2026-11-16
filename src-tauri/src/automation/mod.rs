//! Opt-in automation (spec §9): WS control server + local HTTP API, both token-gated.

pub mod backend;
pub mod clients;
pub mod http;
pub mod protocol;
pub mod relaunch;
pub mod ws;

use crate::roblox::api::quicklogin::{Created as QuickCode, Status as QuickStatus};
use crate::roblox::Secret;
use crate::vault::oswrap::OsWrap;

/// Friend add still allowed (adding on, remote on, same pending add).
pub type AddGate = Arc<dyn Fn() -> bool + Send + Sync>;
use protocol::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use tokio::sync::oneshot;

pub type ApiResult<T> = Result<T, ApiError>;

/// Non-secret account fields the API may expose (camelCase on the wire).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAccount {
    /// `null` when not known yet (pending sign-in).
    pub user_id: Option<u64>,
    pub username: String,
    pub display_name: String,
    pub alias: Option<String>,
    pub note: Option<String>,
    pub group: String,
    pub session: String,
    pub extra: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchReq {
    pub place_id: Option<u64>,
    pub job_id: Option<String>,
    pub private_link: Option<String>,
    pub follow: Option<String>,
}

/// Missing key = `None` (unchanged); JSON `null` = `Some(None)` (clear).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    #[serde(default, deserialize_with = "nullable")]
    pub alias: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub note: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub group: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub extra: Option<Option<BTreeMap<String, String>>>,
}

fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// What the API drives. `name` is a username, matched case-insensitively by the implementor.
#[async_trait::async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn list_accounts(&self) -> ApiResult<Vec<ApiAccount>>;
    async fn get_account(&self, name: &str) -> ApiResult<ApiAccount>;
    async fn presence(&self, name: &str) -> ApiResult<Value>;
    async fn launch(&self, name: &str, req: LaunchReq) -> ApiResult<String>;
    async fn launch_status(&self, id: &str) -> ApiResult<Value>;
    async fn patch_account(&self, name: &str, patch: Patch) -> ApiResult<ApiAccount>;
    async fn import_cookie(&self, cookie: &str) -> ApiResult<ApiAccount>;
    async fn block(&self, name: &str, username: &str) -> ApiResult<()>;
    async fn unblock(&self, name: &str, username: &str) -> ApiResult<()>;
    async fn blocked(&self, name: &str) -> ApiResult<Value>;
    /// Secret family.
    async fn cookie(&self, name: &str) -> ApiResult<String>;
    /// Friend remote (`crate::friend`): `id` is an account id; only `friend_allowed` ones.
    async fn friend_list(&self) -> ApiResult<Vec<crate::friend::FriendAccount>> {
        Err(crate::friend::not_found())
    }
    async fn friend_disconnect(&self, _id: &str) -> ApiResult<()> {
        Err(crate::friend::not_found())
    }
    /// Batch id.
    async fn friend_reconnect(&self, _id: &str, _link: Option<String>) -> ApiResult<String> {
        Err(crate::friend::not_found())
    }
    /// PNG bytes of the cached 48 px headshot.
    async fn friend_avatar(&self, _id: &str) -> ApiResult<Vec<u8>> {
        Err(crate::friend::not_found())
    }
    /// Friend add (Quick Log In): accounts the friend added (`friend::locked` when locked).
    async fn friend_added(&self) -> ApiResult<usize> {
        Err(crate::friend::not_found())
    }
    async fn friend_add_create(&self) -> ApiResult<QuickCode> {
        Err(crate::friend::not_found())
    }
    async fn friend_add_poll(&self, _code: &str, _key: &Secret) -> ApiResult<QuickStatus> {
        Err(crate::friend::not_found())
    }
    async fn friend_add_cancel(&self, _code: &str) -> ApiResult<()> {
        Err(crate::friend::not_found())
    }
    /// Sign in with the approved code and add (or refresh) the shared account: its display name.
    /// `ok` (adding still allowed) is checked before signing in and again under the vault lock
    /// right before the write; false is `add_off`.
    async fn friend_add_finish(
        &self,
        _code: &str,
        _key: &Secret,
        _ok: AddGate,
    ) -> ApiResult<String> {
        Err(crate::friend::not_found())
    }
}

/// Stand-in until the vault/accounts/launcher branches are merged.
pub struct NullBackend;

fn no_account(name: &str) -> ApiError {
    ApiError::not_found(format!("No account named {name}"))
}

#[async_trait::async_trait]
impl Backend for NullBackend {
    async fn list_accounts(&self) -> ApiResult<Vec<ApiAccount>> {
        Ok(vec![])
    }
    async fn get_account(&self, name: &str) -> ApiResult<ApiAccount> {
        Err(no_account(name))
    }
    async fn presence(&self, name: &str) -> ApiResult<Value> {
        Err(no_account(name))
    }
    async fn launch(&self, _: &str, _: LaunchReq) -> ApiResult<String> {
        Err(ApiError::new(
            501,
            "not_implemented",
            "Launching arrives with M4",
        ))
    }
    async fn launch_status(&self, id: &str) -> ApiResult<Value> {
        Err(ApiError::not_found(format!("No launch {id}")))
    }
    async fn patch_account(&self, name: &str, _: Patch) -> ApiResult<ApiAccount> {
        Err(no_account(name))
    }
    async fn import_cookie(&self, _: &str) -> ApiResult<ApiAccount> {
        Err(ApiError::new(
            501,
            "not_implemented",
            "Importing arrives with the vault",
        ))
    }
    async fn block(&self, name: &str, _: &str) -> ApiResult<()> {
        Err(no_account(name))
    }
    async fn unblock(&self, name: &str, _: &str) -> ApiResult<()> {
        Err(no_account(name))
    }
    async fn blocked(&self, name: &str) -> ApiResult<Value> {
        Err(no_account(name))
    }
    async fn cookie(&self, name: &str) -> ApiResult<String> {
        Err(no_account(name))
    }
}

/// Shared by both servers. Token and settings are read live, so a regenerate or toggle
/// takes effect on the next request.
#[derive(Clone)]
pub struct Ctx {
    pub hub: Arc<ws::Hub>,
    pub token: Arc<RwLock<String>>,
    pub settings: Arc<Mutex<Value>>,
    pub backend: Arc<dyn Backend>,
    /// Servers up: a WS upgrade that registers after a stop's `disconnect_all` is refused.
    pub accepting: Arc<std::sync::atomic::AtomicBool>,
}

impl Ctx {
    pub fn token_ok(&self, given: &str) -> bool {
        use subtle::ConstantTimeEq;
        let t = self.token.read().unwrap_or_else(PoisonError::into_inner);
        bool::from(t.as_bytes().ct_eq(given.as_bytes()))
    }
    pub fn flag(&self, ptr: &str) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pointer(ptr)
            == Some(&Value::Bool(true))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub ws_running: bool,
    pub api_running: bool,
    pub ws_port: u16,
    pub api_port: u16,
    pub lan: bool,
}

type Net = (u16, u16, bool);

struct Running {
    cfg: Net,
    ws: SocketAddr,
    api: SocketAddr,
    stops: Vec<oneshot::Sender<()>>,
    /// `[ws, api]` serve tasks; finished = that server is down.
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Running {
    fn up(&self) -> bool {
        self.tasks.iter().all(|t| !t.is_finished())
    }
}

/// Wait this long for a stopped server to release its port, then abort it.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Allowed `Host` header names when LAN is off (DNS-rebinding guard).
const LOOPBACK_HOSTS: &[&str] = &["127.0.0.1", "localhost", "[::1]"];

fn host_name(host: &str) -> &str {
    match host.strip_prefix('[') {
        Some(rest) => rest.find(']').map_or(host, |i| &host[..i + 2]),
        None => host.split(':').next().unwrap_or(host),
    }
}

/// Both servers: refuse browser origins (in-game/script clients send none or `null`),
/// and, unless LAN is on, any `Host` that is not loopback.
pub(crate) async fn guard(
    axum::extract::State(ctx): axum::extract::State<Ctx>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::header::{HOST, ORIGIN};
    use axum::response::IntoResponse;
    let (origin_bad, host_ok) = {
        let h = |n| req.headers().get(n).and_then(|v| v.to_str().ok());
        let host_ok = h(HOST).is_some_and(|v| {
            LOOPBACK_HOSTS
                .iter()
                .any(|l| host_name(v).eq_ignore_ascii_case(l))
        });
        (h(ORIGIN).is_some_and(|o| o != "null"), host_ok)
    };
    if origin_bad {
        return ApiError::new(403, "forbidden_origin", "Browser origins are not allowed")
            .into_response();
    }
    if !host_ok && !ctx.flag("/net/allowLan") {
        return ApiError::new(403, "forbidden_host", "Host must be loopback").into_response();
    }
    next.run(req).await
}

pub struct Automation {
    pub ctx: Ctx,
    pub relaunch: Mutex<relaunch::Engine>,
    /// Account id -> (RMI launch batches running, grace end after the last one). Their own
    /// replace-previous close and fresh client are not crashes to auto-relaunch.
    own: Mutex<HashMap<String, (u32, Option<tokio::time::Instant>)>>,
    running: tokio::sync::Mutex<Option<Running>>,
    /// The saved token was loaded, saved or given up on this session (once only).
    token_loaded: Mutex<bool>,
    /// Server status may have changed (bind, stop, a server task ended); the app turns it
    /// into `state_changed(["autoStatus"])`.
    pub status_changed: Arc<tokio::sync::Notify>,
}

/// `start` while Automation is off in Settings.
pub const AUTOMATION_OFF: &str = "Turn on Automation in Settings first";

/// The OS wrap, or why it is unavailable (keyring down, prompt cancelled).
pub type OsResult = Result<Arc<dyn OsWrap>, String>;

/// OS-wrapped token in the data dir (spec §5.3: generated once at first enable).
const TOKEN_FILE: &str = "automation-token.bin";

impl Automation {
    pub fn new(settings: Arc<Mutex<Value>>, backend: Arc<dyn Backend>) -> Self {
        // In memory until `ensure_token` loads the saved one or saves this one.
        let token = protocol::new_token();
        crate::logging::register_secret(&token);
        Automation {
            ctx: Ctx {
                hub: Arc::new(ws::Hub::default()),
                token: Arc::new(RwLock::new(token)),
                settings,
                backend,
                accepting: Default::default(),
            },
            relaunch: Mutex::default(),
            own: Mutex::default(),
            running: tokio::sync::Mutex::new(None),
            token_loaded: Mutex::default(),
            status_changed: Arc::default(),
        }
    }

    /// A launch batch with these accounts started. Also forgets ended graces.
    pub fn own_begin(&self, ids: &[String]) {
        let mut g = self.own.lock().unwrap_or_else(PoisonError::into_inner);
        let now = tokio::time::Instant::now();
        g.retain(|_, (n, t)| *n > 0 || t.is_some_and(|t| now < t));
        for i in ids {
            g.entry(i.clone()).or_default().0 += 1;
        }
    }

    /// That batch ended (or panicked): ours until `until`, never shortening a later grace.
    pub fn own_end(&self, ids: &[String], until: tokio::time::Instant) {
        let mut g = self.own.lock().unwrap_or_else(PoisonError::into_inner);
        for i in ids {
            let e = g.entry(i.clone()).or_default();
            e.0 = e.0.saturating_sub(1);
            e.1 = e.1.max(Some(until));
        }
    }

    /// An RMI launch of this account is queued/running or ended within its grace.
    /// ponytail: a real crash inside that grace is not relaunched (its edge is consumed
    /// while suppressed); the next disconnect/exit after the grace is.
    pub fn own_busy(&self, id: &str) -> bool {
        let g = self.own.lock().unwrap_or_else(PoisonError::into_inner);
        g.get(id)
            .is_some_and(|(n, t)| *n > 0 || t.is_some_and(|t| tokio::time::Instant::now() < t))
    }

    /// An RMI launch of this account is queued or running (its grace not counted).
    pub fn own_running(&self, id: &str) -> bool {
        let g = self.own.lock().unwrap_or_else(PoisonError::into_inner);
        g.get(id).is_some_and(|(n, _)| *n > 0)
    }

    pub fn token(&self) -> String {
        self.ctx
            .token
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// New token, saved OS-wrapped (it wins over any saved one); existing WS clients are
    /// dropped so the old token stops working everywhere. Takes the load lock first, so a
    /// startup load in progress cannot overwrite it afterwards. Blocking (file + keyring).
    /// If the new one cannot be saved, the old file is deleted (it may have leaked and
    /// must not come back next start): `Some(i18n key)` = memory-only this session.
    pub fn regenerate_token(
        &self,
        root: &Path,
        os: impl FnOnce() -> OsResult,
    ) -> (String, Option<&'static str>) {
        let mut done = self.token_lock();
        let t = protocol::new_token();
        self.set_token(t.clone());
        self.disconnect_all();
        *done = true;
        let path = root.join(TOKEN_FILE);
        let Err(e) = os().and_then(|os| self.save_token(&path, &*os)) else {
            return (t, None);
        };
        log::warn!("new automation token not saved; memory-only this session: {e}");
        match std::fs::remove_file(&path) {
            Ok(()) => log::info!("old automation token file deleted"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::error!("old automation token file not deleted: {e}"),
        }
        (t, Some("auto.tokenMemoryOnly"))
    }

    fn set_token(&self, t: String) {
        crate::logging::register_secret(&t);
        *self
            .ctx
            .token
            .write()
            .unwrap_or_else(PoisonError::into_inner) = t;
    }

    fn token_lock(&self) -> std::sync::MutexGuard<'_, bool> {
        self.token_loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Load the saved token (OS-wrapped: DPAPI/keyring, never plaintext), or save the
    /// current one if none is saved. Runs once per session: after any outcome, including
    /// a failure (then the token is memory-only for this session), later calls do nothing,
    /// so the token never swaps while servers run. A file that reads fine but does not
    /// unwrap/decode is replaced: `Some(i18n key)`. A read or keyring error (for example
    /// a cancelled prompt) leaves the file alone. Blocking.
    pub fn ensure_token(&self, root: &Path, os: impl FnOnce() -> OsResult) -> Option<&'static str> {
        let mut done = self.token_lock();
        if *done {
            return None;
        }
        *done = true;
        let path = root.join(TOKEN_FILE);
        let blob = match std::fs::read(&path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("automation token file unreadable ({e}); memory-only this session");
                return None;
            }
        };
        let os = match os() {
            Ok(o) => o,
            Err(e) => {
                log::warn!("automation token kept in memory only this session: {e}");
                return None;
            }
        };
        let mut warn = None;
        if let Some(b) = blob {
            let t = os
                .unwrap(&b)
                .ok()
                .and_then(|b| String::from_utf8(b.to_vec()).ok())
                .filter(|t| t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit()));
            if let Some(t) = t {
                self.set_token(t);
                return None;
            }
            log::warn!("saved automation token does not decode; a new one replaces it");
            warn = Some("auto.tokenReset");
        }
        if let Err(e) = self.save_token(&path, &*os) {
            log::warn!("automation token not saved; kept in memory only: {e}");
        }
        warn
    }

    fn save_token(&self, path: &Path, os: &dyn OsWrap) -> Result<(), String> {
        let blob = os
            .wrap(self.token().as_bytes())
            .map_err(|e| e.to_string())?;
        crate::vault::store::write_atomic(path, &blob, crate::vault::store::Backup::Skip)
            .map_err(|e| e.to_string())
    }

    /// Drop every WS client without auto-relaunch reading it as a fall (our own close is
    /// final). The engine lock spans both, so the relaunch loop sees all-up or reset.
    fn disconnect_all(&self) {
        let mut e = self.relaunch.lock().unwrap_or_else(PoisonError::into_inner);
        e.reset();
        self.ctx.hub.disconnect_all();
    }

    /// Both servers up. `None` = busy starting/stopping right now: keep the last answer.
    pub fn serving(&self) -> Option<bool> {
        self.running
            .try_lock()
            .ok()
            .map(|r| r.as_ref().is_some_and(Running::up))
    }

    fn net(&self) -> Net {
        let s = self
            .ctx
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let port = |p: &str, d: u16| {
            s.pointer(p)
                .and_then(Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
                .unwrap_or(d)
        };
        let lan = s.pointer("/net/allowLan") == Some(&Value::Bool(true));
        (port("/ws/port", 7810), port("/api/port", 7811), lan)
    }

    fn status_of(&self, run: &Option<Running>) -> Status {
        match run {
            Some(r) => Status {
                ws_running: !r.tasks[0].is_finished(),
                api_running: !r.tasks[1].is_finished(),
                ws_port: r.ws.port(),
                api_port: r.api.port(),
                lan: r.cfg.2,
            },
            None => {
                let (ws_port, api_port, lan) = self.net();
                Status {
                    ws_running: false,
                    api_running: false,
                    ws_port,
                    api_port,
                    lan,
                }
            }
        }
    }

    pub async fn status(&self) -> Status {
        self.status_of(&*self.running.lock().await)
    }

    /// Bind both servers (loopback unless `net.allowLan`). No-op if already running.
    /// Checks `automation.enabled` under the run lock: a disable saved before this call
    /// wins (its `reconcile` either ran already or waits and then stops).
    pub async fn start(&self) -> Result<Status, String> {
        let mut run = self.running.lock().await;
        if !self.ctx.flag("/automation/enabled") {
            return Err(AUTOMATION_OFF.into());
        }
        if !run.as_ref().is_some_and(Running::up) {
            self.shutdown(&mut run).await;
            *run = Some(self.bind().await?);
        }
        Ok(self.status_of(&run))
    }

    pub async fn stop(&self) -> Status {
        let mut run = self.running.lock().await;
        self.shutdown(&mut run).await;
        self.status_of(&run)
    }

    /// After a settings change: stop when automation got disabled, rebind when
    /// ports or the LAN toggle changed or one server died. Does nothing while stopped.
    pub async fn reconcile(&self) -> Result<(), String> {
        let mut run = self.running.lock().await;
        let Some(r) = run.as_ref() else {
            return Ok(());
        };
        let enabled = self.ctx.flag("/automation/enabled");
        if !enabled || r.cfg != self.net() || !r.up() {
            self.shutdown(&mut run).await;
            if enabled {
                *run = Some(self.bind().await?);
            }
        }
        Ok(())
    }

    /// Signal both servers, drop WS clients, and wait (bounded) until the ports are free.
    async fn shutdown(&self, run: &mut Option<Running>) {
        let Some(r) = run.take() else { return };
        for s in r.stops {
            let _ = s.send(());
        }
        // Before the drop: `register` checks it under the hub lock, so no upgrade sneaks in.
        self.ctx
            .accepting
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.disconnect_all();
        for t in r.tasks {
            let abort = t.abort_handle();
            if tokio::time::timeout(STOP_GRACE, t).await.is_err() {
                log::warn!("automation server did not stop in time; aborting it");
                abort.abort();
            }
        }
        log::info!("automation servers stopped");
        self.status_changed.notify_one();
    }

    async fn bind(&self) -> Result<Running, String> {
        let cfg = self.net();
        let ip = if cfg.2 {
            Ipv4Addr::UNSPECIFIED
        } else {
            Ipv4Addr::LOCALHOST
        };
        let open = |port: u16, what: &'static str| async move {
            tokio::net::TcpListener::bind((ip, port))
                .await
                .map_err(|e| format!("Could not open the {what} on port {port}: {e}"))
        };
        let wl = open(cfg.0, "control server").await?;
        let al = open(cfg.1, "local API").await?;
        let (ws, api) = (
            wl.local_addr().map_err(|e| e.to_string())?,
            al.local_addr().map_err(|e| e.to_string())?,
        );
        let (mut stops, mut tasks) = (vec![], vec![]);
        self.ctx
            .accepting
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for (l, router) in [
            (wl, ws::router(self.ctx.clone())),
            (al, http::router(self.ctx.clone())),
        ] {
            let (tx, rx) = oneshot::channel::<()>();
            stops.push(tx);
            let changed = self.status_changed.clone();
            tasks.push(tokio::spawn(async move {
                let stop = async {
                    let _ = rx.await;
                };
                // peer address: the Secrets family is loopback-only (http::cookie)
                let svc = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
                if let Err(e) = axum::serve(l, svc).with_graceful_shutdown(stop).await {
                    log::error!("automation server failed: {e}");
                }
                changed.notify_one(); // a crash leaves it degraded: the status page must say
            }));
        }
        if cfg.2 {
            log::warn!("automation servers listening on all interfaces (LAN enabled)");
        }
        log::info!("automation servers on {ws} (control) and {api} (API)");
        self.status_changed.notify_one();
        Ok(Running {
            cfg,
            ws,
            api,
            stops,
            tasks,
        })
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use serde_json::json;

    /// Two accounts; launch always succeeds.
    pub struct Fake;

    fn acc(name: &str) -> ApiAccount {
        ApiAccount {
            user_id: Some(1),
            username: name.into(),
            display_name: name.into(),
            group: "Unsorted".into(),
            session: "ok".into(),
            ..Default::default()
        }
    }

    fn find(name: &str) -> ApiResult<ApiAccount> {
        ["Alice", "Bob"]
            .iter()
            .find(|n| n.eq_ignore_ascii_case(name))
            .map(|n| acc(n))
            .ok_or_else(|| no_account(name))
    }

    #[async_trait::async_trait]
    impl Backend for Fake {
        async fn list_accounts(&self) -> ApiResult<Vec<ApiAccount>> {
            Ok(vec![acc("Alice"), acc("Bob")])
        }
        async fn get_account(&self, name: &str) -> ApiResult<ApiAccount> {
            find(name)
        }
        async fn presence(&self, name: &str) -> ApiResult<Value> {
            find(name).map(|_| json!({ "state": "online" }))
        }
        async fn launch(&self, name: &str, _: LaunchReq) -> ApiResult<String> {
            find(name).map(|_| "L1".into())
        }
        async fn launch_status(&self, id: &str) -> ApiResult<Value> {
            Ok(json!({ "id": id, "state": "running" }))
        }
        async fn patch_account(&self, name: &str, p: Patch) -> ApiResult<ApiAccount> {
            find(name).map(|mut a| {
                a.alias = p.alias.flatten();
                a
            })
        }
        async fn import_cookie(&self, _: &str) -> ApiResult<ApiAccount> {
            Ok(acc("Carol"))
        }
        async fn block(&self, name: &str, _: &str) -> ApiResult<()> {
            find(name).map(|_| ())
        }
        async fn unblock(&self, name: &str, _: &str) -> ApiResult<()> {
            find(name).map(|_| ())
        }
        async fn blocked(&self, name: &str) -> ApiResult<Value> {
            find(name).map(|_| json!([]))
        }
        async fn cookie(&self, name: &str) -> ApiResult<String> {
            find(name).map(|_| "_|WARNING:-fake".into())
        }
    }

    /// Defaults with ports 0 (ephemeral), automation on, and `extra` merged over `/api`.
    pub fn automation(api: Value) -> Automation {
        let mut s = crate::settings::defaults();
        s["automation"]["enabled"] = json!(true);
        s["ws"]["port"] = json!(0);
        s["api"]["port"] = json!(0);
        for (k, v) in api.as_object().unwrap() {
            s["api"][k] = v.clone();
        }
        Automation::new(Arc::new(Mutex::new(s)), Arc::new(Fake))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lan_off_binds_loopback_and_stop_works() {
        let a = testkit::automation(serde_json::json!({}));
        let s = a.start().await.unwrap();
        assert!(s.ws_running && s.api_running && !s.lan);
        {
            let g = a.running.lock().await;
            let r = g.as_ref().unwrap();
            assert_eq!(r.ws.ip(), Ipv4Addr::LOCALHOST);
            assert_eq!(r.api.ip(), Ipv4Addr::LOCALHOST);
        }
        assert!(!a.stop().await.ws_running);
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn port_change_rebinds_both_and_quick_restart_works() {
        let a = testkit::automation(serde_json::json!({}));
        a.start().await.unwrap();
        let p = free_port();
        a.ctx.settings.lock().unwrap()["ws"]["port"] = serde_json::json!(p);
        a.reconcile().await.unwrap();
        let s = a.status().await;
        assert!(s.ws_running && s.api_running, "{s:?}");
        assert_eq!(s.ws_port, p);
        // Same fixed ports: stop then start must rebind immediately.
        a.ctx.settings.lock().unwrap()["api"]["port"] = serde_json::json!(free_port());
        for _ in 0..3 {
            a.stop().await;
            let s = a.start().await.unwrap();
            assert!(s.ws_running && s.api_running && s.ws_port == p, "{s:?}");
        }
    }

    #[test]
    fn patch_null_clears_missing_keeps() {
        let p: Patch = serde_json::from_str(r#"{"alias":null,"note":"n"}"#).unwrap();
        assert_eq!(p.alias, Some(None));
        assert_eq!(p.note, Some(Some("n".into())));
        assert_eq!(p.group, None);
    }

    #[test]
    fn host_name_strips_port() {
        assert_eq!(host_name("127.0.0.1:7811"), "127.0.0.1");
        assert_eq!(host_name("[::1]:7811"), "[::1]");
        assert_eq!(host_name("localhost"), "localhost");
        assert_eq!(host_name("evil.com:80"), "evil.com");
    }

    #[tokio::test]
    async fn reconcile_stops_when_disabled() {
        let a = testkit::automation(serde_json::json!({}));
        a.start().await.unwrap();
        a.ctx.settings.lock().unwrap()["automation"]["enabled"] = Value::Bool(false);
        a.reconcile().await.unwrap();
        assert!(!a.status().await.ws_running);
        // A start racing the disable (startup, Start button, update resume) never binds.
        assert_eq!(a.start().await.unwrap_err(), AUTOMATION_OFF);
        assert!(!a.status().await.ws_running);
    }

    #[test]
    fn regenerate_changes_token() {
        let a = testkit::automation(serde_json::json!({}));
        let old = a.token();
        let d = tempfile::tempdir().unwrap();
        let (new, _) = a.regenerate_token(d.path(), fake);
        assert_ne!(old, new);
        assert!(!a.ctx.token_ok(&old) && a.ctx.token_ok(&new));
        assert!(!crate::logging::redact(&new).contains(&new));
    }

    #[tokio::test(start_paused = true)]
    async fn own_counts_batches_and_keeps_the_longest_grace() {
        use tokio::time::{advance, Duration, Instant};
        let a = testkit::automation(serde_json::json!({}));
        let ids = ["A".to_string()];
        a.own_begin(&ids);
        a.own_begin(&ids);
        a.own_end(&ids, Instant::now() + Duration::from_secs(30));
        advance(Duration::from_secs(31)).await;
        assert!(a.own_busy("A"), "second batch still running");
        assert!(a.own_running("A"));
        a.own_end(&ids, Instant::now() + Duration::from_secs(30));
        a.own_end(&ids, Instant::now() + Duration::from_secs(5));
        advance(Duration::from_secs(10)).await;
        assert!(a.own_busy("A"), "shorter grace does not cut the longer one");
        assert!(!a.own_running("A"), "grace is not a running launch");
        advance(Duration::from_secs(21)).await;
        assert!(!a.own_busy("A") && !a.own_busy("B"));
    }

    fn fake() -> OsResult {
        Ok(Arc::new(crate::vault::oswrap::FakeOsWrap))
    }

    #[test]
    fn token_is_saved_wrapped_and_reloaded_once() {
        let d = tempfile::tempdir().unwrap();
        let a = testkit::automation(serde_json::json!({}));
        assert_eq!(a.ensure_token(d.path(), fake), None);
        let t = a.token();
        let raw = std::fs::read(d.path().join(TOKEN_FILE)).unwrap();
        assert!(
            !raw.windows(t.len()).any(|w| w == t.as_bytes()),
            "never plaintext"
        );
        // Next run: the saved token comes back.
        let b = testkit::automation(serde_json::json!({}));
        assert_ne!(b.token(), t);
        assert_eq!(b.ensure_token(d.path(), fake), None);
        assert_eq!(b.token(), t);
        // Regenerate saves the new one.
        let (n, warn) = b.regenerate_token(d.path(), fake);
        assert_eq!(warn, None);
        let c = testkit::automation(serde_json::json!({}));
        c.ensure_token(d.path(), fake);
        assert_eq!(c.token(), n);
        // A blob that does not decode: new token, saved, warned.
        std::fs::write(d.path().join(TOKEN_FILE), b"junk").unwrap();
        let e = testkit::automation(serde_json::json!({}));
        let fresh = e.token();
        assert_eq!(e.ensure_token(d.path(), fake), Some("auto.tokenReset"));
        assert_eq!(e.token(), fresh);
        let f = testkit::automation(serde_json::json!({}));
        f.ensure_token(d.path(), fake);
        assert_eq!(f.token(), fresh);
    }

    #[test]
    fn keyring_error_keeps_the_file_and_never_swaps_later() {
        let d = tempfile::tempdir().unwrap();
        let a = testkit::automation(serde_json::json!({}));
        a.ensure_token(d.path(), fake);
        let saved = std::fs::read(d.path().join(TOKEN_FILE)).unwrap();
        // Cancelled prompt: memory-only, file untouched.
        let b = testkit::automation(serde_json::json!({}));
        let mem = b.token();
        assert_eq!(b.ensure_token(d.path(), || Err("cancelled".into())), None);
        assert_eq!(b.token(), mem);
        assert_eq!(std::fs::read(d.path().join(TOKEN_FILE)).unwrap(), saved);
        // Once per session: a later working keyring does not swap the token.
        b.ensure_token(d.path(), fake);
        assert_eq!(b.token(), mem);
    }

    #[test]
    fn regenerate_wins_over_a_later_load() {
        let d = tempfile::tempdir().unwrap();
        let a = testkit::automation(serde_json::json!({}));
        a.ensure_token(d.path(), fake);
        let b = testkit::automation(serde_json::json!({}));
        let (n, _) = b.regenerate_token(d.path(), fake);
        b.ensure_token(d.path(), fake);
        assert_eq!(b.token(), n);
    }

    #[test]
    fn failed_regenerate_save_deletes_the_old_file() {
        let d = tempfile::tempdir().unwrap();
        let a = testkit::automation(serde_json::json!({}));
        a.ensure_token(d.path(), fake);
        let (_, warn) = a.regenerate_token(d.path(), || Err("keyring down".into()));
        assert_eq!(warn, Some("auto.tokenMemoryOnly"));
        assert!(
            !d.path().join(TOKEN_FILE).exists(),
            "old token must not come back"
        );
    }

    #[test]
    fn saved_token_that_is_not_hex_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let not_hex = "z".repeat(64);
        let blob = crate::vault::oswrap::FakeOsWrap
            .wrap(not_hex.as_bytes())
            .unwrap();
        std::fs::write(d.path().join(TOKEN_FILE), blob).unwrap();
        let a = testkit::automation(serde_json::json!({}));
        let fresh = a.token();
        assert_eq!(a.ensure_token(d.path(), fake), Some("auto.tokenReset"));
        assert_eq!(a.token(), fresh);
    }
}
