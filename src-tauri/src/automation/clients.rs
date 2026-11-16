//! Client helpers (spec §9.2, Windows): which RMI-launched clients to close. Pure; the
//! relaunch loop feeds it only tracker-id clients, so nothing else is ever touched.

use std::collections::HashMap;
use tokio::time::{Duration, Instant};

/// A running RMI-launched client: its account's tracker id and when we first saw it.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub tracker: String,
    pub since: Instant,
}

/// Remember first-seen times; forget pids that are gone. `live`: pid → tracker id.
pub fn update(seen: &mut HashMap<u32, Seen>, live: &[(u32, String)], now: Instant) {
    seen.retain(|pid, _| live.iter().any(|(p, _)| p == pid));
    for (pid, tracker) in live {
        seen.entry(*pid).or_insert_with(|| Seen {
            tracker: tracker.clone(),
            since: now,
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// `clients.closeAfterMin` (None = off).
    pub close_after: Option<Duration>,
    /// `clients.closeScope == "global"`: one timer since `global_since` closes all.
    pub global: bool,
    /// `clients.maxInstances` (0 = off).
    pub max: usize,
}

/// Clients to close as `(pid, tracker id)`, oldest first: expired per instance or globally,
/// plus the oldest clients beyond `max`. Per pid, so an account's other client survives.
pub fn to_close(
    seen: &HashMap<u32, Seen>,
    r: Rules,
    global_since: Instant,
    now: Instant,
) -> Vec<(u32, String)> {
    let mut v: Vec<(&u32, &Seen)> = seen.iter().collect();
    v.sort_by_key(|(pid, s)| (s.since, **pid));
    let beyond = v
        .len()
        .saturating_sub(if r.max == 0 { v.len() } else { r.max });
    let mut out = vec![];
    for (i, (pid, s)) in v.iter().enumerate() {
        let expired = r.close_after.is_some_and(|d| {
            let from = if r.global { global_since } else { s.since };
            from + d <= now
        });
        if expired || i < beyond {
            out.push((**pid, s.tracker.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(v: &[(u32, &str)]) -> Vec<(u32, String)> {
        v.iter().map(|(p, t)| (*p, t.to_string())).collect()
    }

    fn trackers(v: Vec<(u32, String)>) -> Vec<String> {
        v.into_iter().map(|c| c.1).collect()
    }

    const OFF: Rules = Rules {
        close_after: None,
        global: false,
        max: 0,
    };

    #[tokio::test(start_paused = true)]
    async fn update_keeps_first_seen_and_drops_gone() {
        let mut s = HashMap::new();
        let t0 = Instant::now();
        update(&mut s, &live(&[(1, "a"), (2, "b")]), t0);
        tokio::time::advance(Duration::from_secs(5)).await;
        update(&mut s, &live(&[(2, "b"), (3, "c")]), Instant::now());
        assert!(!s.contains_key(&1));
        assert_eq!(s[&2].since, t0);
        assert_eq!(s[&3].since, Instant::now());
    }

    #[tokio::test(start_paused = true)]
    async fn max_instances_closes_oldest_beyond_cap() {
        let mut s = HashMap::new();
        let all = [(12, "c"), (10, "a"), (11, "b")];
        for n in 1..=3 {
            // c first, then a, then b: age order differs from pid order.
            update(&mut s, &live(&all[..n]), Instant::now());
            tokio::time::advance(Duration::from_secs(1)).await;
        }
        let now = Instant::now();
        assert_eq!(
            trackers(to_close(&s, Rules { max: 1, ..OFF }, now, now)),
            ["c", "a"]
        );
        assert!(to_close(&s, Rules { max: 3, ..OFF }, now, now).is_empty());
        assert!(to_close(&s, OFF, now, now).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn close_after_per_instance_and_global() {
        let mut s = HashMap::new();
        let t0 = Instant::now();
        update(&mut s, &live(&[(1, "a")]), t0);
        tokio::time::advance(Duration::from_secs(30)).await;
        update(&mut s, &live(&[(1, "a"), (2, "b")]), Instant::now());
        tokio::time::advance(Duration::from_secs(40)).await;
        let now = Instant::now();
        let per = Rules {
            close_after: Some(Duration::from_secs(60)),
            ..OFF
        };
        assert_eq!(trackers(to_close(&s, per, t0, now)), ["a"]);
        let global = Rules {
            global: true,
            ..per
        };
        assert_eq!(trackers(to_close(&s, global, t0, now)), ["a", "b"]);
        assert!(to_close(&s, global, now, now).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn one_tracker_with_two_pids_closes_only_the_old_pid() {
        let mut s = HashMap::new();
        update(&mut s, &live(&[(1, "a")]), Instant::now());
        tokio::time::advance(Duration::from_secs(1)).await;
        let now = Instant::now();
        update(&mut s, &live(&[(1, "a"), (2, "a")]), now);
        assert_eq!(
            to_close(&s, Rules { max: 1, ..OFF }, now, now),
            [(1, "a".to_string())]
        );
    }
}
