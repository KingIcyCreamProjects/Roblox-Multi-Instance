//! OS-specific behaviour. One impl per OS; the UI reads `capabilities()` and hides or
//! disables what the platform can't do, with `*_reason` as the tooltip.

use serde::Serialize;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(windows)]
pub(crate) mod windows;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub os: &'static str,
    pub launch: bool,
    pub launch_reason: Option<String>,
    pub multi_instance: bool,
    pub cookie_lock: bool,
    pub window_rect: bool,
    pub watcher: bool,
    pub client_helpers: bool,
    /// Fullscreen detection works (the watcher reconnect hold needs it).
    pub fullscreen_wait: bool,
    /// Remote access can capture and send sound.
    pub audio: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RobloxInstall {
    /// Folder holding `RobloxPlayerBeta.exe` (Windows) or the `.app` bundle (macOS).
    pub version_dir: Option<PathBuf>,
    /// Name of a third-party bootstrapper owning the protocol handler, if any.
    pub bootstrapper: Option<String>,
}

pub trait Platform: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn roblox_install(&self) -> Option<RobloxInstall>;
    fn open_uri(&self, uri: &str) -> std::io::Result<()>;
    /// `None`: the process list could not be read. Unknown is never "nothing runs": callers
    /// keep the mutex, launch nothing and report no close.
    fn roblox_running(&self) -> Option<bool> {
        Some(false)
    }
    /// Close idle Roblox tray players (they own the singleton mutex). Blocking. Called before
    /// a launch takes the mutex.
    fn close_idle_trays(&self) {}
    /// Hold the Roblox singleton mutex (§6.4) until the guard drops.
    fn hold_multi(&self) -> std::io::Result<Guard> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
    /// Hold an exclusive handle on Roblox's cookie store until the guard drops.
    fn hold_cookie_lock(&self) -> std::io::Result<Guard> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
    /// Close Roblox clients launched with this tracker id (graceful, 1 s, then kill).
    /// Blocking. Returns how many were closed.
    fn close_tracked(&self, _tracker_id: &str) -> std::io::Result<usize> {
        Ok(0)
    }
    /// Close one client `pid`, re-verified on the same handle as `tracker_id`'s Roblox
    /// player (graceful, 1 s, then kill). Blocking. `Ok(true)` = it is gone.
    fn close_pid(&self, _pid: u32, _tracker_id: &str) -> std::io::Result<bool> {
        Ok(false)
    }
    /// Title the game window of the client with this tracker id `name` and, if given, move
    /// it to `(x, y, w, h)`, polling up to 45 s for it to appear. Blocking.
    /// `skip`: pids that existed before this launch.
    /// `minimize`: minimize it (without activating) once it appears.
    fn setup_window(
        &self,
        _tracker_id: &str,
        _name: &str,
        _rect: Option<(i32, i32, i32, i32)>,
        _skip: &[u32],
        _minimize: bool,
    ) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
    /// Running Roblox client pids launched with this tracker id. `None`: unknown (see
    /// `roblox_running`).
    fn tracked_pids(&self, _tracker_id: &str) -> Option<Vec<u32>> {
        Some(vec![])
    }
    /// Running clients of any of `trackers`: `(pid, tracker id)`. `None`: unknown (see
    /// `roblox_running`). Blocking.
    fn tracked_clients(&self, trackers: &[String]) -> Option<Vec<(u32, String)>> {
        let mut out = vec![];
        for t in trackers {
            out.extend(self.tracked_pids(t)?.into_iter().map(|p| (p, t.clone())));
        }
        Some(out)
    }
    /// Minimize the windows of these `(pid, tracker id)` clients, re-checking each pid is
    /// still that tracker's Roblox player. Returns windows minimized.
    fn minimize(&self, _clients: &[(u32, String)]) -> usize {
        0
    }
    /// Milliseconds since the user last touched keyboard or mouse; `None` = unknown.
    fn idle_ms(&self) -> Option<u64> {
        None
    }
    /// Anti-AFK: input to every running Roblox player (Windows: chat button clicked twice;
    /// macOS: Space). Roblox only counts input sent to the focused window, so each is
    /// focused briefly and the user's window gets the focus back. No vault needed, so it
    /// keeps going while the vault is locked. Returns windows nudged. Blocking.
    fn nudge(&self) -> usize {
        0
    }
}

/// Tests: no OS side effects. The real platform shares named OS objects (the Roblox mutex
/// holder's stop event) with a running RMI, so a test dropping a real guard closed the
/// user's Roblox clients.
#[cfg(test)]
pub struct Null;

#[cfg(test)]
impl Platform for Null {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn roblox_install(&self) -> Option<RobloxInstall> {
        None
    }
    fn open_uri(&self, _uri: &str) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// Error of a close that could not read the process list (`close_tracked`).
pub(crate) const UNREADABLE: &str = "Could not read the running processes.";

/// OS resource released on drop.
pub trait Held: Send {
    /// Still held? `false` (e.g. the holder process died) makes the next sync re-acquire.
    fn alive(&self) -> bool {
        true
    }
    /// Held by RMI's own process (released when RMI exits), not the holder process.
    fn in_process(&self) -> bool {
        false
    }
}

/// Opaque OS resource released on drop.
pub type Guard = Box<dyn Held>;

pub fn current() -> Box<dyn Platform> {
    #[cfg(windows)]
    return Box::new(windows::Windows);
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacOs);
    #[cfg(target_os = "linux")]
    return Box::new(linux::Linux);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Null`, but its process list could not be read.
    struct Unknown;
    impl Platform for Unknown {
        fn capabilities(&self) -> Capabilities {
            Null.capabilities()
        }
        fn roblox_install(&self) -> Option<RobloxInstall> {
            Null.roblox_install()
        }
        fn open_uri(&self, uri: &str) -> std::io::Result<()> {
            Null.open_uri(uri)
        }
        fn roblox_running(&self) -> Option<bool> {
            None
        }
        fn tracked_pids(&self, _: &str) -> Option<Vec<u32>> {
            None
        }
    }

    #[test]
    fn an_unknown_process_list_stays_unknown() {
        let t = ["7".to_owned()];
        assert_eq!(Unknown.tracked_clients(&t), None, "never read as none run");
        assert_eq!(Unknown.tracked_clients(&[]), Some(vec![]), "nothing asked");
        // Null knows nothing runs.
        assert_eq!(Null.tracked_clients(&t), Some(vec![]));
        assert_eq!(Null.roblox_running(), Some(false));
        assert_eq!(Unknown.roblox_running(), None);
    }
}
