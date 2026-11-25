//! Scan loop plus the per-OS process/window calls.

use super::{
    is_game_client, is_player_log, log_settled,
    logparse::{LogRules, Srv},
    persist_rect, pick_log,
    rules::{cooldown_open, decide, is_game_title, keep_skipped, rejoin, Cfg, Rejoin, Snap},
    tracker_from_cmdline, AccountLookup, ClientInfo, VaultLookup, PROCESS_NAME,
};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::{AppHandle, Manager};

struct Win {
    /// left, top, right, bottom; `None` while minimized.
    rect: Option<[i32; 4]>,
    title: String,
}

impl Win {
    fn area(&self) -> i64 {
        self.rect.map_or(0, |[l, t, r, b]| {
            i64::from(r - l).max(0) * i64::from(b - t).max(0)
        })
    }
}

/// Win32 lives in `platform::windows`, shared with the launcher.
#[cfg(windows)]
mod sys {
    use super::Win;
    use crate::platform::windows as w;
    use std::collections::HashSet;
    use zeroize::Zeroizing;

    /// Visible, unowned top-level windows of `pids`: (pid, window).
    pub fn windows_of(pids: &HashSet<u32>) -> Vec<(u32, Win)> {
        let pids: Vec<u32> = pids.iter().copied().collect();
        w::windows_of(&pids)
            .into_iter()
            .map(|(p, h)| {
                (
                    p,
                    Win {
                        rect: w::rect(h),
                        title: w::title(h),
                    },
                )
            })
            .collect()
    }

    pub fn close_verified(pid: u32, started: i64) -> Result<bool, String> {
        w::close_verified(pid, Some(started), None)
    }

    pub fn cmdline(pid: u32) -> Option<Zeroizing<String>> {
        w::cmdline(pid)
    }

    pub fn start_ms(pid: u32) -> Option<i64> {
        w::start_ms(pid)
    }

    /// Foreground window (as isize, HWND is not Send) and its pid.
    pub fn foreground() -> Option<(isize, u32)> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };
        // SAFETY: plain queries; a null window is handled.
        unsafe {
            let h = GetForegroundWindow();
            if h.is_null() {
                return None;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(h, &mut pid);
            Some((h as isize, pid))
        }
    }

    /// Give the foreground back to `prev`; Windows may refuse (we are a background
    /// process), then minimize `stolen`: SW_MINIMIZE hands activation to the next window
    /// in z-order (normally `prev`); SW_SHOWMINNOACTIVE would leave a foreground window
    /// foreground.
    pub fn refocus(prev: isize, stolen: isize) {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, IsWindow, SetForegroundWindow, ShowWindowAsync, SW_MINIMIZE,
        };
        let (prev, stolen) = (prev as HWND, stolen as HWND);
        // SAFETY: both handles are checked by the calls themselves (stale handles fail);
        // ShowWindowAsync never blocks on a hung window.
        unsafe {
            if IsWindow(prev) == 0
                || SetForegroundWindow(prev) == 0
                || GetForegroundWindow() != prev
            {
                ShowWindowAsync(stolen, SW_MINIMIZE);
            }
        }
    }

    pub fn fullscreen(h: isize) -> bool {
        w::fullscreen(h)
    }

    pub fn is_tracked(pid: u32, tracker: &str) -> bool {
        w::cmdline(pid).is_some_and(|c| w::has_tracker(&c, tracker))
    }

    /// Retitle `pid`'s game windows that show "Roblox" or `name` but not `want`; true if
    /// one was changed.
    pub fn retitle(pid: u32, name: &str, want: &str) -> bool {
        let mut done = false;
        for (_, h) in w::game_windows(&[pid]) {
            if super::needs_retitle(&w::title(h), name, want) {
                done |= w::set_title(h, want);
            }
        }
        done
    }

    /// Watcher-off path: game windows first (no process snapshot), command lines only of
    /// the few whose title may be wrong. `accounts`: (tracker id, username).
    pub fn retitle_all(accounts: &[(String, String)], hidden: bool) {
        for (pid, h) in w::all_game_windows() {
            let cur = w::title(h);
            if !super::retitle_candidate(&cur, hidden, accounts) {
                continue;
            }
            let Some(tr) = w::cmdline(pid).and_then(|c| super::tracker_from_cmdline(&c)) else {
                continue;
            };
            let Some((_, name)) = accounts.iter().find(|a| a.0 == tr) else {
                continue;
            };
            let want = if hidden { "Roblox" } else { name.as_str() };
            if super::needs_retitle(&cur, name, want) && w::set_title(h, want) {
                log::info!("window of {name} retitled");
            }
        }
    }
}

/// macOS: processes via sysinfo, windows via Accessibility, both in `platform::macos`.
#[cfg(target_os = "macos")]
mod sys {
    use super::Win;
    use crate::platform::macos as m;
    use std::collections::HashSet;
    use zeroize::Zeroizing;

    /// Standard windows of `pids`: (pid, window) with rect as [left, top, right, bottom].
    pub fn windows_of(pids: &HashSet<u32>) -> Vec<(u32, Win)> {
        pids.iter()
            .flat_map(|&p| m::ax::windows(p).into_iter().map(move |w| (p, w)))
            .map(|(p, w)| {
                let rect = (!m::ax::minimized(&w))
                    .then(|| m::ax::frame(&w))
                    .flatten()
                    .map(|[x, y, w, h]| [x, y, x + w, y + h]);
                let title = m::ax::title(&w).unwrap_or_default();
                (p, Win { rect, title })
            })
            .collect()
    }

    pub fn close_verified(pid: u32, started: i64) -> Result<bool, String> {
        m::close_verified(pid, Some(started), None)
    }

    pub fn cmdline(pid: u32) -> Option<Zeroizing<String>> {
        m::cmdline(pid)
    }

    pub fn start_ms(pid: u32) -> Option<i64> {
        m::start_ms(pid)
    }

    /// Frontmost app as (its pid as the handle, pid).
    pub fn foreground() -> Option<(isize, u32)> {
        m::ax::frontmost_pid().map(|p| (p as isize, p))
    }

    /// Make `prev`'s app frontmost again.
    pub fn refocus(prev: isize, _stolen: isize) {
        m::ax::activate(prev as u32);
    }

    /// Windows only.
    pub fn fullscreen(_: isize) -> bool {
        false
    }

    pub fn is_tracked(pid: u32, tracker: &str) -> bool {
        m::cmdline(pid).is_some_and(|c| m::has_tracker(&c, tracker))
    }

    /// Windows only: titles are not re-applied on macOS.
    pub fn retitle(_: u32, _: &str, _: &str) -> bool {
        false
    }
}

/// Linux: processes from /proc; no window access (Wayland), so titles and rects stay empty.
#[cfg(target_os = "linux")]
mod sys {
    use super::Win;
    use std::collections::HashSet;
    use zeroize::Zeroizing;

    pub fn windows_of(_: &HashSet<u32>) -> Vec<(u32, Win)> {
        Vec::new()
    }

    /// Closes the whole client the process belongs to (several processes per Sober client).
    pub fn close_verified(pid: u32, _started: i64) -> Result<bool, String> {
        let cmd = cmdline(pid).ok_or("The client is gone.")?;
        let tracker = super::tracker_from_cmdline(&cmd).ok_or("Not a client RMI launched.")?;
        Ok(crate::platform::linux::stop(&tracker) > 0)
    }

    pub fn cmdline(pid: u32) -> Option<Zeroizing<String>> {
        let raw = Zeroizing::new(std::fs::read(format!("/proc/{pid}/cmdline")).ok()?);
        let args: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
        Some(Zeroizing::new(
            String::from_utf8_lossy(&args.join(&b' ')).into_owned(),
        ))
    }

    /// Start time from /proc/<pid>/stat field 22 (clock ticks since boot) plus the boot time.
    pub fn start_ms(pid: u32) -> Option<i64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // the command name (field 2) may hold spaces and parentheses: count from its end
        let ticks: i64 = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(19)?
            .parse()
            .ok()?;
        let boot_s: i64 = std::fs::read_to_string("/proc/stat")
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("btime "))?
            .trim()
            .parse()
            .ok()?;
        // ponytail: assumes 100 clock ticks a second (every mainstream Linux build); when it
        // is not, `start_ms_or` sees the mismatch and uses the second-resolution start.
        Some(boot_s * 1000 + ticks * 10)
    }

    pub fn foreground() -> Option<(isize, u32)> {
        None
    }

    pub fn refocus(_: isize, _: isize) {}

    pub fn fullscreen(_: isize) -> bool {
        false
    }

    pub fn is_tracked(pid: u32, tracker: &str) -> bool {
        cmdline(pid).is_some_and(|c| super::tracker_from_cmdline(&c).as_deref() == Some(tracker))
    }
    pub fn retitle(_: u32, _: &str, _: &str) -> bool {
        false
    }
}

pub(crate) use sys::start_ms;
use sys::{close_verified, cmdline, retitle, windows_of};

/// A tracked client's window shows Roblox's own title or the username but not the one
/// RMI wants (Roblox resets it to "Roblox" on a return to its home app; privacy toggles
/// flip `want`). Any other title (a bootstrapper's) is left alone.
fn needs_retitle(cur: &str, name: &str, want: &str) -> bool {
    let cur = cur.trim();
    cur != want && (cur == "Roblox" || cur == name)
}

/// Before knowing a window's account: could `needs_retitle` hold for it? Correct titles
/// never qualify, so a settled set of clients costs no command-line reads.
#[cfg(any(windows, test))]
fn retitle_candidate(cur: &str, hidden: bool, accounts: &[(String, String)]) -> bool {
    let cur = cur.trim();
    if hidden {
        cur != "Roblox" && accounts.iter().any(|a| a.1 == cur)
    } else {
        cur == "Roblox"
    }
}

/// Windows: re-apply launch titles every 6 s while the watcher is off (its scan does it
/// when on) and the vault is unlocked.
#[cfg(windows)]
pub fn spawn_retitle(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(6));
        let st = app.state::<crate::AppState>();
        let (watching, hidden) = {
            let s = super::lock(&st.settings);
            (
                s.pointer("/watcher/enabled") == Some(&serde_json::Value::Bool(true)),
                crate::settings::names_hidden(&s),
            )
        };
        if watching {
            continue;
        }
        let accounts: Vec<(String, String)> = match super::lock(&st.vault).as_ref() {
            Some(v) => v
                .payload()
                .accounts
                .iter()
                .map(|a| (a.tracker_id.clone(), a.username.clone()))
                .collect(),
            None => continue,
        };
        // catch_unwind: debug builds only (release aborts on panic).
        if !accounts.is_empty()
            && std::panic::catch_unwind(|| sys::retitle_all(&accounts, hidden)).is_err()
        {
            log::error!("retitle pass panicked; continuing");
        }
    });
}
pub(super) use sys::{foreground, fullscreen, is_tracked, refocus};

/// Precise start (ms) for log picking: same-second batch launches stay apart.
fn start_ms_or(pid: u32, started_s: i64) -> i64 {
    // A reused pid has another start: fall back to the second-resolution one.
    start_ms(pid)
        .filter(|ms| (ms.div_euclid(1000) - started_s).abs() <= 2)
        .unwrap_or(started_s * 1000)
}

/// Running Roblox clients as (pid, start time) pairs.
pub fn running_clients() -> HashSet<(u32, i64)> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes()
        .values()
        .filter(|p| p.name().eq_ignore_ascii_case(PROCESS_NAME))
        .map(|p| (p.pid().as_u32(), p.start_time() as i64))
        .collect()
}

/// `watcher_kill`: only RobloxPlayerBeta.exe game clients (never the tray/desktop app);
/// Err if it survived.
pub fn kill_client(pid: u32) -> Result<(), String> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    let started = sys
        .process(Pid::from_u32(pid))
        .filter(|p| p.name().eq_ignore_ascii_case(PROCESS_NAME))
        .map(|p| p.start_time() as i64)
        .ok_or("Not a Roblox client process.")?;
    if !cmdline(pid).is_some_and(|c| is_game_client(&c)) {
        return Err("Not a Roblox game client.".into());
    }
    if close_verified(pid, started)? {
        Ok(())
    } else {
        Err("The client did not close.".into())
    }
}

struct Track {
    pre: bool,
    account: Option<(String, String)>,
    srv: Srv,
    /// When the client last became idle (start, or last left/disconnect).
    idle_since: i64,
    last_event: Option<i64>,
    log: Option<PathBuf>,
    /// `log` was created at/after the client started (see `log_settled`).
    log_settled: bool,
    /// Bytes of `log` already parsed.
    log_pos: u64,
    /// The log has been read and had content.
    evidence: bool,
    /// The first read skipped the log's start (a long log): a join may sit in the part not
    /// read, so an `Unknown` state there is no evidence of "no server".
    cut: bool,
    rect: Option<[i32; 4]>,
    /// A window titled "Roblox"/expected was seen (past the loading splash).
    game_title_seen: bool,
    /// Last failed close (secs): back off instead of retrying every scan.
    close_failed: Option<i64>,
    /// Last joined (place, jobId) from the log, for auto reconnect.
    last_join: Option<(u64, String)>,
    /// First joined place of the log (the place it was launched into).
    start_place: Option<u64>,
    /// A private/reserved server or teleport marker was seen in the log.
    marked: bool,
    /// Disconnect reason not yet acted on (kept until a reconnect close succeeds).
    reason: Option<u32>,
    /// A log pick was tried: a client first seen past `LOG_AFTER_MS` still gets one.
    looked: bool,
}

impl Track {
    /// The log can show "no server": read with content, this client's own (`log_settled`),
    /// not a home-screen launch, and not a cut log still `Unknown` (it may have joined in
    /// the part not read).
    fn no_server_evidence(&self, home: bool) -> bool {
        self.evidence && self.log_settled && !home && !(self.cut && self.srv == Srv::Unknown)
    }

    /// A new log guess: everything read from the old log goes with it.
    fn repick(&mut self, log: Option<PathBuf>, settled: bool, started: i64) {
        self.log = log;
        self.log_settled = settled;
        self.log_pos = 0;
        self.evidence = false;
        self.cut = false;
        self.srv = Srv::Unknown;
        self.idle_since = started;
        self.last_event = None;
        self.last_join = None;
        self.start_place = None;
        self.marked = false;
        self.reason = None;
    }

    /// The last join was a plain public join of the start place, so its jobId is rejoinable.
    fn plain_join(&self) -> bool {
        !self.marked && self.last_join.as_ref().map(|j| j.0) == self.start_place
    }
}

/// Wait after a failed close before trying that client again.
const RETRY_FAILED_CLOSE_SECS: i64 = 60;

struct Scanner {
    sys: System,
    /// Keyed by (pid, start time) so a reused pid is a new client.
    tracks: HashMap<(u32, i64), Track>,
    rules: LogRules,
    /// A join was matched on some client this run, so the rules are known to work.
    proven: bool,
    epoch: Arc<AtomicU64>,
    my: u64,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Linux: a client writes only to its own Sober profile, `<data>/rmi-sober/<tracker>/...`;
/// one without a tracker uses the user's own profile.
fn in_profile(log: &Path, tracker: Option<&str>) -> bool {
    let has = |name: &str| log.components().any(|c| c.as_os_str() == name);
    match tracker {
        Some(t) => has(t),
        None => !has("rmi-sober"),
    }
}

/// May this client have written `log`? Off Linux all clients share one log folder.
fn own_log(log: &Path, tracker: Option<&str>) -> bool {
    !cfg!(target_os = "linux") || in_profile(log, tracker)
}

/// Where Roblox writes its player logs. Linux: one folder per Sober profile.
fn log_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    return dirs::home_dir()
        .map(|h| h.join("Library/Logs/Roblox"))
        .into_iter()
        .collect();
    #[cfg(target_os = "linux")]
    return crate::platform::linux::log_dirs();
    #[cfg(windows)]
    dirs::data_local_dir()
        .map(|d| d.join("Roblox").join("logs"))
        .into_iter()
        .collect()
}

/// Complete new lines since `*pos` (a partial last line waits for the next scan), and
/// whether this read skipped the start of the log: a first read of a huge log only takes its
/// last MiB. At most `READ_CAP` per read; a line longer than that is skipped.
fn read_new(p: &Path, pos: &mut u64) -> Option<(String, bool)> {
    const READ_CAP: u64 = 4 << 20;
    let mut f = std::fs::File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    if len < *pos {
        *pos = 0;
    }
    let from = if *pos == 0 {
        len.saturating_sub(1 << 20)
    } else {
        *pos
    };
    let cut = *pos == 0 && from > 0;
    f.seek(SeekFrom::Start(from)).ok()?;
    let n = (len - from).min(READ_CAP);
    let mut b = Vec::new();
    f.take(n).read_to_end(&mut b).ok()?;
    let end = b.iter().rposition(|c| *c == b'\n').map_or(0, |i| i + 1);
    if end == 0 && b.len() as u64 == READ_CAP {
        *pos = from + READ_CAP; // no newline in a full read: never re-read it every scan
        return Some((String::new(), cut));
    }
    *pos = from + end as u64;
    Some((String::from_utf8_lossy(&b[..end]).into_owned(), cut))
}

/// Player/crash logs with creation time in ms.
fn logs_created() -> Vec<(PathBuf, i64)> {
    log_dirs()
        .into_iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(is_player_log))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let t = m.created().or_else(|_| m.modified()).ok()?;
            let ms = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
            Some((e.path(), ms as i64))
        })
        .collect()
}

impl Scanner {
    fn live(&self) -> bool {
        self.epoch.load(Ordering::SeqCst) == self.my
    }

    fn scan(&mut self, app: &AppHandle) {
        let st = app.state::<crate::AppState>();
        let (cfg, names_hidden) = {
            let s = st
                .settings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (Cfg::from_settings(&s), crate::settings::names_hidden(&s))
        };
        let lookup = VaultLookup(app.clone());
        let unlocked = lookup.unlocked();
        // Memory for everyone (cheap), command lines only for Roblox clients.
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        let pids: Vec<Pid> = self
            .sys
            .processes()
            .values()
            .filter(|p| p.name().eq_ignore_ascii_case(PROCESS_NAME))
            .map(|p| p.pid())
            .collect();
        let t_now = now();
        // (pid, start secs, private MB, (tracker id, game client, launched to home))
        let live: Vec<_> = pids
            .iter()
            .filter_map(|pid| {
                let p = self.sys.process(*pid)?;
                let cmd = cmdline(pid.as_u32()).unwrap_or_default();
                Some((
                    pid.as_u32(),
                    p.start_time() as i64,
                    // Windows: PagefileUsage = private bytes; working set shrinks when idle.
                    // Linux and macOS: virtual size is address space (many GB); resident is
                    // the real use.
                    if cfg!(windows) {
                        p.virtual_memory()
                    } else {
                        p.memory()
                    } / (1024 * 1024),
                    (
                        tracker_from_cmdline(&cmd),
                        is_game_client(&cmd),
                        super::is_home_launch(&cmd),
                    ),
                ))
            })
            .collect();
        let keys: HashSet<(u32, i64)> = live.iter().map(|c| (c.0, c.1)).collect();
        let mut main_win: HashMap<u32, Win> = HashMap::new();
        for (pid, w) in windows_of(&keys.iter().map(|k| k.0).collect()) {
            let visible_titled = !w.title.is_empty();
            if visible_titled && main_win.get(&pid).is_none_or(|b| w.area() > b.area()) {
                main_win.insert(pid, w);
            }
        }
        let existing = st.watcher.existing.get();
        for (pid, started, _, (_, game, _)) in &live {
            if !game {
                continue;
            }
            let pre = existing.is_some_and(|s| s.contains(&(*pid, *started)));
            self.tracks
                .entry((*pid, *started))
                .or_insert_with(|| Track {
                    pre,
                    account: None,
                    srv: Srv::Unknown,
                    idle_since: *started,
                    last_event: None,
                    log: None,
                    log_settled: false,
                    log_pos: 0,
                    evidence: false,
                    cut: false,
                    rect: None,
                    game_title_seen: false,
                    close_failed: None,
                    last_join: None,
                    start_place: None,
                    marked: false,
                    reason: None,
                    looked: false,
                });
        }
        // Logs: look at the directory only if some client has no confirmed log yet, and only
        // while its own log can still appear (`pick_log` takes none created later), or once
        // for a client first seen later (watcher started late, RMI restarted).
        let unsettled = |k: &(u32, i64), t: &Track| {
            (t.log.is_none() && !t.looked)
                || ((t.log.is_none() || t.srv == Srv::Unknown || !t.log_settled)
                    && (t_now - k.1) * 1000 <= super::LOG_AFTER_MS)
        };
        if self.tracks.iter().any(|(k, t)| unsettled(k, t)) {
            let files = logs_created();
            // The tray/desktop app writes a player log too: never hand it to a game client.
            // ponytail: nearest-log guess for non-game pids, same heuristic as for clients.
            let tray: HashSet<PathBuf> = live
                .iter()
                .filter(|c| !c.3 .1)
                .filter_map(|c| pick_log(&files, start_ms_or(c.0, c.1), &HashSet::new()))
                .map(PathBuf::from)
                .collect();
            // Linux without pasta: every client runs on the user's own Sober profile (as
            // `open_uri`), tracker or not.
            let isolated = !cfg!(target_os = "linux") || st.platform.capabilities().multi_instance;
            let trackers: HashMap<(u32, i64), Option<&str>> = live
                .iter()
                .map(|c| ((c.0, c.1), c.3 .0.as_deref().filter(|_| isolated)))
                .collect();
            // Oldest first, so a later client can't take an earlier one's log first.
            let mut order: Vec<_> = keys.iter().map(|k| (start_ms_or(k.0, k.1), *k)).collect();
            order.sort_unstable();
            for (_, k) in &order {
                let mut claimed: HashSet<PathBuf> = self
                    .tracks
                    .iter()
                    .filter(|(o, _)| *o != k)
                    .filter_map(|(_, t)| t.log.clone())
                    .collect();
                claimed.extend(tray.iter().cloned());
                let Some(t) = self.tracks.get_mut(k) else {
                    continue;
                };
                if unsettled(k, t) {
                    // Linux: clients started seconds apart each write to their own profile;
                    // timing alone swapped their logs.
                    let own: Vec<(PathBuf, i64)> = files
                        .iter()
                        .filter(|(f, _)| own_log(f, trackers.get(k).copied().flatten()))
                        .cloned()
                        .collect();
                    let pick = pick_log(&own, start_ms_or(k.0, k.1), &claimed);
                    if pick != t.log.as_deref() {
                        let settled = pick.is_some_and(|p| {
                            files.iter().any(|(f, c)| f == p && log_settled(*c, k.1))
                        });
                        t.repick(pick.map(PathBuf::from), settled, k.1);
                    }
                    t.looked = true;
                }
            }
        }
        let w = &st.watcher;
        let (epoch, my) = (self.epoch.clone(), self.my);
        let is_live = || epoch.load(Ordering::SeqCst) == my;
        let mut infos = vec![];
        let mut fired = vec![];
        for (pid, started, mem_mb, (tracker, game, home)) in &live {
            if !game {
                infos.push(ClientInfo {
                    pid: *pid,
                    account_id: None,
                    username: None,
                    memory_mb: *mem_mb,
                    title: main_win.get(pid).map(|w| w.title.clone()),
                    started_at: *started,
                    game_client: false,
                    last_server_event: None,
                });
                continue;
            }
            let Some(t) = self.tracks.get_mut(&(*pid, *started)) else {
                continue;
            };
            // Fresh each scan while unlocked: a renamed or deleted account must not act on a
            // stale match.
            if unlocked {
                t.account = tracker.as_deref().and_then(|k| lookup.by_tracker(k));
            }
            // Locked vault: no names on screen, even cached ones.
            let shown = t.account.as_ref().filter(|_| unlocked);
            let mut reason = None;
            if let Some((text, cut)) = t.log.as_deref().and_then(|p| read_new(p, &mut t.log_pos)) {
                t.cut |= cut;
                t.evidence |= !text.is_empty();
                let (join, r, marked) = super::logparse::rejoin_events(&text);
                t.marked |= marked;
                if let Some(j) = join {
                    t.start_place.get_or_insert(j.0);
                    t.last_join = Some(j);
                    t.reason = None; // a later join supersedes an old disconnect
                }
                t.reason = r.or(t.reason);
                reason = r;
                let new = self.rules.state_of(&text, t.srv);
                if new != t.srv {
                    t.srv = new;
                    self.proven |= new == Srv::Joined;
                    t.last_event = Some(t_now);
                    if matches!(new, Srv::Left | Srv::Disconnected) {
                        t.idle_since = t_now;
                    }
                }
            }
            let main = main_win.get(pid);
            if cfg.save_window_rects {
                t.rect = main.and_then(|w| w.rect).or(t.rect);
            }
            let title = main.map(|w| w.title.clone());
            // Re-apply the launch title (as settings::window_title) whenever Roblox reset it
            // or privacy mode flipped since.
            if let (Some((_, name)), Some(cur)) = (shown, title.as_deref()) {
                let want = if names_hidden {
                    "Roblox"
                } else {
                    name.as_str()
                };
                if needs_retitle(cur, name, want) && retitle(*pid, name, want) {
                    log::info!("watcher: window of {name} retitled");
                }
            }
            // Launch and Grid Arrange title the window with the username ("Roblox" in privacy
            // mode): not a wrong title.
            let rule_title = title.clone().map(|x| match &t.account {
                Some(a) if a.1 == x.trim() || x.trim() == "Roblox" => cfg.expected_title.clone(),
                _ => x,
            });
            t.game_title_seen |= rule_title
                .as_deref()
                .is_some_and(|x| is_game_title(x, &cfg));
            infos.push(ClientInfo {
                pid: *pid,
                account_id: shown.map(|a| a.0.clone()),
                username: shown.map(|a| a.1.clone()),
                memory_mb: *mem_mb,
                title: title.clone(),
                started_at: *started,
                game_client: true,
                last_server_event: t.last_event,
            });
            if cfg.reconnect
                && !unlocked
                && t.account.is_none()
                && reason.is_some_and(|r| r != super::rules::REASON_LEFT)
            {
                log::info!("watcher: vault locked; not reconnecting pid {pid}");
            }
            let backoff = t
                .close_failed
                .is_some_and(|f| t_now - f < RETRY_FAILED_CLOSE_SECS);
            // Handed over: another computer runs it now, so no rejoin here either.
            let paused = t.account.as_ref().is_some_and(|(id, _)| {
                super::friend_paused(&st, id) || st.automation.handed_over(id)
            });
            if paused {
                // Drop it: a later unpause or take-back must not fire a stale rejoin.
                t.reason = None;
            }
            // Kicked because the account joined elsewhere (another device, a hand-over):
            // auto-relaunch must not start it here again and kick that session in turn.
            if let (Some(r), Some((_, name))) = (reason, t.account.as_ref()) {
                if super::rules::joined_elsewhere(r) {
                    st.automation
                        .relaunch
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .quiet(name);
                }
            }
            // Home-screen clients are never closed and relaunched as dropped either.
            if let (true, Some(r), Some((id, name)), Some(tr), false) = (
                cfg.reconnect && !home && !paused,
                t.reason,
                t.account.clone(),
                tracker,
                backoff,
            ) {
                t.reason = None;
                let d = rejoin(
                    r,
                    unlocked,
                    super::lock(&w.shared)
                        .rejoins
                        .entry(id.clone())
                        .or_default(),
                    t_now,
                );
                let dest = || {
                    super::saved_target(&st, &id).is_some_and(|tg| {
                        super::has_destination(
                            t.last_join.clone(),
                            t.plain_join(),
                            t.start_place,
                            tg,
                        )
                    })
                };
                match d {
                    Rejoin::Ignore => {}
                    // Locked: keep the reason so it reconnects once unlocked (field doc).
                    // Rate limit: keep it too (rechecked each scan, uncounted), so the client
                    // reconnects once the hour frees up; see `keep_skipped`.
                    Rejoin::Skip(why) => {
                        if keep_skipped(r, unlocked) {
                            t.reason = Some(r);
                        }
                        // Locked: the cached name stays out of the log.
                        let who = if unlocked {
                            name.clone()
                        } else {
                            format!("pid {pid}")
                        };
                        if reason.is_some() {
                            log::info!("watcher: not reconnecting {who}: {why}");
                        } else {
                            log::debug!("watcher: not reconnecting {who} yet: {why}");
                        }
                    }
                    Rejoin::Go { .. } if !dest() => {
                        log::info!("watcher: not reconnecting {name}: no destination known");
                    }
                    Rejoin::Go {
                        delay_s,
                        same_server,
                    } => {
                        if !is_live() {
                            return;
                        }
                        log::info!("watcher: {name} disconnected (reason {r}); reconnecting");
                        // Before the close: auto-relaunch must not read this exit as a crash
                        // and launch a second client.
                        st.automation
                            .relaunch
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .quiet(&name);
                        match close_verified(*pid, *started) {
                            Ok(true) => {
                                super::lock(&w.shared)
                                    .rejoins
                                    .entry(id.clone())
                                    .or_default()
                                    .push(t_now);
                                tauri::async_runtime::spawn(super::rejoin(
                                    app.clone(),
                                    super::RejoinReq {
                                        id,
                                        name,
                                        tracker: tr.clone(),
                                        last_join: t.last_join.clone(),
                                        plain: t.plain_join(),
                                        start_place: t.start_place,
                                        same_server,
                                        delay_s,
                                    },
                                ));
                                continue;
                            }
                            // Keep the reason: retried after the failed-close backoff.
                            Ok(false) => log::warn!("watcher: pid {pid} survived the close"),
                            Err(e) => log::warn!("watcher: close pid {pid} failed: {e}"),
                        }
                        t.reason = Some(r);
                        t.close_failed = Some(t_now);
                        // Hand the account back to auto-relaunch (a live client clears the
                        // handback; a later Skip must not leave it quiet forever).
                        st.automation
                            .relaunch
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .unquiet(&name);
                    }
                }
            }
            let snap = Snap {
                age_s: (t_now - started).max(0) as u64,
                memory_mb: *mem_mb,
                title: rule_title,
                joined: t.srv == Srv::Joined,
                // A home-screen client (empty Play bar) is never judged for No server: it
                // has none on purpose, before and after any game it joins from there.
                evidence: t.no_server_evidence(*home),
                left: matches!(t.srv, Srv::Left | Srv::Disconnected),
                proven: self.proven,
                idle_s: (t_now - t.idle_since).max(0) as u64,
                preexisting: t.pre,
                game_title_seen: t.game_title_seen,
            };
            let Some(rule) = decide(&snap, &cfg) else {
                continue;
            };
            let who = t
                .account
                .as_ref()
                .filter(|_| unlocked)
                .map_or("unknown account", |a| &a.1);
            if !is_live() {
                return;
            }
            if t.close_failed
                .is_some_and(|f| t_now - f < RETRY_FAILED_CLOSE_SECS)
            {
                continue;
            }
            // Unmapped clients get their own key so they are throttled too.
            let key = t
                .account
                .as_ref()
                .map_or_else(|| format!("pid:{pid}:{started}"), |a| a.0.clone());
            if !cooldown_open(
                super::lock(&w.shared).fires.entry(key.clone()).or_default(),
                t_now,
            ) {
                log::warn!("watcher: {who} closed too often; skipping pid {pid}");
                continue;
            }
            log::info!("watcher: closing pid {pid} ({who}) by rule {}", rule.id());
            match close_verified(*pid, *started) {
                Ok(true) => {
                    // Only a successful close counts toward the cooldown.
                    super::lock(&w.shared)
                        .fires
                        .entry(key)
                        .or_default()
                        .push(t_now);
                    fired.push((rule.id(), t_now));
                }
                Ok(false) => {
                    t.close_failed = Some(t_now);
                    log::warn!("watcher: pid {pid} survived the close");
                }
                Err(e) => {
                    t.close_failed = Some(t_now);
                    log::warn!("watcher: close pid {pid} failed: {e}");
                }
            }
        }
        // Exited clients: hand over their last window rect, drop their unmapped cooldown key.
        self.tracks.retain(|k, t| {
            let alive = keys.contains(k);
            if !alive {
                emit_rect(app, &cfg, t);
                super::lock(&w.shared)
                    .fires
                    .remove(&format!("pid:{}:{}", k.0, k.1));
            }
            alive
        });
        {
            let mut sh = super::lock(&w.shared);
            // Under the lock: `stop()` bumps the epoch, then clears clients under it.
            if !self.live() {
                return;
            }
            sh.clients = infos;
            sh.last_fired.extend(fired.iter().copied());
        }
        if !fired.is_empty() {
            crate::commands::state_changed(app, &["watcherStatus"]);
        }
    }

    /// On stop: live clients' last rects are handed over too.
    fn flush(&mut self, app: &AppHandle) {
        let st = app.state::<crate::AppState>();
        let cfg = Cfg::from_settings(
            &st.settings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for t in self.tracks.values() {
            emit_rect(app, &cfg, t);
        }
    }
}

/// Blocking (vault save): call from the scan thread or `spawn_blocking`.
fn emit_rect(app: &AppHandle, cfg: &Cfg, t: &Track) {
    if let (true, Some(rect), Some((id, _))) = (cfg.save_window_rects, t.rect, &t.account) {
        persist_rect(app, id, rect);
    }
}

pub async fn run(app: AppHandle, epoch: Arc<AtomicU64>, my: u64, wake: Arc<tokio::sync::Notify>) {
    // File read: off the async workers.
    let root = app.state::<crate::AppState>().paths.root.clone();
    let Ok((rules, src)) =
        tauri::async_runtime::spawn_blocking(move || LogRules::load(&root)).await
    else {
        log::error!("watcher: rules load failed; stopping");
        app.state::<crate::AppState>().watcher.ended(my);
        return;
    };
    log::info!("watcher: started ({src} rules)");
    let mut sc = Scanner {
        sys: System::new(),
        tracks: HashMap::new(),
        rules,
        proven: false,
        epoch: epoch.clone(),
        my,
    };
    while sc.live() {
        let a = app.clone();
        let Ok(back) = tauri::async_runtime::spawn_blocking(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sc.scan(&a)));
            // Debug builds only: release builds abort on panic (Cargo.toml `panic = "abort"`).
            if r.is_err() {
                // A track may be half-updated: start every client over next scan.
                sc.tracks.clear();
                log::error!("watcher: scan panicked; continuing");
            }
            sc
        })
        .await
        else {
            log::error!("watcher: scan task failed; stopping");
            app.state::<crate::AppState>().watcher.ended(my);
            return;
        };
        sc = back;
        let secs = app
            .state::<crate::AppState>()
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pointer("/watcher/scanSec")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(6);
        // Woken early by a settings save or stop.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(secs), wake.notified()).await;
    }
    let a = app.clone();
    if let Err(e) = tauri::async_runtime::spawn_blocking(move || sc.flush(&a)).await {
        log::warn!("watcher: final flush failed: {e}");
    }
    log::info!("watcher: stopped");
}

#[cfg(test)]
mod tests {
    use super::read_new;
    use std::io::Write;

    #[test]
    fn read_new_only_new_complete_lines() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("l.log");
        std::fs::write(&p, "a\nb\npart").unwrap();
        let mut pos = 0;
        let read = |pos: &mut u64| read_new(&p, pos).unwrap().0;
        assert_eq!(read(&mut pos), "a\nb\n");
        assert_eq!(read(&mut pos), "");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"ial\nc\n")
            .unwrap();
        assert_eq!(read(&mut pos), "partial\nc\n");
        std::fs::write(&p, "z\n").unwrap(); // rotated/truncated
        assert_eq!(read(&mut pos), "z\n");
        assert!(!read_new(&p, &mut 0).unwrap().1, "small log: read whole");
    }

    #[test]
    fn read_new_skips_a_line_longer_than_the_cap() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("long.log");
        std::fs::write(&p, "a\n").unwrap();
        let mut pos = 0;
        read_new(&p, &mut pos).unwrap();
        let body = "a\n".to_string() + &"x".repeat(5 << 20);
        std::fs::write(&p, &body).unwrap();
        assert_eq!(read_new(&p, &mut pos).unwrap().0, "");
        assert_eq!(pos, 2 + (4 << 20), "moved past it, not re-read every scan");
    }

    #[test]
    fn read_new_first_read_of_big_log_takes_last_mib() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big.log");
        let line = format!("{}\n", "x".repeat(999));
        let body = line.repeat(1100) + "last\n";
        std::fs::write(&p, &body).unwrap();
        let mut pos = 0;
        let (got, cut) = read_new(&p, &mut pos).unwrap();
        assert!(cut, "the start was skipped");
        assert!(!got.is_empty() && got.len() <= 1 << 20);
        assert!(got.ends_with("last\n"));
        assert_eq!(pos, body.len() as u64);
        assert_eq!(read_new(&p, &mut pos).unwrap(), (String::new(), false));
    }

    #[test]
    fn a_cut_log_still_unknown_is_no_evidence_of_no_server() {
        use crate::watcher::logparse::Srv;
        let mut t = track(None, None, false);
        (t.srv, t.cut) = (Srv::Unknown, false);
        assert!(t.no_server_evidence(false), "whole log, no join: evidence");
        t.cut = true;
        assert!(
            !t.no_server_evidence(false),
            "join may be in the part not read"
        );
        t.srv = Srv::Left;
        assert!(t.no_server_evidence(false), "a leave seen since: evidence");
        assert!(!t.no_server_evidence(true), "home-screen launch");
    }

    #[test]
    fn read_new_missing_file_is_none() {
        let d = tempfile::tempdir().unwrap();
        assert!(read_new(&d.path().join("nope.log"), &mut 0).is_none());
    }

    #[test]
    fn retitle_only_roblox_or_username_titles() {
        use super::needs_retitle;
        assert!(needs_retitle("Roblox", "bob", "bob"), "reset to home app");
        assert!(needs_retitle("bob", "bob", "Roblox"), "privacy turned on");
        assert!(!needs_retitle(" bob ", "bob", "bob"), "already right");
        assert!(
            !needs_retitle("Roblox", "bob", "Roblox"),
            "privacy: already right"
        );
        assert!(
            !needs_retitle("Bloxstrap", "bob", "bob"),
            "someone else's title"
        );
    }

    #[test]
    fn retitle_candidates_skip_correct_titles() {
        use super::retitle_candidate;
        let acc = [("1".to_string(), "bob".to_string())];
        assert!(
            retitle_candidate("Roblox", false, &acc),
            "reset: look it up"
        );
        assert!(!retitle_candidate("bob", false, &acc), "already named");
        assert!(
            retitle_candidate(" bob ", true, &acc),
            "privacy on: hide the name"
        );
        assert!(
            !retitle_candidate("Roblox", true, &acc),
            "privacy: already right"
        );
        assert!(
            !retitle_candidate("Bloxstrap", true, &acc),
            "not ours to change"
        );
        assert!(!retitle_candidate("Bloxstrap", false, &acc));
    }

    #[test]
    fn window_area_zero_when_minimized_or_inverted() {
        let w = |rect| super::Win {
            rect,
            title: String::new(),
        };
        assert_eq!(w(Some([0, 0, 10, 20])).area(), 200);
        assert_eq!(w(None).area(), 0, "minimized");
        assert_eq!(w(Some([10, 0, 0, 20])).area(), 0, "inverted");
    }

    fn track(
        last_join: Option<(u64, String)>,
        start_place: Option<u64>,
        marked: bool,
    ) -> super::Track {
        use crate::watcher::logparse::Srv;
        super::Track {
            pre: false,
            account: Some(("id".into(), "name".into())),
            srv: Srv::Joined,
            idle_since: 0,
            last_event: Some(9),
            log: Some(std::path::PathBuf::from("a.log")),
            log_settled: true,
            log_pos: 42,
            evidence: true,
            cut: true,
            rect: Some([1, 2, 3, 4]),
            game_title_seen: true,
            close_failed: None,
            last_join,
            start_place,
            marked,
            reason: Some(277),
            looked: true,
        }
    }

    #[test]
    fn logs_belong_to_their_profile() {
        use super::in_profile;
        use std::path::Path;
        let a = Path::new("/d/rmi-sober/111/.var/app/x/logs/a_Player_1_last.log");
        let own = Path::new("/home/u/.var/app/x/logs/b_Player_2_last.log");
        assert!(in_profile(a, Some("111")));
        assert!(!in_profile(a, Some("222"))); // another account's profile
        assert!(!in_profile(a, None));
        assert!(in_profile(own, None));
        assert!(!in_profile(own, Some("111")));
    }

    #[test]
    fn plain_join_is_start_place_and_unmarked() {
        let j = |p| Some((p, "job".to_string()));
        assert!(track(j(1), Some(1), false).plain_join());
        assert!(
            !track(j(2), Some(1), false).plain_join(),
            "joined elsewhere"
        );
        assert!(
            !track(j(1), Some(1), true).plain_join(),
            "private/teleport marker"
        );
    }

    #[test]
    fn repick_resets_log_state_keeps_client() {
        let mut t = track(Some((1, "job".into())), Some(1), true);
        t.repick(None, false, 500);
        assert!(t.log.is_none() && !t.log_settled);
        assert_eq!(
            (t.log_pos, t.evidence, t.cut, t.srv),
            (0, false, false, crate::watcher::logparse::Srv::Unknown)
        );
        assert_eq!(t.idle_since, 500);
        assert_eq!(t.last_event, None);
        assert!(t.last_join.is_none() && t.start_place.is_none());
        assert!(!t.marked && t.reason.is_none());
        assert!(t.account.is_some() && t.game_title_seen);
    }
}
