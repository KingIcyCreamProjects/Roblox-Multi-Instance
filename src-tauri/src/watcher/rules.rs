//! Pure close-rule decisions over a per-client snapshot.

use serde_json::Value;

/// A client younger than this is never judged (it is still loading).
pub const MIN_AGE_S: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    LowMemory,
    WrongTitle,
    NoServer,
}

impl Rule {
    pub const ALL: [Rule; 3] = [Rule::LowMemory, Rule::WrongTitle, Rule::NoServer];

    pub fn id(self) -> &'static str {
        match self {
            Rule::LowMemory => "lowMemory",
            Rule::WrongTitle => "wrongTitle",
            Rule::NoServer => "noServer",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cfg {
    pub close_low_memory: bool,
    pub low_memory_mb: u64,
    pub close_wrong_title: bool,
    pub expected_title: String,
    pub close_no_server: bool,
    pub no_server_sec: u64,
    pub ignore_existing: bool,
    pub save_window_rects: bool,
    pub reconnect: bool,
}

impl Cfg {
    pub fn from_settings(s: &Value) -> Self {
        let b = |p: &str| s.pointer(p).and_then(Value::as_bool).unwrap_or(false);
        let n = |p: &str, d: u64| s.pointer(p).and_then(Value::as_u64).unwrap_or(d);
        Self {
            close_low_memory: b("/watcher/closeLowMemory"),
            low_memory_mb: n("/watcher/lowMemoryMb", 200),
            close_wrong_title: b("/watcher/closeWrongTitle"),
            expected_title: s
                .pointer("/watcher/expectedTitle")
                .and_then(Value::as_str)
                .unwrap_or("Roblox")
                .trim()
                .to_string(),
            close_no_server: b("/watcher/closeNoServer"),
            no_server_sec: n("/watcher/noServerSec", 60),
            ignore_existing: s
                .pointer("/watcher/ignoreExisting")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            save_window_rects: b("/watcher/saveWindowRects"),
            reconnect: b("/watcher/reconnect"),
        }
    }

    pub fn enabled(&self, r: Rule) -> bool {
        match r {
            Rule::LowMemory => self.close_low_memory,
            Rule::WrongTitle => self.close_wrong_title,
            Rule::NoServer => self.close_no_server,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Snap {
    pub age_s: u64,
    pub memory_mb: u64,
    /// `None` = no window yet (never a mismatch).
    pub title: Option<String>,
    pub joined: bool,
    /// A client log was found and read; without it "no server" is unknowable.
    pub evidence: bool,
    /// State is Left/Disconnected (as opposed to nothing matched yet).
    pub left: bool,
    /// The rules have matched a join on some client this run, so they are known to work.
    pub proven: bool,
    /// Seconds since the client started or last left/disconnected, whichever is later.
    pub idle_s: u64,
    /// Was already running when the watcher began.
    pub preexisting: bool,
    /// A window titled "Roblox"/expected was seen, so the client is past its splash.
    pub game_title_seen: bool,
}

/// The real game window: "Roblox" or the expected title (the loading splash is neither).
pub fn is_game_title(t: &str, c: &Cfg) -> bool {
    let t = t.trim();
    t == "Roblox" || t == c.expected_title
}

pub fn decide(s: &Snap, c: &Cfg) -> Option<Rule> {
    if (c.ignore_existing && s.preexisting) || s.age_s < MIN_AGE_S {
        return None;
    }
    if c.close_low_memory && s.memory_mb < c.low_memory_mb {
        return Some(Rule::LowMemory);
    }
    if c.close_wrong_title
        && s.game_title_seen
        && s.title
            .as_deref()
            .is_some_and(|t| t.trim() != c.expected_title)
    {
        return Some(Rule::WrongTitle);
    }
    if c.close_no_server
        && s.evidence
        && !s.joined
        && (s.left || s.proven)
        && s.idle_s >= c.no_server_sec
    {
        return Some(Rule::NoServer);
    }
    None
}

/// At most this many closes of one account per window, so a relaunch loop cannot spin.
pub const COOLDOWN_MAX: usize = 3;
pub const COOLDOWN_WINDOW_S: i64 = 600;

/// Prune old fires; true when the account may be closed again.
pub fn cooldown_open(hist: &mut Vec<i64>, now: i64) -> bool {
    hist.retain(|t| now - t < COOLDOWN_WINDOW_S);
    hist.len() < COOLDOWN_MAX
}

/// Auto reconnect: at most this many rejoins of one account per hour, so it never loops.
pub const REJOIN_MAX: usize = 3;
pub const REJOIN_WINDOW_S: i64 = 3600;
/// Disconnect reason for a normal leave or teleport.
pub const REASON_LEFT: u32 = 285;

#[derive(Debug, PartialEq, Eq)]
pub enum Rejoin {
    /// Normal leave: nothing to do.
    Ignore,
    Skip(&'static str),
    /// `same_server`: try its last jobId (first rejoin in the window only; later ones take
    /// any server of the place, the old one may be gone).
    Go {
        delay_s: u64,
        same_server: bool,
    },
}

/// Decide on a disconnect `reason` of a saved account. A `Go` is not counted here: the
/// caller pushes `now` onto `hist` only once the old client is really closed.
pub fn rejoin(reason: u32, unlocked: bool, hist: &mut Vec<i64>, now: i64) -> Rejoin {
    if reason == REASON_LEFT {
        return Rejoin::Ignore;
    }
    if !unlocked {
        return Rejoin::Skip("the vault is locked");
    }
    hist.retain(|t| now - t < REJOIN_WINDOW_S);
    if hist.len() >= REJOIN_MAX {
        return Rejoin::Skip("rejoined too often in the last hour");
    }
    // The account joined from somewhere else; back off longer each time.
    let delay_s = if joined_elsewhere(reason) {
        60 << hist.len()
    } else {
        5
    };
    Rejoin::Go {
        delay_s,
        same_server: hist.is_empty(),
    }
}

/// 264/273/276: the account joined from somewhere else (another device or app).
pub(super) fn joined_elsewhere(reason: u32) -> bool {
    matches!(reason, 264 | 273 | 276)
}

/// Keep a `Skip`ped disconnect to retry later? Locked: yes, until unlocked. Rate limit: yes,
/// until the hour frees up; except when the account joined from somewhere else, so it never
/// keeps kicking whoever plays it there.
pub fn keep_skipped(reason: u32, unlocked: bool) -> bool {
    !unlocked || !joined_elsewhere(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Cfg {
        Cfg {
            close_low_memory: true,
            low_memory_mb: 200,
            close_wrong_title: true,
            expected_title: "Roblox".into(),
            close_no_server: true,
            no_server_sec: 60,
            ignore_existing: true,
            save_window_rects: false,
            reconnect: true,
        }
    }
    fn snap() -> Snap {
        Snap {
            age_s: 300,
            memory_mb: 800,
            title: Some("Roblox".into()),
            joined: true,
            evidence: true,
            left: true,
            proven: true,
            idle_s: 0,
            preexisting: false,
            game_title_seen: true,
        }
    }

    #[test]
    fn healthy_client_untouched() {
        assert_eq!(decide(&snap(), &cfg()), None);
    }

    #[test]
    fn low_memory_needs_age() {
        let mut s = snap();
        s.memory_mb = 150;
        assert_eq!(decide(&s, &cfg()), Some(Rule::LowMemory));
        s.age_s = 29;
        assert_eq!(decide(&s, &cfg()), None);
        s.age_s = 30;
        assert_eq!(decide(&s, &cfg()), Some(Rule::LowMemory));
    }

    #[test]
    fn memory_at_threshold_is_ok() {
        let mut s = snap();
        s.memory_mb = 200;
        assert_eq!(decide(&s, &cfg()), None);
    }

    #[test]
    fn title_mismatch_only_with_window() {
        let mut s = snap();
        s.title = Some("Roblox Crash".into());
        assert_eq!(decide(&s, &cfg()), Some(Rule::WrongTitle));
        s.title = None;
        assert_eq!(decide(&s, &cfg()), None);
    }

    #[test]
    fn splash_never_wrong_title() {
        let mut s = snap();
        s.title = Some("Roblox Loading".into());
        s.game_title_seen = false;
        assert_eq!(decide(&s, &cfg()), None);
        s.game_title_seen = true;
        assert_eq!(decide(&s, &cfg()), Some(Rule::WrongTitle));
    }

    #[test]
    fn game_title_trimmed() {
        let mut c = cfg();
        c.expected_title = "My Game".into();
        assert!(is_game_title(" Roblox ", &c));
        assert!(is_game_title("My Game ", &c));
        assert!(!is_game_title("Roblox Crash", &c));
        let mut s = snap();
        s.title = Some(" My Game".into());
        assert_eq!(decide(&s, &c), None);
        let c = Cfg::from_settings(&serde_json::json!({"watcher": {"expectedTitle": " X "}}));
        assert_eq!(c.expected_title, "X");
    }

    #[test]
    fn no_server_elapsed() {
        let mut s = snap();
        s.joined = false;
        s.idle_s = 59;
        assert_eq!(decide(&s, &cfg()), None);
        s.idle_s = 60;
        assert_eq!(decide(&s, &cfg()), Some(Rule::NoServer));
        s.joined = true;
        assert_eq!(decide(&s, &cfg()), None);
    }

    #[test]
    fn no_server_needs_evidence() {
        let mut s = snap();
        s.joined = false;
        s.idle_s = 999;
        s.evidence = false;
        assert_eq!(decide(&s, &cfg()), None);
        s.evidence = true;
        assert_eq!(decide(&s, &cfg()), Some(Rule::NoServer));
    }

    #[test]
    fn unknown_state_needs_proven_rules() {
        let mut s = snap();
        s.joined = false;
        s.left = false;
        s.idle_s = 999;
        s.proven = false;
        assert_eq!(decide(&s, &cfg()), None);
        s.proven = true;
        assert_eq!(decide(&s, &cfg()), Some(Rule::NoServer));
        s.proven = false;
        s.left = true;
        assert_eq!(decide(&s, &cfg()), Some(Rule::NoServer));
    }

    #[test]
    fn cooldown_three_per_ten_min() {
        let mut h = vec![];
        for t in [0, 100, 200] {
            assert!(cooldown_open(&mut h, t));
            h.push(t);
        }
        assert!(!cooldown_open(&mut h, 300));
        assert!(cooldown_open(&mut h, 601), "oldest fire aged out");
    }

    #[test]
    fn preexisting_ignored_unless_off() {
        let mut s = snap();
        s.memory_mb = 1;
        s.preexisting = true;
        assert_eq!(decide(&s, &cfg()), None);
        let mut c = cfg();
        c.ignore_existing = false;
        assert_eq!(decide(&s, &c), Some(Rule::LowMemory));
    }

    #[test]
    fn disabled_rules_never_fire() {
        let mut s = snap();
        s.memory_mb = 1;
        s.title = Some("x".into());
        s.joined = false;
        s.idle_s = 999;
        let mut c = cfg();
        c.close_low_memory = false;
        c.close_wrong_title = false;
        c.close_no_server = false;
        assert_eq!(decide(&s, &c), None);
    }

    #[test]
    fn cfg_from_default_settings() {
        let c = Cfg::from_settings(&crate::settings::defaults());
        assert_eq!(c.low_memory_mb, 200);
        assert_eq!(c.no_server_sec, 60);
        assert!(c.ignore_existing && !c.close_low_memory);
        assert_eq!(c.expected_title, "Roblox");
        assert!(!c.reconnect);
    }

    #[test]
    fn rejoin_decisions() {
        let mut h = vec![];
        assert_eq!(rejoin(285, true, &mut h, 0), Rejoin::Ignore);
        assert!(h.is_empty(), "a normal leave is not counted");
        assert!(matches!(rejoin(277, false, &mut h, 0), Rejoin::Skip(_)));
        assert!(h.is_empty(), "locked is not counted");
        let go = |d, s| Rejoin::Go {
            delay_s: d,
            same_server: s,
        };
        assert_eq!(rejoin(277, true, &mut h, 0), go(5, true));
        assert_eq!(
            rejoin(277, true, &mut h, 0),
            go(5, true),
            "a failed close is not counted"
        );
        h.push(0);
        assert_eq!(rejoin(273, true, &mut h, 10), go(120, false));
        h.push(10);
        assert_eq!(rejoin(268, true, &mut h, 20), go(5, false));
        h.push(20);
        assert!(matches!(rejoin(277, true, &mut h, 30), Rejoin::Skip(_)));
        assert_eq!(
            rejoin(279, true, &mut h, 3600),
            go(5, false),
            "oldest aged out"
        );
    }

    /// A rate-limited disconnect is retried once the hour frees up (not dropped), except
    /// when the account joined from elsewhere.
    #[test]
    fn skipped_disconnect_kept_for_later() {
        assert!(keep_skipped(277, true));
        assert!(keep_skipped(273, false), "locked: kept until unlocked");
        assert!(!keep_skipped(273, true));
        assert!(!keep_skipped(264, true) && !keep_skipped(276, true));
    }

    #[test]
    fn disconnect_line_to_rejoin() {
        let l = "2026-10-24T09:00:01.000Z,1.0,5f64,6 [FLog::Network] Sending disconnect with reason: 277";
        let r = super::super::logparse::rejoin_events(l).1.unwrap();
        assert!(matches!(rejoin(r, true, &mut vec![], 0), Rejoin::Go { .. }));
    }

    #[test]
    fn rejoin_backoff_for_other_server_codes() {
        let go = |d, s| Rejoin::Go {
            delay_s: d,
            same_server: s,
        };
        assert_eq!(rejoin(264, true, &mut vec![], 0), go(60, true));
        assert_eq!(rejoin(273, true, &mut vec![0, 10], 20), go(240, false));
        // 276: "joined a game from another device" (Roblox 0.746+).
        assert_eq!(rejoin(276, true, &mut vec![], 0), go(60, true));
    }

    #[test]
    fn cooldown_exact_window_edge() {
        let mut h = vec![0, 0, 0];
        assert!(!cooldown_open(&mut h, 599));
        assert!(cooldown_open(&mut h, 600), "exactly 10 min: aged out");
    }

    #[test]
    fn cfg_missing_keys_are_safe() {
        let c = Cfg::from_settings(&serde_json::json!({}));
        assert!(!c.close_low_memory && !c.close_wrong_title && !c.close_no_server);
        assert!(!c.reconnect && !c.save_window_rects);
        assert!(c.ignore_existing);
        assert_eq!((c.low_memory_mb, c.no_server_sec), (200, 60));
        assert_eq!(c.expected_title, "Roblox");
    }

    #[test]
    fn cfg_reads_every_key() {
        let c = Cfg::from_settings(&serde_json::json!({"watcher": {
            "closeLowMemory": true, "lowMemoryMb": 300,
            "closeWrongTitle": true, "expectedTitle": " My Game ",
            "closeNoServer": true, "noServerSec": 90,
            "ignoreExisting": false, "saveWindowRects": true, "reconnect": true
        }}));
        assert_eq!(
            c,
            Cfg {
                close_low_memory: true,
                low_memory_mb: 300,
                close_wrong_title: true,
                expected_title: "My Game".into(),
                close_no_server: true,
                no_server_sec: 90,
                ignore_existing: false,
                save_window_rects: true,
                reconnect: true,
            }
        );
    }

    #[test]
    fn rule_order_low_memory_then_title_then_no_server() {
        let mut s = snap();
        s.memory_mb = 1;
        s.title = Some("x".into());
        assert_eq!(decide(&s, &cfg()), Some(Rule::LowMemory));
        let mut s = snap();
        s.title = Some("x".into());
        s.joined = false;
        s.idle_s = 999;
        assert_eq!(decide(&s, &cfg()), Some(Rule::WrongTitle));
    }
}
