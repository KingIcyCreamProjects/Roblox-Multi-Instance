//! Linux: Roblox runs through Sober (Flatpak `org.vinegarhq.Sober`). Sober is built for one
//! copy per machine. Multi-instance isolates what the copies would share, one set per account:
//! - network namespace (`pasta`, package `passt`, no root): every copy claims the same
//!   localhost port, a second one stops with "frozen instance";
//! - time namespace (`unshare --time`): without it the newest client aborts 1-3 minutes in
//!   with "status != SL_RESULT_SUCCESS" (seen here; same finding in broreirik/RAM-Linux#1);
//! - Sober profile (`HOME`) and runtime dir (`XDG_RUNTIME_DIR`): a shared runtime dir left
//!   the second client without audio.
//!
//! Without `pasta`: one client at a time.

use super::{Capabilities, Guard, Held, Platform, RobloxInstall};
use std::io::{ErrorKind, Result};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

pub struct Linux;

const SOBER: &str = "org.vinegarhq.Sober";
/// Outbound only: no host ports forwarded in, host loopback not reachable.
const PASTA: [&str; 11] = [
    "--config-net",
    "-t",
    "none",
    "-u",
    "none",
    "-T",
    "none",
    "-U",
    "none",
    "--no-map-gw",
    "-q",
];

/// Only a yes is cached: installing it while RMI runs is picked up on the next check.
fn have(cache: &'static OnceLock<()>, cmd: &str, args: &[&str]) -> bool {
    if cache.get().is_some() {
        return true;
    }
    let ok = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        let _ = cache.set(());
    }
    ok
}

fn sober_installed() -> bool {
    static OK: OnceLock<()> = OnceLock::new();
    have(&OK, "flatpak", &["info", SOBER])
}

fn pasta_installed() -> bool {
    static OK: OnceLock<()> = OnceLock::new();
    have(&OK, "pasta", &["--version"])
}

/// The `browsertrackerid` field of a launch URI or a client's command line.
fn tracker_of(s: &str) -> Option<&str> {
    let t = s.split("browsertrackerid:").nth(1)?;
    let end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    (end > 0).then(|| &t[..end])
}

/// Our processes started with a launch URI: `(pid, tracker id)`. One client is several
/// processes (pasta, flatpak, bwrap, Sober); all carry the URI.
fn players() -> Vec<(u32, String)> {
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    dir.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            if e.metadata().ok().map(|m| m.uid()) != me {
                return None;
            }
            let cmd = std::fs::read(e.path().join("cmdline")).ok()?;
            let cmd = String::from_utf8_lossy(&cmd);
            cmd.contains("roblox-player:")
                .then(|| (pid, tracker_of(&cmd).unwrap_or("").to_owned()))
        })
        .collect()
}

/// One pid per client: the lowest of its processes.
fn clients(all: &[(u32, String)], tracker: Option<&str>) -> Vec<(u32, String)> {
    let mut v: Vec<_> = all
        .iter()
        .filter(|(_, t)| tracker.is_none_or(|x| x == t))
        .cloned()
        .collect();
    v.sort_by(|a, b| (&a.1, a.0).cmp(&(&b.1, b.0)));
    v.dedup_by(|b, a| a.1 == b.1);
    v
}

fn signal(sig: &str, pids: &[u32]) {
    let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
    let _ = Command::new("kill")
        .arg(sig)
        .args(&pids)
        .stderr(Stdio::null())
        .status();
}

/// Close every process of the client with this tracker (TERM, 1 s, then KILL). Returns 1
/// when there was one.
pub(crate) fn stop(tracker: &str) -> usize {
    let of = || -> Vec<u32> {
        players()
            .into_iter()
            .filter(|(_, t)| t == tracker)
            .map(|(p, _)| p)
            .collect()
    };
    let pids = of();
    if pids.is_empty() {
        return 0;
    }
    signal("-TERM", &pids);
    std::thread::sleep(Duration::from_secs(1));
    let left = of();
    if !left.is_empty() {
        signal("-KILL", &left);
    }
    1
}

/// Sober's folder inside every home that may hold a profile: the user's own, then one per
/// account under `<data>/rmi-sober`.
fn sober_dirs() -> Vec<PathBuf> {
    let accounts = dirs::data_dir()
        .and_then(|d| std::fs::read_dir(d.join("rmi-sober")).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path());
    dirs::home_dir()
        .into_iter()
        .chain(accounts)
        .map(|h| h.join(".var/app").join(SOBER))
        .collect()
}

/// Folders Roblox writes its player logs to, one per Sober profile (for the watcher).
pub(crate) fn log_dirs() -> Vec<PathBuf> {
    sober_dirs()
        .into_iter()
        .map(|d| d.join("data/sober/appData/logs"))
        .collect()
}

/// Sober profile of one account: `<data>/rmi-sober/<tracker>`. A new one is copied from
/// the user's own Sober profile or another account's, so Sober's welcome screen and the
/// Roblox download happen once, not per account.
fn profile(tracker: &str) -> Result<PathBuf> {
    let root = dirs::data_dir()
        .ok_or(ErrorKind::NotFound)?
        .join("rmi-sober");
    let home = root.join(tracker);
    if home.is_dir() {
        return Ok(home);
    }
    let app = Path::new(".var/app").join(SOBER);
    let seed = sober_dirs().into_iter().find(|p| p.join("data").is_dir());
    let dest = home.join(&app);
    std::fs::create_dir_all(dest.parent().expect("has a parent"))?;
    if let Some(seed) = seed {
        let ok = Command::new("cp")
            .args(["-a", "--reflink=auto"])
            .arg(&seed)
            .arg(&dest)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            log::warn!("Sober profile not copied; Sober will show its welcome screen");
        }
    }
    Ok(home)
}

/// Runtime dir of one client: `$XDG_RUNTIME_DIR/rmi-<tracker>`, with links to the session's
/// Wayland, PulseAudio and D-Bus sockets. `None` without a session runtime dir.
fn runtime_dir(tracker: &str) -> Option<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let base = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let dir = base.join(format!("rmi-{tracker}"));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .ok()?;
    let wayland = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    for name in [wayland.as_str(), "pulse", "pipewire-0", "bus"] {
        let (from, to) = (base.join(name), dir.join(name));
        if from.exists() && std::fs::symlink_metadata(&to).is_err() {
            if let Err(e) = std::os::unix::fs::symlink(&from, &to) {
                log::warn!("runtime dir link {name} failed: {e}");
            }
        }
    }
    Some(dir)
}

fn spawn(cmd: &mut Command) -> Result<()> {
    cmd.stdin(Stdio::null()).spawn().map(|mut c| {
        std::thread::spawn(move || {
            let _ = c.wait();
        }); // reap: no zombie per launch
    })
}

struct NoLock;
impl Held for NoLock {}

impl Platform for Linux {
    fn capabilities(&self) -> Capabilities {
        let ok = sober_installed();
        Capabilities {
            os: "linux",
            launch: ok,
            launch_reason: (!ok)
                .then(|| "Install Sober (org.vinegarhq.Sober) to launch Roblox on Linux.".into()),
            multi_instance: pasta_installed(),
            watcher: true,
            ..Default::default()
        }
    }

    fn roblox_install(&self) -> Option<RobloxInstall> {
        sober_installed().then_some(RobloxInstall {
            version_dir: None,
            bootstrapper: Some("Sober".into()),
        })
    }

    fn open_uri(&self, uri: &str) -> Result<()> {
        match tracker_of(uri).filter(|_| pasta_installed()) {
            Some(t) => {
                stop(t); // Sober refuses a second copy on one profile
                let home = profile(t)?;
                let mut cmd = Command::new("pasta");
                cmd.args(PASTA)
                    .args([
                        "--", "unshare", "--time", "--fork", "flatpak", "run", SOBER, uri,
                    ])
                    .env("HOME", home);
                if let Some(rt) = runtime_dir(t) {
                    // the session bus lives in the real runtime dir
                    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
                        if let Some(base) = rt.parent() {
                            let bus = format!("unix:path={}", base.join("bus").display());
                            cmd.env("DBUS_SESSION_BUS_ADDRESS", bus);
                        }
                    }
                    cmd.env("XDG_RUNTIME_DIR", rt);
                }
                spawn(&mut cmd)
            }
            None => {
                // Sober aborts with "An instance of Sober is already running": close it first.
                let _ = Command::new("flatpak").args(["kill", SOBER]).status();
                spawn(Command::new("flatpak").args(["run", SOBER, uri]))
            }
        }
    }

    fn roblox_running(&self) -> bool {
        !players().is_empty()
    }

    /// Nothing to hold: each client has its own network namespace.
    fn hold_multi(&self) -> Result<Guard> {
        if pasta_installed() {
            Ok(Box::new(NoLock))
        } else {
            Err(ErrorKind::Unsupported.into())
        }
    }

    fn close_tracked(&self, tracker_id: &str) -> Result<usize> {
        Ok(stop(tracker_id))
    }

    fn close_pid(&self, pid: u32, tracker_id: &str) -> Result<bool> {
        let mine = players().iter().any(|(p, t)| *p == pid && t == tracker_id);
        Ok(mine && stop(tracker_id) > 0)
    }

    fn tracked_pids(&self, tracker_id: &str) -> Vec<u32> {
        clients(&players(), Some(tracker_id))
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    }

    fn tracked_clients(&self, trackers: &[String]) -> Vec<(u32, String)> {
        clients(&players(), None)
            .into_iter()
            .filter(|(_, t)| trackers.contains(t))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_is_read_from_a_launch_uri() {
        let uri = "roblox-player:1+launchmode:play+gameinfo:t+browsertrackerid:123456789012+robloxLocale:en_us";
        assert_eq!(tracker_of(uri), Some("123456789012"));
        assert_eq!(tracker_of("roblox-player:1+launchmode:app"), None);
    }

    #[test]
    fn one_pid_per_client() {
        let all: Vec<(u32, String)> = [(30, "7"), (10, "8"), (12, "7"), (11, "8"), (40, "7")]
            .iter()
            .map(|(p, t)| (*p, (*t).to_owned()))
            .collect();
        assert_eq!(
            clients(&all, None),
            vec![(12, "7".to_owned()), (10, "8".to_owned())]
        );
        assert_eq!(clients(&all, Some("8")), vec![(10, "8".to_owned())]);
        assert!(clients(&all, Some("9")).is_empty());
    }
}
