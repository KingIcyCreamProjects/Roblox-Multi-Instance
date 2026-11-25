//! WS control server (`/control?token=&account=&job=`) and the per-account hub (spec §9.1).

use super::protocol::Msg;
use super::Ctx;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

pub const CONSOLE_MAX: usize = 5000;
/// Lines per account in `snapshot()`; the full buffer stays in the hub.
pub const SNAPSHOT_TAIL: usize = 500;
/// Largest inbound frame/message.
pub const MESSAGE_MAX: usize = 64 * 1024;
// ponytail: worst case ~CONSOLE_MAX * TEXT_MAX * ACCOUNTS_MAX (~1 GB) from a token holder; lower caps if that bites.
const TEXT_MAX: usize = 2 * 1024;
const NAME_MAX: usize = 64;
const CONTROLS_MAX: usize = 200;
const ACCOUNTS_MAX: usize = 100;
const CONNS_MAX: usize = 64;
/// Outbound frames queued per client before it counts as too slow and is dropped (1013).
const QUEUE_MAX: usize = 256;
/// At most one `changed` per account per this window.
const COALESCE: Duration = Duration::from_millis(100);
const KINDS: &[&str] = &["button", "text", "label", "number", "break"];
const CONTROL_FIELDS: &[&str] = &["kind", "name", "value", "min", "max", "step", "decimals"];
const RELAUNCH_FIELDS: &[&str] = &["enabled", "seconds", "placeId", "jobId"];

/// Close codes.
const UNAUTHORIZED: u16 = 4401;
const TRY_AGAIN: u16 = 1013;
const GOING_AWAY: u16 = 1001;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub ts: i64,
    pub level: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountState {
    pub account: String,
    pub connected: bool,
    pub user_id: Option<Value>,
    pub place_id: Option<Value>,
    pub job_id: Option<Value>,
    pub console: VecDeque<Line>,
    /// `ui.create` payloads (whitelisted fields); `value` is kept current by UI events.
    pub controls: Vec<Value>,
    pub relaunch: Value,
    /// Hub counter at its last connect/disconnect (eviction order).
    #[serde(skip)]
    seen: u64,
}

struct Conn {
    key: String,
    tx: mpsc::Sender<Msg>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    conns: HashMap<u64, Conn>,
    /// Dropped for being too slow: id -> account key.
    kicked: HashMap<u64, String>,
    accounts: BTreeMap<String, AccountState>,
    /// For the relaunch loop (keys, deduped): left with no client by a `hello` re-key, and
    /// relaunch turned off → on.
    moved: Vec<String>,
    reenabled: Vec<String>,
}

fn push_once(v: &mut Vec<String>, k: &str) {
    if !v.iter().any(|x| x == k) {
        v.push(k.to_owned());
    }
}

pub struct Hub {
    inner: Mutex<Inner>,
    dirty: Mutex<BTreeSet<String>>,
    wake: Notify,
    secret_tx: mpsc::UnboundedSender<String>,
    secret_rx: Mutex<Option<mpsc::UnboundedReceiver<String>>>,
}

impl Default for Hub {
    fn default() -> Self {
        let (secret_tx, rx) = mpsc::unbounded_channel();
        Hub {
            inner: Mutex::default(),
            dirty: Mutex::default(),
            wake: Notify::new(),
            secret_tx,
            secret_rx: Mutex::new(Some(rx)),
        }
    }
}

fn key(account: &str) -> String {
    // Clipped like the stored display name, so the UI's (clipped) name finds the entry.
    let a: String = account.trim().chars().take(NAME_MAX).collect();
    if a.is_empty() { "unknown" } else { &a }.to_lowercase()
}

fn clip(s: &str) -> String {
    s.chars().take(TEXT_MAX).collect()
}

/// Bool/number as-is, strings clipped; anything else is dropped.
fn scalar(v: &Value) -> Option<Value> {
    match v {
        Value::Bool(_) | Value::Number(_) => Some(v.clone()),
        Value::String(s) => Some(Value::String(clip(s))),
        _ => None,
    }
}

fn pick(d: &Value, fields: &[&str]) -> Map<String, Value> {
    fields
        .iter()
        .filter_map(|f| scalar(&d[*f]).map(|v| (f.to_string(), v)))
        .collect()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Inner {
    /// Get or create; at `ACCOUNTS_MAX` the longest-disconnected account without
    /// auto-relaunch on is evicted first.
    fn account(&mut self, account: &str) -> Option<&mut AccountState> {
        let k = key(account);
        if !self.accounts.contains_key(&k) && self.accounts.len() >= ACCOUNTS_MAX {
            let old = self
                .accounts
                .iter()
                .filter(|(_, a)| !a.connected && a.relaunch["enabled"] != true)
                .min_by_key(|(_, a)| a.seen)
                .map(|(k, _)| k.clone())?;
            self.accounts.remove(&old);
        }
        let a = self.accounts.entry(k).or_default();
        if a.account.is_empty() {
            a.account = account.trim().chars().take(NAME_MAX).collect();
            if a.account.is_empty() {
                a.account = "unknown".into();
            }
        }
        Some(a)
    }
    fn refresh_connected(&mut self, k: &str) {
        let on = self.conns.values().any(|c| c.key == k);
        self.next += 1;
        let n = self.next;
        if let Some(a) = self.accounts.get_mut(k) {
            a.connected = on;
            a.seen = n;
        }
    }
    fn push_line(&mut self, k: &str, level: &str, text: &str) {
        if let Some(a) = self.accounts.get_mut(k) {
            if a.console.len() >= CONSOLE_MAX {
                a.console.pop_front();
            }
            a.console.push_back(Line {
                ts: now_ms(),
                level: level.chars().take(16).collect(),
                text: clip(text),
            });
        }
    }
    /// Queue a frame; a full queue drops that client (it then closes with 1013).
    /// Returns the account key whose status changed, if any.
    fn deliver(&mut self, id: u64, m: &Msg) -> Result<(), Option<String>> {
        let Some(c) = self.conns.get(&id) else {
            return Err(None);
        };
        match c.tx.try_send(m.clone()) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(None),
            Err(mpsc::error::TrySendError::Full(_)) => {
                let c = self.conns.remove(&id).unwrap();
                log::warn!("automation: dropping slow client of {:?}", c.key);
                self.refresh_connected(&c.key);
                self.kicked.insert(id, c.key.clone());
                Err(Some(c.key))
            }
        }
    }
    /// Deliver to `ids`; returns (any delivered, keys whose status changed).
    fn deliver_all(&mut self, ids: Vec<u64>, m: &Msg) -> (bool, Vec<String>) {
        let (mut any, mut changed) = (false, vec![]);
        for id in ids {
            match self.deliver(id, m) {
                Ok(()) => any = true,
                Err(Some(k)) => changed.push(k),
                Err(None) => {}
            }
        }
        (any, changed)
    }
    fn ids_of(&self, k: &str) -> Vec<u64> {
        self.conns
            .iter()
            .filter(|(_, c)| c.key == k)
            .map(|(id, _)| *id)
            .collect()
    }
}

impl Hub {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self, k: &str) {
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(k.to_string());
        self.wake.notify_one();
    }

    /// Accounts that changed, coalesced: waits for a change, then `COALESCE`, then drains.
    pub async fn next_changes(&self) -> Vec<String> {
        self.wake.notified().await;
        tokio::time::sleep(COALESCE).await;
        let mut d = self.dirty.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *d).into_iter().collect()
    }

    /// Secret-read notices (account names). Separate, unbounded, so nothing can crowd them
    /// out. Taken once by the app.
    pub fn take_secret_events(&self) -> Option<mpsc::UnboundedReceiver<String>> {
        self.secret_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    pub fn notify_secret(&self, account: &str) {
        let _ = self.secret_tx.send(account.to_string());
    }

    /// `authorized` runs under the hub lock, so a concurrent token regenerate either
    /// rejects this client or disconnects it. Err = close code.
    fn register(
        &self,
        account: &str,
        job: Option<&String>,
        authorized: impl FnOnce() -> bool,
    ) -> Result<(u64, mpsc::Receiver<Msg>), u16> {
        let mut g = self.lock();
        if !authorized() {
            return Err(UNAUTHORIZED);
        }
        if g.conns.len() >= CONNS_MAX {
            return Err(TRY_AGAIN);
        }
        let a = g.account(account).ok_or(TRY_AGAIN)?;
        if let Some(j) = job {
            a.job_id = Some(Value::String(clip(j)));
        }
        let k = key(account);
        let (tx, rx) = mpsc::channel(QUEUE_MAX);
        g.next += 1;
        let id = g.next;
        g.conns.insert(id, Conn { key: k.clone(), tx });
        g.refresh_connected(&k);
        drop(g);
        self.changed(&k);
        Ok((id, rx))
    }

    /// Returns true if the client had been dropped for being slow.
    fn unregister(&self, id: u64) -> bool {
        let mut g = self.lock();
        if let Some(c) = g.conns.remove(&id) {
            g.refresh_connected(&c.key);
            drop(g);
            self.changed(&c.key);
            return false;
        }
        g.kicked.remove(&id).is_some()
    }

    /// Drop every client (stop / token regenerate). Their sockets close on the next poll.
    /// Every account is marked changed: the caller also reset the relaunch engine, which
    /// clears pending relaunches of disconnected accounts too.
    pub fn disconnect_all(&self) {
        let mut g = self.lock();
        let keys: Vec<String> = g.conns.drain().map(|(_, c)| c.key).collect();
        for k in &keys {
            g.refresh_connected(k);
        }
        drop(g);
        self.touch_all();
    }

    /// Apply one client frame (§9.1). Replies go back through the client's own queue.
    pub(crate) fn handle(&self, id: u64, m: Msg) {
        let mut g = self.lock();
        let Some(conn) = g.conns.get(&id) else { return };
        let mut k = conn.key.clone();
        let mut also = vec![];
        let d = &m.data;
        let s = |f: &str| d[f].as_str().unwrap_or_default();
        match m.t.as_str() {
            "hello" => {
                let name = s("account");
                if !name.trim().is_empty() && key(name) != k && g.account(name).is_some() {
                    let old = std::mem::replace(&mut k, key(name));
                    g.conns.get_mut(&id).unwrap().key = k.clone();
                    g.refresh_connected(&old);
                    g.refresh_connected(&k);
                    if g.accounts.get(&old).is_some_and(|a| !a.connected) {
                        push_once(&mut g.moved, &old);
                    }
                    also.push(old);
                }
                let a = g.accounts.get_mut(&k).unwrap();
                for (f, slot) in [
                    ("userId", &mut a.user_id),
                    ("placeId", &mut a.place_id),
                    ("jobId", &mut a.job_id),
                ] {
                    if let Some(v) = scalar(&d[f]) {
                        *slot = Some(v);
                    }
                }
            }
            "log" => {
                let level = d["level"].as_str().unwrap_or("info");
                g.push_line(&k, level, s("text"));
            }
            "echo" => {
                let from = g.accounts.get(&k).map(|a| a.account.clone());
                let out = Msg::new(
                    "message",
                    None,
                    json!({ "text": clip(s("text")), "from": from }),
                );
                let others = g.conns.keys().copied().filter(|c| *c != id).collect();
                also = g.deliver_all(others, &out).1;
                drop(g);
                also.iter().for_each(|k| self.changed(k));
                return;
            }
            "ui.create" => {
                let name = s("name");
                // Script authors see why in the console, not just a debug log.
                if name.is_empty() || name.chars().count() > NAME_MAX {
                    g.push_line(&k, "warn", "ui.create ignored: name missing or too long");
                } else if !KINDS.contains(&s("kind")) {
                    g.push_line(&k, "warn", "ui.create ignored: unknown kind");
                } else {
                    let c = Value::Object(pick(d, CONTROL_FIELDS));
                    let controls = &mut g.accounts.get_mut(&k).unwrap().controls;
                    match controls.iter().position(|x| x["name"] == name) {
                        Some(i) => controls[i] = c,
                        None if controls.len() < CONTROLS_MAX => controls.push(c),
                        None => g.push_line(&k, "warn", "ui.create ignored: too many controls"),
                    }
                }
            }
            "ui.get" => {
                let value = g.accounts[&k]
                    .controls
                    .iter()
                    .find(|c| c["name"] == d["name"])
                    .map_or(Value::Null, |c| c["value"].clone());
                let name = scalar(&d["name"]).unwrap_or(Value::Null);
                let reply = Msg::new("ui.value", m.id, json!({ "name": name, "value": value }));
                if let Err(Some(k)) = g.deliver(id, &reply) {
                    drop(g);
                    self.changed(&k);
                }
                return;
            }
            "relaunch.set" => {
                let patch = pick(d, RELAUNCH_FIELDS);
                let r = &mut g.accounts.get_mut(&k).unwrap().relaunch;
                let was = r["enabled"] == true;
                if !r.is_object() {
                    *r = json!({});
                }
                for (f, v) in patch {
                    r[f] = v;
                }
                if !was && r["enabled"] == true {
                    push_once(&mut g.reenabled, &k);
                }
            }
            "ping" => {
                if let Err(Some(k)) = g.deliver(id, &Msg::new("pong", m.id, json!({}))) {
                    drop(g);
                    self.changed(&k);
                }
                return;
            }
            other => {
                log::debug!("automation: unknown frame type {other:?} ignored");
                let t: String = other.chars().take(NAME_MAX).collect();
                g.push_line(&k, "warn", &format!("unknown frame type {t:?} ignored"));
            }
        }
        drop(g);
        self.changed(&k);
        also.iter().for_each(|k| self.changed(k));
    }

    /// Console "Send": deliver `message` to every client of `account`.
    pub fn send_message(&self, account: &str, text: &str) -> Result<(), String> {
        let k = key(account);
        let mut g = self.lock();
        let m = Msg::new("message", None, json!({ "text": clip(text) }));
        let ids = g.ids_of(&k);
        let (any, _) = g.deliver_all(ids, &m);
        if any {
            g.push_line(&k, "sent", text);
        }
        drop(g);
        self.changed(&k);
        if any {
            Ok(())
        } else {
            Err(format!("{account} is not connected"))
        }
    }

    /// Panel button click / field change: remember the value, send `ui.event`.
    pub fn fire_ui_event(&self, account: &str, name: &str, value: Value) -> Result<(), String> {
        let k = key(account);
        let mut g = self.lock();
        if let Some(a) = g.accounts.get_mut(&k) {
            if let Some(c) = a.controls.iter_mut().find(|c| c["name"] == name) {
                c["value"] = value.clone();
            }
        }
        let m = Msg::new("ui.event", None, json!({ "name": name, "value": value }));
        let ids = g.ids_of(&k);
        let (any, changed) = g.deliver_all(ids, &m);
        drop(g);
        if !changed.is_empty() {
            self.changed(&k);
        }
        if any {
            Ok(())
        } else {
            Err(format!("{account} is not connected"))
        }
    }

    /// Console line from the app itself (auto-relaunch notes).
    pub fn note(&self, account: &str, level: &str, text: &str) {
        let k = key(account);
        self.lock().push_line(&k, level, text);
        self.changed(&k);
    }

    /// Replace an account's relaunch config (UI); same whitelist as `relaunch.set`.
    /// Unknown account = Err (only accounts a script has connected as are listed).
    pub fn set_relaunch(&self, account: &str, cfg: &Value) -> Result<(), String> {
        let k = key(account);
        let mut g = self.lock();
        let a = g
            .accounts
            .get_mut(&k)
            .ok_or_else(|| format!("{account} has never connected"))?;
        let was = a.relaunch["enabled"] == true;
        a.relaunch = Value::Object(pick(cfg, RELAUNCH_FIELDS));
        if !was && a.relaunch["enabled"] == true {
            push_once(&mut g.reenabled, &k);
        }
        drop(g);
        self.changed(&k);
        Ok(())
    }

    /// Test seam: a connected client for `account` (its queue is dropped).
    #[cfg(test)]
    pub(crate) fn fake_connect(&self, account: &str) -> u64 {
        self.register(account, None, || true).unwrap().0
    }

    #[cfg(test)]
    pub(crate) fn fake_disconnect(&self, id: u64) {
        self.unregister(id);
    }

    /// `(account, connected, relaunch, last reported placeId)` per account, no console,
    /// plus (taken in the same lock) the account keys a `hello` re-key left without a client.
    #[allow(clippy::type_complexity)]
    pub fn relaunch_view(&self) -> (Vec<(String, bool, Value, Option<Value>)>, Vec<String>) {
        let mut g = self.lock();
        let view = g
            .accounts
            .values()
            .map(|a| {
                (
                    a.account.clone(),
                    a.connected,
                    a.relaunch.clone(),
                    a.place_id.clone(),
                )
            })
            .collect();
        (view, std::mem::take(&mut g.moved))
    }

    /// Account keys whose relaunch went off → on since the last call.
    pub fn take_reenabled(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().reenabled)
    }

    /// Something outside the hub (relaunch engine) changed this account's snapshot.
    pub fn touch(&self, account: &str) {
        self.changed(&key(account));
    }

    /// Every account's snapshot changed (relaunch engine reset).
    pub fn touch_all(&self) {
        let keys: Vec<String> = self.lock().accounts.keys().cloned().collect();
        keys.iter().for_each(|k| self.changed(k));
    }

    /// The whole console (not just the snapshot tail) as plain text, one line per entry.
    pub fn console_text(&self, account: &str) -> Option<String> {
        use chrono::TimeZone;
        let g = self.lock();
        let a = g.accounts.get(&key(account))?;
        Some(
            a.console
                .iter()
                .map(|l| {
                    let t = chrono::Local
                        .timestamp_millis_opt(l.ts)
                        .single()
                        .map_or_else(String::new, |t| {
                            t.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
                        });
                    format!("{t} [{}] {}\n", esc(&l.level), esc(&l.text))
                })
                .collect(),
        )
    }

    /// All known accounts with the last `SNAPSHOT_TAIL` console lines each.
    pub fn snapshot(&self) -> Vec<AccountState> {
        self.lock()
            .accounts
            .values()
            .map(|a| {
                let skip = a.console.len().saturating_sub(SNAPSHOT_TAIL);
                AccountState {
                    account: a.account.clone(),
                    connected: a.connected,
                    user_id: a.user_id.clone(),
                    place_id: a.place_id.clone(),
                    job_id: a.job_id.clone(),
                    console: a.console.iter().skip(skip).cloned().collect(),
                    controls: a.controls.clone(),
                    relaunch: a.relaunch.clone(),
                    seen: a.seen,
                }
            })
            .collect()
    }
}

/// One console entry stays one line in a saved file: CR/LF/backslash are escaped.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/control", get(control))
        .layer(axum::middleware::map_response(super::http::json_errors))
        .layer(axum::middleware::from_fn_with_state(
            ctx.clone(),
            super::guard,
        ))
        .with_state(ctx)
}

/// Longest wait for one outgoing frame. A client that stops reading fills the TCP buffer and
/// would block `send` forever, keeping the task and socket alive after the hub dropped it.
const SEND_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(10)
};

const PING_EVERY: Duration = Duration::from_secs(30);

/// False when the send failed or timed out: the caller drops the client.
async fn send(socket: &mut WebSocket, m: Message) -> bool {
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, socket.send(m)).await,
        Ok(Ok(()))
    )
}

async fn close(mut socket: WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    if !send(&mut socket, Message::Close(Some(frame))).await {
        return;
    }
    // Wait (briefly) for the client's close reply: dropping now resets the TCP connection under
    // frames it is still sending, and the client sees 1006 instead of `code`.
    let reply = async {
        while let Some(Ok(m)) = socket.recv().await {
            if matches!(m, Message::Close(_)) {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_millis(500), reply).await;
}

async fn control(
    State(ctx): State<Ctx>,
    Query(q): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(MESSAGE_MAX)
        .max_frame_size(MESSAGE_MAX)
        .on_upgrade(move |socket| serve(ctx, socket, q))
}

async fn serve(ctx: Ctx, mut socket: WebSocket, q: HashMap<String, String>) {
    let account = q.get("account").cloned().unwrap_or_default();
    let token = q.get("token").cloned().unwrap_or_default();
    let (id, mut rx) = match ctx.hub.register(&account, q.get("job"), || {
        ctx.accepting.load(std::sync::atomic::Ordering::SeqCst) && ctx.token_ok(&token)
    }) {
        Ok(r) => r,
        // Right token, server stopping or rebinding: try again, not "wrong token".
        Err(UNAUTHORIZED) if ctx.token_ok(&token) => {
            return close(socket, GOING_AWAY, "server stopping").await
        }
        Err(UNAUTHORIZED) => return close(socket, UNAUTHORIZED, "unauthorized").await,
        Err(code) => return close(socket, code, "too many clients").await,
    };
    // A ping now and then: a half-open LAN client (network gone) then fails at the TCP level
    // and is dropped. ponytail: no idle deadline, a client that never pongs stays connected.
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if !send(&mut socket, Message::Ping(Default::default())).await {
                    break;
                }
            }
            // Closed = the hub dropped this client (too slow, stop, regenerate): close now
            // instead of flushing its queue while inbound frames are already ignored.
            out = rx.recv() => match out.filter(|_| !rx.is_closed()) {
                Some(m) => {
                    let text = serde_json::to_string(&m).unwrap_or_default();
                    if !send(&mut socket, Message::Text(text.into())).await {
                        break;
                    }
                }
                None => {
                    if ctx.hub.unregister(id) {
                        close(socket, TRY_AGAIN, "client too slow").await;
                    } else {
                        close(socket, GOING_AWAY, "server closed").await;
                    }
                    return;
                }
            },
            inc = socket.recv() => match inc {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<Msg>(&t) {
                    Ok(m) => ctx.hub.handle(id, m),
                    Err(e) => log::debug!("automation: bad frame ignored: {e}"),
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    ctx.hub.unregister(id);
}

#[cfg(test)]
mod tests {
    use super::super::testkit;
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TMsg;

    type Client = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn setup() -> (super::super::Automation, u16) {
        let a = testkit::automation(json!({}));
        let port = a.start().await.unwrap().ws_port;
        (a, port)
    }

    async fn connect(port: u16, token: &str, account: &str) -> Client {
        let url = format!("ws://127.0.0.1:{port}/control?token={token}&account={account}&job=J1");
        tokio_tungstenite::connect_async(url).await.unwrap().0
    }

    async fn send(c: &mut Client, v: Value) {
        c.send(TMsg::text(v.to_string())).await.unwrap();
    }

    async fn recv(c: &mut Client) -> Msg {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(3), c.next())
                .await
                .expect("timed out")
                .unwrap()
                .unwrap();
            if let TMsg::Text(t) = m {
                return serde_json::from_str(&t).unwrap();
            }
        }
    }

    /// Round-trip a ping so earlier frames from this client are processed.
    async fn sync(c: &mut Client) {
        send(c, json!({ "t": "ping", "id": "sync" })).await;
        assert_eq!(recv(c).await.t, "pong");
    }

    #[tokio::test]
    async fn bad_or_missing_token_closes_4401() {
        let (_a, port) = setup().await;
        for url in [
            format!("ws://127.0.0.1:{port}/control?token=nope&account=a"),
            format!("ws://127.0.0.1:{port}/control?account=a"),
        ] {
            let (mut c, _) = tokio_tungstenite::connect_async(url).await.unwrap();
            let m = tokio::time::timeout(Duration::from_secs(3), c.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let TMsg::Close(Some(f)) = m else {
                panic!("expected close, got {m:?}")
            };
            assert_eq!(u16::from(f.code), 4401);
        }
    }

    /// The reference client sends frames right after the upgrade. The server must keep the
    /// socket open after its 4401 close until the client's close reply (or 500 ms): dropping
    /// at once resets the connection and the client reports 1006.
    #[tokio::test]
    async fn rejected_client_gets_time_to_reply_to_close() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (_a, port) = setup().await;
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let req = [
            "GET /control?token=nope&account=a HTTP/1.1",
            &format!("Host: 127.0.0.1:{port}"),
            "Upgrade: websocket",
            "Connection: Upgrade",
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
            "Sec-WebSocket-Version: 13",
            "",
            "",
        ]
        .join("\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        // 101 response, then the close frame: 0x88, len, code 4401 (0x11 0x31)
        let mut buf = Vec::new();
        let close = loop {
            let mut b = [0u8; 512];
            let n = tokio::time::timeout(Duration::from_secs(3), s.read(&mut b))
                .await
                .unwrap()
                .unwrap();
            assert!(n > 0, "closed before the close frame");
            buf.extend_from_slice(&b[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                if buf.len() >= i + 8 {
                    break buf[i + 4..i + 8].to_vec();
                }
            }
        };
        assert_eq!((close[0], close[2], close[3]), (0x88, 0x11, 0x31));
        // still talking (masked text "hi", zero mask): the server keeps listening
        s.write_all(&[0x81, 0x82, 0, 0, 0, 0, b'h', b'i'])
            .await
            .unwrap();
        let mut b = [0u8; 64];
        let open = tokio::time::timeout(Duration::from_millis(150), s.read(&mut b)).await;
        assert!(
            open.is_err(),
            "server dropped the socket before the close reply"
        );
        // close reply: the server ends the connection cleanly
        s.write_all(&[0x88, 0x80, 0, 0, 0, 0]).await.unwrap();
        let end = tokio::time::timeout(Duration::from_secs(1), s.read(&mut b)).await;
        assert!(matches!(end, Ok(Ok(0))), "expected EOF, got {end:?}");
    }

    #[tokio::test]
    async fn ping_pong_echoes_id() {
        let (a, port) = setup().await;
        let mut c = connect(port, &a.token(), "Alice").await;
        send(&mut c, json!({ "t": "ping", "id": "42", "data": {} })).await;
        let m = recv(&mut c).await;
        assert_eq!((m.t.as_str(), m.id), ("pong", Some(json!("42"))));
    }

    #[tokio::test]
    async fn ui_create_then_ui_event_and_ui_get() {
        let (a, port) = setup().await;
        let mut c = connect(port, &a.token(), "Alice").await;
        send(
            &mut c,
            json!({ "t": "ui.create", "data": { "kind": "number", "name": "speed", "value": 1 } }),
        )
        .await;
        sync(&mut c).await;
        let snap = a.ctx.hub.snapshot();
        assert_eq!(snap[0].controls[0]["name"], "speed");
        a.ctx.hub.fire_ui_event("alice", "speed", json!(5)).unwrap();
        let m = recv(&mut c).await;
        assert_eq!(m.t, "ui.event");
        assert_eq!(m.data, json!({ "name": "speed", "value": 5 }));
        send(
            &mut c,
            json!({ "t": "ui.get", "id": 9, "data": { "name": "speed" } }),
        )
        .await;
        let m = recv(&mut c).await;
        assert_eq!(m.t, "ui.value");
        assert_eq!(m.id, Some(json!(9)));
        assert_eq!(m.data, json!({ "name": "speed", "value": 5 }));
    }

    #[tokio::test]
    async fn echo_reaches_others_not_sender() {
        let (a, port) = setup().await;
        let mut alice = connect(port, &a.token(), "Alice").await;
        let mut bob = connect(port, &a.token(), "Bob").await;
        sync(&mut bob).await;
        send(&mut alice, json!({ "t": "echo", "data": { "text": "hi" } })).await;
        let m = recv(&mut bob).await;
        assert_eq!(m.t, "message");
        assert_eq!(m.data, json!({ "text": "hi", "from": "Alice" }));
        // Alice's next frame is her own pong, not the echo.
        sync(&mut alice).await;
    }

    #[tokio::test]
    async fn log_and_hello_and_relaunch_update_state() {
        let (a, port) = setup().await;
        let mut c = connect(port, &a.token(), "Alice").await;
        send(
            &mut c,
            json!({ "t": "hello", "data": { "account": "Alice", "placeId": 606 } }),
        )
        .await;
        send(
            &mut c,
            json!({ "t": "log", "data": { "text": "started", "level": "warn" } }),
        )
        .await;
        send(
            &mut c,
            json!({ "t": "relaunch.set", "data": { "enabled": true, "seconds": 30, "evil": {} } }),
        )
        .await;
        sync(&mut c).await;
        let s = &a.ctx.hub.snapshot()[0];
        assert!(s.connected);
        assert_eq!(s.place_id, Some(json!(606)));
        assert_eq!(s.job_id, Some(json!("J1")));
        assert_eq!(
            (s.console[0].level.as_str(), s.console[0].text.as_str()),
            ("warn", "started")
        );
        assert_eq!(s.relaunch, json!({ "enabled": true, "seconds": 30 }));
    }

    #[tokio::test]
    async fn console_text_has_every_line() {
        let (a, port) = setup().await;
        let mut c = connect(port, &a.token(), "Alice").await;
        send(
            &mut c,
            json!({ "t": "log", "data": { "text": "one\ntwo\r", "level": "warn" } }),
        )
        .await;
        sync(&mut c).await;
        let txt = a.ctx.hub.console_text("alice").unwrap();
        assert!(txt.contains("[warn] one\\ntwo\\r\n"), "{txt}");
        assert_eq!(txt.lines().count(), 1, "{txt}");
        assert!(a.ctx.hub.console_text("nobody").is_none());
    }

    #[tokio::test]
    async fn send_message_reaches_client_and_disconnect_marks_status() {
        let (a, port) = setup().await;
        let mut c = connect(port, &a.token(), "Alice").await;
        sync(&mut c).await;
        a.ctx.hub.send_message("ALICE", "go").unwrap();
        let m = recv(&mut c).await;
        assert_eq!(
            (m.t.as_str(), &m.data),
            ("message", &json!({ "text": "go" }))
        );
        drop(c);
        for _ in 0..50 {
            if !a.ctx.hub.snapshot()[0].connected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("still connected");
    }

    #[test]
    fn console_is_a_ring_buffer() {
        let h = Hub::default();
        let mut g = h.lock();
        g.account("Alice");
        for i in 0..CONSOLE_MAX + 10 {
            g.push_line("alice", "info", &i.to_string());
        }
        let c = &g.accounts["alice"].console;
        assert_eq!(c.len(), CONSOLE_MAX);
        assert_eq!(c[0].text, "10");
    }

    #[tokio::test]
    async fn ui_create_keeps_whitelisted_fields_only() {
        let (a, port) = setup().await;
        let mut c = connect(port, &a.token(), "Alice").await;
        let long = "x".repeat(NAME_MAX + 1);
        send(&mut c, json!({ "t": "ui.create", "data": { "kind": "button", "name": "go", "junk": [1, 2], "value": { "deep": 1 } } })).await;
        send(
            &mut c,
            json!({ "t": "ui.create", "data": { "kind": "button", "name": long } }),
        )
        .await;
        send(
            &mut c,
            json!({ "t": "ui.create", "data": { "kind": "iframe", "name": "x" } }),
        )
        .await;
        sync(&mut c).await;
        let controls = &a.ctx.hub.snapshot()[0].controls;
        assert_eq!(controls, &vec![json!({ "kind": "button", "name": "go" })]);
    }

    #[tokio::test]
    async fn browser_origin_is_rejected() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let (a, port) = setup().await;
        let url = format!(
            "ws://127.0.0.1:{port}/control?token={}&account=a",
            a.token()
        );
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("origin", "http://evil.example".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(req).await.is_err());
    }

    #[tokio::test]
    async fn non_loopback_host_is_rejected_before_upgrade() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let (a, port) = setup().await;
        let url = format!(
            "ws://127.0.0.1:{port}/control?token={}&account=a",
            a.token()
        );
        let mut req = url.clone().into_client_request().unwrap();
        req.headers_mut()
            .insert("host", "evil.example:7810".parse().unwrap());
        match tokio_tungstenite::connect_async(req).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status(), 403),
            r => panic!("expected 403, got {:?}", r.map(|_| ())),
        }
        // Loopback Host still upgrades.
        assert!(tokio_tungstenite::connect_async(url).await.is_ok());
    }

    #[tokio::test]
    async fn oversized_message_drops_the_client() {
        let (a, port) = setup().await;
        let mut c = connect(port, &a.token(), "Alice").await;
        let big = "x".repeat(MESSAGE_MAX + 1);
        let _ = c
            .send(TMsg::text(
                json!({ "t": "log", "data": { "text": big } }).to_string(),
            ))
            .await;
        let next = tokio::time::timeout(Duration::from_secs(3), c.next())
            .await
            .unwrap();
        assert!(!matches!(next, Some(Ok(TMsg::Text(_)))), "{next:?}");
    }

    #[tokio::test]
    async fn connection_cap_closes_1013() {
        let (a, port) = setup().await;
        let mut held = vec![];
        for i in 0..CONNS_MAX {
            let mut c = connect(port, &a.token(), &format!("u{i}")).await;
            sync(&mut c).await;
            held.push(c);
        }
        let mut extra = connect(port, &a.token(), "extra").await;
        let m = tokio::time::timeout(Duration::from_secs(3), extra.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let TMsg::Close(Some(f)) = m else {
            panic!("expected close, got {m:?}")
        };
        assert_eq!(u16::from(f.code), TRY_AGAIN);
    }

    #[test]
    fn slow_client_is_dropped() {
        let h = Hub::default();
        let (id, _rx) = h.register("Alice", None, || true).unwrap();
        for _ in 0..QUEUE_MAX {
            h.send_message("alice", "x").unwrap();
        }
        assert!(h.send_message("alice", "x").is_err());
        assert!(!h.snapshot()[0].connected);
        assert!(h.unregister(id), "kicked flag");
    }

    /// A client that stops reading must not pin its serve task: once the hub drops it, the
    /// blocked send times out and the task unregisters (clearing the `kicked` entry).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_that_stops_reading_is_released() {
        let (a, port) = setup().await;
        let mut c = connect(port, &a.token(), "Alice").await;
        sync(&mut c).await;
        let text = "x".repeat(TEXT_MAX);
        let mut dropped = false;
        for i in 0..200_000 {
            if a.ctx.hub.send_message("alice", &text).is_err() {
                dropped = true;
                break;
            }
            if i % 64 == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
        assert!(dropped, "the hub never dropped the client");
        for _ in 0..100 {
            if a.ctx.hub.lock().kicked.is_empty() {
                return drop(c);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("serve task still blocked in send");
    }

    #[test]
    fn unauthorized_register_is_refused() {
        let h = Hub::default();
        assert_eq!(h.register("a", None, || false).unwrap_err(), UNAUTHORIZED);
        assert!(h.snapshot().is_empty());
    }

    #[test]
    fn accounts_are_capped() {
        let h = Hub::default();
        for i in 0..ACCOUNTS_MAX + 50 {
            let (id, _rx) = h.register(&format!("u{i}"), None, || true).unwrap();
            h.unregister(id);
        }
        assert_eq!(h.snapshot().len(), ACCOUNTS_MAX);
    }

    #[test]
    fn eviction_takes_oldest_without_relaunch() {
        let h = Hub::default();
        for i in 0..ACCOUNTS_MAX {
            let (id, _rx) = h.register(&format!("u{i:03}"), None, || true).unwrap();
            h.unregister(id);
        }
        // u000 is the oldest but has relaunch on; u001 is next.
        h.set_relaunch("u000", &json!({ "enabled": true })).unwrap();
        let (_id, _rx) = h.register("new", None, || true).unwrap();
        let names: Vec<String> = h.snapshot().into_iter().map(|a| a.account).collect();
        assert!(names.contains(&"u000".to_string()) && !names.contains(&"u001".to_string()));
    }

    #[tokio::test]
    async fn changes_are_coalesced() {
        let h = Hub::default();
        for _ in 0..50 {
            h.changed("alice");
        }
        h.changed("bob");
        assert_eq!(h.next_changes().await, vec!["alice", "bob"]);
        let again = tokio::time::timeout(Duration::from_millis(300), h.next_changes()).await;
        assert!(again.is_err(), "no stray second emit");
    }

    #[test]
    fn keys_clips_and_whitelists() {
        assert_eq!(key("  Alice "), "alice");
        assert_eq!(key("   "), "unknown");
        assert_eq!(key(&"A".repeat(NAME_MAX + 5)), "a".repeat(NAME_MAX));
        assert_eq!(clip(&"x".repeat(TEXT_MAX + 1)).chars().count(), TEXT_MAX);
        assert_eq!(scalar(&json!(null)), None);
        assert_eq!(scalar(&json!([1])), None);
        assert_eq!(scalar(&json!({"a": 1})), None);
        assert_eq!(scalar(&json!(true)), Some(json!(true)));
        let d = json!({ "kind": "button", "value": { "deep": 1 }, "min": 0, "junk": "x" });
        assert_eq!(
            Value::Object(pick(&d, &["kind", "value", "min", "missing"])),
            json!({ "kind": "button", "min": 0 })
        );
        assert_eq!(esc("a\\b\nc\r"), "a\\\\b\\nc\\r");
    }

    #[test]
    fn ui_create_warns_and_caps_controls() {
        let h = Hub::default();
        let (id, _rx) = h.register("Alice", None, || true).unwrap();
        let long = json!({ "kind": "button", "name": "x".repeat(NAME_MAX + 1) });
        h.handle(id, Msg::new("ui.create", None, long));
        let iframe = json!({ "kind": "iframe", "name": "x" });
        h.handle(id, Msg::new("ui.create", None, iframe));
        for i in 0..=CONTROLS_MAX {
            let c = json!({ "kind": "button", "name": format!("b{i}") });
            h.handle(id, Msg::new("ui.create", None, c));
        }
        // Same name replaces in place, so it does not hit the cap.
        let again = json!({ "kind": "text", "name": "b0", "value": "new" });
        h.handle(id, Msg::new("ui.create", None, again));
        let s = &h.snapshot()[0];
        assert_eq!(s.controls.len(), CONTROLS_MAX);
        assert_eq!(
            s.controls[0],
            json!({ "kind": "text", "name": "b0", "value": "new" })
        );
        let warns = s
            .console
            .iter()
            .filter(|l| l.text.starts_with("ui.create ignored"))
            .count();
        assert_eq!(warns, 3, "long name, unknown kind, too many");
    }

    #[test]
    fn ui_get_replies_with_stored_value_or_null() {
        let h = Hub::default();
        let (id, mut rx) = h.register("Alice", None, || true).unwrap();
        h.handle(
            id,
            Msg::new("ui.create", None, json!({ "kind": "number", "name": "n" })),
        );
        h.fire_ui_event("alice", "n", json!(5)).unwrap();
        assert_eq!(rx.try_recv().unwrap().t, "ui.event");
        h.handle(
            id,
            Msg::new("ui.get", Some(json!("q1")), json!({ "name": "n" })),
        );
        let r = rx.try_recv().unwrap();
        assert_eq!(r.t, "ui.value");
        assert_eq!(r.id, Some(json!("q1")));
        assert_eq!(r.data, json!({ "name": "n", "value": 5 }));
        h.handle(id, Msg::new("ui.get", None, json!({ "name": "missing" })));
        assert_eq!(
            rx.try_recv().unwrap().data,
            json!({ "name": "missing", "value": null })
        );
    }

    #[test]
    fn send_and_fire_need_a_live_client() {
        let h = Hub::default();
        assert_eq!(
            h.send_message("ghost", "x").unwrap_err(),
            "ghost is not connected"
        );
        let (id, _rx) = h.register("Alice", None, || true).unwrap();
        h.fake_disconnect(id);
        assert_eq!(
            h.fire_ui_event("alice", "n", json!(1)).unwrap_err(),
            "alice is not connected"
        );
    }

    #[test]
    fn set_relaunch_needs_known_account_and_flags_reenable() {
        let h = Hub::default();
        assert!(h
            .set_relaunch("ghost", &json!({ "enabled": true }))
            .is_err());
        h.register("Alice", None, || true).unwrap();
        h.set_relaunch(
            "alice",
            &json!({ "enabled": true, "seconds": 5, "junk": 1 }),
        )
        .unwrap();
        assert_eq!(h.take_reenabled(), ["alice"]);
        assert!(h.take_reenabled().is_empty());
        // Already on: a new delay is not a re-enable.
        h.set_relaunch("alice", &json!({ "enabled": true, "seconds": 9 }))
            .unwrap();
        assert!(h.take_reenabled().is_empty());
        assert_eq!(
            h.snapshot()[0].relaunch,
            json!({ "enabled": true, "seconds": 9 })
        );
    }

    #[test]
    fn hello_rekeys_client_and_reports_the_moved_account() {
        let h = Hub::default();
        let (id, _rx) = h.register("Alice", None, || true).unwrap();
        h.handle(
            id,
            Msg::new(
                "hello",
                None,
                json!({ "account": "Bob", "userId": 7, "placeId": 606 }),
            ),
        );
        let (view, moved) = h.relaunch_view();
        assert_eq!(moved, ["alice"]);
        assert!(view.iter().any(|v| v.0 == "Bob" && v.1));
        let snap = h.snapshot();
        assert!(snap.iter().any(|a| a.account == "Alice" && !a.connected));
        let bob = snap.iter().find(|a| a.account == "Bob").unwrap();
        assert_eq!(
            (bob.user_id.clone(), bob.place_id.clone()),
            (Some(json!(7)), Some(json!(606)))
        );
    }

    #[test]
    fn snapshot_carries_only_the_console_tail() {
        let h = Hub::default();
        let (_id, _rx) = h.register("Alice", None, || true).unwrap();
        for i in 0..SNAPSHOT_TAIL + 3 {
            h.note("Alice", "info", &i.to_string());
        }
        let snap = h.snapshot();
        let console = &snap[0].console;
        assert_eq!(console.len(), SNAPSHOT_TAIL);
        assert_eq!(console[0].text, "3");
        assert_eq!(
            console.back().unwrap().text,
            (SNAPSHOT_TAIL + 2).to_string()
        );
    }

    #[test]
    fn full_list_of_relaunch_accounts_refuses_a_new_name() {
        let h = Hub::default();
        for i in 0..ACCOUNTS_MAX {
            let name = format!("u{i:03}");
            let (id, _rx) = h.register(&name, None, || true).unwrap();
            h.fake_disconnect(id);
            h.set_relaunch(&name, &json!({ "enabled": true })).unwrap();
        }
        assert_eq!(h.register("new", None, || true).unwrap_err(), TRY_AGAIN);
        assert_eq!(h.snapshot().len(), ACCOUNTS_MAX);
    }
}
