//! Auto-relaunch (spec §9.2). `Engine` is the pure decision core (no I/O, explicit `now`);
//! `Loop` is the app loop that feeds it hub/process/presence observations and launches.
//! Closes RMI makes itself (client helpers, replace-previous, server stop / token
//! regenerate) never relaunch: the close is final (docs/DECISIONS.md).
//! Watcher closes (M9) are not ours in that sense (spec §13: a watcher close counts as a
//! disconnect): the process poll sees the tracked client gone and sets `Obs::exited`,
//! which relaunches. Except a watcher auto reconnect close: the watcher relaunches that
//! one itself, so it calls `Engine::quiet` first (no double launch).

use super::backend::bar_of;
use super::clients::{self, Rules, Seen};
use super::LaunchReq;
use crate::accounts::PresenceState;
use crate::commands::{guard, LOCKED, WAS_LOCKED};
use crate::launch::launcher::{batch_status, start_batch, Host};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, PoisonError};
use tokio::time::{Duration, Instant};

/// Stop relaunching an account after this many relaunches inside `COOLDOWN_WINDOW`.
pub const COOLDOWN_MAX: usize = 5;
pub const COOLDOWN_WINDOW: Duration = Duration::from_secs(600);

/// One account's relaunch config (hub `relaunch` object, `seconds` = delay).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Cfg {
    pub enabled: bool,
    pub delay: Duration,
    pub place_id: Option<u64>,
    pub job_id: Option<String>,
}

impl Cfg {
    /// From the hub's `relaunch` value; a missing/invalid `seconds` uses `default_delay`.
    pub fn from_value(v: &Value, default_delay: u64) -> Self {
        Cfg {
            enabled: v["enabled"] == Value::Bool(true),
            delay: Duration::from_secs(
                v["seconds"]
                    .as_u64()
                    .map_or(default_delay, |s| s.min(86_400)),
            ),
            place_id: v["placeId"]
                .as_u64()
                .or_else(|| v["placeId"].as_str().and_then(|s| s.trim().parse().ok())),
            job_id: v["jobId"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        }
    }
}

/// What the loop saw for one account this tick.
#[derive(Clone, Debug, Default)]
pub struct Obs {
    pub account: String,
    pub cfg: Cfg,
    /// Socket connected (or, with `relaunch.usePresence`, presence in game).
    pub up: bool,
    /// Its tracked client process was running and now is not.
    pub exited: bool,
    /// An RMI launch of it is running or just ended (its own close is not a crash).
    pub own: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Launch {
        account: String,
        place_id: Option<u64>,
        job_id: Option<String>,
    },
    Note {
        account: String,
        text: String,
    },
}

#[derive(Default)]
struct St {
    prev_up: Option<bool>,
    /// `Obs::own` last tick: its rise clears `quiet`.
    prev_own: bool,
    prev_enabled: bool,
    /// Due time; `true` = a re-arm after our failed relaunch (the batch's own-launch
    /// grace must not cancel it). One field, so clearing the pending clears both.
    pending: Option<(Instant, bool)>,
    history: VecDeque<Instant>,
    last: Option<Instant>,
    count: u32,
    stopped: bool,
    /// RMI closed this client itself: no trigger until it is up again (or RMI launches it).
    quiet: bool,
    /// `unquiet`: the watcher gave up its reconnect; arm once if still down (not own).
    handback: bool,
    /// Our relaunch is running; cleared by `finished`/`retry`.
    inflight: bool,
}

#[derive(Default)]
pub struct Engine {
    st: HashMap<String, St>,
    last_launch: Option<Instant>,
    /// Bumped by `reset`: a relaunch started before it never launches or re-arms after.
    epoch: u64,
}

fn k(account: &str) -> String {
    account.trim().to_lowercase()
}

fn note(out: &mut Vec<Act>, a: &str, t: &str) {
    out.push(Act::Note {
        account: a.into(),
        text: t.into(),
    })
}

impl Engine {
    /// Feed one tick. At most one `Launch` per call, `spacing` apart globally.
    pub fn tick(&mut self, now: Instant, obs: &[Obs], spacing: Duration) -> Vec<Act> {
        let mut out = vec![];
        for o in obs {
            let s = self.st.entry(k(&o.account)).or_default();
            s.prev_enabled = o.cfg.enabled;
            let (fell, rose) = (
                s.prev_up == Some(true) && !o.up,
                s.prev_up == Some(false) && o.up,
            );
            // Up again, also when the edge was forgotten by `reset` (no "rise" then). Or a
            // new RMI launch of it started (e.g. the watcher's reconnect): its client is
            // watched again. Only the start: a close during that launch stays quiet.
            if (o.up && s.prev_up != Some(true)) || (o.own && !s.prev_own) {
                s.quiet = false;
            }
            s.prev_own = o.own;
            if o.up {
                s.handback = false;
            }
            s.prev_up = Some(o.up);
            if !o.cfg.enabled {
                s.pending = None;
                s.handback = false;
            } else if rose {
                if s.pending.take().is_some() {
                    note(&mut out, &o.account, "Reconnected; relaunch cancelled");
                }
            } else if o.own {
                if s.pending.is_some_and(|p| !p.1) {
                    s.pending = None;
                    note(
                        &mut out,
                        &o.account,
                        "Launched from RMI; relaunch cancelled",
                    );
                }
            } else if (fell || o.exited || s.handback)
                && s.pending.is_none()
                && !s.stopped
                && !s.quiet
                && !s.inflight
            {
                s.handback = false;
                s.pending = Some((now + o.cfg.delay, false));
                let why = if o.exited {
                    "Client exited"
                } else {
                    "Disconnected"
                };
                let msg = format!("{why}; relaunching in {} s", o.cfg.delay.as_secs());
                note(&mut out, &o.account, &msg);
            }
        }
        let mut due: Vec<(Instant, &Obs)> = obs
            .iter()
            .filter_map(|o| Some((self.st[&k(&o.account)].pending?.0, o)))
            .filter(|(p, _)| *p <= now)
            .collect();
        due.sort_by(|a, b| (a.0, &a.1.account).cmp(&(b.0, &b.1.account)));
        if self.last_launch.is_some_and(|l| now < l + spacing) {
            return out;
        }
        for (_, o) in due {
            let s = self.st.get_mut(&k(&o.account)).unwrap();
            s.pending = None;
            while s
                .history
                .front()
                .is_some_and(|t| *t + COOLDOWN_WINDOW <= now)
            {
                s.history.pop_front();
            }
            if s.history.len() >= COOLDOWN_MAX {
                s.stopped = true;
                let msg = format!(
                    "Stopped auto-relaunch: {COOLDOWN_MAX} relaunches in {} min. Turn it off and on to resume.",
                    COOLDOWN_WINDOW.as_secs() / 60
                );
                log::warn!("relaunch {:?}: {msg}", o.account);
                note(&mut out, &o.account, &msg);
                continue;
            }
            s.history.push_back(now);
            s.last = Some(now);
            s.count += 1;
            s.inflight = true;
            self.last_launch = Some(now);
            out.push(Act::Launch {
                account: o.account.clone(),
                place_id: o.cfg.place_id,
                job_id: o.cfg.job_id.clone(),
            });
            break;
        }
        out
    }

    /// Vault locked: due relaunches are dropped (with a note); nothing else changes.
    pub fn locked(&mut self, now: Instant) -> Vec<Act> {
        let mut out = vec![];
        for (a, s) in &mut self.st {
            if s.pending.is_some_and(|p| p.0 <= now) {
                s.pending = None;
                note(&mut out, a, "Vault locked; relaunch skipped");
            }
        }
        out
    }

    /// RMI closed this account's client itself: final, no relaunch until it is up again.
    pub fn quiet(&mut self, account: &str) {
        let s = self.st.entry(k(account)).or_default();
        s.quiet = true;
        s.pending = None;
    }

    /// The watcher's reconnect after `quiet` gave up or failed: auto-relaunch owns the
    /// account again and relaunches it if it is still down. Also when `quiet` is already
    /// clear: the reconnect's own launch start clears it, and that batch may launch nothing.
    /// `tick` still arms only while the account is down and not in an RMI launch.
    pub fn unquiet(&mut self, account: &str) {
        if let Some(s) = self.st.get_mut(&k(account)) {
            s.quiet = false;
            s.handback = true;
        }
    }

    /// The account's relaunch toggle went off → on: clear the cooldown stop. Returns
    /// whether that changed anything.
    pub fn resume(&mut self, account: &str) -> bool {
        let Some(s) = self.st.get_mut(&k(account)) else {
            return false;
        };
        let was = s.stopped;
        s.stopped = false;
        s.history.clear();
        was
    }

    /// Current epoch; hand it back to `finished`/`retry`.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Our relaunch batch ended. `rearm` = it failed: try again after that delay (counted).
    /// Ignored after a `reset` since it started.
    pub fn finished(&mut self, account: &str, now: Instant, rearm: Option<Duration>, epoch: u64) {
        let cur = self.epoch == epoch;
        if let Some(s) = self.st.get_mut(&k(account)) {
            s.inflight = false;
            if let Some(d) = rearm.filter(|_| cur && s.prev_enabled && !s.stopped) {
                s.pending = Some((now + d, true));
            }
        }
    }

    /// Launch was not attempted (offline, unknown account): undo its count; `delay` = try
    /// again after it (not after a `reset` since it started).
    /// Returns whether it re-armed.
    pub fn retry(
        &mut self,
        account: &str,
        now: Instant,
        delay: Option<Duration>,
        epoch: u64,
    ) -> bool {
        let cur = self.epoch == epoch;
        let Some(s) = self.st.get_mut(&k(account)) else {
            return false;
        };
        s.history.pop_back();
        s.count = s.count.saturating_sub(1);
        s.inflight = false;
        let d = delay.filter(|_| cur && s.prev_enabled && !s.stopped);
        if let Some(d) = d {
            s.pending = Some((now + d, false));
        }
        d.is_some()
    }

    /// Forget edges and pending relaunches (server stop, token regenerate, automation
    /// toggled): what follows is not a fall. Counts and cooldowns stay. Relaunches already
    /// spawned neither launch nor re-arm after this (`epoch`).
    pub fn reset(&mut self) {
        self.epoch += 1;
        for s in self.st.values_mut() {
            s.prev_up = None;
            s.pending = None;
        }
    }

    /// `{pendingAt, lastRelaunch, count, stopped, launching}`; times are unix ms.
    pub fn status(&self, account: &str, now: Instant, now_ms: i64) -> Value {
        let Some(s) = self.st.get(&k(account)) else {
            return json!({ "pendingAt": null, "lastRelaunch": null, "count": 0, "stopped": false, "launching": false });
        };
        let ms = |t: Instant| {
            let d = |a: Instant, b: Instant| a.saturating_duration_since(b).as_millis() as i64;
            now_ms + d(t, now) - d(now, t)
        };
        json!({
            "pendingAt": s.pending.map(|p| ms(p.0)),
            "lastRelaunch": s.last.map(ms),
            "count": s.count,
            "stopped": s.stopped,
            "launching": s.inflight,
        })
    }
}

// ---- App loop ----------------------------------------------------------------------------

/// Process poll / helper cadence, in loop ticks (1 s each).
const POLL_EVERY: u64 = 5;
/// Stop following a relaunch batch after this long (counts as failed).
const FOLLOW_MAX: Duration = Duration::from_secs(600);

/// Any HTTP answer from roblox.com within 5 s counts as online. The app has no proxy
/// setting, so a plain client matches how the Roblox client itself connects.
pub(crate) async fn online() -> bool {
    static C: LazyLock<Option<reqwest::Client>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok()
    });
    match C.as_ref() {
        Some(c) => c.head("https://www.roblox.com").send().await.is_ok(),
        None => false,
    }
}

fn num(s: &Value, p: &str, d: u64) -> u64 {
    s.pointer(p).and_then(Value::as_u64).unwrap_or(d)
}

/// A relaunch to run: hub account name, saved account id, target, retry delay.
#[derive(Debug)]
struct Relaunch {
    account: String,
    id: String,
    place_id: Option<u64>,
    job_id: Option<String>,
    delay: Duration,
    check: bool,
    /// Engine epoch when it was decided (`Engine::reset` cancels it).
    epoch: u64,
}

#[derive(Default)]
struct Loop {
    n: u64,
    /// Automation was on / servers were up at the last tick.
    active: bool,
    serving: bool,
    /// Trackers whose client was running at the last poll.
    had: HashSet<String>,
    seen: HashMap<u32, Seen>,
    global_since: Option<Instant>,
    last_min: Option<Instant>,
    /// Helper-close survivors: pid -> when the close failed (retried after a back-off).
    failed: HashMap<u32, Instant>,
}

/// Back-off before a helper close survivor is tried again (as the watcher's).
const RETRY_FAILED_CLOSE: Duration = Duration::from_secs(60);

/// Start the 1 s relaunch + helper loop (runs for the app's lifetime; idle while
/// automation is off).
pub fn spawn<H: Host>(h: H) {
    tauri::async_runtime::spawn(async move {
        let h2 = h.clone();
        let helpers =
            tokio::task::spawn_blocking(move || h2.st().platform.capabilities().client_helpers)
                .await
                .unwrap_or(false);
        let mut l = Loop::default();
        let mut serving = false;
        let mut iv = tokio::time::interval(Duration::from_secs(1));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            iv.tick().await;
            serving = h.st().automation.serving().unwrap_or(serving);
            for r in l.step(&h, helpers, serving).await {
                tokio::spawn(relaunch(h.clone(), r));
            }
        }
    });
}

/// Clears the relaunch's in-flight mark on drop (also if the task panics); `rearm` set =
/// it failed, try again after that delay.
struct Fin<'a> {
    auto: &'a super::Automation,
    account: &'a str,
    rearm: Option<Duration>,
    epoch: u64,
}

impl Drop for Fin<'_> {
    fn drop(&mut self) {
        let mut e = self
            .auto
            .relaunch
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        e.finished(self.account, Instant::now(), self.rearm, self.epoch);
        drop(e);
        // "Launching" ends now: refresh the snapshot (not only via an earlier note).
        self.auto.ctx.hub.touch(self.account);
    }
}

/// Launch, then follow the batch to its end: success clears the in-flight mark, failure
/// re-arms (counts toward the cooldown) and posts why. A reset since it was decided
/// (server stop, token regenerate) cancels it before launch and drops its re-arm.
async fn relaunch<H: Host>(h: H, r: Relaunch) {
    let auto = &h.st().automation;
    let eng = || auto.relaunch.lock().unwrap_or_else(PoisonError::into_inner);
    let hub = &auto.ctx.hub;
    let mut fin = Fin {
        auto,
        account: &r.account,
        rearm: None,
        epoch: r.epoch,
    };
    if r.check && !online().await {
        if eng().retry(&r.account, Instant::now(), Some(r.delay), r.epoch) {
            let msg = format!("Offline; retrying in {} s", r.delay.as_secs());
            hub.note(&r.account, "warn", &msg);
        }
        return;
    }
    let req = LaunchReq {
        place_id: r.place_id,
        job_id: r.job_id,
        ..Default::default()
    };
    let (bar, given) = bar_of(&req);
    // Engine lock spans the epoch check and the start: a reset either precedes both or
    // follows the start (then only the re-arm is dropped).
    let started = {
        let mut e = eng();
        if e.epoch() != r.epoch {
            // never launched: undo its count, like the offline path
            e.retry(&r.account, Instant::now(), None, r.epoch);
            return;
        }
        // Backstop: paused, or launched by another path (Launch, friend, watcher), after this
        // relaunch was decided.
        if h.st().automation.own_running(&r.id)
            || guard(&h.st().vault)
                .as_ref()
                .is_some_and(|v| crate::accounts::friend_paused(v.payload(), &r.id))
        {
            e.retry(&r.account, Instant::now(), None, r.epoch);
            return;
        }
        start_batch(&h, vec![r.id], bar, given, false)
    };
    let why = match started {
        Err(e) if e == LOCKED || e == WAS_LOCKED => "the vault is locked".into(),
        Err(e) => e,
        Ok(b) => {
            log::info!("relaunch {:?}: launch {b}", r.account);
            hub.note(&r.account, "relaunch", "Relaunching");
            let end = Instant::now() + FOLLOW_MAX;
            let done = loop {
                match batch_status(&b) {
                    Some(v) if v["done"] == true => break Some(v),
                    None => break None,
                    _ if Instant::now() >= end => break None,
                    _ => tokio::time::sleep(Duration::from_secs(1)).await,
                }
            };
            match done {
                Some(v) if v["launched"].as_u64().unwrap_or(0) > 0 => {
                    hub.note(&r.account, "relaunch", "Relaunched");
                    return;
                }
                Some(v) => v["message"]
                    .as_str()
                    .or(v["state"].as_str())
                    .unwrap_or("launch failed")
                    .to_owned(),
                None => {
                    // Evicted or still running after FOLLOW_MAX: it may yet launch, so a
                    // re-arm could launch twice. The next exit/disconnect re-triggers.
                    log::warn!("relaunch {:?}: launch status lost", r.account);
                    let t = "Relaunch status lost; not retried";
                    hub.note(&r.account, "warn", t);
                    return;
                }
            }
        }
    };
    fin.rearm = Some(r.delay);
    log::warn!("relaunch {:?} failed: {why}", r.account);
    let msg = format!(
        "Relaunch failed: {why}; retrying in {} s",
        r.delay.as_secs()
    );
    hub.note(&r.account, "error", &msg);
}

impl Loop {
    /// One 1 s tick. Returns the relaunches to start.
    async fn step<H: Host>(&mut self, h: &H, helpers: bool, serving: bool) -> Vec<Relaunch> {
        let st = h.st();
        let auto = &st.automation;
        let eng = || auto.relaunch.lock().unwrap_or_else(PoisonError::into_inner);
        let s = st
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if s.pointer("/automation/enabled") != Some(&Value::Bool(true)) {
            if self.active {
                *self = Loop::default();
                eng().reset();
                auto.ctx.hub.touch_all();
            }
            return vec![];
        }
        self.active = true;
        // Toggles turned off → on since the last tick (any path, even while it slept).
        for a in auto.ctx.hub.take_reenabled() {
            if eng().resume(&a) {
                auto.ctx.hub.touch(&a);
            }
        }
        if self.serving && !serving {
            eng().reset();
            auto.ctx.hub.touch_all();
        }
        self.serving = serving;
        let now = Instant::now();
        // (username, id, trackerId, friendPaused) of every saved account.
        let accts: Vec<(String, String, String, bool)> = {
            let g = guard(&st.vault);
            match g.as_ref() {
                Some(v) => v
                    .payload()
                    .accounts
                    .iter()
                    .map(|a| {
                        let (u, i, t) = (a.username.clone(), a.id.clone(), a.tracker_id.clone());
                        (u, i, t, a.friend_paused)
                    })
                    .collect(),
                None => {
                    drop(g);
                    // Frozen while locked: no polls, helper timers or edges; only due
                    // relaunches are skipped.
                    if serving {
                        for a in eng().locked(now) {
                            if let Act::Note { account, text } = a {
                                auto.ctx.hub.note(&account, "relaunch", &text);
                            }
                        }
                    }
                    return vec![];
                }
            }
        };
        let mut exited = HashSet::new();
        if self.n.is_multiple_of(POLL_EVERY) {
            let trackers: Vec<String> = accts.iter().map(|a| a.2.clone()).collect();
            let h2 = h.clone();
            let live =
                tokio::task::spawn_blocking(move || h2.st().platform.tracked_clients(&trackers))
                    .await
                    .unwrap_or_default();
            let alive: HashSet<String> = live.iter().map(|(_, t)| t.clone()).collect();
            exited = self.had.difference(&alive).cloned().collect();
            self.had = alive;
            if helpers {
                self.helpers(h, &s, &live, &accts, now).await;
            }
        }
        self.n += 1;
        if !serving {
            return vec![];
        }

        // Presence switched off empties the presence map: fall back to the socket, else
        // every account would look disconnected.
        let use_presence = s.pointer("/relaunch/usePresence") == Some(&Value::Bool(true))
            && s.pointer("/presence/enabled") != Some(&Value::Bool(false));
        let presence = st
            .accounts
            .presence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let default_delay = num(&s, "/relaunch/defaultDelaySec", 60);
        let find = |name: &str| accts.iter().find(|a| a.0.eq_ignore_ascii_case(name));
        let spacing = Duration::from_secs(num(&s, "/relaunch/spacingSec", 9));
        // Engine lock spans the hub read: a stop/regenerate either precedes it or resets.
        let mut e = eng();
        let (view, moved) = auto.ctx.hub.relaunch_view();
        // Re-keyed by `hello`: the client lives on under its new name, not a fall.
        for a in &moved {
            e.quiet(a);
        }
        let obs: Vec<Obs> = view
            .into_iter()
            .map(|(account, connected, r, hub_place)| {
                let mut cfg = Cfg::from_value(&r, default_delay);
                if cfg.place_id.is_none() {
                    // Fall back to the place the script last reported (no job: it may be gone).
                    cfg.place_id = hub_place
                        .and_then(|p| Cfg::from_value(&json!({ "placeId": p }), 0).place_id);
                }
                let acct = find(&account);
                // A friend disconnected it: never relaunch (cfg off clears any pending).
                if acct.is_some_and(|a| a.3) {
                    cfg.enabled = false;
                }
                let up = if use_presence {
                    acct.and_then(|a| presence.get(&a.1))
                        .is_some_and(|p| p.state == PresenceState::InGame)
                } else {
                    connected
                };
                Obs {
                    exited: acct.is_some_and(|a| exited.contains(&a.2)),
                    own: acct.is_some_and(|a| auto.own_busy(&a.1)),
                    account,
                    cfg,
                    up,
                }
            })
            .collect();
        let acts = e.tick(now, &obs, spacing);
        let check = s.pointer("/relaunch/checkInternet") != Some(&Value::Bool(false));
        let mut out = vec![];
        let mut notes = vec![];
        for a in acts {
            match a {
                Act::Note { account, text } => notes.push((account, "relaunch", text)),
                Act::Launch {
                    account,
                    place_id,
                    job_id,
                } => match find(&account) {
                    None => {
                        let ep = e.epoch();
                        e.retry(&account, now, None, ep); // not a relaunch: uncounted
                        let t = "No saved account with this name; not relaunched";
                        notes.push((account, "warn", t.into()));
                    }
                    Some(a) => out.push(Relaunch {
                        id: a.1.clone(),
                        delay: obs
                            .iter()
                            .find(|o| o.account == account)
                            .map_or(Duration::from_secs(default_delay), |o| o.cfg.delay),
                        account,
                        place_id,
                        job_id,
                        check,
                        epoch: e.epoch(),
                    }),
                },
            }
        }
        drop(e);
        for (account, level, text) in notes {
            log::info!("relaunch {account:?}: {text}");
            auto.ctx.hub.note(&account, level, &text);
        }
        out
    }

    /// Minimize / close RMI-launched clients per `clients.*`. Only tracker-id clients are
    /// in `live`, so nothing else is touched. A helper close is final (no relaunch).
    async fn helpers<H: Host>(
        &mut self,
        h: &H,
        s: &Value,
        live: &[(u32, String)],
        accts: &[(String, String, String, bool)],
        now: Instant,
    ) {
        clients::update(&mut self.seen, live, now);
        if self.seen.is_empty() {
            self.global_since = None;
            return;
        }
        let global_since = *self.global_since.get_or_insert(now);
        let every = num(s, "/clients/minimizeEverySec", 0);
        if every > 0
            && self
                .last_min
                .is_none_or(|t| now >= t + Duration::from_secs(every))
        {
            self.last_min = Some(now);
            let c: Vec<(u32, String)> = self
                .seen
                .iter()
                .map(|(p, x)| (*p, x.tracker.clone()))
                .collect();
            let h = h.clone();
            let _ = tokio::task::spawn_blocking(move || h.st().platform.minimize(&c)).await;
        }
        let mins = num(s, "/clients/closeAfterMin", 0);
        let r = Rules {
            close_after: (mins > 0).then(|| Duration::from_secs(mins * 60)),
            global: s.pointer("/clients/closeScope").and_then(Value::as_str) == Some("global"),
            max: num(s, "/clients/maxInstances", 0) as usize,
        };
        self.failed.retain(|p, _| self.seen.contains_key(p));
        let mut close = clients::to_close(&self.seen, r, global_since, now);
        close.retain(|c| {
            self.failed
                .get(&c.0)
                .is_none_or(|t| now >= *t + RETRY_FAILED_CLOSE)
        });
        if r.global && r.close_after.is_some_and(|d| global_since + d <= now) {
            self.global_since = Some(now);
        }
        if close.is_empty() {
            return;
        }
        // Forget them now so the next poll does not close them twice while they exit.
        let stash: Vec<(u32, clients::Seen)> = close
            .iter()
            .filter_map(|c| self.seen.remove(&c.0).map(|x| (c.0, x)))
            .collect();
        // Quiet only accounts left with no client: an account's exit fires only then.
        let quieted: Vec<(String, String)> = accts
            .iter()
            .filter(|a| {
                close.iter().any(|c| c.1 == a.2) && !self.seen.values().any(|x| x.tracker == a.2)
            })
            .map(|a| (a.0.clone(), a.2.clone()))
            .collect();
        {
            let mut e = h
                .st()
                .automation
                .relaunch
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for (name, _) in &quieted {
                e.quiet(name);
            }
        }
        let h2 = h.clone();
        let failed = tokio::task::spawn_blocking(move || {
            let mut failed = vec![];
            for (pid, t) in close {
                // Verified close of this pid only: re-checked as this tracker's player.
                match h2.st().platform.close_pid(pid, &t) {
                    Ok(true) => log::info!("client helper closed pid {pid}"),
                    Ok(false) => {
                        log::warn!("client helper: pid {pid} survived the close");
                        failed.push(pid);
                    }
                    Err(e) => {
                        log::warn!("client helper close pid {pid} failed: {e}");
                        failed.push(pid);
                    }
                }
            }
            failed
        })
        .await
        .unwrap_or_default();
        // Survivors: keep their first-seen time (retried, not "newest") and hand their
        // account back to auto-relaunch, as the watcher does after a failed close.
        for (pid, x) in stash.into_iter().filter(|(p, _)| failed.contains(p)) {
            if let Some((name, _)) = quieted.iter().find(|(_, t)| *t == x.tracker) {
                h.st()
                    .automation
                    .relaunch
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .unquiet(name);
            }
            self.failed.insert(pid, now);
            self.seen.insert(pid, x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(delay: u64) -> Cfg {
        Cfg {
            enabled: true,
            delay: Duration::from_secs(delay),
            place_id: Some(5),
            job_id: None,
        }
    }

    fn obs(account: &str, up: bool) -> Obs {
        Obs {
            account: account.into(),
            cfg: cfg(60),
            up,
            ..Default::default()
        }
    }

    const SP: Duration = Duration::from_secs(9);

    fn launches(a: &[Act]) -> Vec<&str> {
        a.iter()
            .filter_map(|a| match a {
                Act::Launch { account, .. } => Some(account.as_str()),
                Act::Note { .. } => None,
            })
            .collect()
    }

    async fn at(s: u64) -> Instant {
        tokio::time::advance(Duration::from_secs(s)).await;
        Instant::now()
    }

    #[test]
    fn cfg_parses_hub_value_with_default_delay() {
        let c = Cfg::from_value(&json!({"enabled": true, "placeId": "77", "jobId": " "}), 60);
        assert_eq!(
            c,
            Cfg {
                enabled: true,
                delay: Duration::from_secs(60),
                place_id: Some(77),
                job_id: None
            }
        );
        let c = Cfg::from_value(&json!({"seconds": 30, "jobId": "j"}), 60);
        assert!(
            !c.enabled && c.delay == Duration::from_secs(30) && c.job_id.as_deref() == Some("j")
        );
        assert!(!Cfg::from_value(&Value::Null, 60).enabled);
    }

    #[tokio::test(start_paused = true)]
    async fn disconnect_relaunches_after_delay() {
        let mut e = Engine::default();
        let t0 = Instant::now();
        assert!(e.tick(t0, &[obs("a", true)], SP).is_empty());
        assert!(launches(&e.tick(at(1).await, &[obs("a", false)], SP)).is_empty());
        assert!(launches(&e.tick(at(59).await, &[obs("a", false)], SP)).is_empty());
        let acts = e.tick(at(1).await, &[obs("a", false)], SP);
        assert_eq!(
            acts,
            vec![Act::Launch {
                account: "a".into(),
                place_id: Some(5),
                job_id: None
            }]
        );
        let s = e.status("A", Instant::now(), 1_000);
        assert_eq!(s["count"], 1);
        assert_eq!(s["launching"], true);
        assert_eq!(s["lastRelaunch"], 1_000);
        assert_eq!(s["pendingAt"], Value::Null);
        // Fires once.
        assert!(launches(&e.tick(at(100).await, &[obs("a", false)], SP)).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn never_up_does_not_relaunch_and_disabled_does_nothing() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", false)], SP);
        assert!(launches(&e.tick(at(120).await, &[obs("a", false)], SP)).is_empty());
        let mut off = obs("b", true);
        off.cfg.enabled = false;
        e.tick(Instant::now(), &[off.clone()], SP);
        off.up = false;
        e.tick(Instant::now(), &[off.clone()], SP);
        assert!(launches(&e.tick(at(120).await, &[off], SP)).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn process_exit_triggers_even_while_never_connected() {
        let mut e = Engine::default();
        let mut o = obs("a", false);
        e.tick(Instant::now(), std::slice::from_ref(&o), SP);
        o.exited = true;
        e.tick(at(1).await, &[o.clone()], SP);
        o.exited = false;
        let s = e.status("a", Instant::now(), 0);
        assert_eq!(s["pendingAt"], 60_000);
        assert_eq!(launches(&e.tick(at(60).await, &[o], SP)), ["a"]);
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_cancels_pending() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        let acts = e.tick(at(10).await, &[obs("a", true)], SP);
        assert!(acts.iter().any(|a| matches!(a, Act::Note { .. })));
        assert!(launches(&e.tick(at(100).await, &[obs("a", true)], SP)).is_empty());
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
    }

    #[tokio::test(start_paused = true)]
    async fn spacing_staggers_due_accounts_in_order() {
        let mut e = Engine::default();
        let all = |up| [obs("a", up), obs("b", up), obs("c", up)];
        e.tick(Instant::now(), &all(true), SP);
        e.tick(
            at(1).await,
            &[obs("b", false), obs("a", true), obs("c", true)],
            SP,
        );
        e.tick(at(1).await, &all(false), SP);
        // b due at 61, a/c at 62.
        assert_eq!(launches(&e.tick(at(60).await, &all(false), SP)), ["b"]);
        assert!(launches(&e.tick(at(8).await, &all(false), SP)).is_empty());
        assert_eq!(launches(&e.tick(at(1).await, &all(false), SP)), ["a"]);
        assert_eq!(launches(&e.tick(at(9).await, &all(false), SP)), ["c"]);
    }

    #[tokio::test(start_paused = true)]
    async fn locked_vault_skips_and_drops_pending() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert!(e.locked(at(30).await).is_empty(), "not due yet: kept");
        let acts = e.locked(at(30).await);
        assert!(acts.iter().any(|a| matches!(a, Act::Note { .. })));
        assert!(launches(&e.tick(at(60).await, &[obs("a", false)], SP)).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn cooldown_after_five_in_ten_minutes_until_reenabled() {
        let mut e = Engine::default();
        let mut o = obs("a", true);
        o.cfg.delay = Duration::from_secs(10);
        let cycle = |e: &mut Engine, o: &Obs| {
            let mut d = o.clone();
            d.up = false;
            e.tick(Instant::now(), std::slice::from_ref(o), SP);
            e.tick(Instant::now(), &[d], SP);
        };
        for _ in 0..5 {
            cycle(&mut e, &o);
            let mut d = o.clone();
            d.up = false;
            assert_eq!(launches(&e.tick(at(10).await, &[d], SP)), ["a"]);
            e.finished("a", Instant::now(), None, 0);
            at(5).await;
        }
        cycle(&mut e, &o);
        let mut d = o.clone();
        d.up = false;
        let acts = e.tick(at(10).await, &[d.clone()], SP);
        assert!(launches(&acts).is_empty());
        assert!(acts.iter().any(|a| matches!(a, Act::Note { .. })));
        assert_eq!(e.status("a", Instant::now(), 0)["stopped"], true);
        // Re-enabling clears the stop.
        assert!(e.resume("A"));
        cycle(&mut e, &o);
        assert_eq!(launches(&e.tick(at(10).await, &[d], SP)), ["a"]);
    }

    /// A reset forgets the edge: an account already up after it is "up again" for quiet,
    /// so its next real crash relaunches.
    #[tokio::test(start_paused = true)]
    async fn quiet_clears_when_up_after_reset() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.quiet("a");
        e.reset();
        e.tick(at(1).await, &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_ne!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
    }

    /// A relaunch decided before a reset neither re-arms nor retries after it.
    #[tokio::test(start_paused = true)]
    async fn reset_drops_inflight_rearm_and_retry() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_eq!(
            launches(&e.tick(at(60).await, &[obs("a", false)], SP)),
            ["a"]
        );
        let ep = e.epoch();
        e.reset();
        e.finished("a", Instant::now(), Some(Duration::from_secs(5)), ep);
        assert!(!e.retry("a", Instant::now(), Some(Duration::from_secs(5)), ep));
        let s = e.status("a", Instant::now(), 0);
        assert_eq!(
            (s["pendingAt"].clone(), s["launching"].clone()),
            (Value::Null, json!(false))
        );
    }

    /// A lock drops a due re-arm; a later RMI launch still cancels the next normal pending.
    #[tokio::test(start_paused = true)]
    async fn lock_clears_rearm_flag_with_pending() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        e.tick(at(60).await, &[obs("a", false)], SP);
        e.finished("a", Instant::now(), Some(Duration::from_secs(5)), e.epoch());
        e.locked(at(5).await);
        // An exit with no reconnect in between arms a normal pending.
        let ex = Obs {
            exited: true,
            ..obs("a", false)
        };
        e.tick(at(1).await, &[ex], SP);
        assert_ne!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        let own = Obs {
            own: true,
            ..obs("a", false)
        };
        e.tick(at(1).await, &[own], SP);
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_rearms_without_counting() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_eq!(
            launches(&e.tick(at(60).await, &[obs("a", false)], SP)),
            ["a"]
        );
        e.retry("a", Instant::now(), Some(Duration::from_secs(30)), 0);
        let s = e.status("a", Instant::now(), 0);
        assert_eq!(s["count"], 0);
        assert_eq!(s["pendingAt"], 30_000);
        assert!(launches(&e.tick(at(29).await, &[obs("a", false)], SP)).is_empty());
        assert_eq!(
            launches(&e.tick(at(1).await, &[obs("a", false)], SP)),
            ["a"]
        );
    }

    /// I2: our own launch (and its replace-previous close) is not a crash; while our
    /// relaunch is in flight, its old client's exit is ignored too.
    #[tokio::test(start_paused = true)]
    async fn own_launch_and_inflight_suppress_triggers() {
        let mut e = Engine::default();
        let own = |up, exited| Obs {
            own: true,
            exited,
            ..obs("a", up)
        };
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[own(false, true)], SP);
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        // A pending relaunch is cancelled by an RMI launch.
        e.tick(at(1).await, &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        let acts = e.tick(at(1).await, &[own(false, false)], SP);
        assert!(acts.iter().any(|a| matches!(a, Act::Note { .. })));
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        // In flight: the old client's exit seen after launch does not re-arm.
        e.tick(at(1).await, &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_eq!(
            launches(&e.tick(at(60).await, &[obs("a", false)], SP)),
            ["a"]
        );
        let ex = Obs {
            exited: true,
            ..obs("a", false)
        };
        e.tick(at(5).await, std::slice::from_ref(&ex), SP);
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        e.finished("a", Instant::now(), None, 0);
        e.tick(at(1).await, &[ex], SP);
        assert_ne!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
    }

    /// I4: a failed relaunch re-arms and counts toward the cooldown.
    #[tokio::test(start_paused = true)]
    async fn failed_relaunch_rearms_and_counts() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_eq!(
            launches(&e.tick(at(60).await, &[obs("a", false)], SP)),
            ["a"]
        );
        e.finished("a", Instant::now(), Some(Duration::from_secs(60)), 0);
        let s = e.status("a", Instant::now(), 0);
        assert_eq!(
            (s["count"].clone(), s["pendingAt"].clone()),
            (json!(1), json!(60_000))
        );
        assert_eq!(
            launches(&e.tick(at(60).await, &[obs("a", false)], SP)),
            ["a"]
        );
        assert_eq!(e.status("a", Instant::now(), 0)["count"], 2);
    }

    /// Helper close: final until the account is up again.
    #[tokio::test(start_paused = true)]
    async fn quiet_until_up_again() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.quiet("A");
        e.tick(at(1).await, &[obs("a", false)], SP);
        e.tick(
            at(5).await,
            &[Obs {
                exited: true,
                ..obs("a", false)
            }],
            SP,
        );
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        e.tick(at(1).await, &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_ne!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
    }

    /// Watcher reconnect launched: the new client crashing before it is up relaunches. A
    /// close during that launch (helper) stays quiet.
    #[tokio::test(start_paused = true)]
    async fn quiet_clears_when_rmi_launches_again() {
        let own = |own| Obs {
            own,
            ..obs("a", false)
        };
        let exited = Obs {
            exited: true,
            ..obs("a", false)
        };
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", false)], SP);
        e.quiet("a");
        e.tick(at(1).await, std::slice::from_ref(&exited), SP); // the watcher's close
        e.tick(at(5).await, &[own(true)], SP); // its reconnect launch
        e.quiet("a"); // closed again during that launch
        e.tick(at(1).await, &[own(true)], SP);
        e.tick(at(30).await, &[own(false)], SP);
        e.tick(at(1).await, std::slice::from_ref(&exited), SP);
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        e.tick(at(5).await, &[own(true)], SP); // a later launch
        e.tick(at(30).await, &[own(false)], SP);
        e.tick(at(1).await, &[exited], SP); // its client crashed
        assert_ne!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
    }

    /// Watcher reconnect gave up: relaunch takes over while the account is still down.
    #[tokio::test(start_paused = true)]
    async fn unquiet_hands_back() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.quiet("a");
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        e.unquiet("a");
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_ne!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        // The reconnect's launch start cleared quiet, then the batch launched nothing:
        // still handed back once it is no longer an RMI launch.
        let own = |own| Obs {
            own,
            ..obs("a", false)
        };
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.quiet("a");
        e.tick(at(1).await, &[obs("a", false)], SP);
        e.tick(at(5).await, &[own(true)], SP);
        e.unquiet("a");
        e.tick(at(1).await, &[own(true)], SP);
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        e.tick(at(1).await, &[own(false)], SP);
        assert_ne!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
        // Up: no hand-back.
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.unquiet("a");
        e.tick(at(1).await, &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", true)], SP);
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
    }

    // ---- Loop with a fake platform + host --------------------------------------------

    use crate::accounts::Presence;
    use crate::automation::backend::tests::{host_with, TH};
    use crate::platform::{Capabilities, Platform, RobloxInstall};
    use std::sync::{Arc, Mutex};

    const TA: &str = "100000";
    const TB: &str = "200000";

    #[derive(Clone, Default)]
    struct Plat {
        live: Arc<Mutex<Vec<(u32, String)>>>,
        closed: Arc<Mutex<Vec<String>>>,
        /// close_pid leaves the client running (survivor).
        stuck: Arc<Mutex<bool>>,
    }
    impl Platform for Plat {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn roblox_install(&self) -> Option<RobloxInstall> {
            None
        }
        fn open_uri(&self, _: &str) -> std::io::Result<()> {
            panic!("no real launches in tests")
        }
        fn tracked_clients(&self, t: &[String]) -> Vec<(u32, String)> {
            let l = self.live.lock().unwrap();
            l.iter().filter(|c| t.contains(&c.1)).cloned().collect()
        }
        fn close_pid(&self, pid: u32, t: &str) -> std::io::Result<bool> {
            self.closed.lock().unwrap().push(t.into());
            if *self.stuck.lock().unwrap() {
                return Ok(false);
            }
            self.live.lock().unwrap().retain(|c| c.0 != pid);
            Ok(true)
        }
    }

    impl Plat {
        fn set(&self, v: &[(u32, &str)]) {
            *self.live.lock().unwrap() = v.iter().map(|(p, t)| (*p, t.to_string())).collect();
        }
    }

    fn fixture(dir: &std::path::Path) -> (TH, Plat) {
        let p = Plat::default();
        let h = host_with(dir, Box::new(p.clone()));
        set(&h, "/automation/enabled", json!(true));
        (h, p)
    }

    fn set(h: &TH, ptr: &str, v: Value) {
        *h.0.settings.lock().unwrap().pointer_mut(ptr).unwrap() = v;
    }

    /// Connected hub client with relaunch on (`delay` s).
    fn arm(h: &TH, name: &str, delay: u64) -> u64 {
        let hub = &h.0.automation.ctx.hub;
        let id = hub.fake_connect(name);
        let r = json!({ "enabled": true, "seconds": delay, "placeId": 5 });
        hub.set_relaunch(name, &r).unwrap();
        id
    }

    /// One process poll (5 ticks, 1 s apart). Returns relaunched account ids.
    async fn poll(l: &mut Loop, h: &TH, serving: bool) -> Vec<String> {
        let mut out = vec![];
        for _ in 0..POLL_EVERY {
            out.extend(l.step(h, true, serving).await.into_iter().map(|r| r.id));
            tokio::time::advance(Duration::from_secs(1)).await;
        }
        out
    }

    fn pending(h: &TH, a: &str) -> Value {
        let e = h.0.automation.relaunch.lock().unwrap();
        e.status(a, Instant::now(), 0)["pendingAt"].clone()
    }

    fn in_game() -> Presence {
        Presence {
            state: PresenceState::InGame,
            place_id: None,
            universe_id: None,
            game_name: None,
            job_id: None,
            updated_at: String::new(),
        }
    }

    fn console(h: &TH, a: &str) -> String {
        h.0.automation
            .ctx
            .hub
            .snapshot()
            .into_iter()
            .filter(|s| s.account.eq_ignore_ascii_case(a))
            .flat_map(|s| s.console.into_iter().map(|l| l.text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn pause(h: &TH, id: &str) {
        let mut g = h.0.vault.lock().unwrap();
        for a in g.as_mut().unwrap().payload_mut().accounts.iter_mut() {
            a.friend_paused = a.id == id;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn paused_account_never_relaunches() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        let mut l = Loop::default();
        arm(&h, "Alice", 0);
        p.set(&[(1, TA)]);
        assert!(poll(&mut l, &h, true).await.is_empty());
        pause(&h, "A");
        p.set(&[]);
        for _ in 0..3 {
            assert!(poll(&mut l, &h, true).await.is_empty());
        }
    }

    /// Review Focus 3: a relaunch already counting down is dropped by a later pause.
    #[tokio::test(start_paused = true)]
    async fn pending_relaunch_dropped_when_paused() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        let mut l = Loop::default();
        arm(&h, "Alice", 60);
        p.set(&[(1, TA)]);
        poll(&mut l, &h, true).await;
        p.set(&[]);
        assert!(poll(&mut l, &h, true).await.is_empty());
        assert_ne!(pending(&h, "Alice"), Value::Null);
        pause(&h, "A");
        for _ in 0..14 {
            assert!(poll(&mut l, &h, true).await.is_empty());
        }
        assert_eq!(pending(&h, "Alice"), Value::Null);
    }

    #[tokio::test(start_paused = true)]
    async fn loop_relaunches_on_process_exit() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        let mut l = Loop::default();
        arm(&h, "Alice", 0);
        p.set(&[(1, TA)]);
        assert!(poll(&mut l, &h, true).await.is_empty());
        p.set(&[]);
        assert_eq!(poll(&mut l, &h, true).await, ["A"]);
    }

    /// Watcher auto reconnect quiets the account before its close: no second launch.
    #[tokio::test(start_paused = true)]
    async fn loop_watcher_reconnect_close_never_relaunches() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        let mut l = Loop::default();
        arm(&h, "Alice", 0);
        p.set(&[(1, TA)]);
        assert!(poll(&mut l, &h, true).await.is_empty());
        h.0.automation.relaunch.lock().unwrap().quiet("alice");
        p.set(&[]);
        for _ in 0..3 {
            assert!(poll(&mut l, &h, true).await.is_empty());
        }
        assert_eq!(pending(&h, "Alice"), Value::Null);
    }

    #[tokio::test(start_paused = true)]
    async fn loop_presence_mode_uses_in_game() {
        let d = tempfile::tempdir().unwrap();
        let (h, _) = fixture(d.path());
        set(&h, "/relaunch/usePresence", json!(true));
        let mut l = Loop::default();
        arm(&h, "Alice", 0);
        h.0.accounts
            .presence
            .lock()
            .unwrap()
            .insert("A".into(), in_game());
        assert!(poll(&mut l, &h, true).await.is_empty());
        h.0.accounts.presence.lock().unwrap().clear();
        assert_eq!(poll(&mut l, &h, true).await, ["A"]);
    }

    /// Presence off (empty map): usePresence falls back to the socket.
    #[tokio::test(start_paused = true)]
    async fn loop_presence_mode_with_presence_off_uses_socket() {
        let d = tempfile::tempdir().unwrap();
        let (h, _) = fixture(d.path());
        set(&h, "/relaunch/usePresence", json!(true));
        set(&h, "/presence/enabled", json!(false));
        let mut l = Loop::default();
        let ca = arm(&h, "Alice", 0);
        for _ in 0..3 {
            assert!(poll(&mut l, &h, true).await.is_empty());
        }
        h.0.automation.ctx.hub.fake_disconnect(ca);
        assert_eq!(poll(&mut l, &h, true).await, ["A"]);
    }

    /// I3: a maxInstances close is final; its exit and socket drop never relaunch.
    #[tokio::test(start_paused = true)]
    async fn loop_helper_close_never_relaunches() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        set(&h, "/clients/maxInstances", json!(1));
        let mut l = Loop::default();
        let ca = arm(&h, "Alice", 0);
        arm(&h, "bob", 0);
        p.set(&[(1, TA)]);
        assert!(poll(&mut l, &h, true).await.is_empty());
        p.set(&[(1, TA), (2, TB)]);
        assert!(poll(&mut l, &h, true).await.is_empty());
        assert_eq!(*p.closed.lock().unwrap(), [TA]);
        h.0.automation.ctx.hub.fake_disconnect(ca);
        for _ in 0..3 {
            assert!(poll(&mut l, &h, true).await.is_empty());
        }
        assert_eq!(pending(&h, "Alice"), Value::Null);
        assert_eq!(*p.closed.lock().unwrap(), [TA], "no rotation");
    }

    /// A close survivor is retried after a back-off, not on every poll.
    #[tokio::test(start_paused = true)]
    async fn helper_close_survivor_backs_off() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        set(&h, "/clients/maxInstances", json!(1));
        *p.stuck.lock().unwrap() = true;
        let mut l = Loop::default();
        p.set(&[(1, TA)]);
        poll(&mut l, &h, true).await;
        p.set(&[(1, TA), (2, TB)]);
        for _ in 0..6 {
            poll(&mut l, &h, true).await;
        }
        assert_eq!(*p.closed.lock().unwrap(), [TA], "one try in 30 s");
        for _ in 0..7 {
            poll(&mut l, &h, true).await;
        }
        assert_eq!(*p.closed.lock().unwrap(), [TA, TA], "retried after 60 s");
    }

    /// I1: no relaunch while the servers are down; a token regenerate is not a fall.
    #[tokio::test(start_paused = true)]
    async fn loop_ignores_server_stop_and_token_regenerate() {
        let d = tempfile::tempdir().unwrap();
        let (h, _) = fixture(d.path());
        let mut l = Loop::default();
        let ca = arm(&h, "Alice", 0);
        arm(&h, "bob", 0);
        assert!(poll(&mut l, &h, true).await.is_empty());
        h.0.automation.ctx.hub.fake_disconnect(ca);
        assert!(poll(&mut l, &h, false).await.is_empty());
        assert!(
            poll(&mut l, &h, true).await.is_empty(),
            "stop reset the edge"
        );
        let _ =
            h.0.automation
                .regenerate_token(d.path(), || Err("no keyring in tests".into()));
        assert!(poll(&mut l, &h, true).await.is_empty());
        assert_eq!(pending(&h, "bob"), Value::Null);
    }

    /// I5: automation off → on starts clean.
    #[tokio::test(start_paused = true)]
    async fn loop_resets_when_automation_toggles() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        let mut l = Loop::default();
        let ca = arm(&h, "Alice", 60);
        p.set(&[(1, TA)]);
        poll(&mut l, &h, true).await;
        h.0.automation.ctx.hub.fake_disconnect(ca);
        poll(&mut l, &h, true).await;
        assert_ne!(pending(&h, "Alice"), Value::Null);
        set(&h, "/automation/enabled", json!(false));
        poll(&mut l, &h, true).await;
        p.set(&[]);
        set(&h, "/automation/enabled", json!(true));
        assert_eq!(pending(&h, "Alice"), Value::Null);
        assert!(l.had.is_empty());
        assert!(poll(&mut l, &h, true).await.is_empty());
    }

    /// Locked vault freezes helpers and edges: no timer reset, no spurious notes.
    #[tokio::test(start_paused = true)]
    async fn loop_freezes_while_locked() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        set(&h, "/relaunch/usePresence", json!(true));
        let mut l = Loop::default();
        arm(&h, "Alice", 0);
        h.0.accounts
            .presence
            .lock()
            .unwrap()
            .insert("A".into(), in_game());
        p.set(&[(1, TA)]);
        poll(&mut l, &h, true).await;
        let since = l.seen[&1].since;
        let v = h.0.vault.lock().unwrap().take();
        poll(&mut l, &h, true).await;
        poll(&mut l, &h, true).await;
        *h.0.vault.lock().unwrap() = v;
        assert!(poll(&mut l, &h, true).await.is_empty());
        assert_eq!(l.seen[&1].since, since);
        assert!(!console(&h, "Alice").contains("Disconnected"));
    }

    /// I4: a relaunch whose batch fails re-arms (counted) and says why.
    #[tokio::test]
    async fn relaunch_follows_batch_and_rearms_on_failure() {
        let d = tempfile::tempdir().unwrap();
        let (h, _) = fixture(d.path());
        arm(&h, "Alice", 0);
        {
            let mut e = h.0.automation.relaunch.lock().unwrap();
            let o = Obs {
                account: "Alice".into(),
                cfg: cfg(0),
                ..Default::default()
            };
            e.tick(Instant::now(), &[Obs { exited: true, ..o }], SP);
        }
        let r = Relaunch {
            account: "Alice".into(),
            id: "A".into(),
            place_id: Some(5),
            job_id: None,
            delay: Duration::from_secs(60),
            check: false,
            epoch: 0,
        };
        relaunch(h.clone(), r).await;
        assert!(console(&h, "Alice").contains("Relaunch failed"));
        assert_ne!(pending(&h, "Alice"), Value::Null);
        let e = h.0.automation.relaunch.lock().unwrap();
        assert_eq!(e.status("Alice", Instant::now(), 0)["count"], 1);
    }

    /// Another RMI launch of the account started after this relaunch was decided: no second
    /// launch (it would close the new client or run two of the account).
    #[tokio::test]
    async fn relaunch_skips_when_account_already_launching() {
        let d = tempfile::tempdir().unwrap();
        let (h, _) = fixture(d.path());
        arm(&h, "Alice", 0);
        {
            let mut e = h.0.automation.relaunch.lock().unwrap();
            let o = Obs {
                account: "Alice".into(),
                cfg: cfg(0),
                ..Default::default()
            };
            e.tick(Instant::now(), &[Obs { exited: true, ..o }], SP);
        }
        h.0.automation.own_begin(&["A".into()]);
        let r = Relaunch {
            account: "Alice".into(),
            id: "A".into(),
            place_id: Some(5),
            job_id: None,
            delay: Duration::from_secs(60),
            check: false,
            epoch: 0,
        };
        relaunch(h.clone(), r).await;
        assert!(!console(&h, "Alice").contains("Relaunch"), "never launched");
        let e = h.0.automation.relaunch.lock().unwrap();
        assert_eq!(
            e.status("Alice", Instant::now(), 0)["count"],
            0,
            "not counted"
        );
    }

    /// I4 in the loop: a failed relaunch re-arms, and its batch's own-launch grace does
    /// not cancel that re-arm, so a second launch happens.
    #[tokio::test]
    async fn loop_failed_relaunch_launches_again() {
        let d = tempfile::tempdir().unwrap();
        let (h, p) = fixture(d.path());
        set(&h, "/relaunch/checkInternet", json!(false));
        set(&h, "/relaunch/spacingSec", json!(0));
        let mut l = Loop::default();
        arm(&h, "Alice", 0);
        p.set(&[(1, TA)]);
        let steps = async |l: &mut Loop| {
            let mut out = vec![];
            for _ in 0..POLL_EVERY {
                out.extend(l.step(&h, true, true).await);
            }
            out
        };
        assert!(steps(&mut l).await.is_empty());
        p.set(&[]);
        let mut r = steps(&mut l).await;
        assert_eq!(r.len(), 1);
        relaunch(h.clone(), r.remove(0)).await;
        assert!(console(&h, "Alice").contains("Relaunch failed"));
        assert!(h.0.automation.own_busy("A"), "inside the batch grace");
        let again: Vec<String> = steps(&mut l).await.into_iter().map(|r| r.id).collect();
        assert_eq!(again, ["A"]);
        assert!(!console(&h, "Alice").contains("relaunch cancelled"));
    }

    /// Toggle off → on clears the cooldown stop even while the servers are down.
    #[tokio::test(start_paused = true)]
    async fn loop_reenable_clears_stop_while_not_serving() {
        let d = tempfile::tempdir().unwrap();
        let (h, _) = fixture(d.path());
        let mut l = Loop::default();
        arm(&h, "Alice", 0);
        h.0.automation
            .relaunch
            .lock()
            .unwrap()
            .st
            .entry("alice".into())
            .or_default()
            .stopped = true;
        let hub = &h.0.automation.ctx.hub;
        hub.set_relaunch("Alice", &json!({ "enabled": false }))
            .unwrap();
        hub.set_relaunch("Alice", &json!({ "enabled": true }))
            .unwrap();
        poll(&mut l, &h, false).await;
        let e = h.0.automation.relaunch.lock().unwrap();
        assert_eq!(e.status("Alice", Instant::now(), 0)["stopped"], false);
    }

    /// A `hello` re-key moves the client to another name: the old name did not crash.
    #[tokio::test(start_paused = true)]
    async fn loop_hello_rekey_is_not_a_fall() {
        let d = tempfile::tempdir().unwrap();
        let (h, _) = fixture(d.path());
        let mut l = Loop::default();
        let id = arm(&h, "Alice", 60);
        poll(&mut l, &h, true).await;
        let m = crate::automation::protocol::Msg::new("hello", None, json!({ "account": "bob" }));
        h.0.automation.ctx.hub.handle(id, m);
        poll(&mut l, &h, true).await;
        assert_eq!(pending(&h, "Alice"), Value::Null);
    }

    /// A launch for a hub name with no saved account does not count toward the cooldown.
    #[tokio::test(start_paused = true)]
    async fn loop_unknown_account_is_uncounted() {
        let d = tempfile::tempdir().unwrap();
        let (h, _) = fixture(d.path());
        let mut l = Loop::default();
        let id = arm(&h, "Nobody", 0);
        poll(&mut l, &h, true).await;
        h.0.automation.ctx.hub.fake_disconnect(id);
        assert!(poll(&mut l, &h, true).await.is_empty());
        assert!(console(&h, "Nobody").contains("No saved account"));
        let e = h.0.automation.relaunch.lock().unwrap();
        assert_eq!(e.status("Nobody", Instant::now(), 0)["count"], 0);
    }

    #[test]
    fn cfg_clamps_delay_and_rejects_bad_values() {
        let c = Cfg::from_value(&json!({"enabled": "true"}), 0);
        assert!(!c.enabled, "enabled needs a real true");
        let c = Cfg::from_value(&json!({"seconds": 999_999_999u64}), 60);
        assert_eq!(c.delay, Duration::from_secs(86_400));
        assert_eq!(
            Cfg::from_value(&json!({"seconds": "30"}), 60).delay,
            Duration::from_secs(60)
        );
        assert_eq!(
            Cfg::from_value(&json!({"placeId": " 12 "}), 0).place_id,
            Some(12)
        );
        assert_eq!(
            Cfg::from_value(&json!({"placeId": "abc"}), 0).place_id,
            None
        );
        assert_eq!(Cfg::from_value(&json!({"placeId": -1}), 0).place_id, None);
    }

    /// Relaunches older than `COOLDOWN_WINDOW` no longer count: a sixth one after the
    /// window is allowed, not a stop.
    #[tokio::test(start_paused = true)]
    async fn cooldown_forgets_relaunches_older_than_window() {
        let mut e = Engine::default();
        let mut o = obs("a", true);
        o.cfg.delay = Duration::from_secs(1);
        let mut d = o.clone();
        d.up = false;
        e.tick(Instant::now(), std::slice::from_ref(&o), SP);
        for _ in 0..COOLDOWN_MAX {
            e.tick(Instant::now(), std::slice::from_ref(&d), SP);
            let acts = e.tick(at(1).await, std::slice::from_ref(&d), SP);
            assert_eq!(launches(&acts), ["a"]);
            e.finished("a", Instant::now(), None, 0);
            e.tick(Instant::now(), std::slice::from_ref(&o), SP);
            at(10).await;
        }
        at(COOLDOWN_WINDOW.as_secs()).await;
        e.tick(Instant::now(), std::slice::from_ref(&d), SP);
        let acts = e.tick(at(1).await, std::slice::from_ref(&d), SP);
        assert_eq!(launches(&acts), ["a"]);
        assert_eq!(e.status("a", Instant::now(), 0)["stopped"], false);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_relaunch_does_not_rearm_when_toggle_is_off() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_eq!(
            launches(&e.tick(at(60).await, &[obs("a", false)], SP)),
            ["a"]
        );
        let mut off = obs("a", false);
        off.cfg.enabled = false;
        e.tick(Instant::now(), &[off], SP);
        e.finished("a", Instant::now(), Some(Duration::from_secs(5)), e.epoch());
        assert_eq!(e.status("a", Instant::now(), 0)["pendingAt"], Value::Null);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_without_delay_gives_up_and_uncounts() {
        let mut e = Engine::default();
        e.tick(Instant::now(), &[obs("a", true)], SP);
        e.tick(at(1).await, &[obs("a", false)], SP);
        assert_eq!(
            launches(&e.tick(at(60).await, &[obs("a", false)], SP)),
            ["a"]
        );
        assert!(!e.retry("a", Instant::now(), None, 0));
        let s = e.status("a", Instant::now(), 0);
        assert_eq!(
            (
                s["count"].clone(),
                s["pendingAt"].clone(),
                s["launching"].clone()
            ),
            (json!(0), Value::Null, json!(false))
        );
        assert!(!e.resume("never-seen"), "nothing to clear");
    }
}
