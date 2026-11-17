//! Remote access host (docs/remote-protocol.md, docs/remote-access.md): another RMI on the
//! owner's tailnet views this screen and controls mouse + keyboard.
//!
//! - Off by default (`/remote/enabled`); only the local Settings page writes it.
//! - Own listener, bound only to this device's Tailscale IPv4 (100.64.0.0/10), never
//!   0.0.0.0 and never the friend / Funnel server. Off = no listener at all.
//! - Pairing: the owner shows a one-time code here (8 chars, 5 min, 5 tries); the viewer
//!   trades it once for a random token. Only its SHA-256 is stored here.
//! - One session at a time, logged; an always-on-top indicator with Stop while live.
//! - Capture and encode (JPEG, or H.264 when offered) run on two plain threads (overlapping), input on a third; held keys
//!   and buttons are released when the session ends. A writer task owns the socket's send half, so a slow
//!   frame never holds up input, pings or Stop.

pub mod audio;
pub mod desk;
mod files;
mod h264;
pub mod handover;
mod keys;
pub mod viewer;

use crate::automation::protocol::{new_token, ApiError};
use crate::automation::ApiResult;
use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::ConnectInfo;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::header::{AUTHORIZATION, HOST, ORIGIN};
use axum::http::HeaderMap;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use desk::{Desk, Input, Screen};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch, Notify};

pub const DEFAULT_PORT: u16 = 7813;
/// No 0/O/1/I: read off one screen and typed on another.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 8;
const CODE_TTL: Duration = Duration::from_secs(300);
/// Wrong guesses before the code is burned (the owner shows a new one).
const CODE_TRIES: u32 = 5;
const LOG_MAX: usize = 20;
const MAX_PAIRED: usize = 32;
/// A paired computer reports the accounts it runs this often (`viewer::push_running`); a
/// report is forgotten after `PEER_TTL` without a new one.
pub const PUSH_EVERY: Duration = Duration::from_secs(15);
const PEER_TTL: Duration = Duration::from_secs(45);
/// Most Roblox user ids one report may carry.
const MAX_RUNNING: usize = 256;
/// Run here gives the other computer this long to close the accounts (a queued hand-over
/// waits for its next report, `PUSH_EVERY`).
pub(crate) const HANDOVER_WAIT: Duration = Duration::from_secs(40);
const STORE_FILE: &str = "remote.json";
const AUTH_WAIT: Duration = Duration::from_secs(10);
const MAX_UNACKED: usize = 4;
/// Highest `maxFps` the host honours.
const MAX_FPS: u32 = 60;
/// Waiting for Tailscale to come up (or the port to free).
const RETRY: Duration = Duration::from_secs(15);
/// Pair request body.
const MAX_TEXT: usize = 4096;
/// One viewer text message (spec: hosts accept `text` up to 8 KB).
const MAX_MSG: usize = 8192;
/// One viewer binary message (`files`: `0x10`, id, a 256 KB chunk); after auth only.
const MAX_BIN: usize = files::CHUNK + 5;
/// Session end waits this long for the input thread to let go of held keys and buttons.
const INPUT_WAIT: Duration = Duration::from_millis(500);
pub const INDICATOR: &str = "remote-indicator";
/// No message from the viewer this long (it pings every 2 s): the session ends.
const IDLE: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(10)
};
/// Any single control send to the viewer.
const SEND_WAIT: Duration = Duration::from_secs(3);
/// One frame (big on a slow link); the writer task waits, the session does not.
const FRAME_WAIT: Duration = IDLE;
/// Sockets connected but not yet authenticated.
const MAX_PRE_AUTH: usize = 8;
/// Page origins allowed to open the socket or pair (requests without Origin come from Rust).
const ORIGINS: &[&str] = &["tauri://localhost", "http://tauri.localhost"];
/// After Stop at the device, new sessions are refused this long.
const STOP_HOLD: Duration = Duration::from_secs(30);
/// How long a replacing session waits for the old one to let go.
const REPLACE_WAIT: Duration = Duration::from_millis(1500);
/// Screen id of the whole virtual desktop (`all` cap).
const ALL: u8 = 255;
/// Pointer updates (`cursor` cap): at most 30 per second.
const CUR_EVERY: Duration = Duration::from_millis(33);
/// H.264: the last picture is encoded again this many times once the screen stops changing.
/// Rate control gives a frame only its share of the bitrate, so a big change arrives soft;
/// the repeats (cheap, mostly skipped blocks) sharpen it instead of leaving it blurry.
const REFINE: u32 = 15;
/// Keyframe requests (`idr`) honoured at most this often.
const IDR_EVERY: Duration = Duration::from_millis(250);
const STATS_EVERY: Duration = Duration::from_secs(2);
/// `auto` frames are never wider than this (4K and Retina screens are scaled down).
const AUTO_MAX_W: u32 = 1920;
/// `auto` steps: (JPEG quality, scale), best first; sessions start at index 1.
const LEVELS: [(u8, f32); 5] = [(80, 1.0), (70, 1.0), (60, 0.75), (50, 0.75), (40, 0.5)];
/// `auto` H.264 steps, same index as `LEVELS`: (target kbit/s, scale); bitrate first, scale
/// only as the last resort.
const H264_LEVELS: [(u32, f32); 5] = [
    (25_000, 1.0),
    (16_000, 1.0),
    (10_000, 1.0),
    (6_000, 1.0),
    (3_000, 0.75),
];

/// The only addresses the host listens on: Tailscale's CGNAT range 100.64.0.0/10.
pub fn bind_ok(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && o[1] & 0xC0 == 64
}

pub fn hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn valid_token(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit())
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `bye{code, why}`: the viewer words each code itself and falls back to `why`.
fn bye_msg(code: &str, by: Option<&str>) -> Value {
    let why = match code {
        "off" => "Remote access is switched off on this computer.",
        "busy" => "Someone else is connected to this computer.",
        "auth" => "This computer no longer knows this device. Pair again.",
        "removed" => "This device was removed on that computer. Pair again.",
        "stopped" => "Stopped at the device.",
        "timeout" => "Nothing arrived from this device for a while, so the session ended.",
        "network" => "That computer's network address changed. Connect again.",
        "replaced" => "The session was taken over.",
        _ => "The session ended.",
    };
    let mut v = json!({ "t": "bye", "code": code, "why": why });
    if let Some(by) = by {
        v["by"] = json!(by);
    }
    v
}

/// Peer / device names: trimmed, control characters dropped, at most 40 characters.
/// Invisible format characters (bidi overrides and isolates, zero-width marks) go too: a name
/// with U+202E would show reversed in the session list and on the friend page. U+200C and
/// U+200D (zero-width non-joiner and joiner) stay: Persian, Indic scripts and emoji need them.
pub fn clean_name(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}' | '\u{200e}' | '\u{200f}'
                    | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'
                    | '\u{fff9}'..='\u{fffb}')
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(40)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Paired {
    /// SHA-256 of the device's token (hex).
    hash: String,
    name: String,
    paired_at: String,
    /// Tailscale IP the device paired from; the token only works from there. An entry
    /// without one is invalid and dropped at load.
    #[serde(default)]
    ip: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub peer: String,
    pub started_at: String,
    pub duration_sec: u64,
    /// Any input was applied (else view only).
    pub control: bool,
    /// Transfers started in the session.
    #[serde(default)]
    pub files: Vec<files::FileLog>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    paired: Vec<Paired>,
    #[serde(default)]
    log: Vec<LogEntry>,
}

struct Code {
    code: String,
    until: Instant,
    tries: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    pub peer: String,
    pub since: String,
    pub control: bool,
    #[serde(skip)]
    id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedView {
    pub id: String,
    pub name: String,
    pub paired_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    /// `ip:port` while listening.
    pub listening: Option<String>,
    pub name: String,
    pub session: Option<Live>,
    pub paired: Vec<PairedView>,
    /// Newest first.
    pub log: Vec<LogEntry>,
    /// A pairing code can still be used (not expired, used or burned).
    pub code_live: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairCode {
    pub code: String,
    pub expires_in_sec: u64,
}

type Hook = Arc<dyn Fn() + Send + Sync>;

struct Inner {
    settings: Arc<Mutex<Value>>,
    file: PathBuf,
    store: Mutex<Store>,
    code: Mutex<Option<Code>>,
    live: Mutex<Option<Live>>,
    stop: Mutex<Option<oneshot::Sender<&'static str>>>,
    desk: Arc<dyn Desk>,
    bound: Mutex<Option<SocketAddr>>,
    hook: RwLock<Option<Hook>>,
    pre_auth: Arc<tokio::sync::Semaphore>,
    /// Last Stop at the device (`STOP_HOLD`).
    stopped_at: Mutex<Option<Instant>>,
    /// Where uploads land instead of the Downloads folder (tests).
    downloads: Mutex<Option<PathBuf>>,
    /// What each paired computer last said it runs, by paired id.
    peers: Mutex<HashMap<String, Report>>,
    /// Vault, process list, auto-relaunch (set at app start; tests use a fake).
    local: RwLock<Option<Arc<dyn handover::Local>>>,
    /// What each computer this one holds a token for said it runs (its `running` answer), by
    /// its IP.
    hosts: Mutex<HashMap<String, Report>>,
    /// Hand-overs waiting for a paired computer's next report, by paired id.
    wants: Mutex<HashMap<String, (Instant, Vec<u64>)>>,
    /// Woken after every stored report (a queued hand-over checks it).
    tick: Notify,
}

/// A computer's report: when, its name, the Roblox user ids with a client there.
struct Report {
    at: Instant,
    name: String,
    ids: Vec<u64>,
    ip: String,
    /// It takes hand-overs (an older RMI does not).
    handover: bool,
}

type Ctx = Arc<Inner>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    fn setting(&self, k: &str) -> Option<Value> {
        lock(&self.settings)
            .pointer(&format!("/remote/{k}"))
            .cloned()
    }

    /// Switched on, and this platform can host (Linux has no capture: never on there).
    fn on(&self) -> bool {
        self.setting("enabled") == Some(Value::Bool(true)) && self.desk.os() != "linux"
    }

    /// `/remote/files`: off unless switched on.
    fn files_on(&self) -> bool {
        self.setting("files") == Some(Value::Bool(true))
    }

    /// `/remote/audio` (on unless switched off) and this desk can capture sound.
    fn audio_on(&self) -> bool {
        self.setting("audio") != Some(Value::Bool(false)) && self.desk.has_audio()
    }

    fn port(&self) -> u16 {
        self.setting("port")
            .and_then(|v| v.as_u64())
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(DEFAULT_PORT)
    }

    /// `/remote/deviceName`, else the computer name.
    fn name(&self) -> String {
        let set = self
            .setting("deviceName")
            .and_then(|v| v.as_str().map(clean_name))
            .unwrap_or_default();
        if !set.is_empty() {
            return set;
        }
        sysinfo::System::host_name()
            .map(|h| clean_name(&h))
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "This computer".into())
    }

    fn changed(&self) {
        let h = self
            .hook
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(h) = h {
            h();
        }
    }

    fn save(&self, st: &Store) -> Result<(), String> {
        let r = serde_json::to_vec_pretty(st)
            .map_err(|e| e.to_string())
            .and_then(|b| {
                crate::vault::store::write_atomic(&self.file, &b, crate::vault::store::Backup::Skip)
                    .map_err(|e| e.to_string())
            });
        if let Err(e) = &r {
            log::error!("remote: could not save paired devices: {e}");
        }
        r
    }

    /// Consumes the pairing code if `given` matches (case, spaces and dashes ignored).
    fn take_code(&self, given: &str) -> ApiResult<()> {
        use subtle::ConstantTimeEq;
        let mut slot = lock(&self.code);
        let Some(c) = slot.as_mut().filter(|c| Instant::now() < c.until) else {
            *slot = None;
            return Err(ApiError::new(
                403,
                "no_code",
                "Show a pairing code on that computer first.",
            ));
        };
        let g: String = given
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if bool::from(g.as_bytes().ct_eq(c.code.as_bytes())) {
            *slot = None;
            return Ok(());
        }
        c.tries += 1;
        if c.tries >= CODE_TRIES {
            *slot = None;
            drop(slot);
            log::warn!("remote: pairing code burned after {CODE_TRIES} wrong tries");
            self.changed(); // Settings stops showing it
        }
        Err(ApiError::new(
            403,
            "bad_code",
            "Wrong code. Check it and try again.",
        ))
    }

    /// The paired device's (name, id) for `token` used from `ip` (the IP it paired from);
    /// every entry is compared (constant time).
    fn who(&self, token: &str, ip: &str) -> Option<(String, String)> {
        use subtle::ConstantTimeEq;
        if !valid_token(token) || ip.is_empty() {
            return None;
        }
        let h = hash(token);
        let st = lock(&self.store);
        let mut found = None;
        for p in &st.paired {
            if bool::from(p.hash.as_bytes().ct_eq(h.as_bytes())) && p.ip == ip {
                found = Some((p.name.clone(), p.hash[..12].to_owned()));
            }
        }
        found
    }

    /// Ends the live session (if any) with `bye{code}`. Under the `live` lock: a session
    /// claiming the slot right now either sees the stop / off first or is kicked here.
    fn kick(&self, code: &'static str) {
        let _live = lock(&self.live);
        if let Some(tx) = lock(&self.stop).take() {
            let _ = tx.send(code);
        }
    }
}

struct Running {
    port: u16,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

pub struct Remote {
    c: Ctx,
    running: tokio::sync::Mutex<Option<Running>>,
}

impl Remote {
    /// `root`: data folder (`remote.json` lives there). `desk`: `desk::system()` in the app.
    pub fn new(settings: Arc<Mutex<Value>>, root: &std::path::Path, desk: Arc<dyn Desk>) -> Self {
        let file = root.join(STORE_FILE);
        let mut store: Store = match std::fs::read(&file) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                // kept aside, never overwritten by the next save
                let bad = file.with_extension("json.bad");
                match std::fs::rename(&file, &bad) {
                    Ok(()) => log::error!(
                        "remote: {STORE_FILE} unreadable ({e}); kept as {STORE_FILE}.bad, starting empty"
                    ),
                    Err(re) => log::error!(
                        "remote: {STORE_FILE} unreadable ({e}) and could not be set aside ({re})"
                    ),
                }
                Store::default()
            }),
            Err(_) => Store::default(),
        };
        let n = store.paired.len();
        store
            .paired
            .retain(|p| valid_token(&p.hash) && !p.ip.is_empty());
        if store.paired.len() < n {
            log::warn!(
                "remote: dropped {} invalid paired device entries",
                n - store.paired.len()
            );
        }
        Remote {
            c: Arc::new(Inner {
                settings,
                file,
                store: Mutex::new(store),
                code: Mutex::default(),
                live: Mutex::default(),
                stop: Mutex::default(),
                desk,
                bound: Mutex::default(),
                hook: RwLock::default(),
                pre_auth: Arc::new(tokio::sync::Semaphore::new(MAX_PRE_AUTH)),
                stopped_at: Mutex::default(),
                downloads: Mutex::default(),
                peers: Mutex::default(),
                local: RwLock::default(),
                hosts: Mutex::default(),
                wants: Mutex::default(),
                tick: Notify::new(),
            }),
            running: tokio::sync::Mutex::new(None),
        }
    }

    /// Called whenever the status the UI shows changes (session, pairing, listener).
    pub fn set_hook(&self, h: Hook) {
        *self.c.hook.write().unwrap_or_else(PoisonError::into_inner) = Some(h);
    }

    pub fn enabled(&self) -> bool {
        self.c.on()
    }

    /// `/remote/port` (the viewer assumes peers use the same one).
    pub fn port(&self) -> u16 {
        self.c.port()
    }

    pub fn device_name(&self) -> String {
        self.c.name()
    }

    /// A paired computer's report, as the `running` route stores it.
    #[cfg(test)]
    pub(crate) fn note_running(&self, peer: &str, name: &str, ids: Vec<u64>) {
        let r = Report {
            at: Instant::now(),
            name: name.into(),
            ids,
            ip: String::new(),
            handover: true,
        };
        lock(&self.c.peers).insert(peer.into(), r);
    }

    /// What the computer at `ip` (this one holds its token) answered a report with.
    pub(crate) fn note_host(&self, ip: &str, name: &str, mut ids: Vec<u64>) {
        ids.truncate(MAX_RUNNING);
        let r = Report {
            at: Instant::now(),
            name: name.into(),
            ids,
            ip: ip.into(),
            handover: true,
        };
        lock(&self.c.hosts).insert(ip.into(), r);
    }

    pub fn set_local(&self, l: Arc<dyn handover::Local>) {
        *self.c.local.write().unwrap_or_else(PoisonError::into_inner) = Some(l);
    }

    /// Every account a paired computer runs, from both directions, within `PEER_TTL`. An
    /// account both directions report is listed once, from the computer this one can ask
    /// directly.
    pub fn elsewhere(&self) -> Vec<handover::Elsewhere> {
        let mut out: Vec<handover::Elsewhere> = Vec::new();
        for m in [&self.c.hosts, &self.c.peers] {
            let mut m = lock(m);
            m.retain(|_, r| r.at.elapsed() < PEER_TTL);
            for r in m.values() {
                for &user_id in &r.ids {
                    if !out.iter().any(|e| e.user_id == user_id) {
                        out.push(handover::Elsewhere {
                            user_id,
                            device: r.name.clone(),
                            ip: r.ip.clone(),
                        });
                    }
                }
            }
        }
        out
    }

    /// (Roblox user id, computer name) for every account a paired computer runs.
    pub fn running_elsewhere(&self) -> Vec<(u64, String)> {
        self.elsewhere()
            .into_iter()
            .map(|e| (e.user_id, e.device))
            .collect()
    }

    /// Run here: the computers running `ids` close them, all at once. `direct(ip, name, ids)`
    /// asks a computer this one holds a token for; the others get it with their next report.
    /// The released ids; `Err` (the first reason) only when none were.
    pub async fn run_here<F, Fut>(&self, ids: &[u64], direct: F) -> Result<Vec<u64>, String>
    where
        F: Fn(String, String, Vec<u64>) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u64>, String>>,
    {
        let mut by: Vec<(String, String, Vec<u64>)> = Vec::new();
        for e in self
            .elsewhere()
            .into_iter()
            .filter(|e| ids.contains(&e.user_id))
        {
            match by.iter_mut().find(|g| g.0 == e.ip) {
                Some(g) => g.2.push(e.user_id),
                None => by.push((e.ip, e.device, vec![e.user_id])),
            }
        }
        if by.is_empty() {
            return Err("Those accounts are not running on another computer.".into());
        }
        let direct = &direct;
        let asks = by.into_iter().map(|(ip, name, ids)| async move {
            let has_token = lock(&self.c.hosts).contains_key(&ip);
            if has_token {
                direct(ip, name, ids).await
            } else {
                self.queued(&ip, &name, ids).await
            }
        });
        let (mut out, mut why) = (Vec::new(), None);
        for r in futures_util::future::join_all(asks).await {
            match r {
                Ok(v) => out.extend(v.into_iter().filter(|i| ids.contains(i))),
                Err(e) => {
                    why.get_or_insert(e);
                }
            }
        }
        if out.is_empty() {
            return Err(why.unwrap_or_else(|| "The other computer could not close them.".into()));
        }
        // Gone from there now: don't offer Run here again until that computer's next report.
        for m in [&self.c.hosts, &self.c.peers] {
            for r in lock(m).values_mut() {
                r.ids.retain(|i| !out.contains(i));
            }
        }
        Ok(out)
    }

    /// A hand-over to the paired computer at `ip` (it holds the token): its next report's
    /// answer asks for `ids`; done once a later report no longer lists some of them.
    /// ponytail: one queued hand-over per computer at a time (a second replaces the first).
    async fn queued(&self, ip: &str, name: &str, ids: Vec<u64>) -> Result<Vec<u64>, String> {
        let found = lock(&self.c.peers)
            .iter()
            .find(|(_, r)| r.ip == ip)
            .map(|(id, r)| (id.clone(), r.handover));
        let (id, can) = found.ok_or_else(|| format!("Could not reach {name}."))?;
        if !can {
            return Err(format!("Update RMI on {name} first."));
        }
        let since = Instant::now();
        lock(&self.c.wants).insert(id.clone(), (since, ids.clone()));
        let gone = async {
            loop {
                // registered before the check: a report landing in between still wakes it
                let n = self.c.tick.notified();
                tokio::pin!(n);
                n.as_mut().enable();
                let left: Option<Vec<u64>> = lock(&self.c.peers)
                    .get(&id)
                    .filter(|r| r.at >= since)
                    .map(|r| ids.iter().filter(|i| !r.ids.contains(i)).copied().collect());
                if let Some(g) = left.filter(|g| !g.is_empty()) {
                    return g;
                }
                n.await;
            }
        };
        let r = tokio::time::timeout(HANDOVER_WAIT, gone).await;
        lock(&self.c.wants).remove(&id);
        r.map_err(|_| format!("{name} did not answer. Is RMI open there?"))
    }

    pub fn live(&self) -> Option<Live> {
        lock(&self.c.live).clone()
    }

    pub fn status(&self) -> RemoteStatus {
        let st = lock(&self.c.store);
        RemoteStatus {
            listening: lock(&self.c.bound).map(|a| a.to_string()),
            name: self.c.name(),
            session: self.live(),
            paired: st
                .paired
                .iter()
                .map(|p| PairedView {
                    id: p.hash[..12].to_owned(),
                    name: p.name.clone(),
                    paired_at: p.paired_at.clone(),
                })
                .collect(),
            log: st.log.iter().rev().cloned().collect(),
            code_live: lock(&self.c.code)
                .as_ref()
                .is_some_and(|c| Instant::now() < c.until),
        }
    }

    /// A new one-time pairing code (replaces any earlier one). Remote access must be on.
    pub fn new_code(&self) -> Result<PairCode, String> {
        if !self.c.on() {
            return Err("Turn on remote access and save first.".into());
        }
        let mut b = [0u8; CODE_LEN];
        getrandom::fill(&mut b).map_err(|e| e.to_string())?;
        // 32 symbols: every byte maps without bias
        let code: String = b
            .iter()
            .map(|x| CODE_CHARS[*x as usize % CODE_CHARS.len()] as char)
            .collect();
        crate::logging::register_secret(&code);
        *lock(&self.c.code) = Some(Code {
            code: code.clone(),
            until: Instant::now() + CODE_TTL,
            tries: 0,
        });
        Ok(PairCode {
            code,
            expires_in_sec: CODE_TTL.as_secs(),
        })
    }

    /// Forget a paired device; its live session ends.
    pub fn unpair(&self, id: &str) {
        {
            let mut st = lock(&self.c.store);
            st.paired.retain(|p| p.hash[..12] != *id);
            let _ = self.c.save(&st); // logged
        }
        if lock(&self.c.live).as_ref().is_some_and(|l| l.id == id) {
            self.c.kick("removed");
        }
        self.c.changed();
    }

    /// Stop button (indicator, Settings): ends the live session; new ones are refused for
    /// `STOP_HOLD`.
    pub fn stop_session(&self) {
        // always: a session claiming the slot right now must not slip past Stop
        *lock(&self.c.stopped_at) = Some(Instant::now());
        self.c.kick("stopped");
    }

    /// Quit or update: ends a live session (the viewer gets its `bye`, the input thread lets
    /// go of held keys and buttons) and waits for that, at most 1 s.
    pub async fn stop_and_wait(&self) {
        self.stop_session();
        for _ in 0..50 {
            if self.live().is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Match the settings: listen on the Tailscale IPv4 while enabled (retrying until
    /// Tailscale is up), rebind on a port change, stop and end any session when off.
    pub async fn sync(&self) {
        let (on, port) = (self.c.on(), self.c.port());
        if !on {
            self.c.kick("off");
            *lock(&self.c.code) = None;
        }
        let mut run = self.running.lock().await;
        if run
            .as_ref()
            .is_some_and(|r| on && r.port == port && !r.task.is_finished())
        {
            return;
        }
        if let Some(r) = run.take() {
            let _ = r.stop.send(());
            r.task.abort();
            *lock(&self.c.bound) = None;
            log::info!("remote access stopped");
            self.c.changed();
        }
        if on {
            let (stop, rx) = oneshot::channel();
            let task = tokio::spawn(listen(self.c.clone(), port, rx));
            *run = Some(Running { port, stop, task });
        }
    }
}

/// Bind `<tailscale ip>:port` once Tailscale reports an address in 100.64.0.0/10.
async fn listen(c: Ctx, port: u16, mut stop: oneshot::Receiver<()>) {
    let mut warned = false;
    loop {
        let ip = tokio::task::spawn_blocking(|| {
            crate::friend::tailscale::status_json()
                .and_then(|j| crate::friend::tailscale::self_ipv4(&j))
        })
        .await
        .ok()
        .flatten();
        match ip.filter(|ip| bind_ok(*ip)) {
            Some(ip) => match tokio::net::TcpListener::bind((ip, port)).await {
                Ok(l) => {
                    if serve(c.clone(), l, &mut stop).await {
                        return;
                    }
                    warned = false;
                    continue; // the Tailscale IP changed: bind the new one
                }
                Err(e) if !warned => {
                    warned = true;
                    log::warn!("remote: could not listen on port {port}: {e}");
                }
                Err(_) => {}
            },
            None if !warned => {
                warned = true;
                log::info!("remote: waiting for Tailscale before listening");
            }
            None => {}
        }
        tokio::select! {
            _ = &mut stop => return,
            _ = tokio::time::sleep(RETRY) => {}
        }
    }
}

/// The Tailscale IPv4 now differs from `ip` (checked every `RETRY`). Never for addresses
/// outside 100.64/10 (tests on 127.0.0.1).
async fn ip_changed(ip: std::net::IpAddr) {
    if !matches!(ip, std::net::IpAddr::V4(v4) if bind_ok(v4)) {
        return std::future::pending().await;
    }
    loop {
        tokio::time::sleep(RETRY).await;
        let now = tokio::task::spawn_blocking(|| {
            crate::friend::tailscale::status_json()
                .and_then(|j| crate::friend::tailscale::self_ipv4(&j))
        })
        .await
        .ok()
        .flatten();
        // no answer (a CLI hiccup) is not a change; the next tick sees a real one
        if now.is_some_and(|n| std::net::IpAddr::V4(n) != ip) {
            return;
        }
    }
}

/// Serves until `stop` (true) or the Tailscale IP changes (false; any session is ended).
async fn serve(c: Ctx, l: tokio::net::TcpListener, stop: &mut oneshot::Receiver<()>) -> bool {
    let Ok(addr) = l.local_addr() else {
        return true;
    };
    *lock(&c.bound) = Some(addr);
    c.changed();
    log::info!("remote access listening on {addr}");
    // small control messages (acks, pongs, input echoes) must not wait for Nagle
    let l = axum::serve::ListenerExt::tap_io(l, |s| {
        if let Err(e) = s.set_nodelay(true) {
            log::debug!("remote: TCP_NODELAY failed: {e}");
        }
    });
    let server = axum::serve(
        l,
        router(c.clone()).into_make_service_with_connect_info::<SocketAddr>(),
    );
    // dropping the server closes the listener; open connections run on their own tasks
    let stopped = tokio::select! {
        r = server => {
            if let Err(e) = r {
                log::error!("remote server failed: {e}");
            }
            true
        }
        _ = stop => true,
        _ = ip_changed(addr.ip()) => {
            log::info!("remote: the Tailscale address changed, listening again");
            c.kick("network");
            false
        }
    };
    *lock(&c.bound) = None;
    c.changed();
    stopped
}

fn router(c: Ctx) -> Router {
    Router::new()
        .route("/v1/remote/hello", get(hello))
        .route("/v1/remote/pair", post(pair))
        .route("/v1/remote/running", post(running))
        .route("/v1/remote/handover", post(handover))
        .route("/v1/remote/ws", get(ws))
        .fallback(|| async { ApiError::not_found("No such endpoint") })
        .layer(DefaultBodyLimit::max(MAX_TEXT))
        .layer(tower::limit::GlobalConcurrencyLimitLayer::new(16))
        .layer(middleware::from_fn_with_state(c.clone(), host_guard))
        .layer(middleware::map_response(crate::friend::harden))
        .with_state(c)
}

/// A page origin may call the host: RMI's own webview only (the dev server in debug builds).
/// No Origin (Rust's reqwest): allowed.
fn origin_ok(o: Option<&str>) -> bool {
    o.is_none_or(|o| {
        ORIGINS.contains(&o) || (cfg!(debug_assertions) && o == "http://localhost:1420")
    })
}

/// DNS rebinding guard: Host must be exactly the bound `ip:port`; other web pages are refused.
async fn host_guard(State(c): State<Ctx>, req: Request, next: Next) -> Response {
    let host = req.headers().get(HOST).and_then(|v| v.to_str().ok());
    let want = lock(&c.bound).map(|a| a.to_string());
    if host.is_none() || host.map(str::to_owned) != want {
        return ApiError::new(403, "forbidden_host", "Wrong host").into_response();
    }
    let origin = req.headers().get(ORIGIN).map(|v| v.to_str().unwrap_or("?"));
    if !origin_ok(origin) {
        return ApiError::new(403, "forbidden_origin", "Wrong origin").into_response();
    }
    next.run(req).await
}

/// Unauthenticated discovery: only enough to show "RMI is here" in the Devices list.
async fn hello(State(c): State<Ctx>) -> Response {
    let busy = lock(&c.live).is_some();
    Json(json!({ "app": "rmi", "proto": 1, "name": c.name(), "busy": busy })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunningBody {
    name: String,
    user_ids: Vec<u64>,
    /// It takes hand-overs (an older RMI leaves this out).
    #[serde(default)]
    can_hand_over: bool,
}

/// The paired device's (name, id) calling with its Bearer token from the IP it paired from,
/// while remote access is on. Same check as a session.
fn paired_caller(c: &Inner, headers: &HeaderMap, from: SocketAddr) -> ApiResult<(String, String)> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    c.who(token, &from.ip().to_string())
        .filter(|_| c.on())
        .ok_or_else(|| ApiError::new(403, "forbidden", "Not paired with this device."))
}

/// Runs `f` on the app's `Local` off the async threads; `None` while none is set.
async fn on_local<T: Send + 'static>(
    c: &Inner,
    f: impl FnOnce(&dyn handover::Local) -> T + Send + 'static,
) -> Option<T> {
    let l = c
        .local
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    tokio::task::spawn_blocking(move || f(&*l)).await.ok()
}

/// A paired computer says which Roblox accounts (user ids) have a client there right now, so
/// the friend page here can name the computer an account is on. The answer says the same of
/// this computer, plus any hand-over queued for that one (Run here).
async fn running(
    State(c): State<Ctx>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    b: Result<Json<RunningBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (paired_name, id) = paired_caller(&c, &headers, from)?;
    let b = b
        .map_err(|_| ApiError::new(400, "bad_request", "Bad request"))?
        .0;
    if b.user_ids.len() > MAX_RUNNING {
        return Err(ApiError::new(400, "bad_request", "Too many accounts"));
    }
    // The name that computer gives itself, else the one it paired under.
    let name = Some(clean_name(&b.name))
        .filter(|n| !n.is_empty())
        .unwrap_or(paired_name);
    let r = Report {
        at: Instant::now(),
        name,
        ids: b.user_ids,
        ip: from.ip().to_string(),
        handover: b.can_hand_over,
    };
    lock(&c.peers).insert(id.clone(), r);
    c.tick.notify_waiters();
    let ids = on_local(&c, |l| l.running()).await.flatten();
    let mut v = json!({ "name": c.name(), "userIds": ids.unwrap_or_default() });
    // asked once: the next report shows whether they closed
    let want = lock(&c.wants).remove(&id);
    if let Some((_, ids)) = want.filter(|(at, _)| at.elapsed() < HANDOVER_WAIT) {
        v["handover"] = json!(ids);
    }
    Ok(Json(v).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandoverBody {
    user_ids: Vec<u64>,
}

/// Run here on a paired computer: close these accounts' clients here for good. Answers with
/// the ids no longer running here.
async fn handover(
    State(c): State<Ctx>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    b: Result<Json<HandoverBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    paired_caller(&c, &headers, from)?;
    let b = b
        .map_err(|_| ApiError::new(400, "bad_request", "Bad request"))?
        .0;
    if b.user_ids.len() > MAX_RUNNING {
        return Err(ApiError::new(400, "bad_request", "Too many accounts"));
    }
    let released = on_local(&c, move |l| l.release(&b.user_ids))
        .await
        .ok_or_else(|| ApiError::new(503, "unavailable", "RMI is still starting."))?
        .ok_or_else(|| ApiError::new(409, "locked", "RMI is locked on this computer."))?;
    Ok(Json(json!({ "released": released })).into_response())
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
    name: String,
}

async fn pair(
    State(c): State<Ctx>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    b: Result<Json<PairBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    if !c.on() {
        return Err(ApiError::new(
            403,
            "off",
            "Remote access is switched off on this device.",
        ));
    }
    let b = b
        .map_err(|_| ApiError::new(400, "bad_request", "Bad request"))?
        .0;
    c.take_code(&b.code)?;
    let token = new_token();
    let name = Some(clean_name(&b.name))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Device".into());
    let ip = from.ip().to_string();
    let gone = {
        let mut st = lock(&c.store);
        let before = st.paired.clone();
        // pairing again from the same device replaces its old entry (that token is dead);
        // entries without an IP are invalid and go too
        let same: Vec<String> = st
            .paired
            .iter()
            .filter(|p| p.ip == ip)
            .map(|p| p.hash[..12].to_owned())
            .collect();
        st.paired.retain(|p| p.ip != ip && !p.ip.is_empty());
        let evicted =
            (st.paired.len() >= MAX_PAIRED).then(|| st.paired.remove(0).hash[..12].to_owned());
        st.paired.push(Paired {
            hash: hash(&token),
            name,
            paired_at: now_iso(),
            ip,
        });
        if c.save(&st).is_err() {
            st.paired = before;
            return Err(ApiError::new(
                500,
                "save_failed",
                "That computer could not save the pairing. Try again.",
            ));
        }
        (same, evicted)
    };
    let live = lock(&c.live).as_ref().map(|l| l.id.clone());
    if let Some(id) = live {
        // the same device's stale session: replaced, its new token stays; evicted: removed
        if gone.0.contains(&id) {
            c.kick("replaced");
        } else if gone.1.as_ref() == Some(&id) {
            c.kick("removed");
        }
    }
    log::info!("remote: a device was paired");
    c.changed();
    Ok(Json(json!({ "token": token, "name": c.name() })).into_response())
}

async fn ws(
    State(c): State<Ctx>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    up: WebSocketUpgrade,
) -> Response {
    // held until the socket authenticates or drops
    let Ok(permit) = c.pre_auth.clone().try_acquire_owned() else {
        return ApiError::new(503, "busy", "Too many connections").into_response();
    };
    // file chunks; text stays at `MAX_MSG`, and nothing binary is taken before auth
    up.max_message_size(MAX_BIN)
        .max_frame_size(MAX_BIN)
        .on_upgrade(move |s| session(c, s, from.ip().to_string(), permit))
}

/// One send, given up after `SEND_WAIT`.
async fn put(ws: &mut WebSocket, m: Message) -> Result<(), ()> {
    match tokio::time::timeout(SEND_WAIT, ws.send(m)).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

/// A control message (`0x02` + JSON).
fn ctl_msg(v: &Value) -> Message {
    let mut b = vec![0x02];
    b.extend(serde_json::to_vec(v).unwrap_or_default());
    Message::Binary(b.into())
}

async fn ctl(ws: &mut WebSocket, v: Value) -> Result<(), ()> {
    put(ws, ctl_msg(&v)).await
}

/// Refusal before a session starts.
async fn bye(ws: &mut WebSocket, code: &str, by: Option<&str>) {
    let _ = ctl(ws, bye_msg(code, by)).await;
    let _ = put(ws, Message::Close(None)).await;
}

async fn session(
    c: Ctx,
    mut ws: WebSocket,
    from: String,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    let first = tokio::time::timeout(AUTH_WAIT, ws.recv()).await;
    let auth = match first {
        Ok(Some(Ok(Message::Text(t)))) if t.len() <= MAX_MSG => serde_json::from_str::<Value>(&t)
            .ok()
            .filter(|v| v["t"] == "auth"),
        _ => None,
    };
    let token = auth.as_ref().and_then(|v| v["token"].as_str());
    let takeover = auth.as_ref().is_some_and(|v| v["takeover"] == true);
    if !c.on() {
        let _ = ctl(
            &mut ws,
            json!({ "t": "state", "state": "off", "canInput": false }),
        )
        .await;
        return bye(&mut ws, "off", None).await;
    }
    let Some((peer, id)) = token.and_then(|t| c.who(t, &from)) else {
        log::warn!("remote: refused a session with a missing or unknown token");
        return bye(&mut ws, "auth", None).await;
    };
    drop(permit);
    if lock(&c.stopped_at).is_some_and(|t| t.elapsed() < STOP_HOLD) {
        return bye(&mut ws, "stopped", None).await;
    }
    // take the slot; the same device (a stale session of its own) or a takeover replaces
    // the holder, anyone else is told who has it
    let (tx, rx) = oneshot::channel();
    let mut tx = Some(tx);
    let until = Instant::now() + REPLACE_WAIT;
    loop {
        let holder = {
            let mut live = lock(&c.live);
            match live.as_ref() {
                // Stop or off may have come while the old session let go (`kick` takes
                // this lock too, so nothing slips in between)
                None if !c.on() => Err("off"),
                None if lock(&c.stopped_at).is_some_and(|t| t.elapsed() < STOP_HOLD) => {
                    Err("stopped")
                }
                None => {
                    *live = Some(Live {
                        peer: peer.clone(),
                        since: now_iso(),
                        control: false,
                        id: id.clone(),
                    });
                    *lock(&c.stop) = tx.take();
                    Ok(None)
                }
                Some(l) => Ok(Some((l.id == id || takeover, l.peer.clone()))),
            }
        };
        match holder {
            Err(code) => return bye(&mut ws, code, None).await,
            Ok(None) => break,
            Ok(Some((true, _))) if Instant::now() < until => {
                c.kick("replaced");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(Some((_, by))) => return bye(&mut ws, "busy", Some(&by)).await,
        }
    }
    log::info!("remote: session started");
    c.changed();
    let (started_at, t0) = (now_iso(), Instant::now());
    let end = run(&c, ws, rx).await;
    {
        let mut st = lock(&c.store);
        st.log.push(LogEntry {
            peer,
            started_at,
            duration_sec: t0.elapsed().as_secs(),
            control: end.control,
            files: end.files,
        });
        let n = st.log.len();
        st.log.drain(..n.saturating_sub(LOG_MAX));
        let _ = c.save(&st); // logged
    }
    // input is released already; free the slot (stop first: a waiting replacement then
    // finds both empty) before the goodbye
    *lock(&c.stop) = None;
    *lock(&c.live) = None;
    log::info!("remote: session ended");
    c.changed();
    if let Some(code) = end.code {
        let _ = end.out.try_send(ctl_msg(&bye_msg(code, None)));
    }
    let _ = end.out.try_send(Message::Close(None));
    drop(end.out);
    let _ = tokio::time::timeout(SEND_WAIT, end.writer).await;
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    #[default]
    Auto,
    Speed,
    Quality,
}

impl Quality {
    /// (JPEG quality, widest frame) for a `w` px wide picture; `auto` follows `level`.
    fn enc(self, level: usize, w: u32) -> (u8, u32) {
        match self {
            Quality::Auto => {
                let (q, scale) = LEVELS[level.min(LEVELS.len() - 1)];
                (q, (w.min(AUTO_MAX_W) as f32 * scale) as u32)
            }
            Quality::Speed => (50, 1920),
            Quality::Quality => (85, u32::MAX),
        }
    }

    /// (H.264 target kbit/s, widest frame) for a `w` px wide picture; `auto` follows `level`.
    fn h264(self, level: usize, w: u32) -> (u32, u32) {
        match self {
            Quality::Auto => {
                let (k, scale) = H264_LEVELS[level.min(H264_LEVELS.len() - 1)];
                (k, (w.min(AUTO_MAX_W) as f32 * scale) as u32)
            }
            Quality::Speed => (6_000, 1280),
            Quality::Quality => (25_000, u32::MAX),
        }
    }
}

/// How the encoder thread makes a frame: JPEG quality, or H.264 (target kbit/s, frame rate,
/// IDR wanted).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Codec {
    Jpeg(u8),
    H264 { kbps: u32, fps: u32, key: bool },
}

/// Auto quality and the `stats` numbers: the frames in flight (window full = congested),
/// capture and encode times; written by the capture, encode and writer sides.
struct Tune {
    level: usize,
    /// Send times of unacknowledged frames, oldest first.
    sent: VecDeque<Instant>,
    /// Since when all `MAX_UNACKED` frames are in flight, and how long they were this period.
    full_since: Option<Instant>,
    full: Duration,
    /// Round trip of the last acknowledged frame and the session's shortest one.
    rtt: Duration,
    min_rtt: Option<Duration>,
    /// Calm periods in a row; auto steps up after `CALM_TICKS`.
    calm: u32,
    frames: u32,
    bytes: u64,
    /// Capture (grab, pointer, compare) and resize + encode time this period, with counts.
    cap: (Duration, u32),
    enc: (Duration, u32),
    /// Width of the last encoded frame.
    w: u32,
    /// No H.264 encoder works here: the grabber asks for JPEG.
    jpeg_only: bool,
    /// The H.264 encoder that made the last frame (`None`: JPEG); `stats` then carries `br`
    /// and `encoder`, not `q`.
    coder: Option<String>,
}

/// Calm `STATS_EVERY` periods before auto steps quality up again (6 s).
const CALM_TICKS: u32 = 3;
/// Ack round trips below this never count as congestion.
const SLOW_RTT: Duration = Duration::from_millis(150);

impl Default for Tune {
    fn default() -> Self {
        Tune {
            level: 1,
            sent: VecDeque::new(),
            full_since: None,
            full: Duration::ZERO,
            rtt: Duration::ZERO,
            min_rtt: None,
            calm: 0,
            frames: 0,
            bytes: 0,
            cap: (Duration::ZERO, 0),
            enc: (Duration::ZERO, 0),
            w: 0,
            jpeg_only: false,
            coder: None,
        }
    }
}

impl Tune {
    fn on_sent(&mut self, len: usize) {
        self.sent.push_back(Instant::now()); // at most MAX_UNACKED long
        if self.sent.len() >= MAX_UNACKED && self.full_since.is_none() {
            self.full_since = Some(Instant::now());
        }
        self.frames += 1;
        self.bytes += len as u64;
    }

    fn on_ack(&mut self) {
        if let Some(t) = self.sent.pop_front() {
            self.rtt = t.elapsed();
            self.min_rtt = Some(self.min_rtt.map_or(self.rtt, |m| m.min(self.rtt)));
        }
        if self.sent.len() < MAX_UNACKED {
            if let Some(t) = self.full_since.take() {
                self.full += t.elapsed();
            }
        }
    }

    fn on_captured(&mut self, took: Duration) {
        self.cap.0 += took;
        self.cap.1 += 1;
    }

    fn on_encoded(&mut self, took: Duration, w: u32, coder: Option<&str>) {
        if self.coder.as_deref() != coder {
            self.coder = coder.map(str::to_owned);
        }
        self.enc.0 += took;
        self.enc.1 += 1;
        self.w = w;
    }

    /// Every `STATS_EVERY` while streaming (`auto` only): one step down when the window was
    /// full over half the period while acks take well over the link's own round trip (a far
    /// but fast link fills the window too, at its base RTT), or a frame is unacknowledged for
    /// over a second; one step up
    /// after `CALM_TICKS` periods that were nearly never full or kept the base round trip.
    /// Then the `stats` message.
    fn tick(&mut self, q: Quality, w: u32) -> Value {
        let now = Instant::now();
        let full = self.full + self.full_since.map_or(Duration::ZERO, |t| now - t);
        self.full = Duration::ZERO;
        self.full_since = self.full_since.map(|_| now);
        if q == Quality::Auto {
            let late = self
                .sent
                .front()
                .is_some_and(|t| t.elapsed() > Duration::from_secs(1));
            // queueing: the round trip grew past twice the session's best (and 150 ms)
            let slow = self.rtt > (self.min_rtt.unwrap_or_default() * 2).max(SLOW_RTT);
            if late || (full > STATS_EVERY / 2 && slow) {
                self.level = (self.level + 1).min(LEVELS.len() - 1);
                self.calm = 0;
            } else if self.frames > 0 && (full < STATS_EVERY / 10 || !slow) {
                self.calm += 1;
                if self.calm >= CALM_TICKS {
                    self.level = self.level.saturating_sub(1);
                    self.calm = 0;
                }
            } else {
                self.calm = 0;
            }
        }
        let ((jq, max_w), (br, h_w)) = (q.enc(self.level, w), q.h264(self.level, w));
        let max_w = if self.coder.is_some() { h_w } else { max_w };
        // the real scale: the adaptive step and the 1920 px cap
        let scale = (max_w.min(w) as f32 / w.max(1) as f32 * 100.0).round() / 100.0;
        let secs = STATS_EVERY.as_secs_f64();
        let mut v = json!({
            "t": "stats", "scale": scale,
            "fps": (f64::from(self.frames) / secs).round(),
            "kbps": (self.bytes as f64 * 8.0 / 1000.0 / secs).round(),
            "inflight": self.sent.len(),
        });
        // H.264: the target bitrate (kbit/s) instead of a JPEG quality
        if let Some(name) = &self.coder {
            (v["codec"], v["br"], v["encoder"]) = ("h264".into(), br.into(), name.as_str().into());
        } else {
            (v["codec"], v["q"]) = ("jpeg".into(), jq.into());
        }
        let tenth = |x: f64| (x * 10.0).round() / 10.0;
        let ms = |(d, n): (Duration, u32)| tenth(d.as_secs_f64() * 1000.0 / f64::from(n));
        if self.cap.1 > 0 {
            v["cap"] = ms(self.cap).into();
        }
        if self.enc.1 > 0 {
            v["enc"] = ms(self.enc).into();
            v["w"] = self.w.into();
        }
        if self.frames > 0 {
            v["kb"] = tenth(self.bytes as f64 / 1000.0 / f64::from(self.frames)).into();
        }
        self.frames = 0;
        self.bytes = 0;
        self.cap = (Duration::ZERO, 0);
        self.enc = (Duration::ZERO, 0);
        v
    }
}

fn fps15() -> u32 {
    15
}

/// Viewer to host (text JSON). Unknown `t` is `Other` and ignored.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
enum In {
    Start {
        #[serde(default)]
        screen: u8,
        #[serde(default)]
        quality: Quality,
        #[serde(default = "fps15", rename = "maxFps")]
        max_fps: u32,
        /// `cursor` cap: pointer left out of frames, sent as `cur`.
        #[serde(default)]
        cursor: bool,
        /// `h264` cap: codecs the viewer decodes besides JPEG (`["h264"]`).
        #[serde(default)]
        codecs: Vec<String>,
    },
    Stop,
    Ack,
    /// `h264` cap: the viewer's decoder needs a keyframe (after an error).
    Idr,
    Mm {
        x: f64,
        y: f64,
    },
    Md {
        b: u8,
        x: f64,
        y: f64,
    },
    Mu {
        b: u8,
        x: f64,
        y: f64,
    },
    Wh {
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    Kd {
        code: String,
    },
    Ku {
        code: String,
    },
    Text {
        s: String,
    },
    Combo {
        name: String,
    },
    View {
        on: bool,
    },
    /// `audio` cap: start or stop the sound stream.
    Audio {
        on: bool,
    },
    Ping {
        #[serde(default)]
        n: u64,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Stream {
    screen: u8,
    quality: Quality,
    fps: u32,
    cursor: bool,
    /// H.264 frames (`0x06`) instead of JPEG (`0x01`).
    h264: bool,
}

enum Ev {
    Move(i32, i32),
    Button(u8, bool),
    Wheel(i32, i32),
    Key(String, bool),
    Text(String),
    Lock,
}

/// 0..1 of `s` to absolute input coordinates; non-finite: `None`.
fn to_abs(s: &Screen, x: f64, y: f64) -> Option<(i32, i32)> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let f = |o: i32, size: u32, v: f64| o + (v.clamp(0.0, 1.0) * f64::from(size.max(1) - 1)) as i32;
    Some((f(s.x, s.w, x), f(s.y, s.h, y)))
}

/// The screen `id` streams: one monitor, or `ALL` (2+ monitors) as their bounding rect.
fn target(screens: &[Screen], id: u8) -> Option<Screen> {
    if id != ALL {
        return screens.iter().find(|s| s.id == id).copied();
    }
    if screens.len() < 2 {
        return None;
    }
    let x = screens.iter().map(|s| s.x).min()?;
    let y = screens.iter().map(|s| s.y).min()?;
    let r = screens.iter().map(|s| s.x + s.w as i32).max()?;
    let b = screens.iter().map(|s| s.y + s.h as i32).max()?;
    Some(Screen {
        id: ALL,
        x,
        y,
        w: (r - x).max(1) as u32,
        h: (b - y).max(1) as u32,
        primary: false,
    })
}

/// The primary screen (else the first).
fn primary(screens: &[Screen]) -> Option<Screen> {
    screens
        .iter()
        .find(|s| s.primary)
        .or(screens.first())
        .copied()
}

fn shown(screens: &[Screen]) -> Value {
    screens
        .iter()
        .map(
            |s| json!({ "id": s.id, "w": s.w, "h": s.h, "x": s.x, "y": s.y, "primary": s.primary }),
        )
        .collect()
}

/// Keys and buttons a session holds down, released when it ends.
#[derive(Default)]
struct Held {
    keys: Vec<String>,
    buttons: [bool; 3],
}

impl Held {
    fn apply(&mut self, i: &mut dyn Input, ev: Ev) {
        match ev {
            Ev::Move(x, y) => i.move_to(x, y),
            Ev::Button(b, d) => {
                if let Some(s) = self.buttons.get_mut(usize::from(b)) {
                    *s = d;
                    i.button(b, d);
                }
            }
            Ev::Wheel(dx, dy) => i.wheel(dx, dy),
            // known codes only, and only keys tracked as held go up: nothing stays stuck
            Ev::Key(code, d) => {
                let held = self.keys.contains(&code);
                if d && (held || (self.keys.len() < 32 && keys::known(&code))) {
                    if !held {
                        self.keys.push(code.clone());
                    }
                    i.key(&code, true);
                } else if !d && held {
                    self.keys.retain(|k| *k != code);
                    i.key(&code, false);
                }
            }
            Ev::Text(s) => i.text(&s),
            Ev::Lock => i.lock(),
        }
    }

    fn release(&mut self, i: &mut dyn Input) {
        for k in self.keys.drain(..).rev() {
            i.key(&k, false);
        }
        for (b, d) in self.buttons.iter_mut().enumerate() {
            if std::mem::take(d) {
                i.button(b as u8, false);
            }
        }
    }
}

/// The session's input thread: applies events in order while `alive`, then drops what is
/// still queued (a paste backlog never types on after the session) and releases everything.
fn spawn_input(
    desk: Arc<dyn Desk>,
    alive: Arc<AtomicBool>,
) -> (
    std::sync::mpsc::Sender<Ev>,
    Option<std::thread::JoinHandle<()>>,
) {
    let (tx, rx) = std::sync::mpsc::channel::<Ev>();
    let r = std::thread::Builder::new()
        .name("remote-input".into())
        .spawn(move || {
            let mut i = desk.input();
            let mut held = Held::default();
            for ev in rx {
                if !alive.load(SeqCst) {
                    break;
                }
                held.apply(&mut *i, ev);
            }
            held.release(&mut *i);
        });
    match r {
        Ok(h) => (tx, Some(h)),
        Err(e) => {
            log::error!("remote: input thread failed to start: {e}");
            (tx, None)
        }
    }
}

/// `img` resized to `w` x `h` (SIMD bilinear, like `FilterType::Triangle`).
fn scale(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    use fast_image_resize::{images, FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
    let mut out = RgbaImage::new(w, h);
    let mut r = || -> Result<(), Box<dyn std::error::Error>> {
        let src = images::ImageRef::new(img.width(), img.height(), img.as_raw(), PixelType::U8x4)?;
        let mut dst = images::Image::from_slice_u8(w, h, &mut out, PixelType::U8x4)?;
        let o = ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
            .use_alpha(false); // screens are opaque
        Ok(Resizer::new().resize(&src, &mut dst, &o)?)
    };
    if let Err(e) = r() {
        log::warn!("remote: resize failed: {e}");
    }
    out
}

/// JPEG frame message: `0x01`, screen id, JPEG (4:2:0); scaled down to at most `max_w` px
/// wide. Empty if it cannot be encoded.
pub fn encode(img: &RgbaImage, screen: u8, q: u8, max_w: u32) -> Bytes {
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
    let small;
    let img = if img.width() > max_w.max(1) {
        let w = max_w.max(1);
        let h = (u64::from(img.height()) * u64::from(w) / u64::from(img.width())).max(1) as u32;
        small = scale(img, w, h);
        &small
    } else {
        img
    };
    let (Ok(w), Ok(h)) = (u16::try_from(img.width()), u16::try_from(img.height())) else {
        log::warn!(
            "remote: frame too large for JPEG: {}x{}",
            img.width(),
            img.height()
        );
        return Bytes::new();
    };
    // a screen frame is roughly a tenth of its raw size or less
    let mut out = Vec::with_capacity(img.as_raw().len() / 10 + 2);
    out.extend([0x01, screen]);
    let mut e = Encoder::new(&mut out, q);
    e.set_sampling_factor(SamplingFactor::F_2_2);
    if let Err(e) = e.encode(img.as_raw(), w, h, ColorType::Rgba) {
        log::warn!("remote: JPEG encode failed: {e}");
        return Bytes::new();
    }
    out.into()
}

/// The picture of `t`: one monitor, or every monitor placed by its position (`ALL`, in
/// `Screen` units, so Retina captures are scaled to points).
fn grab(desk: &dyn Desk, screens: &[Screen], t: &Screen) -> Option<RgbaImage> {
    if t.id != ALL {
        return desk.grab(t.id);
    }
    let mut img = RgbaImage::new(t.w, t.h);
    for s in screens {
        let g = desk.grab(s.id)?;
        let g = if (g.width(), g.height()) == (s.w, s.h) {
            g
        } else {
            scale(&g, s.w, s.h)
        };
        image::imageops::overlay(&mut img, &g, i64::from(s.x - t.x), i64::from(s.y - t.y));
    }
    Some(img)
}

/// The last picture sent: its stream, the pointer drawn into it, the image.
type Last = Option<(Stream, Option<(i32, i32)>, Arc<RgbaImage>)>;

/// The picture of `want` (screen id, image), or `None` if it is the same as the last one
/// (the desk says nothing changed and the drawn pointer stayed, or compared byte for byte:
/// cheaper than hashing it).
fn snap(
    desk: &dyn Desk,
    screens: &[Screen],
    want: Stream,
    last: &mut Last,
) -> Option<(u8, Arc<RgbaImage>)> {
    let s = target(screens, want.screen)?;
    let cur = desk.cursor().filter(|_| !want.cursor);
    if last
        .as_ref()
        .is_some_and(|(w, c, _)| *w == want && *c == cur)
        && match s.id {
            ALL => screens.iter().all(|m| desk.unchanged(m.id)),
            id => desk.unchanged(id),
        }
    {
        return None;
    }
    let mut img = grab(desk, screens, &s)?;
    if let Some((cx, cy)) = cur {
        // screen coordinates to image pixels (the capture can be scaled, e.g. Retina)
        let px = |c: i32, o: i32, size: u32, img: u32| {
            i64::from(c - o) * i64::from(img) / i64::from(size.max(1))
        };
        let (x, y) = (
            px(cx, s.x, s.w, img.width()),
            px(cy, s.y, s.h, img.height()),
        );
        desk::draw_cursor(&mut img, x, y);
    }
    if last
        .as_ref()
        .is_some_and(|(w, _, l)| *w == want && **l == img)
    {
        return None;
    }
    let img = Arc::new(img);
    *last = Some((want, cur, img.clone()));
    Some((s.id, img))
}

/// `cur` for the pointer over `t` (0..1 of it, `vis` false outside it or hidden).
fn cur_msg(p: Option<(i32, i32)>, shape: Option<&str>, t: &Screen) -> Value {
    let rel = |c: i32, o: i32, size: u32| f64::from(c - o) / f64::from(size.max(1));
    let (x, y) = p.map_or((0.0, 0.0), |(x, y)| (rel(x, t.x, t.w), rel(y, t.y, t.h)));
    let inside = p.is_some() && (0.0..1.0).contains(&x) && (0.0..1.0).contains(&y);
    let r = |v: f64| (v.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0;
    json!({
        "t": "cur", "x": r(x), "y": r(y),
        "shape": shape.unwrap_or("default"), "vis": inside && shape.is_some(),
    })
}

struct Capture {
    desk: Arc<dyn Desk>,
    screens: watch::Sender<Vec<Screen>>,
    want: watch::Receiver<Option<Stream>>,
    frames: mpsc::Sender<Bytes>,
    /// The viewer asked for an H.264 keyframe (`idr`).
    idr: Arc<AtomicBool>,
    /// Control messages to the viewer (`cur`).
    out: mpsc::Sender<Message>,
    state: watch::Sender<(&'static str, bool)>,
    inflight: Arc<AtomicUsize>,
    tune: Arc<Mutex<Tune>>,
    alive: Arc<AtomicBool>,
}

/// A captured picture waiting for the encoder (screen id, image, codec, widest); a newer one
/// replaces it unencoded. Frames are only ever dropped here, before encoding: an H.264 frame,
/// once encoded, must reach the viewer (its decoder needs every one until the next IDR).
type Slot = (Mutex<Option<(u8, Arc<RgbaImage>, Codec, u32)>>, Condvar);

/// Capture thread: grabs at most `maxFps` (paced by deadline), only while streaming, the
/// screen is viewable and fewer than `MAX_UNACKED` frames are in flight; checks the state
/// and the monitor list every second and follows the pointer (`cursor`). A second thread
/// encodes the newest picture meanwhile, so grabbing and encoding overlap.
fn capture(c: Capture) {
    let slot: Slot = Default::default();
    std::thread::scope(|sc| {
        let (slot, frames, tune, alive) = (&slot, &c.frames, &*c.tune, &*c.alive);
        let enc = std::thread::Builder::new()
            .name("remote-encode".into())
            .spawn_scoped(sc, move || {
                encoder(slot, frames, tune, alive, Default::default())
            });
        match enc {
            Ok(_) => {
                grabber(&c, slot);
                // wake the encoder now: this thread's DXGI duplication goes once both are
                // done, and a session taking over wants it at once
                drop(lock(&slot.0));
                slot.1.notify_one();
            }
            Err(e) => log::error!("remote: encode thread failed to start: {e}"),
        }
    });
}

/// Puts a picture in `slot` for the encoder, replacing one it has not taken (that one's
/// keyframe request carries over).
fn offer(slot: &Slot, job: (u8, Arc<RgbaImage>, Codec, u32)) {
    let (id, img, mut codec, max_w) = job;
    let mut g = lock(&slot.0);
    if let (Codec::H264 { key, .. }, Some((_, _, Codec::H264 { key: true, .. }, _))) =
        (&mut codec, g.as_ref())
    {
        *key = true;
    }
    *g = Some((id, img, codec, max_w));
    drop(g);
    slot.1.notify_one();
}

/// Encodes what the grabber leaves in `slot` until the session ends or the writer is gone.
/// Each frame waits for the writer to take the one before (never replaced once encoded).
fn encoder(
    slot: &Slot,
    frames: &mpsc::Sender<Bytes>,
    tune: &Mutex<Tune>,
    alive: &AtomicBool,
    mut h264: h264::State,
) {
    while alive.load(SeqCst) {
        let job = {
            let g = lock(&slot.0);
            // the timeout only re-checks `alive`
            let (mut g, _) = slot
                .1
                .wait_timeout_while(g, Duration::from_millis(100), |j| {
                    j.is_none() && alive.load(SeqCst)
                })
                .unwrap_or_else(PoisonError::into_inner);
            g.take()
        };
        let Some((id, img, codec, max_w)) = job else {
            continue;
        };
        let t = Instant::now();
        let out = match codec {
            Codec::H264 { kbps, fps, key } => {
                match h264::frame(&mut h264, &img, id, kbps, fps, max_w, key) {
                    Ok(f) => Some(f),
                    Err(e) => {
                        // the grabber makes JPEG jobs from now on
                        log::warn!("remote: no H.264 encoder, streaming JPEG: {e}");
                        lock(tune).jpeg_only = true;
                        None
                    }
                }
            }
            _ => None,
        };
        let jpeg = out.is_none();
        let fs = out.unwrap_or_else(|| {
            h264.reset(); // a JPEG stream: the H.264 encoder's memory goes
            let q = match codec {
                Codec::Jpeg(q) => q,
                Codec::H264 { .. } => LEVELS[1].0,
            };
            vec![encode(&img, id, q, max_w)]
        });
        let coder = h264.name().filter(|_| !jpeg);
        lock(tune).on_encoded(t.elapsed(), img.width().min(max_w.max(1)), coder);
        // the writer takes each once the window has room; meanwhile the grabber keeps
        // replacing the unencoded picture in `slot`
        for mut f in fs.into_iter().filter(|f| !f.is_empty()) {
            loop {
                match frames.try_send(f) {
                    Ok(()) => break,
                    Err(mpsc::error::TrySendError::Full(b)) if alive.load(SeqCst) => {
                        f = b;
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => return,
                }
            }
        }
    }
}

fn grabber(c: &Capture, slot: &Slot) {
    let mut last = None;
    let mut checked: Option<Instant> = None;
    let mut next = Instant::now();
    let mut prev = None;
    // H.264: the next frame encoded must be an IDR (a new stream or the viewer asked)
    let mut key = false;
    // H.264 repeats of the last picture still to send (`REFINE`)
    let mut refine = 0;
    let mut cur_sent: (Instant, Option<Value>) = (Instant::now(), None);
    while c.alive.load(SeqCst) {
        if checked.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            checked = Some(Instant::now());
            // hot-plug: a new list is passed on; an empty one (display asleep, a failed
            // enumeration) keeps the old list and only stops input
            let now = c.desk.screens();
            let s = if now.is_empty() {
                (c.state.borrow().0, false)
            } else {
                c.screens.send_if_modified(|v| {
                    let new = *v != now;
                    if new {
                        v.clone_from(&now);
                    }
                    new
                });
                c.desk.state()
            };
            c.state.send_if_modified(|v| std::mem::replace(v, s) != s);
        }
        let want = *c.want.borrow();
        if want.is_none() {
            last = None; // a later start always gets a first frame
        }
        if want != prev {
            prev = want;
            next = Instant::now(); // a new start, quality or rate applies at once
            key = true;
        }
        if c.idr.swap(false, SeqCst) {
            key = true;
            last = None; // a still screen too gets its keyframe
        }
        let screens = c.screens.borrow().clone();
        // a backed-up link: pointer updates wait (the newest goes once there is room) and
        // never crowd out pongs and state changes
        if let Some(t) = want
            .filter(|w| w.cursor && cur_sent.0.elapsed() >= CUR_EVERY && c.out.capacity() > 16)
            .and_then(|w| target(&screens, w.screen))
        {
            let m = cur_msg(c.desk.cursor(), c.desk.cursor_shape(), &t);
            if cur_sent.1.as_ref() != Some(&m) && c.out.try_send(ctl_msg(&m)).is_ok() {
                cur_sent = (Instant::now(), Some(m));
            }
        }
        let ready = c.state.borrow().0 == "ready";
        let streaming = want.filter(|_| ready);
        let can = streaming.filter(|_| c.inflight.load(SeqCst) < MAX_UNACKED);
        let now = Instant::now();
        if let Some(w) = can.filter(|_| now >= next) {
            // deadline pacing: no drift; when behind, start over from now (no burst)
            let every = Duration::from_secs(1) / w.fps.clamp(1, MAX_FPS);
            next += every;
            if next <= now {
                next = now + every;
            }
            let (level, jpeg_only) = {
                let t = lock(&c.tune);
                (t.level, t.jpeg_only)
            };
            // the viewer takes H.264 and an encoder works here
            let h264 = w.h264 && !jpeg_only;
            let width = target(&screens, w.screen).map_or(0, |t| t.w);
            let (codec, max_w) = if h264 {
                let (kbps, max_w) = w.quality.h264(level, width);
                let fps = w.fps;
                (Codec::H264 { kbps, fps, key }, max_w)
            } else {
                let (q, max_w) = w.quality.enc(level, width);
                (Codec::Jpeg(q), max_w)
            };
            let shot = match snap(&*c.desk, &screens, w, &mut last) {
                Some(s) => {
                    refine = if h264 { REFINE } else { 0 };
                    Some(s)
                }
                // only once the encoder took the job before: a repeat offered over one it
                // has not taken would replace it unencoded and still be counted
                None if refine > 0 && h264 && lock(&slot.0).is_none() => {
                    refine -= 1;
                    last.as_ref()
                        .filter(|(lw, _, _)| *lw == w)
                        .map(|(_, _, img)| (w.screen, img.clone()))
                }
                None => None,
            };
            lock(&c.tune).on_captured(now.elapsed());
            if let Some((id, img)) = shot {
                offer(slot, (id, img, codec, max_w));
                key = false;
            }
        }
        let wait = match (can, streaming) {
            (Some(_), _) => next.saturating_duration_since(Instant::now()),
            // window full: an ack can come any moment
            (None, Some(_)) => Duration::from_millis(2),
            (None, None) => Duration::from_millis(30),
        };
        std::thread::sleep(wait.min(CUR_EVERY));
    }
}

/// What `run` leaves: whether input was applied, the `bye` code still to send, and the way
/// to the viewer (the writer task ends after a `Close`).
struct End {
    control: bool,
    code: Option<&'static str>,
    out: mpsc::Sender<Message>,
    writer: tokio::task::JoinHandle<()>,
    files: Vec<files::FileLog>,
}

/// What the writer task sends besides control messages.
struct Feeds {
    frames: mpsc::Receiver<Bytes>,
    inflight: Arc<AtomicUsize>,
    acked: Arc<Notify>,
    tune: Arc<Mutex<Tune>>,
    /// Downloads (header, chunks, end in order).
    bulk: mpsc::Receiver<Message>,
    sound: Arc<audio::AudioQ>,
}

/// The writer task: owns the send half. Control messages first, then sound, then the next
/// frame (each in order, none dropped) while fewer than `MAX_UNACKED` are unacknowledged,
/// then file chunks. Ends after
/// `Close` or a failed send.
async fn writer(
    mut sink: SplitSink<WebSocket, Message>,
    mut ctl: mpsc::Receiver<Message>,
    f: Feeds,
) {
    let Feeds {
        mut frames,
        inflight,
        acked,
        tune,
        mut bulk,
        sound,
    } = f;
    let (mut capturing, mut bulking) = (true, true);
    loop {
        let (m, wait) = tokio::select! {
            biased;
            m = ctl.recv() => match m {
                Some(m) => (m, SEND_WAIT),
                None => return,
            },
            () = std::future::ready(()), if sound.waiting() => match sound.pop() {
                Some(a) => (Message::Binary(a), SEND_WAIT),
                None => continue,
            },
            // new sound wakes the loop (the branch above sends it)
            () = sound.ready.notified() => continue,
            r = frames.recv(), if capturing && inflight.load(SeqCst) < MAX_UNACKED => {
                let Some(f) = r else {
                    capturing = false;
                    continue;
                };
                inflight.fetch_add(1, SeqCst);
                lock(&tune).on_sent(f.len());
                (Message::Binary(f), FRAME_WAIT)
            }
            m = bulk.recv(), if bulking => match m {
                Some(m) => (m, FRAME_WAIT),
                None => {
                    bulking = false;
                    continue;
                }
            },
            // an ack may re-enable the frame branch
            _ = acked.notified() => continue,
        };
        let close = matches!(m, Message::Close(_));
        if !matches!(tokio::time::timeout(wait, sink.send(m)).await, Ok(Ok(()))) || close {
            return;
        }
    }
}

/// First input or file action: the session (indicator, log) becomes "controlled".
fn mark_control(c: &Ctx, control: &mut bool) {
    if !*control {
        *control = true;
        if let Some(l) = lock(&c.live).as_mut() {
            l.control = true;
        }
        c.changed();
    }
}

/// The authenticated session: reads the viewer until it ends. Held input is released
/// before it returns.
async fn run(c: &Ctx, ws: WebSocket, mut stop: oneshot::Receiver<&'static str>) -> End {
    let desk = c.desk.clone();
    let mut screens = desk.screens();
    let (state, can_input) = desk.state();
    let (sink, mut rx) = ws.split();
    let (out, out_rx) = mpsc::channel::<Message>(64);
    // one encoded frame waits for the writer; the encoder holds the next until it is taken
    let (frame_tx, frame_rx) = mpsc::channel::<Bytes>(1);
    let idr = Arc::new(AtomicBool::new(false));
    let inflight = Arc::new(AtomicUsize::new(0));
    let acked = Arc::new(Notify::new());
    let tune = Arc::new(Mutex::new(Tune::default()));
    let (bulk, bulk_rx) = mpsc::channel::<Message>(4);
    let sound = Arc::new(audio::AudioQ::default());
    let writer = tokio::spawn(writer(
        sink,
        out_rx,
        Feeds {
            frames: frame_rx,
            inflight: inflight.clone(),
            acked: acked.clone(),
            tune: tune.clone(),
            bulk: bulk_rx,
            sound: sound.clone(),
        },
    ));
    let downloads = lock(&c.downloads).clone().or_else(dirs::download_dir);
    let files = files::Files::new(out.clone(), bulk, downloads);
    let mut sound_on: Option<Arc<AtomicBool>> = None;
    let mut screen = primary(&screens);
    // `all` always (every platform here can compose it): monitors come and go mid-session,
    // and the viewer offers All screens only with 2 or more
    let mut caps = vec!["screens", "cursor", "stats", "takeover", "media", "all"];
    if h264::available() {
        caps.push("h264");
    }
    if c.files_on() {
        caps.push("files");
    }
    if c.audio_on() {
        caps.push("audio");
    }
    let hello = json!({
        "t": "hello", "proto": 1, "host": c.name(), "os": desk.os(), "screens": shown(&screens),
        "screen": screen.map(|s| s.id), "state": state, "canInput": can_input,
        "terminal": false, "combos": ["lock"], "caps": caps,
    });
    let _ = out.try_send(ctl_msg(&hello));
    let (want_tx, want_rx) = watch::channel(None::<Stream>);
    let (screens_tx, mut screens_rx) = watch::channel(screens.clone());
    let (state_tx, mut state_rx) = watch::channel((state, can_input));
    let alive = Arc::new(AtomicBool::new(true));
    let cap = Capture {
        desk: desk.clone(),
        screens: screens_tx,
        want: want_rx,
        frames: frame_tx,
        idr: idr.clone(),
        out: out.clone(),
        state: state_tx,
        inflight: inflight.clone(),
        tune: tune.clone(),
        alive: alive.clone(),
    };
    if let Err(e) = std::thread::Builder::new()
        .name("remote-capture".into())
        .spawn(move || capture(cap))
    {
        log::error!("remote: capture thread failed to start: {e}");
    }
    let (input, input_thread) = spawn_input(desk.clone(), alive.clone());
    // false only once the writer is gone; a full queue (writer stuck on a frame, which its
    // own timeout ends) drops the message
    let say = |v: Value| {
        !matches!(
            out.try_send(ctl_msg(&v)),
            Err(mpsc::error::TrySendError::Closed(_))
        )
    };
    let (mut view_only, mut can, mut control) = (false, can_input, false);
    let mut idle = tokio::time::Instant::now() + IDLE;
    let mut last_idr: Option<Instant> = None;
    let mut stats =
        tokio::time::interval_at(tokio::time::Instant::now() + STATS_EVERY, STATS_EVERY);
    let mut why = None;
    loop {
        tokio::select! {
            w = &mut stop => {
                why = Some(w.unwrap_or("stopped"));
                break;
            }
            _ = tokio::time::sleep_until(idle) => {
                log::info!("remote: viewer silent for {} s, ending the session", IDLE.as_secs());
                why = Some("timeout");
                break;
            }
            // the writer gave up (a send failed or timed out)
            _ = out.closed() => break,
            _ = stats.tick() => {
                // settings switched off mid-session take effect here
                if !c.audio_on()
                    && sound_on.take().is_some_and(|s| s.swap(false, SeqCst))
                    && !say(json!({ "t": "audio", "on": false }))
                {
                    break;
                }
                if !c.files_on() {
                    files.stop_all("File transfer was switched off on the device.");
                }
                let Some(w) = *want_tx.borrow() else { continue };
                let width = screen.map_or(0, |s| s.w);
                let v = lock(&tune).tick(w.quality, width);
                if !say(v) {
                    break;
                }
            }
            m = rx.next() => {
                let Some(Ok(m)) = m else { break };
                idle = tokio::time::Instant::now() + IDLE;
                let t = match m {
                    Message::Text(t) if t.len() <= MAX_MSG => t,
                    // upload chunks (the only binary a viewer sends); only uploads that were
                    // allowed exist, and they finish like under view only
                    Message::Binary(b) => {
                        files.chunk(&b);
                        continue;
                    }
                    Message::Close(_) => break,
                    _ => continue,
                };
                // `fs` / `file` are read as plain JSON below
                let msg = serde_json::from_str::<In>(&t).unwrap_or(In::Other);
                let ev = match msg {
                    In::Start { screen: id, quality, max_fps, cursor, codecs } => {
                        if let Some(s) = target(&screens, id) {
                            screen = Some(s);
                            let fps = max_fps.clamp(1, MAX_FPS);
                            // H.264 when the viewer offers it, else JPEG (older viewers)
                            let h264 = codecs.iter().any(|c| c == "h264");
                            let _ = want_tx.send(Some(Stream { screen: id, quality, fps, cursor, h264 }));
                        }
                        continue;
                    }
                    // at most one keyframe per IDR_EVERY, however often the viewer asks
                    In::Idr if last_idr.is_none_or(|t| t.elapsed() >= IDR_EVERY) => {
                        last_idr = Some(Instant::now());
                        idr.store(true, SeqCst);
                        continue;
                    }
                    In::Idr => continue,
                    In::Stop => {
                        let _ = want_tx.send(None);
                        continue;
                    }
                    In::Ack => {
                        let _ = inflight.try_update(SeqCst, SeqCst, |n| n.checked_sub(1));
                        lock(&tune).on_ack();
                        acked.notify_one();
                        continue;
                    }
                    In::Ping { n } => {
                        if !say(json!({ "t": "pong", "n": n })) {
                            break;
                        }
                        continue;
                    }
                    In::View { on } => {
                        view_only = on;
                        continue;
                    }
                    In::Audio { on } => {
                        if !on {
                            if let Some(s) = sound_on.take() {
                                s.store(false, SeqCst);
                            }
                        // a capture that failed stored false: a new request tries again
                        } else if !sound_on.as_ref().is_some_and(|s| s.load(SeqCst)) && c.audio_on() {
                            sound_on = Some(audio::start(desk.clone(), alive.clone(), out.clone(), sound.clone()));
                        }
                        continue;
                    }
                    In::Other => {
                        let v: Value = serde_json::from_str(&t).unwrap_or_default();
                        if v["t"] == "fs" || v["t"] == "file" {
                            let refuse = if !c.files_on() {
                                Some("File transfer is switched off on that computer.")
                            } else if view_only {
                                Some("View only is on. Switch it off to transfer files.")
                            } else {
                                None
                            };
                            // a listing or transfer that started counts as control
                            if files.handle(&v, refuse) {
                                mark_control(c, &mut control);
                            }
                        }
                        continue;
                    }
                    // releases always pass (no move when blocked): nothing stays held
                    In::Mu { b, .. } if view_only || !can => Some(vec![Ev::Button(b, false)]),
                    In::Ku { code } if view_only || !can => Some(vec![Ev::Key(code, false)]),
                    _ if view_only || !can => continue,
                    In::Mm { x, y } => screen.and_then(|s| to_abs(&s, x, y)).map(|(x, y)| vec![Ev::Move(x, y)]),
                    In::Md { b, x, y } => screen.and_then(|s| to_abs(&s, x, y)).map(|(x, y)| vec![Ev::Move(x, y), Ev::Button(b, true)]),
                    In::Mu { b, x, y } => screen.and_then(|s| to_abs(&s, x, y)).map(|(x, y)| vec![Ev::Move(x, y), Ev::Button(b, false)]),
                    In::Wh { dx, dy } => Some(vec![Ev::Wheel(dx.clamp(-1200, 1200), dy.clamp(-1200, 1200))]),
                    In::Kd { code } if code.len() <= 32 => Some(vec![Ev::Key(code, true)]),
                    In::Ku { code } if code.len() <= 32 => Some(vec![Ev::Key(code, false)]),
                    // bounded by MAX_MSG
                    In::Text { s } => Some(vec![Ev::Text(s)]),
                    In::Combo { name } if name == "lock" => Some(vec![Ev::Lock]),
                    // cad: Ctrl+Alt+Del cannot be injected from a user process (refused)
                    _ => None,
                };
                let Some(ev) = ev else { continue };
                if !view_only && can {
                    mark_control(c, &mut control);
                }
                for e in ev {
                    let _ = input.send(e);
                }
            }
            r = screens_rx.changed() => {
                if r.is_err() {
                    break;
                }
                screens = screens_rx.borrow_and_update().clone();
                // keep the streamed screen if it is still there, else the primary one
                let want = *want_tx.borrow();
                let id = want.map_or(screen.map(|s| s.id), |w| Some(w.screen));
                screen = id.and_then(|id| target(&screens, id)).or_else(|| primary(&screens));
                if let (Some(w), Some(s)) = (want, screen) {
                    let _ = want_tx.send(Some(Stream { screen: s.id, ..w }));
                }
                let id = screen.map(|s| s.id);
                if !say(json!({ "t": "screens", "screens": shown(&screens), "screen": id })) {
                    break;
                }
            }
            r = state_rx.changed() => {
                if r.is_err() {
                    break;
                }
                let (s, ci) = *state_rx.borrow_and_update();
                can = ci;
                if !say(json!({ "t": "state", "state": s, "canInput": ci })) {
                    break;
                }
            }
        }
    }
    alive.store(false, SeqCst); // also stops the capture and sound threads
    files.end(); // partial uploads are deleted
    drop(input); // the input thread drops its backlog, releases held keys and buttons, ends
    if let Some(h) = input_thread {
        // ponytail: polled, not joined: a stuck OS input call must not hang the session task
        let until = Instant::now() + INPUT_WAIT;
        while !h.is_finished() && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if !h.is_finished() {
            log::warn!("remote: input thread still busy after {INPUT_WAIT:?}");
        }
    }
    let files = std::mem::take(&mut *lock(&files.log));
    End {
        control,
        code: why,
        out,
        writer,
        files,
    }
}

// ---- Tauri side -------------------------------------------------------------------------

use crate::AppState;
use tauri::Manager;

/// Hook target: show or hide the "being controlled" indicator, refresh the UI. Serialized,
/// and the window is built once then only shown and hidden: an ending session can never
/// remove the bar a replacing one just asked for.
pub fn on_change(app: &tauri::AppHandle) {
    static ONE: Mutex<()> = Mutex::new(());
    {
        let _one = lock(&ONE);
        let live = app.state::<AppState>().remote.live().is_some();
        match (live, app.get_webview_window(INDICATOR)) {
            (true, None) => open_indicator(app),
            (true, Some(w)) => {
                if let Err(e) = show_quietly(&w, true) {
                    // never controlled without the bar: fail closed
                    log::error!("remote: indicator failed to show, ending the session: {e}");
                    app.state::<AppState>().remote.stop_session();
                }
            }
            (false, Some(w)) => {
                if let Err(e) = show_quietly(&w, false) {
                    log::warn!("remote: indicator failed to hide: {e}");
                }
            }
            (false, None) => {}
        }
    }
    crate::commands::state_changed(app, &["remoteStatus"]);
}

/// Show without taking focus from what the viewer is controlling (tao's `show` activates
/// the window on Windows after its first appearance), or hide.
#[cfg(windows)]
fn show_quietly(w: &tauri::WebviewWindow, show: bool) -> tauri::Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE};
    let hw = w.hwnd()?;
    // SAFETY: the indicator's own live top-level window handle.
    unsafe { ShowWindow(hw.0 as _, if show { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
    Ok(())
}

#[cfg(not(windows))]
fn show_quietly(w: &tauri::WebviewWindow, show: bool) -> tauri::Result<()> {
    if show {
        w.show()
    } else {
        w.hide()
    }
}

fn open_indicator(app: &tauri::AppHandle) {
    const W: f64 = 380.0;
    let mut b = tauri::WebviewWindowBuilder::new(
        app,
        INDICATOR,
        tauri::WebviewUrl::App("index.html#remote-indicator".into()),
    )
    .title("Remote session")
    .inner_size(W, 56.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false);
    if let Ok(Some(m)) = app.primary_monitor() {
        let (pos, size) = (
            m.position().to_logical::<f64>(m.scale_factor()),
            m.size().to_logical::<f64>(m.scale_factor()),
        );
        b = b.position(pos.x + (size.width - W) / 2.0, pos.y + 12.0);
    }
    if let Err(e) = b.build() {
        // never controlled without the bar: fail closed
        log::error!("remote: indicator window failed, ending the session: {e}");
        app.state::<AppState>().remote.stop_session();
    }
}

#[tauri::command]
pub fn remote_status(state: tauri::State<'_, AppState>) -> RemoteStatus {
    state.remote.status()
}

#[tauri::command]
pub fn remote_pair_code(state: tauri::State<'_, AppState>) -> Result<PairCode, String> {
    state.remote.new_code()
}

/// Async: it can open or close the indicator window, which deadlocks in a sync command on
/// Windows.
#[tauri::command]
pub async fn remote_unpair(app: tauri::AppHandle, id: String) {
    app.state::<AppState>().remote.unpair(&id);
}

#[tauri::command]
pub fn remote_stop(state: tauri::State<'_, AppState>) {
    state.remote.stop_session();
}

#[cfg(test)]
mod tests;
