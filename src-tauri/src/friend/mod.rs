//! Friend remote: a loopback-only server (Tailscale Funnel exposes it) where one friend
//! logs in (owner-set username, generated password) for the friend token, then can
//! Disconnect/Reconnect the accounts the owner allowed. No sign-up.
//! Host guard (DNS rebinding): only the publicUrl host or loopback. No Origin guard: the web
//! app is same-origin and auth is a bearer token in a header (no cookies, so no CSRF).

pub mod tailscale;

use crate::automation::protocol::{new_token, ApiError};
use crate::automation::{ApiResult, Backend, OsResult};
use crate::launch::resolve::Bar;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

/// One allowed account as the friend sees it (camelCase on the wire).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendAccount {
    pub id: String,
    pub display_name: String,
    pub username: String,
    /// "inGame" | "online" | "starting" | "offline" | "paused"
    pub status: &'static str,
    pub game: Option<String>,
    pub uptime_sec: Option<u64>,
    /// Roblox reports the account in a game but this RMI has no client for it: it runs on
    /// another computer. The status stays "offline" (nothing here to disconnect).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub elsewhere: bool,
    /// The computer the account's client is on, by name (never an address): this one, or a
    /// paired computer that reported it. Only for the owner's own login.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computer: Option<String>,
}

/// Unknown and not-allowed ids get this same answer: the friend cannot probe which exist.
pub fn not_found() -> ApiError {
    ApiError::not_found("No such account")
}

/// Adding is off (or was turned off, or the remote/password changed under the add).
pub fn add_off() -> ApiError {
    ApiError::new(
        403,
        "add_off",
        "The owner has not turned on adding accounts.",
    )
}

pub fn locked() -> ApiError {
    ApiError::new(503, "locked", "The owner's vault is locked")
}

/// Fixed 500 for the friend; the real error goes to the log only.
pub fn internal(e: impl std::fmt::Display) -> ApiError {
    log::error!("friend: internal error: {e}");
    ApiError::new(
        500,
        "internal",
        "Something went wrong on the owner's computer.",
    )
}

/// Roblox failed or refused; the real error goes to the log only. A `Message` is already
/// plain words for the user.
pub fn roblox(e: crate::roblox::RobloxError) -> ApiError {
    log::warn!("friend: Roblox call failed: {e}");
    let msg = match e {
        crate::roblox::RobloxError::Message(m) => m,
        _ => "Roblox did not answer. Try again in a minute.".into(),
    };
    ApiError::new(502, "roblox", msg)
}

/// The automation backend's 423 vault lock is the friend API's 503. A 500 is already the
/// generic one; anything else (raw vault or launcher text, maybe with paths) becomes it too.
pub fn lock_err(e: ApiError) -> ApiError {
    match e.status {
        423 => locked(),
        500 => e,
        _ => internal(e.message),
    }
}

/// `lock_err` for a reconnect: the launcher's 400 (an ordinary refusal) stays a 400 with
/// fixed words, so the friend learns the game did not start; its text goes to the log only.
pub fn launch_err(e: ApiError) -> ApiError {
    if e.status != 400 {
        return lock_err(e);
    }
    log::warn!("friend: reconnect refused: {}", e.message);
    ApiError::new(400, "bad_request", "Could not start the game.")
}

/// Reconnect destination: the link given, else the account's saved target (the launcher
/// applies it: `false`), else the place it was in at Disconnect.
pub fn reconnect_bar(
    link: Option<String>,
    has_target: bool,
    last: Option<u64>,
) -> ApiResult<(Bar, bool)> {
    let to = |destination: String| {
        let bar = Bar {
            destination,
            ..Default::default()
        };
        Ok((bar, true))
    };
    match link.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()) {
        // Game links and place ids only: a name search would probe the owner's recents, and
        // an @username would follow another user.
        Some(l) => match crate::launch::dest::parse_destination(&l) {
            None
            | Some(
                crate::launch::dest::Destination::Query(_)
                | crate::launch::dest::Destination::Follow(_),
            ) => Err(ApiError::new(
                400,
                "bad_link",
                "Paste a Roblox game link or place ID.",
            )),
            Some(_) => to(l),
        },
        None if has_target => Ok((Bar::default(), false)),
        None => match last {
            Some(p) => to(p.to_string()),
            None => Err(ApiError::new(
                409,
                "no_game",
                "No game to rejoin: paste a game link",
            )),
        },
    }
}

/// The login page address: `publicUrl` without a trailing slash, no secret in it.
pub fn link_of(public_url: &str) -> Result<String, String> {
    let u = public_url.trim().trim_end_matches('/');
    if u.is_empty() {
        return Err("Set the friend remote's public URL first".into());
    }
    if !u.to_ascii_lowercase().starts_with("https://") {
        return Err("Public address must start with https://".into());
    }
    Ok(u.to_owned())
}

/// OS-wrapped token and password in the data dir.
const TOKEN_FILE: &str = "friend-token.bin";
const PASSWORD_FILE: &str = "friend-password.bin";
/// The owner's own login (`owner_username`), saved the same way.
const OWNER_TOKEN_FILE: &str = "friend-owner-token.bin";
const OWNER_PASSWORD_FILE: &str = "friend-owner-password.bin";
/// No look-alikes (I l O o 0 1): 56 symbols.
const PW_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
const PW_LEN: usize = 20;
/// Brute-force guard: at most this many failed logins per client IP per rolling window.
const MAX_FAILS: usize = 10;
const FAIL_WINDOW: Duration = Duration::from_secs(60);
/// Most client IPs tracked; beyond it the stalest is dropped.
const MAX_FAIL_KEYS: usize = 1024;
/// One Disconnect/Reconnect per account per this long.
const COOLDOWN: Duration = Duration::from_secs(20);
/// Distinct rejected Host values logged (then silent).
const MAX_HOST_LOGS: usize = 64;
/// Wait this long for a stopped server to release its port, then abort it.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// Most accounts the friend may add.
pub const MAX_ADDED: usize = 10;
/// At most this many add starts per window.
const MAX_STARTS: usize = 5;
const START_WINDOW: Duration = Duration::from_secs(600);
/// A Quick Log In code lives at most this long (or less, if Roblox says so).
const ADD_TTL: Duration = Duration::from_secs(300);
/// Roblox is asked about a pending code at most this often, however often the page polls.
const ADD_POLL: Duration = Duration::from_secs(2);

#[derive(Clone)]
enum AddState {
    /// The account name once the code was typed in Roblox (not yet confirmed).
    Waiting(Option<String>),
    /// Confirmed; signing in and adding.
    Linked(Option<String>),
    Done(String),
    Failed(String),
    Cancelled,
}

/// The one Quick Log In in progress (or its outcome). No Debug: holds the private key.
struct Pending {
    code: String,
    /// Never leaves this PC.
    key: Zeroizing<String>,
    /// Waiting: the code's expiry. Linked: the sign-in deadline (`LINK_TTL`).
    until: Instant,
    checked: Option<Instant>,
    state: AddState,
}

fn add_json(s: &AddState) -> Value {
    match s {
        AddState::Waiting(n) => json!({ "status": "waiting", "accountName": n }),
        AddState::Linked(n) => json!({ "status": "linked", "accountName": n }),
        AddState::Done(n) => json!({ "status": "done", "accountName": n }),
        AddState::Failed(m) => json!({ "status": "failed", "message": m }),
        AddState::Cancelled => json!({ "status": "cancelled" }),
    }
}

/// Code lifetime: Roblox's `expirationTime` or `ADD_TTL`, whichever is sooner.
fn add_ttl(exp: Option<&str>) -> Duration {
    exp.and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
        .map(|t| {
            (t.to_utc() - chrono::Utc::now())
                .to_std()
                .unwrap_or_default()
        })
        .map_or(ADD_TTL, |d| d.min(ADD_TTL))
}

/// 20 chars from `PW_CHARS`, same OS CSPRNG as the token (rejection sampling: no bias).
fn random_password() -> String {
    let lim = (256 / PW_CHARS.len() * PW_CHARS.len()) as u8;
    let mut out = String::with_capacity(PW_LEN);
    let mut b = [0u8; 32];
    while out.len() < PW_LEN {
        getrandom::fill(&mut b).expect("OS random source unavailable");
        let need = PW_LEN - out.len();
        out.extend(
            b.iter()
                .filter(|&&x| x < lim)
                .map(|&x| PW_CHARS[x as usize % PW_CHARS.len()] as char)
                .take(need),
        );
    }
    out
}

fn valid_token(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit())
}

fn valid_password(p: &str) -> bool {
    p.len() == PW_LEN && p.bytes().all(|c| PW_CHARS.contains(&c))
}

fn read_secret(cell: &RwLock<String>) -> String {
    cell.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn write_secret(cell: &RwLock<String>, v: String) {
    crate::logging::register_secret(&v);
    *cell.write().unwrap_or_else(PoisonError::into_inner) = v;
}

/// (file, value, validator) for each saved secret.
type Slot<'a> = (&'static str, &'a RwLock<String>, fn(&str) -> bool);

#[derive(Clone)]
struct Ctx {
    token: Arc<RwLock<String>>,
    password: Arc<RwLock<String>>,
    /// The owner login's token and password (see `owner_username`).
    owner_token: Arc<RwLock<String>>,
    owner_password: Arc<RwLock<String>>,
    settings: Arc<Mutex<Value>>,
    /// Times of recent failed logins per client IP (the brute-force window).
    fails: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
    /// Last Disconnect/Reconnect per account id.
    actions: Arc<Mutex<HashMap<String, Instant>>>,
    /// Bound port (loopback Host check).
    port: Arc<AtomicU16>,
    /// Rejected Host values already logged.
    bad_hosts: Arc<Mutex<HashSet<String>>>,
    /// X-Forwarded-For presence logged this session.
    xff_logged: Arc<AtomicBool>,
    backend: Arc<dyn Backend>,
    /// The pending friend add (Quick Log In), if any.
    add: Arc<Mutex<Option<Pending>>>,
    /// Bumped whenever the pending add is dropped: a start racing it does not store its code.
    add_gen: Arc<AtomicU64>,
    /// Times of recent add starts (the rate window).
    add_starts: Arc<Mutex<VecDeque<Instant>>>,
    /// One start at a time (the Roblox create call is awaited).
    add_gate: Arc<tokio::sync::Mutex<()>>,
}

impl Ctx {
    /// `/friend/canAdd`, read live.
    fn can_add(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pointer("/friend/canAdd")
            == Some(&Value::Bool(true))
    }

    fn add_slot(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.add.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Drop the pending add (its key is zeroized); its code is never signed in with.
    fn drop_add(&self) {
        let mut g = self.add_slot();
        self.add_gen.fetch_add(1, Ordering::SeqCst);
        *g = None;
    }

    /// Adding is on, else the pending add is dropped and 403.
    fn allow_add(&self) -> ApiResult<()> {
        if self.can_add() {
            return Ok(());
        }
        self.drop_add();
        Err(add_off())
    }

    /// `/friend/enabled`, read live.
    fn remote_on(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pointer("/friend/enabled")
            == Some(&Value::Bool(true))
    }

    /// The login a bearer token belongs to: `Some(true)` the owner's (every account),
    /// `Some(false)` the friend's (shared accounts only). The owner's token works only
    /// while an owner username is set.
    fn token_role(&self, given: &str) -> Option<bool> {
        use subtle::ConstantTimeEq;
        let eq = |t: &RwLock<String>| {
            let t = t.read().unwrap_or_else(PoisonError::into_inner);
            bool::from(t.as_bytes().ct_eq(given.as_bytes()))
        };
        let owner_on =
            owner_username(&self.settings.lock().unwrap_or_else(PoisonError::into_inner)).is_some();
        match (eq(&self.token), eq(&self.owner_token) && owner_on) {
            (true, _) => Some(false),
            (_, true) => Some(true),
            _ => None,
        }
    }

    /// The login a username and password are for (as `token_role`). Username
    /// case-insensitive; both compared in constant time (per length), never short-circuited.
    fn login_role(&self, user: &str, pw: &str) -> Option<bool> {
        use subtle::ConstantTimeEq;
        let (friend, owner) = {
            let s = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
            (username(&s), owner_username(&s))
        };
        let user = user.trim().to_lowercase();
        let hit = |name: &str, secret: &RwLock<String>| {
            let p = secret.read().unwrap_or_else(PoisonError::into_inner);
            let u_ok = name.to_lowercase().as_bytes().ct_eq(user.as_bytes());
            bool::from(u_ok & p.as_bytes().ct_eq(pw.as_bytes()))
        };
        let as_friend = hit(&friend, &self.password);
        let as_owner = owner.is_some_and(|n| hit(&n, &self.owner_password));
        match (as_friend, as_owner) {
            (true, _) => Some(false),
            (_, true) => Some(true),
            _ => None,
        }
    }
}

/// `/friend/username` (settings normalize it; "friend" if missing).
fn username(s: &Value) -> String {
    s.pointer("/friend/username")
        .and_then(Value::as_str)
        .unwrap_or("friend")
        .to_owned()
}

/// `/friend/ownerUsername`: the owner's own login name, if set (trimmed, 1 to 32 chars).
/// That login sees and controls every account, not only the shared ones.
/// ponytail: no settings row yet: the name is set in settings.json, and its password
/// (`friend-owner-password.bin`) changes only when that file is deleted. Add the row and a
/// "New password" for it when more than the app's author uses this.
fn owner_username(s: &Value) -> Option<String> {
    s.pointer("/friend/ownerUsername")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| (1..=32).contains(&n.chars().count()))
        .map(str::to_owned)
}

/// The login behind a request: the owner (true) sees every account, the friend only the
/// shared ones.
#[derive(Clone, Copy)]
struct Owner(bool);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendStatus {
    pub running: bool,
    pub port: u16,
    pub public_url: String,
}

struct Running {
    /// Configured port (0 in tests); `addr` has the real one.
    port: u16,
    addr: SocketAddr,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

pub struct Friend {
    ctx: Ctx,
    running: tokio::sync::Mutex<Option<Running>>,
    /// The saved token and password were loaded or saved this session (once only).
    token_loaded: Mutex<bool>,
}

impl Friend {
    pub fn new(settings: Arc<Mutex<Value>>, backend: Arc<dyn Backend>) -> Self {
        // In memory until `ensure_token` loads the saved ones or saves these.
        let (token, password) = (new_token(), random_password());
        let (owner_token, owner_password) = (new_token(), random_password());
        for s in [&token, &password, &owner_token, &owner_password] {
            crate::logging::register_secret(s);
        }
        Friend {
            ctx: Ctx {
                token: Arc::new(RwLock::new(token)),
                password: Arc::new(RwLock::new(password)),
                owner_token: Arc::new(RwLock::new(owner_token)),
                owner_password: Arc::new(RwLock::new(owner_password)),
                settings,
                fails: Arc::default(),
                actions: Arc::default(),
                port: Arc::default(),
                bad_hosts: Arc::default(),
                xff_logged: Arc::default(),
                backend,
                add: Arc::default(),
                add_gen: Arc::default(),
                add_starts: Arc::default(),
                add_gate: Arc::default(),
            },
            running: tokio::sync::Mutex::new(None),
            token_loaded: Mutex::default(),
        }
    }

    pub fn token(&self) -> String {
        read_secret(&self.ctx.token)
    }

    fn secrets(&self) -> [Slot<'_>; 4] {
        [
            (TOKEN_FILE, &*self.ctx.token, valid_token),
            (PASSWORD_FILE, &*self.ctx.password, valid_password),
            (OWNER_TOKEN_FILE, &*self.ctx.owner_token, valid_token),
            (
                OWNER_PASSWORD_FILE,
                &*self.ctx.owner_password,
                valid_password,
            ),
        ]
    }

    /// (enabled, port, publicUrl)
    fn cfg(&self) -> (bool, u16, String) {
        let s = self
            .ctx
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let port = s
            .pointer("/friend/port")
            .and_then(Value::as_u64)
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(7812);
        let url = s.pointer("/friend/publicUrl").and_then(Value::as_str);
        (
            s.pointer("/friend/enabled") == Some(&Value::Bool(true)),
            port,
            url.unwrap_or_default().to_owned(),
        )
    }

    pub fn enabled(&self) -> bool {
        self.cfg().0
    }

    /// The login page link the owner sends the friend (no secret in it).
    pub fn link(&self) -> Result<String, String> {
        link_of(&self.cfg().2)
    }

    /// Secret: the friend password, for the owner to view or copy.
    pub fn password(&self) -> Result<String, String> {
        // An unsaved in-memory password would stop working after a restart.
        if !*self.loaded() {
            return Err("The friend password isn't ready yet. Try again in a moment.".into());
        }
        Ok(read_secret(&self.ctx.password))
    }

    /// Secret: the owner login's password (same rule as `password`).
    pub fn owner_password(&self) -> Result<String, String> {
        if !*self.loaded() {
            return Err("The owner password isn't ready yet. Try again in a moment.".into());
        }
        Ok(read_secret(&self.ctx.owner_password))
    }

    fn loaded(&self) -> std::sync::MutexGuard<'_, bool> {
        self.token_loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Load the saved token and password, or save the current ones. Once per session, so
    /// they never swap under a friend already logged in. Blocking (file + keyring).
    /// ponytail: unlike the automation token, a file that does not unwrap or is malformed is
    /// simply replaced (the owner just shares the new password); a keyring or other read
    /// error leaves it alone and retries next call.
    pub fn ensure_token(&self, root: &std::path::Path, os: impl FnOnce() -> OsResult) {
        let mut done = self.loaded();
        if *done {
            return;
        }
        // Keyring error: not marked loaded, so the next call retries.
        let os = match os() {
            Ok(o) => o,
            Err(e) => return log::warn!("friend secrets kept in memory for now: {e}"),
        };
        *done = self
            .secrets()
            .into_iter()
            .all(|(file, cell, valid)| load_or_save(&root.join(file), cell, valid, &*os));
    }

    /// New password and token (old ones get 401 from the next request on), saved OS-wrapped;
    /// returns the password. A secret that cannot be saved has its old file deleted so it
    /// never comes back. Blocking.
    pub fn new_password(&self, root: &std::path::Path, os: impl FnOnce() -> OsResult) -> String {
        let mut done = self.loaded();
        let os = os();
        // Not loaded yet (remote off at startup, or a keyring error): load the owner login's
        // saved secrets first, or `ensure_token` never will once this marks them loaded.
        if let (false, Ok(os)) = (*done, &os) {
            for (file, cell, valid) in self.secrets().into_iter().skip(2) {
                load_or_save(&root.join(file), cell, valid, &**os);
            }
        }
        *done = true;
        write_secret(&self.ctx.token, new_token());
        self.ctx.drop_add(); // the signed-out friend's code must not add anything
        write_secret(&self.ctx.password, random_password());
        // The friend's two only: the owner's are not replaced here, so their files stay.
        for (file, cell, _) in self.secrets().into_iter().take(2) {
            let path = root.join(file);
            let r = os
                .clone()
                .and_then(|os| save(&path, &read_secret(cell), &*os));
            if let Err(e) = r {
                log::warn!("new friend secret not saved; memory-only this session: {e}");
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => log::error!("old friend secret file not deleted: {e}"),
                }
            }
        }
        log::info!("friend password and token replaced");
        read_secret(&self.ctx.password)
    }

    pub async fn status(&self) -> FriendStatus {
        let (_, port, public_url) = self.cfg();
        let run = self.running.lock().await;
        let up = run.as_ref().filter(|r| !r.task.is_finished());
        FriendStatus {
            running: up.is_some(),
            port: up.map_or(port, |r| r.addr.port()),
            public_url,
        }
    }

    /// Match the settings: serve on `127.0.0.1:/friend/port` while `/friend/enabled`,
    /// rebind on a port change or a dead server, stop when disabled.
    /// A bind error keeps its kind (`AddrInUse`: another program may hold the port).
    pub async fn sync(&self) -> std::io::Result<()> {
        // Settings saved: adding turned off (or the remote) ends a pending add now.
        if !self.ctx.can_add() || !self.enabled() {
            self.ctx.drop_add();
        }
        let mut run = self.running.lock().await;
        let (on, port, _) = self.cfg();
        if run
            .as_ref()
            .is_some_and(|r| on && r.port == port && !r.task.is_finished())
        {
            return Ok(());
        }
        if let Some(r) = run.take() {
            let _ = r.stop.send(());
            let abort = r.task.abort_handle();
            if tokio::time::timeout(STOP_GRACE, r.task).await.is_err() {
                log::warn!("friend server did not stop in time; aborting it");
                abort.abort();
            }
            log::info!("friend server stopped");
        }
        if on {
            *run = Some(self.bind(port).await?);
        }
        Ok(())
    }

    async fn bind(&self, port: u16) -> std::io::Result<Running> {
        let l = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|e| {
                let msg = format!("Could not open the friend server on port {port}: {e}");
                std::io::Error::new(e.kind(), msg)
            })?;
        let addr = l.local_addr()?;
        self.ctx.port.store(addr.port(), Ordering::Relaxed);
        let (stop, rx) = oneshot::channel::<()>();
        let app = router(self.ctx.clone());
        let task = tokio::spawn(async move {
            let stop = async {
                let _ = rx.await;
            };
            if let Err(e) = axum::serve(l, app).with_graceful_shutdown(stop).await {
                log::error!("friend server failed: {e}");
            }
        });
        log::info!("friend server on {addr}");
        Ok(Running {
            port,
            addr,
            stop,
            task,
        })
    }
}

/// Load a valid saved secret into `cell`, else save `cell`'s. False: file not read (retry).
fn load_or_save(
    path: &std::path::Path,
    cell: &RwLock<String>,
    valid: fn(&str) -> bool,
    os: &dyn crate::vault::oswrap::OsWrap,
) -> bool {
    // Missing file: save ours. Other read errors: keep the file, retry next call.
    let raw = match std::fs::read(path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("friend secret file not read: {e}");
            return false;
        }
    };
    let saved = raw
        .and_then(|b| os.unwrap(&b).ok())
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
        .filter(|t| valid(t));
    match saved {
        Some(t) => write_secret(cell, t),
        None => {
            if let Err(e) = save(path, &read_secret(cell), os) {
                log::warn!("friend secret not saved; kept in memory only: {e}");
            }
        }
    }
    true
}

fn save(
    path: &std::path::Path,
    token: &str,
    os: &dyn crate::vault::oswrap::OsWrap,
) -> Result<(), String> {
    let blob = os.wrap(token.as_bytes()).map_err(|e| e.to_string())?;
    crate::vault::store::write_atomic(path, &blob, crate::vault::store::Backup::Skip)
        .map_err(|e| e.to_string())
}

type Res = Result<Response, ApiError>;

fn not_found_route() -> ApiError {
    ApiError::not_found("No such endpoint")
}

const INDEX: &str = include_str!("web/index.html");
const APP_JS: &str = include_str!("web/app.js");
const APP_CSS: &str = include_str!("web/app.css");
const MANIFEST: &str = include_str!("web/manifest.json");
const ICON: &[u8] = include_bytes!("web/icon.png");
/// No inline script/style, no external origins, no framing.
const CSP: &str = "default-src 'self'; img-src 'self' blob:; connect-src 'self'; style-src 'self'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

async fn stat(ct: &'static str, body: &'static [u8]) -> impl IntoResponse {
    (
        [
            (CONTENT_TYPE, ct),
            (axum::http::header::CONTENT_SECURITY_POLICY, CSP),
        ],
        body,
    )
}

/// `/v1/*`: every path, unknown ones too, needs the bearer token; 16 requests in flight.
/// Outer router: `/login` and the static app; body limit + timeout for everything.
fn router(ctx: Ctx) -> Router {
    let api = Router::new()
        .route("/accounts", get(list))
        .route("/accounts/{id}/avatar", get(avatar))
        .route("/accounts/{id}/disconnect", post(disconnect))
        .route("/accounts/{id}/reconnect", post(reconnect))
        .route("/add", get(add_info))
        .route("/add/start", post(add_start))
        .route("/add/status", get(add_status))
        .route("/add/cancel", post(add_cancel))
        .fallback(|| async { not_found_route() })
        // Global: Router::layer wraps each route, so share one semaphore.
        .layer(tower::limit::GlobalConcurrencyLimitLayer::new(16))
        .layer(middleware::from_fn_with_state(ctx.clone(), auth));
    Router::new()
        .nest("/v1", api)
        .route("/login", post(login))
        // Static phone web app: fixed embedded assets, outside auth and the /v1 limit.
        .route(
            "/",
            get(|| stat("text/html; charset=utf-8", INDEX.as_bytes())),
        )
        .route(
            "/app.js",
            get(|| stat("text/javascript; charset=utf-8", APP_JS.as_bytes())),
        )
        .route(
            "/app.css",
            get(|| stat("text/css; charset=utf-8", APP_CSS.as_bytes())),
        )
        .route(
            "/manifest.json",
            get(|| stat("application/manifest+json", MANIFEST.as_bytes())),
        )
        .route("/icon.png", get(|| stat("image/png", ICON)))
        .fallback(|| async { not_found_route() })
        .layer(DefaultBodyLimit::max(4096))
        .layer(
            tower::ServiceBuilder::new()
                .layer(axum::error_handling::HandleErrorLayer::new(|_| async {
                    ApiError::new(504, "timeout", "The request took too long")
                }))
                .timeout(Duration::from_secs(10)),
        )
        .layer(middleware::from_fn_with_state(ctx.clone(), host_guard))
        .layer(middleware::map_response(harden))
        .with_state(ctx)
}

/// Every response, errors and rejections too. Cache-Control only if the route set none
/// (the avatar's own private cache stays).
pub(crate) async fn harden(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.entry(CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    res
}

/// `host[:port]` of a URL ("" if none).
fn authority(url: &str) -> &str {
    let u = url.trim();
    let u = u.split_once("://").map_or(u, |(_, r)| r);
    u.split(['/', '?', '#']).next().unwrap_or_default()
}

/// DNS-rebinding guard: Host must be publicUrl's host (with or without its port) or
/// loopback on the bound port. publicUrl read live; empty means loopback only.
fn host_ok(host: &str, public_url: &str, port: u16) -> bool {
    let host = host.trim();
    let public = authority(public_url);
    let bare = public
        .rsplit_once(':')
        .filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .map_or(public, |(h, _)| h);
    let public_ok = !public.is_empty()
        && (host.eq_ignore_ascii_case(public) || host.eq_ignore_ascii_case(bare));
    public_ok
        || ["127.0.0.1", "localhost", "[::1]"]
            .iter()
            .any(|l| host.eq_ignore_ascii_case(&format!("{l}:{port}")))
}

async fn host_guard(State(c): State<Ctx>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let public = c
        .settings
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .pointer("/friend/publicUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if host_ok(host, &public, c.port.load(Ordering::Relaxed)) {
        return next.run(req).await;
    }
    let shown: String = host.chars().take(64).collect();
    let mut seen = c.bad_hosts.lock().unwrap_or_else(PoisonError::into_inner);
    // ponytail: first 64 distinct values only, so a flood can't grow the set.
    if seen.len() < MAX_HOST_LOGS && seen.insert(shown.clone()) {
        log::warn!("friend: rejected Host {shown:?}");
    }
    ApiError::new(403, "forbidden_host", "Wrong host for the friend remote").into_response()
}

/// Client IP from the last X-Forwarded-For value (trimmed, max 64 chars). Only the loopback
/// Funnel proxy reaches the port, and it drops the caller's X-Forwarded-For and sets its own
/// (tailscale ipn/ipnlocal/serve.go, addProxyForwardedHeaders). The last value is the one
/// the nearest proxy wrote, so a caller can't rotate it even behind a proxy that appends.
/// No header: one shared fallback key.
fn client_key(h: &HeaderMap) -> (String, bool) {
    match h
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.rsplit(',').next().unwrap_or_default().trim())
        .filter(|v| !v.is_empty())
    {
        Some(ip) => (ip.chars().take(64).collect(), true),
        None => ("-".into(), false),
    }
}

/// One Disconnect/Reconnect per account per `COOLDOWN`. The slot is taken up front (so
/// parallel requests can't slip past) and handed back if the action fails.
fn take_slot(c: &Ctx, id: &str) -> ApiResult<()> {
    let mut m = c.actions.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    m.retain(|_, t| now.duration_since(*t) < COOLDOWN);
    if m.contains_key(id) {
        return Err(ApiError::new(
            429,
            "slow_down",
            "Wait a few seconds and try again.",
        ));
    }
    m.insert(id.to_owned(), now);
    Ok(())
}

fn give_back<T>(c: &Ctx, id: &str, r: ApiResult<T>) -> ApiResult<T> {
    if r.is_err() {
        c.actions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }
    r
}

async fn auth(State(c): State<Ctx>, mut req: Request, next: Next) -> Response {
    let role = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .and_then(|(_, t)| c.token_role(t.trim()));
    let Some(owner) = role else {
        return ApiError::new(401, "unauthorized", "Missing or wrong friend token").into_response();
    };
    req.extensions_mut().insert(Owner(owner));
    next.run(req).await
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

fn body_err(e: JsonRejection) -> ApiError {
    if e.status().as_u16() == 413 {
        ApiError::new(413, "too_large", "Request body is over 4 KB")
    } else {
        ApiError::new(400, "bad_request", e.body_text())
    }
}

/// Username + password for the bearer token. Same 401 for a wrong username or password.
async fn login(
    State(c): State<Ctx>,
    headers: HeaderMap,
    b: Result<Json<LoginBody>, JsonRejection>,
) -> Res {
    let b = b.map_err(body_err)?.0;
    let (key, fwd) = client_key(&headers);
    if !c.xff_logged.swap(true, Ordering::Relaxed) {
        log::info!("friend: first login attempt; X-Forwarded-For present: {fwd}");
    }
    // One lock across check, compare and record: parallel guesses can't slip past the cap.
    let mut all = c.fails.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    let live = |t: &Instant| now.duration_since(*t) < FAIL_WINDOW;
    if !all.contains_key(&key) && all.len() >= MAX_FAIL_KEYS {
        all.retain(|_, q| q.back().is_some_and(live));
        // Still full: drop the client whose last fail is oldest.
        if all.len() >= MAX_FAIL_KEYS {
            let stale = all
                .iter()
                .min_by_key(|(_, q)| q.back().copied())
                .map(|(k, _)| k.clone());
            stale.map(|k| all.remove(&k));
        }
    }
    let fails = all.entry(key).or_default();
    while fails.front().is_some_and(|t| !live(t)) {
        fails.pop_front();
    }
    if fails.len() >= MAX_FAILS {
        return Err(ApiError::new(
            429,
            "too_many",
            "Too many tries. Wait a minute.",
        ));
    }
    if let Some(owner) = c.login_role(&b.username, &b.password) {
        let t = if owner { &c.owner_token } else { &c.token };
        return Ok(Json(json!({ "token": read_secret(t) })).into_response());
    }
    fails.push_back(now);
    log::warn!("friend: failed login");
    Err(ApiError::new(
        401,
        "bad_login",
        "Wrong username or password",
    ))
}

async fn list(State(c): State<Ctx>, Extension(Owner(owner)): Extension<Owner>) -> Res {
    Ok(Json(c.backend.friend_list(owner).await?).into_response())
}

async fn avatar(
    State(c): State<Ctx>,
    Extension(Owner(owner)): Extension<Owner>,
    Path(id): Path<String>,
) -> Res {
    let png = c.backend.friend_avatar(&id, owner).await?;
    let h = [
        (CONTENT_TYPE, "image/png"),
        (CACHE_CONTROL, "private, max-age=300"),
    ];
    Ok((h, png).into_response())
}

async fn disconnect(
    State(c): State<Ctx>,
    Extension(Owner(owner)): Extension<Owner>,
    Path(id): Path<String>,
) -> Res {
    take_slot(&c, &id)?;
    give_back(&c, &id, c.backend.friend_disconnect(&id, owner).await)?;
    Ok(Json(json!({})).into_response())
}

#[derive(Default, Deserialize)]
struct ReconnectBody {
    link: Option<String>,
}

async fn reconnect(
    State(c): State<Ctx>,
    Extension(Owner(owner)): Extension<Owner>,
    Path(id): Path<String>,
    b: Result<Json<ReconnectBody>, JsonRejection>,
) -> Res {
    let body = match b {
        Ok(Json(v)) => v,
        // no body (so no Content-Type): no link
        Err(JsonRejection::MissingJsonContentType(_)) => ReconnectBody::default(),
        Err(e) => return Err(body_err(e)),
    };
    take_slot(&c, &id)?;
    let batch = give_back(
        &c,
        &id,
        c.backend.friend_reconnect(&id, body.link, owner).await,
    )?;
    Ok(Json(json!({ "batch": batch })).into_response())
}

/// Adding is the friend's only: the owner's login gets "adding is off" (he adds accounts in
/// the app), and never touches the friend's pending add or its slot.
fn friend_only(Owner(owner): Owner) -> ApiResult<()> {
    if owner {
        return Err(add_off());
    }
    Ok(())
}

/// May the friend add accounts, and how many he has.
async fn add_info(State(c): State<Ctx>, Extension(Owner(owner)): Extension<Owner>) -> Res {
    let added = c.backend.friend_added().await?;
    let allowed = !owner && c.can_add();
    Ok(Json(json!({ "allowed": allowed, "max": MAX_ADDED, "added": added })).into_response())
}

/// A Quick Log In code for the friend to type into Roblox. A live one is handed out again.
async fn add_start(State(c): State<Ctx>, Extension(o): Extension<Owner>) -> Res {
    friend_only(o)?;
    let _one = c.add_gate.lock().await;
    c.allow_add()?;
    if c.backend.friend_added().await? >= MAX_ADDED {
        return Err(ApiError::new(
            409,
            "add_full",
            "You already added the most accounts allowed. Ask the owner to remove one.",
        ));
    }
    let code_json = |p: &Pending| {
        let left = p.until.saturating_duration_since(Instant::now()).as_secs();
        Json(json!({ "code": p.code, "expiresInSec": left })).into_response()
    };
    let gen = {
        let g = c.add_slot();
        // `until` is the code's expiry while waiting, the sign-in deadline once linked.
        let live = g.as_ref().filter(|p| {
            matches!(p.state, AddState::Waiting(_) | AddState::Linked(_))
                && Instant::now() < p.until
        });
        if let Some(p) = live {
            return Ok(code_json(p));
        }
        c.add_gen.load(Ordering::SeqCst)
    };
    // Only a new code counts toward the rate window: checked here, recorded once Roblox made
    // it (one start at a time: `add_gate`, so nothing slips in between).
    {
        let mut q = c.add_starts.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= START_WINDOW)
        {
            q.pop_front();
        }
        if q.len() >= MAX_STARTS {
            return Err(ApiError::new(
                429,
                "slow_down",
                "Too many codes. Wait a few minutes and try again.",
            ));
        }
    }
    let new = c.backend.friend_add_create().await?;
    c.add_starts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push_back(Instant::now());
    let p = Pending {
        until: Instant::now() + add_ttl(new.expiration_time.as_deref()),
        code: new.code,
        key: new.private_key,
        checked: None,
        state: AddState::Waiting(None),
    };
    let mut g = c.add_slot();
    // Dropped meanwhile (adding turned off, new password): this code is not kept.
    if c.add_gen.load(Ordering::SeqCst) != gen || !c.can_add() {
        return Err(add_off());
    }
    let r = code_json(&p);
    *g = Some(p);
    Ok(r)
}

/// Where the pending add is. Asks Roblox at most every `ADD_POLL`; once the friend confirms,
/// signs in and adds the account in the background (the page keeps polling).
async fn add_status(State(c): State<Ctx>, Extension(o): Extension<Owner>) -> Res {
    friend_only(o)?;
    c.allow_add()?;
    let expired = || Ok(Json(json!({ "status": "expired" })).into_response());
    let (code, key, gen) = {
        let mut g = c.add_slot();
        let Some(p) = g.as_mut() else {
            return expired();
        };
        let now = Instant::now();
        match p.state {
            AddState::Waiting(_) if now >= p.until => {
                *g = None;
                return expired();
            }
            // The background sign-in never reported back.
            AddState::Linked(_) if now >= p.until => {
                log::warn!("friend: add sign-in timed out");
                p.state = AddState::Failed(ADD_STUCK.into());
                return Ok(Json(add_json(&p.state)).into_response());
            }
            AddState::Waiting(_) if p.checked.is_none_or(|t| now.duration_since(t) >= ADD_POLL) => {
                p.checked = Some(now);
                (
                    p.code.clone(),
                    p.key.clone(),
                    c.add_gen.load(Ordering::SeqCst),
                )
            }
            _ => return Ok(Json(add_json(&p.state)).into_response()),
        }
    };
    let r = c
        .backend
        .friend_add_poll(&code, &crate::roblox::Secret::new(key.as_str()))
        .await;
    let mut g = c.add_slot();
    let Some(p) = g
        .as_mut()
        .filter(|p| p.code == code && c.add_gen.load(Ordering::SeqCst) == gen)
    else {
        return expired();
    };
    let AddState::Waiting(name) = &p.state else {
        return Ok(Json(add_json(&p.state)).into_response());
    };
    match r {
        // A failed check keeps waiting: the code times out on its own.
        Err(e) => log::debug!("friend: add status check failed: {}", e.message),
        Ok(s) => {
            let name = s.account_name.or_else(|| name.clone());
            match s.status.as_str() {
                "UserLinked" => p.state = AddState::Waiting(name),
                "Cancelled" => p.state = AddState::Cancelled,
                "Validated" => {
                    p.state = AddState::Linked(name);
                    p.until = Instant::now() + LINK_TTL;
                    tokio::spawn(finish_add(c.clone(), code, key, gen));
                }
                _ => {}
            }
        }
    }
    Ok(Json(add_json(&p.state)).into_response())
}

/// A sign-in still linked this long after confirming has failed.
const LINK_TTL: Duration = Duration::from_secs(60);
const ADD_STUCK: &str = "Adding the account did not finish. Try again.";

/// Marks the add failed if `finish_add` ends without an outcome (panic, task dropped).
struct LinkedGuard(Ctx, String);

impl Drop for LinkedGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.add_slot().as_mut().filter(|p| p.code == self.1) {
            if matches!(p.state, AddState::Linked(_)) {
                p.state = AddState::Failed(ADD_STUCK.into());
            }
        }
    }
}

/// Sign in with the confirmed code and add the account; the outcome lands in the slot.
/// `gen`: the add generation when it was confirmed; a drop since (adding off, new password,
/// remote off) stops it before sign-in and again right before the vault write.
async fn finish_add(c: Ctx, code: String, key: Zeroizing<String>, gen: u64) {
    let _stuck = LinkedGuard(c.clone(), code.clone());
    let (g, c2) = (c.clone(), code.clone());
    // Still linked too: once status marked it stuck (the friend was told it failed), the
    // account must not be added after all.
    let ok: crate::automation::AddGate = Arc::new(move || {
        g.can_add()
            && g.remote_on()
            && g.add_gen.load(Ordering::SeqCst) == gen
            && g.add_slot()
                .as_ref()
                .is_some_and(|p| p.code == c2 && matches!(p.state, AddState::Linked(_)))
    });
    let r = c
        .backend
        .friend_add_finish(&code, &crate::roblox::Secret::new(key.as_str()), ok)
        .await;
    // Own statement: the slot lock is released before `_stuck` drops (it locks it too).
    let mut g = c.add_slot();
    let linked = |p: &&mut Pending| p.code == code && matches!(p.state, AddState::Linked(_));
    if let Some(p) = g.as_mut().filter(linked) {
        p.state = match r {
            Ok(name) => AddState::Done(name),
            Err(e) => AddState::Failed(e.message),
        };
    }
    drop(g);
}

/// Stop the pending add (Roblox is told too, best effort).
async fn add_cancel(State(c): State<Ctx>, Extension(o): Extension<Owner>) -> Res {
    friend_only(o)?;
    c.allow_add()?;
    let code = {
        let mut g = c.add_slot();
        match g.as_mut() {
            Some(p) if matches!(p.state, AddState::Waiting(_)) => {
                p.state = AddState::Cancelled;
                Some(p.code.clone())
            }
            _ => None,
        }
    };
    if let Some(code) = code {
        if let Err(e) = c.backend.friend_add_cancel(&code).await {
            log::debug!("friend: add cancel not sent: {}", e.message);
        }
    }
    Ok(Json(json!({})).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{Presence, PresenceState};
    use crate::automation::backend::{
        tests::{host_with, NoPlat, TH},
        AppBackend,
    };
    use crate::platform::{Capabilities, Platform, RobloxInstall};
    use crate::AppState;
    use axum::body::Body;
    use std::sync::{OnceLock, Weak};
    use tower::ServiceExt;

    fn fake_os() -> OsResult {
        Ok(Arc::new(crate::vault::oswrap::FakeOsWrap))
    }

    /// Records each close with whether the account was already paused at that moment.
    struct Rec(Arc<OnceLock<Weak<AppState>>>, Arc<Mutex<Vec<String>>>);
    impl Platform for Rec {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn roblox_install(&self) -> Option<RobloxInstall> {
            None
        }
        fn open_uri(&self, _: &str) -> std::io::Result<()> {
            panic!("no real launches in tests")
        }
        fn close_tracked(&self, t: &str) -> std::io::Result<usize> {
            let st = self.0.get().unwrap().upgrade().unwrap();
            let paused = crate::commands::guard(&st.vault)
                .as_ref()
                .unwrap()
                .payload()
                .accounts
                .iter()
                .any(|a| a.tracker_id == t && a.friend_paused);
            self.1
                .lock()
                .unwrap()
                .push(format!("close paused={paused}"));
            Ok(1)
        }
    }

    /// alice ("A") friend-allowed, bob ("B") not. Server settings: enabled, port 0.
    fn setup(dir: &std::path::Path, plat: Box<dyn Platform>) -> (TH, Friend) {
        let h = host_with(dir, plat);
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[0]
            .friend_allowed = true;
        let b = AppBackend::default();
        b.set_host(h.clone());
        let mut s = crate::settings::defaults();
        s["friend"] = json!({ "enabled": true, "port": 0, "publicUrl": "https://x.ts.net/" });
        (h, Friend::new(Arc::new(Mutex::new(s)), Arc::new(b)))
    }

    async fn call(f: &Friend, method: &str, path: &str, auth: Option<&str>) -> (u16, Value) {
        call_body(f, method, path, auth, None).await
    }

    async fn call_body(
        f: &Friend,
        method: &str,
        path: &str,
        auth: Option<&str>,
        body: Option<&str>,
    ) -> (u16, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:0");
        if let Some(t) = auth {
            req = req.header("authorization", t);
        }
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req
            .body(Body::from(body.unwrap_or_default().to_owned()))
            .unwrap();
        let res = router(f.ctx.clone()).oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn bearer(f: &Friend) -> String {
        format!("Bearer {}", f.token())
    }

    fn paused(h: &TH) -> (bool, Option<u64>) {
        let g = h.0.vault.lock().unwrap();
        let a = &g.as_ref().unwrap().payload().accounts[0];
        (a.friend_paused, a.friend_last_place)
    }

    async fn raw_get(f: &Friend, path: &str) -> Response {
        get_host(f, path, "127.0.0.1:0").await
    }

    async fn get_host(f: &Friend, path: &str, host: &str) -> Response {
        let req = Request::builder()
            .uri(path)
            .header("host", host)
            .body(Body::empty())
            .unwrap();
        router(f.ctx.clone()).oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn host_guard_allows_public_and_loopback_only() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        f.ctx.settings.lock().unwrap()["friend"]["publicUrl"] = json!("https://x.ts.net:8443/");
        f.ctx.port.store(7812, Ordering::Relaxed);
        for host in [
            "x.ts.net:8443",
            "X.ts.net",
            "127.0.0.1:7812",
            "localhost:7812",
            "[::1]:7812",
        ] {
            assert_eq!(get_host(&f, "/", host).await.status(), 200, "{host}");
        }
        for host in [
            "evil.com",
            "x.ts.net:1",
            "127.0.0.1:9",
            "",
            "x.ts.net.evil.com",
        ] {
            let res = get_host(&f, "/login", host).await;
            assert_eq!(res.status(), 403, "{host}");
            assert_eq!(res.headers()["x-content-type-options"], "nosniff");
            let b = axum::body::to_bytes(res.into_body(), 1 << 20)
                .await
                .unwrap();
            let v: Value = serde_json::from_slice(&b).unwrap();
            assert_eq!(v["error"]["code"], "forbidden_host");
        }
        assert_eq!(get_host(&f, "/v1/accounts", "evil.com").await.status(), 403);
        // Empty publicUrl: loopback only.
        f.ctx.settings.lock().unwrap()["friend"]["publicUrl"] = json!("");
        assert_eq!(get_host(&f, "/", "x.ts.net").await.status(), 403);
        assert_eq!(get_host(&f, "/", "localhost:7812").await.status(), 200);
    }

    #[test]
    fn host_ok_without_port_in_public_url() {
        assert!(host_ok("x.ts.net", "https://x.ts.net", 1));
        assert!(!host_ok("x.ts.net:443", "https://x.ts.net", 1));
        assert!(!host_ok("", "", 1));
    }

    #[tokio::test]
    async fn hardening_headers_on_errors() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        for (p, code) in [("/v1/accounts", 401), ("/nope", 404)] {
            let res = raw_get(&f, p).await;
            assert_eq!(res.status(), code);
            let h = res.headers();
            assert_eq!(h["x-content-type-options"], "nosniff");
            assert_eq!(h["cache-control"], "no-store");
            assert_eq!(h["referrer-policy"], "no-referrer");
        }
    }

    #[test]
    fn internal_error_message_is_fixed() {
        let e = internal("secret path C:\\x");
        assert_eq!(
            (e.status, e.message.as_str()),
            (500, "Something went wrong on the owner's computer.")
        );
    }

    #[test]
    fn launch_err_keeps_a_refusal_400_with_fixed_words() {
        let e = launch_err(ApiError::new(400, "bad_request", "raw C:\\x text"));
        assert_eq!(
            (e.status, e.message.as_str()),
            (400, "Could not start the game.")
        );
        assert_eq!(launch_err(ApiError::new(409, "x", "C:\\x")).status, 500);
        assert_eq!(launch_err(ApiError::new(423, "x", "C:\\x")).status, 503);
    }

    #[tokio::test]
    async fn static_index_served_without_token() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let res = raw_get(&f, "/").await;
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        for (p, ct) in [
            ("/app.js", "text/javascript; charset=utf-8"),
            ("/app.css", "text/css; charset=utf-8"),
            ("/manifest.json", "application/manifest+json"),
            ("/icon.png", "image/png"),
        ] {
            let res = raw_get(&f, p).await;
            assert_eq!(res.status(), 200, "{p}");
            assert_eq!(res.headers()[CONTENT_TYPE], ct, "{p}");
        }
        assert_eq!(raw_get(&f, "/v1/accounts").await.status(), 401);
        assert_eq!(raw_get(&f, "/web/app.js").await.status(), 404);
        // Login sits outside /v1 auth.
        assert_eq!(login(&f, "friend", &pw(&f)).await.0, 200);
    }

    #[tokio::test]
    async fn index_has_csp() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        for p in ["/", "/app.js", "/app.css", "/manifest.json", "/icon.png"] {
            let h = raw_get(&f, p).await.headers().clone();
            let csp = h["content-security-policy"].to_str().unwrap();
            assert!(csp.contains("default-src 'self'") && csp.contains("frame-ancestors 'none'"));
            assert!(csp.contains("script-src 'self'") && !csp.contains("unsafe-inline"));
            assert_eq!(h["x-content-type-options"], "nosniff");
            assert_eq!(h["referrer-policy"], "no-referrer");
            assert_eq!(h["cache-control"], "no-store");
            assert!(!h.contains_key("access-control-allow-origin"));
        }
        let body = axum::body::to_bytes(raw_get(&f, "/").await.into_body(), 1 << 20)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(!html.contains("<style") && !html.contains("onclick"));
        assert!(!html.contains("<script>"));
    }

    #[tokio::test]
    async fn no_or_wrong_token_401() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let raw = f.token();
        for auth in [None, Some("Bearer nope"), Some(raw.as_str())] {
            for (m, p) in [
                ("GET", "/v1/accounts"),
                ("GET", "/v1/accounts/A/avatar"),
                ("POST", "/v1/accounts/A/disconnect"),
                ("POST", "/v1/accounts/A/reconnect"),
                ("GET", "/v1/nope"),
                ("DELETE", "/v1/accounts"),
                ("GET", "/v1/add"),
                ("POST", "/v1/add/start"),
                ("GET", "/v1/add/status"),
                ("POST", "/v1/add/cancel"),
            ] {
                let (s, b) = call(&f, m, p, auth).await;
                assert_eq!(
                    (s, &b["error"]["code"]),
                    (401, &json!("unauthorized")),
                    "{m} {p}"
                );
            }
        }
        // With the token: unknown /v1 path and non-API paths are 404.
        let t = bearer(&f);
        assert_eq!(call(&f, "GET", "/v1/nope", Some(&t)).await.0, 404);
        assert_eq!(call(&f, "GET", "/index.html", None).await.0, 404);
    }

    #[test]
    fn keyring_error_does_not_lock_in_memory_token() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        f.ensure_token(d.path(), fake_os);
        let saved = f.token();
        let d2 = tempfile::tempdir().unwrap();
        let (_h2, g) = setup(d2.path(), Box::new(NoPlat));
        g.ensure_token(d.path(), || Err("cancelled".into()));
        assert_ne!(g.token(), saved);
        g.ensure_token(d.path(), fake_os); // retried
        assert_eq!(g.token(), saved);
    }

    #[tokio::test]
    async fn lists_only_allowed() {
        let d = tempfile::tempdir().unwrap();
        let (h, f) = setup(d.path(), Box::new(NoPlat));
        h.0.accounts.presence.lock().unwrap().insert(
            "A".into(),
            Presence {
                state: PresenceState::InGame,
                place_id: Some(42),
                universe_id: None,
                game_name: Some("Game".into()),
                job_id: None,
                updated_at: String::new(),
            },
        );
        let (s, b) = call(&f, "GET", "/v1/accounts", Some(&bearer(&f))).await;
        assert_eq!(s, 200);
        assert_eq!(
            b,
            // presence alone (no RMI client) is not live: in a game on another computer
            json!([{ "id": "A", "displayName": "Alice", "username": "Alice",
                     "status": "offline", "game": "Game", "uptimeSec": null,
                     "elsewhere": true }])
        );
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[0]
            .friend_paused = true;
        let (_, b) = call(&f, "GET", "/v1/accounts", Some(&bearer(&f))).await;
        assert_eq!(b[0]["status"], "paused");
    }

    #[tokio::test]
    async fn owner_login_gets_every_account_friend_only_shared() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let owner_pw = read_secret(&f.ctx.owner_password);
        // No owner username set: no owner login, and its token is refused.
        assert_eq!(login(&f, "boss", &owner_pw).await.0, 401);
        let owner_t = format!("Bearer {}", read_secret(&f.ctx.owner_token));
        assert_eq!(call(&f, "GET", "/v1/accounts", Some(&owner_t)).await.0, 401);
        f.ctx.settings.lock().unwrap()["friend"]["ownerUsername"] = json!("Boss");
        // The friend's password does not open the owner login, nor the other way round.
        assert_eq!(login(&f, "boss", &pw(&f)).await.0, 401);
        assert_eq!(login(&f, "friend", &owner_pw).await.0, 401);
        let (s, b) = login(&f, " BOSS ", &owner_pw).await;
        assert_eq!(s, 200);
        let t = format!("Bearer {}", b["token"].as_str().unwrap());
        assert_ne!(t, bearer(&f), "its own token");
        let ids = |b: Value| -> Vec<String> {
            let l = b.as_array().unwrap().iter();
            l.map(|a| a["id"].as_str().unwrap().to_owned()).collect()
        };
        assert_eq!(
            ids(call(&f, "GET", "/v1/accounts", Some(&t)).await.1),
            ["A", "B"]
        );
        let friend_list = call(&f, "GET", "/v1/accounts", Some(&bearer(&f))).await.1;
        assert_eq!(ids(friend_list), ["A"]);
        // Bob is not shared: the friend still gets 404, the owner can act on him.
        let (s, _) = call(&f, "POST", "/v1/accounts/B/disconnect", Some(&bearer(&f))).await;
        assert_eq!(s, 404);
        let (s, _) = call(&f, "POST", "/v1/accounts/B/disconnect", Some(&t)).await;
        assert_eq!(s, 200);
        // Owner username cleared: the owner token stops working at once.
        f.ctx.settings.lock().unwrap()["friend"]["ownerUsername"] = json!("");
        assert_eq!(call(&f, "GET", "/v1/accounts", Some(&t)).await.0, 401);
    }

    #[tokio::test]
    async fn not_allowed_is_404_same_as_missing() {
        let d = tempfile::tempdir().unwrap();
        let (h, f) = setup(d.path(), Box::new(NoPlat));
        let t = bearer(&f);
        for (m, action) in [
            ("GET", "avatar"),
            ("POST", "disconnect"),
            ("POST", "reconnect"),
        ] {
            let bob = call(&f, m, &format!("/v1/accounts/B/{action}"), Some(&t)).await;
            let none = call(&f, m, &format!("/v1/accounts/nope/{action}"), Some(&t)).await;
            assert_eq!(bob.0, 404, "{action}");
            assert_eq!(bob, none, "{action}");
        }
        let g = h.0.vault.lock().unwrap();
        assert!(!g.as_ref().unwrap().payload().accounts[1].friend_paused);
    }

    #[tokio::test]
    async fn locked_is_503() {
        let d = tempfile::tempdir().unwrap();
        let (h, f) = setup(d.path(), Box::new(NoPlat));
        *h.0.vault.lock().unwrap() = None;
        let t = bearer(&f);
        for (m, p) in [
            ("GET", "/v1/accounts"),
            ("GET", "/v1/accounts/A/avatar"),
            ("POST", "/v1/accounts/A/disconnect"),
            ("POST", "/v1/accounts/A/reconnect"),
        ] {
            let (s, b) = call(&f, m, p, Some(&t)).await;
            assert_eq!((s, &b["error"]["code"]), (503, &json!("locked")), "{p}");
        }
    }

    #[tokio::test]
    async fn disconnect_pauses_then_closes() {
        let d = tempfile::tempdir().unwrap();
        let (st, calls) = (Arc::new(OnceLock::new()), Arc::new(Mutex::new(vec![])));
        let (h, f) = setup(d.path(), Box::new(Rec(st.clone(), calls.clone())));
        st.set(Arc::downgrade(&h.0)).unwrap();
        h.0.accounts.presence.lock().unwrap().insert(
            "A".into(),
            Presence {
                state: PresenceState::InGame,
                place_id: Some(42),
                universe_id: None,
                game_name: None,
                job_id: None,
                updated_at: String::new(),
            },
        );
        let (s, b) = call(&f, "POST", "/v1/accounts/A/disconnect", Some(&bearer(&f))).await;
        assert_eq!((s, b), (200, json!({})));
        assert_eq!(*calls.lock().unwrap(), ["close paused=true"]);
        assert_eq!(paused(&h), (true, Some(42)));
        {
            let ev = h.1.lock().unwrap();
            assert!(ev.contains(&"state://changed".into()) && ev.contains(&"friend_action".into()));
        }
        // Again while paused (after the cooldown): closes, no vault save (place kept).
        f.ctx.actions.lock().unwrap().clear();
        h.0.accounts
            .presence
            .lock()
            .unwrap()
            .get_mut("A")
            .unwrap()
            .place_id = Some(99);
        let (s, _) = call(&f, "POST", "/v1/accounts/A/disconnect", Some(&bearer(&f))).await;
        assert_eq!(s, 200);
        assert_eq!(calls.lock().unwrap().len(), 2);
        assert_eq!(paused(&h), (true, Some(42)));
    }

    #[tokio::test]
    async fn actions_share_a_per_account_cooldown() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let t = bearer(&f);
        let rc = || {
            call_body(
                &f,
                "POST",
                "/v1/accounts/A/reconnect",
                Some(&t),
                Some(r#"{"link":"5"}"#),
            )
        };
        assert_eq!(
            call(&f, "POST", "/v1/accounts/A/disconnect", Some(&t))
                .await
                .0,
            200
        );
        let (s, b) = rc().await;
        assert_eq!((s, &b["error"]["code"]), (429, &json!("slow_down")));
        assert_eq!(b["error"]["message"], "Wait a few seconds and try again.");
        // Failed actions hand the slot back: a 404 does not block.
        assert_eq!(
            call(&f, "POST", "/v1/accounts/B/reconnect", Some(&t))
                .await
                .0,
            404
        );
        assert!(!f.ctx.actions.lock().unwrap().contains_key("B"));
        // Window over: allowed. Cleared, not back-dated: `Instant::now() - COOLDOWN` panics
        // on a machine booted less than that long ago.
        f.ctx.actions.lock().unwrap().clear();
        assert_eq!(rc().await.0, 200);
    }

    #[tokio::test]
    async fn reconnect_while_busy_is_409() {
        let d = tempfile::tempdir().unwrap();
        let (h, f) = setup(d.path(), Box::new(NoPlat));
        h.0.automation.own_begin(&["A".to_owned()]);
        let (s, b) = call_body(
            &f,
            "POST",
            "/v1/accounts/A/reconnect",
            Some(&bearer(&f)),
            Some(r#"{"link":"5"}"#),
        )
        .await;
        assert_eq!((s, &b["error"]["code"]), (409, &json!("busy")));
        assert_eq!(b["error"]["message"], "Already reconnecting");
        assert!(f.ctx.actions.lock().unwrap().is_empty(), "slot handed back");
        // Still paused (first Reconnect mid pause-save): a running launch refuses too.
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[0]
            .friend_paused = true;
        let (s, _) = call_body(
            &f,
            "POST",
            "/v1/accounts/A/reconnect",
            Some(&bearer(&f)),
            Some(r#"{"link":"5"}"#),
        )
        .await;
        assert_eq!(s, 409);
    }

    #[test]
    fn reconnect_uses_link_then_target_then_last_place() {
        let dest = |l: Option<&str>, t, last| {
            reconnect_bar(l.map(str::to_owned), t, last)
                .map(|(b, all)| (b.destination, all))
                .unwrap()
        };
        let link = "https://www.roblox.com/games/5/x";
        assert_eq!(
            dest(Some(&format!(" {link} ")), true, Some(7)),
            (link.into(), true)
        );
        assert_eq!(dest(Some("  "), true, Some(7)), (String::new(), false));
        assert_eq!(dest(None, false, Some(7)), ("7".into(), true));
    }

    #[test]
    fn reconnect_name_search_is_bad_link() {
        for l in ["Adopt Me", "@x", "@Builderman", "https://x"] {
            let e = reconnect_bar(Some(l.into()), true, Some(7)).unwrap_err();
            assert_eq!((e.status, e.code), (400, "bad_link"), "{l}");
        }
    }

    #[tokio::test]
    async fn reconnect_clears_pause_and_returns_batch() {
        let d = tempfile::tempdir().unwrap();
        let (h, f) = setup(d.path(), Box::new(NoPlat));
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[0]
            .friend_paused = true;
        let (s, b) = call_body(
            &f,
            "POST",
            "/v1/accounts/A/reconnect",
            Some(&bearer(&f)),
            Some(r#"{"link":"5"}"#),
        )
        .await;
        assert_eq!(s, 200, "{b}");
        assert!(b["batch"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(!paused(&h).0);
        assert!(h.1.lock().unwrap().contains(&"friend_action".into()));
    }

    #[tokio::test]
    async fn reconnect_without_game_is_no_game() {
        let d = tempfile::tempdir().unwrap();
        let (h, f) = setup(d.path(), Box::new(NoPlat));
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[0]
            .friend_paused = true;
        let (s, b) = call(&f, "POST", "/v1/accounts/A/reconnect", Some(&bearer(&f))).await;
        assert_eq!((s, &b["error"]["code"]), (409, &json!("no_game")));
        assert!(paused(&h).0, "still paused");
        let big = format!(r#"{{"link":"{}"}}"#, "x".repeat(5000));
        let (s, _) = call_body(
            &f,
            "POST",
            "/v1/accounts/A/reconnect",
            Some(&bearer(&f)),
            Some(&big),
        )
        .await;
        assert_eq!(s, 413);
    }

    async fn login(f: &Friend, user: &str, pw: &str) -> (u16, Value) {
        let body = json!({ "username": user, "password": pw }).to_string();
        call_body(f, "POST", "/login", None, Some(&body)).await
    }

    fn pw(f: &Friend) -> String {
        read_secret(&f.ctx.password)
    }

    #[tokio::test]
    async fn login_gives_working_token() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let (s, b) = login(&f, "friend", &pw(&f)).await;
        assert_eq!(s, 200);
        let t = format!("Bearer {}", b["token"].as_str().unwrap());
        assert_eq!(call(&f, "GET", "/v1/accounts", Some(&t)).await.0, 200);
        // Username is case-insensitive (and trimmed); the password is not.
        assert_eq!(login(&f, " FRIEND ", &pw(&f)).await.0, 200);
        assert_eq!(login(&f, "friend", &pw(&f).to_lowercase()).await.0, 401);
        f.ctx.settings.lock().unwrap()["friend"]["username"] = json!("Bob");
        assert_eq!(login(&f, "bob", &pw(&f)).await.0, 200);
        assert_eq!(login(&f, "friend", &pw(&f)).await.0, 401);
    }

    #[tokio::test]
    async fn bad_login_401_then_429_after_ten() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let good = pw(&f);
        for (u, p) in [("nope", good.as_str()), ("friend", "wrong")] {
            let (s, b) = login(&f, u, p).await;
            assert_eq!((s, &b["error"]["code"]), (401, &json!("bad_login")));
            assert!(b.get("token").is_none());
        }
        for _ in 0..8 {
            assert_eq!(login(&f, "friend", "wrong").await.0, 401);
        }
        // 11th try in the window: 429 even with the right password.
        let (s, b) = login(&f, "friend", &good).await;
        assert_eq!((s, &b["error"]["code"]), (429, &json!("too_many")));
        // Window frees: right password works again (cleared, not back-dated: see above).
        f.ctx.fails.lock().unwrap().clear();
        assert_eq!(login(&f, "friend", &good).await.0, 200);
        // Malformed body: 400, not counted.
        let (s, _) = call_body(&f, "POST", "/login", None, Some("{}")).await;
        assert_eq!(s, 400);
        assert!(f.ctx.fails.lock().unwrap()["-"].is_empty());
    }

    async fn login_from(f: &Friend, ip: Option<&str>, pw: &str) -> u16 {
        let body = json!({ "username": "friend", "password": pw }).to_string();
        let mut req = Request::builder()
            .method("POST")
            .uri("/login")
            .header("host", "127.0.0.1:0")
            .header("content-type", "application/json");
        if let Some(ip) = ip {
            req = req.header("x-forwarded-for", ip);
        }
        let req = req.body(Body::from(body)).unwrap();
        router(f.ctx.clone())
            .oneshot(req)
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    #[tokio::test]
    async fn login_guard_is_per_client_ip() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let good = pw(&f);
        // A caller-supplied first value rotates; the proxy's last value keys the guard.
        for i in 0..MAX_FAILS {
            let xff = format!(" 7.7.7.{i} , 1.1.1.1 ");
            assert_eq!(login_from(&f, Some(&xff), "x").await, 401);
        }
        assert_eq!(login_from(&f, Some("1.1.1.1"), &good).await, 429);
        // Another IP and the no-header fallback are not locked out.
        assert_eq!(login_from(&f, Some("2.2.2.2"), &good).await, 200);
        assert_eq!(login_from(&f, None, &good).await, 200);
        for _ in 0..MAX_FAILS {
            assert_eq!(login_from(&f, None, "x").await, 401);
        }
        assert_eq!(login_from(&f, None, &good).await, 429);
        assert_eq!(login_from(&f, Some("2.2.2.2"), &good).await, 200);
        // Bounded map: oldest client dropped past the cap.
        for i in 0..MAX_FAIL_KEYS + 5 {
            login_from(&f, Some(&format!("9.9.{}.{}", i / 256, i % 256)), "x").await;
        }
        assert!(f.ctx.fails.lock().unwrap().len() <= MAX_FAIL_KEYS);
    }

    #[tokio::test]
    async fn new_password_signs_friend_out() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        f.ensure_token(d.path(), fake_os);
        let old_pw = f.password().unwrap();
        let old = format!(
            "Bearer {}",
            login(&f, "friend", &old_pw).await.1["token"]
                .as_str()
                .unwrap()
        );
        assert_eq!(call(&f, "GET", "/v1/accounts", Some(&old)).await.0, 200);
        let new = f.new_password(d.path(), fake_os);
        assert!(valid_password(&new) && new != old_pw);
        assert_eq!(f.password().unwrap(), new);
        assert_eq!(call(&f, "GET", "/v1/accounts", Some(&old)).await.0, 401);
        assert_eq!(login(&f, "friend", &old_pw).await.0, 401);
        let (s, b) = login(&f, "friend", &new).await;
        assert_eq!(s, 200);
        let nb = format!("Bearer {}", b["token"].as_str().unwrap());
        assert_eq!(call(&f, "GET", "/v1/accounts", Some(&nb)).await.0, 200);
        assert!(!crate::logging::redact(&new).contains(&new));
        // Saved wrapped, and the next session loads the new ones.
        for (file, secret) in [(PASSWORD_FILE, new.clone()), (TOKEN_FILE, f.token())] {
            let raw = std::fs::read(d.path().join(file)).unwrap();
            let n = secret.len();
            assert!(!raw.windows(n).any(|w| w == secret.as_bytes()), "{file}");
        }
        let d2 = tempfile::tempdir().unwrap();
        let (_h2, g) = setup(d2.path(), Box::new(NoPlat));
        g.ensure_token(d.path(), fake_os);
        assert_eq!((g.password().unwrap(), g.token()), (new, f.token()));
    }

    #[test]
    fn password_shape() {
        for _ in 0..50 {
            let p = random_password();
            assert!(valid_password(&p), "{p}");
            assert!(!p.contains(['I', 'l', 'O', 'o', '0', '1']));
        }
        assert_eq!(PW_CHARS.len(), 56);
    }

    #[test]
    fn friend_link_format() {
        assert_eq!(
            link_of("https://x.ts.net:8443/").unwrap(),
            "https://x.ts.net:8443"
        );
        assert_eq!(link_of(" https://x.ts.net ").unwrap(), "https://x.ts.net");
        assert!(link_of("  ").is_err());
        assert_eq!(
            link_of("http://x.ts.net").unwrap_err(),
            "Public address must start with https://"
        );
        assert!(link_of("HTTPS://x.ts.net").is_ok());
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let l = f.link().unwrap();
        assert!(!l.contains('#') && !l.contains(&f.token()) && l == "https://x.ts.net");
    }

    #[test]
    fn password_waits_for_the_saved_one() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        assert!(f.password().is_err(), "not loaded yet");
        f.ensure_token(d.path(), || Err("cancelled".into()));
        assert!(
            f.password().is_err(),
            "keyring failed: memory-only password stays private"
        );
        f.ensure_token(d.path(), fake_os);
        assert_eq!(f.password().unwrap(), pw(&f));
    }

    #[test]
    fn owner_password_waits_for_the_saved_one() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        assert!(f.owner_password().is_err(), "not loaded yet");
        f.ensure_token(d.path(), fake_os);
        let own = f.owner_password().unwrap();
        assert_eq!(own, read_secret(&f.ctx.owner_password));
        assert_ne!(own, f.password().unwrap());
    }

    #[test]
    fn secret_files_replaced_only_when_missing_or_bad() {
        for file in [TOKEN_FILE, PASSWORD_FILE] {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join(file);
            // Unreadable (a directory): left alone and not marked loaded.
            std::fs::create_dir(&p).unwrap();
            let (_h, f) = setup(d.path(), Box::new(NoPlat));
            f.ensure_token(d.path(), fake_os);
            assert!(p.is_dir() && f.password().is_err(), "{file}");
            // Garbage file: replaced with the current secret.
            std::fs::remove_dir(&p).unwrap();
            std::fs::write(&p, b"junk").unwrap();
            f.ensure_token(d.path(), fake_os);
            assert!(f.password().is_ok(), "{file}");
            assert_ne!(std::fs::read(&p).unwrap(), b"junk");
        }
    }

    const COOKIE: &str = "_|WARNING:-DO-NOT-SHARE-THIS.--Sharing-this-will-allow-someone-to-log-in-as-you-and-to-steal-your-ROBUX-and-items.|_FRIENDADDTEST";

    /// Fake Roblox Quick Log In: `status` is what login/status answers; login signs in as
    /// `user` (id, name).
    #[derive(Clone)]
    struct Rbx {
        status: Arc<Mutex<&'static str>>,
        user: Arc<Mutex<(u64, &'static str)>>,
        creates: Arc<Mutex<usize>>,
        /// /v2/login calls (counted before waiting on `hold`).
        logins: Arc<Mutex<usize>>,
        /// /v2/logout calls that carried the new session cookie.
        logouts: Arc<Mutex<usize>>,
        /// /v2/login waits for a read lock: a test holding the write lock blocks sign-in.
        hold: Arc<tokio::sync::RwLock<()>>,
        /// users/authenticated answers 500.
        auth_fail: Arc<AtomicBool>,
    }

    /// The fake's private key: long and unique, so a leak is unmistakable.
    const KEY: &str = "PRIVATE-KEY-MARKER-8e41c07d2b9f53a6";

    async fn fake_roblox() -> (String, Rbx) {
        use axum::http::HeaderMap;
        let r = Rbx {
            status: Arc::new(Mutex::new("Created")),
            user: Arc::new(Mutex::new((50, "Kid"))),
            creates: Arc::default(),
            logins: Arc::default(),
            logouts: Arc::default(),
            hold: Arc::default(),
            auth_fail: Arc::default(),
        };
        let (a, b, cr, li, lo) = (r.clone(), r.clone(), r.clone(), r.clone(), r.clone());
        let app = Router::new()
            .route(
                "/auth-token-service/v1/login/create",
                post(move || async move {
                    *cr.creates.lock().unwrap() += 1;
                    let n = *cr.creates.lock().unwrap();
                    Json(json!({ "code": format!("CODE{n:02}"), "status": "Created",
                        "privateKey": KEY, "expirationTime": "2099-01-01T00:00:00.000Z" }))
                }),
            )
            .route(
                "/auth-token-service/v1/login/status",
                post(move || async move {
                    let s = *a.status.lock().unwrap();
                    Json(json!({ "status": s, "accountName": "Kid" }))
                }),
            )
            .route(
                "/auth-token-service/v1/login/cancel",
                post(|| async { Json(json!({})) }),
            )
            .route(
                "/v2/login",
                post(move || async move {
                    *li.logins.lock().unwrap() += 1;
                    let _wait = li.hold.read().await;
                    (
                        [("set-cookie", format!(".ROBLOSECURITY={COOKIE}; HttpOnly"))],
                        Json(json!({})),
                    )
                }),
            )
            .route(
                "/v2/logout",
                post(move |h: HeaderMap| async move {
                    let ok = h
                        .get("cookie")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v.contains(COOKIE));
                    *lo.logouts.lock().unwrap() += usize::from(ok);
                    Json(json!({}))
                }),
            )
            .route(
                "/v1/users/authenticated",
                get(move |h: HeaderMap| async move {
                    let ok = h
                        .get("cookie")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v.contains(COOKIE));
                    if !ok {
                        return axum::http::StatusCode::UNAUTHORIZED.into_response();
                    }
                    if b.auth_fail.load(Ordering::SeqCst) {
                        return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                    let (id, name) = *b.user.lock().unwrap();
                    Json(json!({ "id": id, "name": name, "displayName": name })).into_response()
                }),
            );
        (crate::roblox::mock_url(app).await, r)
    }

    /// `setup` with Roblox faked and adding on.
    async fn setup_add(dir: &std::path::Path) -> (TH, Friend, Rbx) {
        let (url, r) = fake_roblox().await;
        let h = crate::automation::backend::tests::host_at(dir, Box::new(NoPlat), &url);
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[0]
            .friend_allowed = true;
        let b = AppBackend::default();
        b.set_host(h.clone());
        let mut s = crate::settings::defaults();
        s["friend"] =
            json!({ "enabled": true, "port": 0, "publicUrl": "https://x.ts.net/", "canAdd": true });
        (h, Friend::new(Arc::new(Mutex::new(s)), Arc::new(b)), r)
    }

    /// Status now, skipping the 2 s Roblox throttle.
    async fn add_poll(f: &Friend) -> (u16, Value) {
        if let Some(p) = f.ctx.add_slot().as_mut() {
            p.checked = None;
        }
        call(f, "GET", "/v1/add/status", Some(&bearer(f))).await
    }

    fn accounts(h: &TH) -> Vec<crate::accounts::Account> {
        h.0.vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .payload()
            .accounts
            .clone()
    }

    fn cookie_of(h: &TH, uid: u64) -> Option<String> {
        let g = h.0.vault.lock().unwrap();
        let p = g.as_ref().unwrap().payload();
        let a = p.accounts.iter().find(|a| a.user_id == Some(uid))?;
        p.secrets.get(&a.id).map(|s| s.cookie.to_string())
    }

    /// Poll until the status leaves "linked" (the background finish).
    async fn settle_add(f: &Friend, bodies: &mut Vec<String>) -> Value {
        for _ in 0..200 {
            let (_, b) = add_poll(f).await;
            bodies.push(b.to_string());
            if b["status"] != "linked" {
                return b;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("add never finished");
    }

    #[tokio::test]
    async fn add_off_is_403_and_info_says_so() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let t = bearer(&f);
        let (s, b) = call(&f, "GET", "/v1/add", Some(&t)).await;
        assert_eq!(
            (s, b),
            (200, json!({ "allowed": false, "max": 10, "added": 0 }))
        );
        for (m, p) in [("POST", "/v1/add/start"), ("GET", "/v1/add/status")] {
            let (s, b) = call(&f, m, p, Some(&t)).await;
            assert_eq!((s, &b["error"]["code"]), (403, &json!("add_off")), "{p}");
            assert_eq!(
                b["error"]["message"],
                "The owner has not turned on adding accounts."
            );
        }
    }

    #[tokio::test]
    async fn add_start_reuses_code_and_limits_starts() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f, r) = setup_add(d.path()).await;
        let t = bearer(&f);
        let (s, a) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
        assert_eq!(s, 200, "{a}");
        assert_eq!(a["code"], "CODE01");
        assert!((290..=300).contains(&a["expiresInSec"].as_u64().unwrap()));
        // Reopening the sheet hands out the same live code and uses no start.
        for _ in 0..MAX_STARTS + 2 {
            let (s, b) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
            assert_eq!((s, &b["code"]), (200, &json!("CODE01")));
        }
        assert_eq!(*r.creates.lock().unwrap(), 1);
        assert_eq!(f.ctx.add_starts.lock().unwrap().len(), 1);
        // Each expired code: a new one, until the window is full.
        let expire = || f.ctx.add_slot().as_mut().unwrap().until = Instant::now();
        for n in 2..=MAX_STARTS {
            expire();
            let (s, b) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
            assert_eq!((s, &b["code"]), (200, &json!(format!("CODE{n:02}"))));
        }
        expire();
        let (s, b) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
        assert_eq!((s, &b["error"]["code"]), (429, &json!("slow_down")));
        assert_eq!(*r.creates.lock().unwrap(), MAX_STARTS);
        // Window over: a new code again. Cleared, not back-dated: `Instant::now() -
        // START_WINDOW` panics on a machine booted less than 10 minutes ago.
        f.ctx.add_starts.lock().unwrap().clear();
        let (s, b) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
        assert_eq!((s, &b["code"]), (200, &json!("CODE06")));
    }

    #[tokio::test]
    async fn add_waiting_linked_done_adds_one_shared_account() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        let t = bearer(&f);
        let mut bodies = vec![];
        let (_, b) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
        bodies.push(b.to_string());
        let (s, b) = add_poll(&f).await;
        assert_eq!((s, &b["status"]), (200, &json!("waiting")));
        *r.status.lock().unwrap() = "UserLinked";
        let (_, b) = add_poll(&f).await;
        assert_eq!(
            (&b["status"], &b["accountName"]),
            (&json!("waiting"), &json!("Kid"))
        );
        // Throttled: no Roblox call, same answer.
        let (_, b) = call(&f, "GET", "/v1/add/status", Some(&t)).await;
        assert_eq!(b["status"], "waiting");
        *r.status.lock().unwrap() = "Validated";
        let (_, b) = add_poll(&f).await;
        bodies.push(b.to_string());
        assert_eq!(b["status"], "linked");
        let b = settle_add(&f, &mut bodies).await;
        assert_eq!(b, json!({ "status": "done", "accountName": "Kid" }));
        let accs = accounts(&h);
        assert_eq!(accs.len(), 3);
        let kid = accs.iter().find(|a| a.user_id == Some(50)).unwrap();
        assert!(kid.friend_allowed && kid.friend_added && kid.username == "Kid");
        assert_eq!(cookie_of(&h, 50).as_deref(), Some(COOKIE));
        let ev = h.1.lock().unwrap().clone();
        assert!(ev.contains(&"accounts://changed".into()) && ev.contains(&"friend_action".into()));
        let (_, b) = call(&f, "GET", "/v1/add", Some(&t)).await;
        assert_eq!(b, json!({ "allowed": true, "max": 10, "added": 1 }));
        let (_, l) = call(&f, "GET", "/v1/accounts", Some(&t)).await;
        bodies.push(l.to_string());
        assert_eq!(l.as_array().unwrap().len(), 2, "now shared with the friend");
        // Done stays done; the next start is a new code.
        assert_eq!(add_poll(&f).await.1["status"], "done");
        assert_eq!(accounts(&h).len(), 3);
        let (_, b) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
        assert_eq!(b["code"], "CODE02");
        for b in bodies {
            assert!(!b.contains("_|WARNING") && !b.contains(KEY), "{b}");
        }
    }

    #[tokio::test]
    async fn add_refuses_owner_only_account_and_refreshes_shared_one() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        let t = bearer(&f);
        *r.user.lock().unwrap() = (2, "bob"); // in the vault, not shared
        let before = accounts(&h);
        *r.status.lock().unwrap() = "Validated";
        call(&f, "POST", "/v1/add/start", Some(&t)).await;
        let b = settle_add(&f, &mut vec![]).await;
        assert_eq!(b["status"], "failed");
        assert_eq!(b["message"], "This account can't be added. Ask the owner.");
        assert_eq!(accounts(&h), before);
        assert_eq!(cookie_of(&h, 2), None);
        // The refused session was signed out on Roblox.
        assert_eq!(*r.logouts.lock().unwrap(), 1);
        // Alice is shared: her session is refreshed, she is not marked friend-added.
        *r.user.lock().unwrap() = (1, "Alice");
        call(&f, "POST", "/v1/add/start", Some(&t)).await;
        assert_eq!(settle_add(&f, &mut vec![]).await["status"], "done");
        assert_eq!(cookie_of(&h, 1).as_deref(), Some(COOKIE));
        let accs = accounts(&h);
        assert_eq!(accs.len(), 2);
        assert!(accs[0].friend_allowed && !accs[0].friend_added);
        assert_eq!(*r.logouts.lock().unwrap(), 1, "refreshed: not signed out");
    }

    #[tokio::test]
    async fn add_full_at_ten() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, _r) = setup_add(d.path()).await;
        {
            let mut g = h.0.vault.lock().unwrap();
            let p = g.as_mut().unwrap().payload_mut();
            for i in 0..MAX_ADDED as u64 {
                p.accounts.push(crate::accounts::Account {
                    user_id: Some(100 + i),
                    friend_added: true,
                    friend_allowed: true,
                    ..Default::default()
                });
            }
        }
        let t = bearer(&f);
        let (s, b) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
        assert_eq!((s, &b["error"]["code"]), (409, &json!("add_full")));
        assert_eq!(call(&f, "GET", "/v1/add", Some(&t)).await.1["added"], 10);
        // The vault rule holds even if the cap is reached while a code is pending.
        let mut p =
            h.0.vault
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .payload()
                .clone();
        let who = crate::accounts::Identity {
            user_id: 50,
            username: "Kid".into(),
            display_name: "Kid".into(),
        };
        let e =
            crate::accounts::friend_upsert(&mut p, &who, Zeroizing::new(COOKIE.into()), MAX_ADDED);
        assert!(
            matches!(e, Err(crate::accounts::FriendUpsertError::Refused(m)) if m.contains("at most 10"))
        );
    }

    #[tokio::test]
    async fn add_locked_is_503() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, _r) = setup_add(d.path()).await;
        *h.0.vault.lock().unwrap() = None;
        let t = bearer(&f);
        for (m, p) in [("GET", "/v1/add"), ("POST", "/v1/add/start")] {
            let (s, b) = call(&f, m, p, Some(&t)).await;
            assert_eq!((s, &b["error"]["code"]), (503, &json!("locked")), "{p}");
        }
    }

    #[tokio::test]
    async fn add_dropped_by_new_password_canadd_off_or_expiry() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f, _r) = setup_add(d.path()).await;
        call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        f.new_password(d.path(), fake_os);
        assert!(f.ctx.add_slot().is_none());
        let (s, b) = add_poll(&f).await;
        assert_eq!((s, &b["status"]), (200, &json!("expired")));
        // Adding turned off: the pending code goes with it.
        call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        f.ctx.settings.lock().unwrap()["friend"]["canAdd"] = json!(false);
        f.sync().await.unwrap();
        assert!(f.ctx.add_slot().is_none());
        f.ctx.port.store(0, Ordering::Relaxed); // sync bound a real port; tests call host :0
        f.ctx.settings.lock().unwrap()["friend"]["canAdd"] = json!(true);
        // Code past its time: expired, and cancel answers {} with nothing pending.
        call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        f.ctx.add_slot().as_mut().unwrap().until = Instant::now();
        assert_eq!(add_poll(&f).await.1["status"], "expired");
        let (s, b) = call(&f, "POST", "/v1/add/cancel", Some(&bearer(&f))).await;
        assert_eq!((s, b), (200, json!({})));
        // Cancel while waiting: cancelled.
        call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        call(&f, "POST", "/v1/add/cancel", Some(&bearer(&f))).await;
        assert_eq!(add_poll(&f).await.1["status"], "cancelled");
        f.sync().await.unwrap();
    }

    async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        for _ in 0..300 {
            if ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn slot_state(f: &Friend) -> Option<Value> {
        f.ctx.add_slot().as_ref().map(|p| add_json(&p.state))
    }

    /// Start, confirm, and park the background sign-in inside /v2/login (until the guard drops).
    async fn hold_in_login(f: &Friend, r: &Rbx) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        let hold = r.hold.clone().write_owned().await;
        *r.status.lock().unwrap() = "Validated";
        call(f, "POST", "/v1/add/start", Some(&bearer(f))).await;
        assert_eq!(add_poll(f).await.1["status"], "linked");
        wait_for("login", || *r.logins.lock().unwrap() == 1).await;
        hold
    }

    #[tokio::test]
    async fn add_gate_rechecked_before_vault_write() {
        for case in ["canAdd off", "new password", "remote off"] {
            let d = tempfile::tempdir().unwrap();
            let (h, f, r) = setup_add(d.path()).await;
            let before = accounts(&h);
            let hold = hold_in_login(&f, &r).await;
            match case {
                "canAdd off" => f.ctx.settings.lock().unwrap()["friend"]["canAdd"] = json!(false),
                "remote off" => f.ctx.settings.lock().unwrap()["friend"]["enabled"] = json!(false),
                _ => drop(f.new_password(d.path(), fake_os)),
            }
            drop(hold);
            wait_for("sign-out", || *r.logouts.lock().unwrap() == 1).await;
            wait_for("outcome", || {
                slot_state(&f).is_none_or(|s| s["status"] != "linked")
            })
            .await;
            assert_eq!(accounts(&h), before, "{case}");
            assert_eq!(cookie_of(&h, 50), None, "{case}");
            match slot_state(&f) {
                None => assert_eq!(case, "new password"),
                Some(s) => assert_eq!(
                    s,
                    json!({ "status": "failed", "message": "The owner has not turned on adding accounts." }),
                    "{case}"
                ),
            }
        }
    }

    #[tokio::test]
    async fn add_gate_checked_before_sign_in() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        let before = accounts(&h);
        let ok: crate::automation::AddGate = Arc::new(|| false);
        let e = f
            .ctx
            .backend
            .friend_add_finish("CODE01", &crate::roblox::Secret::new(KEY), ok)
            .await
            .unwrap_err();
        assert_eq!(e.code, "add_off");
        assert_eq!(*r.logins.lock().unwrap(), 0);
        assert_eq!(accounts(&h), before);
    }

    #[tokio::test]
    async fn add_linked_expires_and_drop_guard_fails_it() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f, _r) = setup_add(d.path()).await;
        let t = bearer(&f);
        call(&f, "POST", "/v1/add/start", Some(&t)).await;
        let set_linked = |until: Instant| {
            let mut g = f.ctx.add_slot();
            let p = g.as_mut().unwrap();
            p.state = AddState::Linked(None);
            p.until = until;
        };
        // Within the deadline: start hands the same code back.
        set_linked(Instant::now() + LINK_TTL);
        assert_eq!(
            call(&f, "POST", "/v1/add/start", Some(&t)).await.1["code"],
            "CODE01"
        );
        // Past it: status says failed, and start makes a new code.
        set_linked(Instant::now());
        let (_, b) = call(&f, "GET", "/v1/add/status", Some(&t)).await;
        assert_eq!(b, json!({ "status": "failed", "message": ADD_STUCK }));
        set_linked(Instant::now());
        assert_eq!(
            call(&f, "POST", "/v1/add/start", Some(&t)).await.1["code"],
            "CODE02"
        );
        // A finish that ends without an outcome (panic, dropped task) leaves it failed.
        set_linked(Instant::now() + LINK_TTL);
        drop(LinkedGuard(f.ctx.clone(), "CODE02".into()));
        assert_eq!(slot_state(&f).unwrap()["status"], "failed");
        // A guard for another code leaves the slot alone.
        set_linked(Instant::now() + LINK_TTL);
        drop(LinkedGuard(f.ctx.clone(), "OTHER".into()));
        assert_eq!(slot_state(&f).unwrap()["status"], "linked");
    }

    #[tokio::test]
    async fn add_vault_error_is_generic_and_session_signed_out() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        // Saving fails: the vault file's place is taken by a directory.
        let path = h.0.paths.vault();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        *r.status.lock().unwrap() = "Validated";
        call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        let b = settle_add(&f, &mut vec![]).await;
        assert_eq!(
            b,
            json!({ "status": "failed", "message": "Something went wrong on the owner's computer." })
        );
        assert!(accounts(&h).iter().all(|a| a.user_id != Some(50)));
        assert_eq!(*r.logouts.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn add_concurrent_status_signs_in_once() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        *r.status.lock().unwrap() = "Validated";
        let all = tokio::join!(
            add_poll(&f),
            add_poll(&f),
            add_poll(&f),
            add_poll(&f),
            add_poll(&f),
            add_poll(&f)
        );
        assert_eq!(all.0 .0, 200);
        assert_eq!(settle_add(&f, &mut vec![]).await["status"], "done");
        assert_eq!(*r.logins.lock().unwrap(), 1);
        assert_eq!(accounts(&h).len(), 3);
    }

    #[tokio::test]
    async fn readd_shared_keeps_owner_settings() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        {
            let mut g = h.0.vault.lock().unwrap();
            let a = &mut g.as_mut().unwrap().payload_mut().accounts[0];
            a.friend_paused = true;
            a.friend_last_place = Some(9);
            a.group = "Team".into();
            a.launch_target = Some(crate::accounts::LaunchTarget {
                place_id: Some(5),
                job_id: None,
                private_link: None,
                mode: None,
            });
        }
        let before = accounts(&h)[0].clone();
        *r.user.lock().unwrap() = (1, "Alice");
        *r.status.lock().unwrap() = "Validated";
        call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        assert_eq!(settle_add(&f, &mut vec![]).await["status"], "done");
        let a = accounts(&h)[0].clone();
        assert_eq!(
            (
                a.friend_paused,
                a.friend_last_place,
                &a.group,
                &a.launch_target
            ),
            (true, Some(9), &"Team".to_owned(), &before.launch_target)
        );
        assert!(a.friend_allowed && !a.friend_added);
        assert_eq!(*r.logouts.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn unshared_friend_added_account_refused_and_counted() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        let push = |uid: u64, allowed: bool| {
            let mut g = h.0.vault.lock().unwrap();
            g.as_mut()
                .unwrap()
                .payload_mut()
                .accounts
                .push(crate::accounts::Account {
                    user_id: Some(uid),
                    username: format!("u{uid}"),
                    friend_added: true,
                    friend_allowed: allowed,
                    ..Default::default()
                });
        };
        push(60, false); // the owner un-shared it
        *r.user.lock().unwrap() = (60, "u60");
        *r.status.lock().unwrap() = "Validated";
        let t = bearer(&f);
        call(&f, "POST", "/v1/add/start", Some(&t)).await;
        let b = settle_add(&f, &mut vec![]).await;
        assert_eq!(b["message"], "This account can't be added. Ask the owner.");
        assert_eq!(cookie_of(&h, 60), None);
        for uid in 61..61 + MAX_ADDED as u64 - 1 {
            push(uid, true);
        }
        assert_eq!(call(&f, "GET", "/v1/add", Some(&t)).await.1["added"], 10);
        let (s, b) = call(&f, "POST", "/v1/add/start", Some(&t)).await;
        assert_eq!((s, &b["error"]["code"]), (409, &json!("add_full")));
    }

    #[tokio::test]
    async fn add_cancel_off_is_403() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        let (s, b) = call(&f, "POST", "/v1/add/cancel", Some(&bearer(&f))).await;
        assert_eq!((s, &b["error"]["code"]), (403, &json!("add_off")));
    }

    static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Cap;
    impl log::Log for Cap {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, r: &log::Record) {
            LOGS.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(r.args().to_string());
        }
        fn flush(&self) {}
    }

    /// Every log line from here on (raw, before the app's redaction) lands in `LOGS`.
    fn capture_logs() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            log::set_logger(&Cap).expect("no other test logger");
            log::set_max_level(log::LevelFilter::Debug);
        });
    }

    #[tokio::test]
    async fn add_never_logs_cookie_or_private_key() {
        capture_logs();
        // Success, Roblox error after sign-in, vault error.
        for case in ["ok", "roblox", "vault"] {
            let d = tempfile::tempdir().unwrap();
            let (h, f, r) = setup_add(d.path()).await;
            match case {
                "roblox" => r.auth_fail.store(true, Ordering::SeqCst),
                "vault" => {
                    let path = h.0.paths.vault();
                    std::fs::remove_file(&path).unwrap();
                    std::fs::create_dir(&path).unwrap();
                }
                _ => {}
            }
            *r.status.lock().unwrap() = "Validated";
            call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
            let b = settle_add(&f, &mut vec![]).await;
            assert_eq!(
                b["status"],
                if case == "ok" { "done" } else { "failed" },
                "{case}"
            );
        }
        let logs = LOGS.lock().unwrap().join("\n");
        assert!(logs.contains("friend: add Kid"), "logger captured nothing");
        assert!(logs.contains("internal error"), "vault error was logged");
        assert!(
            !logs.contains("FRIENDADDTEST") && !logs.contains(KEY),
            "{logs}"
        );
    }

    #[test]
    fn add_ttl_is_the_sooner_one() {
        assert_eq!(add_ttl(None), ADD_TTL);
        assert_eq!(add_ttl(Some("junk")), ADD_TTL);
        assert_eq!(add_ttl(Some("2099-01-01T00:00:00.000Z")), ADD_TTL);
        assert_eq!(add_ttl(Some("2000-01-01T00:00:00Z")), Duration::ZERO);
        let soon = (chrono::Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
        assert!((55..=60).contains(&add_ttl(Some(&soon)).as_secs()));
    }

    #[tokio::test]
    async fn server_binds_loopback_and_follows_settings() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        f.sync().await.unwrap();
        let s = f.status().await;
        assert!(s.running && s.port != 0 && s.public_url == "https://x.ts.net/");
        assert_eq!(
            f.running.lock().await.as_ref().unwrap().addr.ip(),
            Ipv4Addr::LOCALHOST
        );
        f.ctx.settings.lock().unwrap()["friend"]["enabled"] = json!(false);
        f.sync().await.unwrap();
        assert!(!f.status().await.running);
    }

    #[tokio::test]
    async fn port_taken_marks_the_funnel_entry_for_removal() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        f.sync().await.unwrap();
        assert_eq!(tailscale::orphan_port(&f.status().await), None, "running");
        let other = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = other.local_addr().unwrap().port();
        f.ctx.settings.lock().unwrap()["friend"]["port"] = json!(port);
        let e = f.sync().await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse, "{e}");
        assert!(e.to_string().contains(&format!("port {port}")), "{e}");
        assert_eq!(tailscale::orphan_port(&f.status().await), Some(port));
        // Not an RMI-made address: never ours to remove.
        f.ctx.settings.lock().unwrap()["friend"]["publicUrl"] = json!("https://friend.example.com");
        assert_eq!(tailscale::orphan_port(&f.status().await), None);
    }

    #[tokio::test]
    async fn owner_login_cannot_add_or_touch_the_friends_add() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        f.ctx.settings.lock().unwrap()["friend"]["ownerUsername"] = json!("Boss");
        let owner = format!("Bearer {}", read_secret(&f.ctx.owner_token));
        let (_, b) = call(&f, "GET", "/v1/add", Some(&owner)).await;
        assert_eq!(b, json!({ "allowed": false, "max": 10, "added": 0 }));
        // The friend's pending code is not shown to, cancelled or dropped by the owner.
        let (_, code) = call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        for (m, p) in [
            ("POST", "/v1/add/start"),
            ("GET", "/v1/add/status"),
            ("POST", "/v1/add/cancel"),
        ] {
            let (s, b) = call(&f, m, p, Some(&owner)).await;
            assert_eq!((s, &b["error"]["code"]), (403, &json!("add_off")), "{p}");
        }
        assert_eq!(slot_state(&f).unwrap()["status"], "waiting");
        let (_, again) = call(&f, "POST", "/v1/add/start", Some(&bearer(&f))).await;
        assert_eq!(again["code"], code["code"]);
        assert_eq!(*r.creates.lock().unwrap(), 1);
        assert_eq!(accounts(&h).len(), 2);
    }

    #[test]
    fn new_password_before_load_keeps_the_saved_owner_login() {
        let d = tempfile::tempdir().unwrap();
        let (_h, f) = setup(d.path(), Box::new(NoPlat));
        f.ensure_token(d.path(), fake_os);
        let owner = read_secret(&f.ctx.owner_password);
        // Next session: remote off at startup, so nothing loaded before New password.
        let d2 = tempfile::tempdir().unwrap();
        let (_h2, g) = setup(d2.path(), Box::new(NoPlat));
        g.new_password(d.path(), fake_os);
        assert_eq!(read_secret(&g.ctx.owner_password), owner);
        assert_eq!(
            read_secret(&g.ctx.owner_token),
            read_secret(&f.ctx.owner_token)
        );
    }

    #[tokio::test]
    async fn add_marked_stuck_is_not_stored_later() {
        let d = tempfile::tempdir().unwrap();
        let (h, f, r) = setup_add(d.path()).await;
        let before = accounts(&h);
        let hold = hold_in_login(&f, &r).await;
        // The sign-in outlives its deadline: the friend is told it failed.
        f.ctx.add_slot().as_mut().unwrap().until = Instant::now();
        let (_, b) = call(&f, "GET", "/v1/add/status", Some(&bearer(&f))).await;
        assert_eq!(b, json!({ "status": "failed", "message": ADD_STUCK }));
        drop(hold);
        wait_for("sign-out", || *r.logouts.lock().unwrap() == 1).await;
        assert_eq!(accounts(&h), before);
        assert_eq!(cookie_of(&h, 50), None);
        assert_eq!(
            slot_state(&f).unwrap(),
            json!({ "status": "failed", "message": ADD_STUCK })
        );
    }

    #[tokio::test]
    async fn disconnect_vault_error_is_generic() {
        let d = tempfile::tempdir().unwrap();
        let (h, f) = setup(d.path(), Box::new(NoPlat));
        // Saving fails: the vault file's place is taken by a directory.
        let path = h.0.paths.vault();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let (s, b) = call(&f, "POST", "/v1/accounts/A/disconnect", Some(&bearer(&f))).await;
        assert_eq!((s, &b["error"]["code"]), (500, &json!("internal")));
        assert_eq!(
            b["error"]["message"],
            "Something went wrong on the owner's computer."
        );
        assert!(!f.ctx.actions.lock().unwrap().contains_key("A"));
    }
}
