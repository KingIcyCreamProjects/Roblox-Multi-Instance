//! macOS: clients start by running `Roblox.app/Contents/MacOS/RobloxPlayer -protocolString
//! <roblox-player URI>` directly. `open` hands the URI to the running copy, but a direct run
//! starts a new client each time, and the URI (with `browsertrackerid`) shows in the process
//! arguments, so clients map to accounts exactly as on Windows. Windows are handled through
//! the Accessibility API (`ax`), which needs RMI allowed under Privacy & Security.

use super::{Capabilities, Guard, Held, Platform, RobloxInstall};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use zeroize::Zeroizing;

pub const PLAYER: &str = "RobloxPlayer";

pub struct MacOs;

fn app_path() -> Option<PathBuf> {
    let home = dirs::home_dir().map(|h| h.join("Applications/Roblox.app"));
    [Some(PathBuf::from("/Applications/Roblox.app")), home]
        .into_iter()
        .flatten()
        .find(|p| p.join("Contents/MacOS").join(PLAYER).exists())
}

/// Arguments for one client: the player reads the URI from `-protocolString`.
fn launch_args(uri: &str) -> [&str; 2] {
    ["-protocolString", uri]
}

/// Roblox players with their command line (arguments joined by spaces) and start time (s).
fn players() -> Vec<(u32, Zeroizing<String>, i64)> {
    let mut s = System::new();
    // Names first (cheap), arguments only for Roblox players: an argv read per process
    // costs a sysctl each.
    s.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let pids: Vec<Pid> = s
        .processes()
        .values()
        .filter(|p| p.name() == PLAYER)
        .map(|p| p.pid())
        .collect();
    if pids.is_empty() {
        return vec![];
    }
    s.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    pids.iter()
        .filter_map(|pid| s.process(*pid))
        .map(|p| {
            let cmd = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            (p.pid().as_u32(), Zeroizing::new(cmd), p.start_time() as i64)
        })
        .collect()
}

pub fn cmdline(pid: u32) -> Option<Zeroizing<String>> {
    players().into_iter().find(|p| p.0 == pid).map(|p| p.1)
}

pub fn start_ms(pid: u32) -> Option<i64> {
    players()
        .into_iter()
        .find(|p| p.0 == pid)
        .map(|p| p.2 * 1000)
}

pub fn has_tracker(cmd: &str, t: &str) -> bool {
    crate::watcher::tracker_from_cmdline(cmd).as_deref() == Some(t)
}

fn alive(pid: u32, started: i64) -> bool {
    players().iter().any(|p| p.0 == pid && p.2 == started)
}

/// Close `pid` if it is still the same Roblox player (`started` s, `tracker`): SIGTERM,
/// then SIGKILL after 1 s (Roblox ignores SIGTERM while in a game). `Ok(true)` = gone.
pub fn close_verified(
    pid: u32,
    started: Option<i64>,
    tracker: Option<&str>,
) -> Result<bool, String> {
    let Some((_, cmd, st)) = players().into_iter().find(|p| p.0 == pid) else {
        return Ok(true);
    };
    if started.is_some_and(|s| (s - st).abs() > 2) || tracker.is_some_and(|t| !has_tracker(&cmd, t))
    {
        return Err("That process is no longer this Roblox client.".into());
    }
    let pid_t = pid as libc::pid_t;
    for sig in [libc::SIGTERM, libc::SIGKILL] {
        // SAFETY: plain signal to a pid just re-checked to be this client.
        unsafe { libc::kill(pid_t, sig) };
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            if !alive(pid, st) {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(!alive(pid, st))
}

fn tracked(t: &str) -> Vec<u32> {
    players()
        .into_iter()
        .filter(|p| has_tracker(&p.1, t))
        .map(|p| p.0)
        .collect()
}

struct NoLock;
impl Held for NoLock {}

impl Platform for MacOs {
    fn capabilities(&self) -> Capabilities {
        let launch = app_path().is_some();
        Capabilities {
            os: "macos",
            launch,
            launch_reason: (!launch).then(|| "Install Roblox from roblox.com first.".into()),
            multi_instance: true,
            cookie_lock: false,
            window_rect: true,
            watcher: true,
            client_helpers: true,
        }
    }

    fn roblox_install(&self) -> Option<RobloxInstall> {
        Some(RobloxInstall {
            // ClientSettings live next to the player binary on macOS.
            version_dir: Some(app_path()?.join("Contents/MacOS")),
            bootstrapper: None,
        })
    }

    fn open_uri(&self, uri: &str) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;
        let bin = app_path()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "Roblox is not installed.")
            })?
            .join("Contents/MacOS")
            .join(PLAYER);
        std::process::Command::new(bin)
            .args(launch_args(uri))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0) // outlives RMI, untouched by its signals
            .spawn()
            .map(|mut c| {
                // Reap on exit: a zombie still shows in sysinfo, so alive() would stay true.
                std::thread::spawn(move || {
                    let _ = c.wait();
                });
            })
    }

    fn roblox_running(&self) -> bool {
        !players().is_empty()
    }

    /// macOS has no singleton lock to hold: each direct run is its own client.
    fn hold_multi(&self) -> std::io::Result<Guard> {
        Ok(Box::new(NoLock))
    }

    fn close_tracked(&self, tracker_id: &str) -> std::io::Result<usize> {
        let n = tracked(tracker_id)
            .into_iter()
            .filter(|&p| close_verified(p, None, Some(tracker_id)).unwrap_or(false))
            .count();
        Ok(n)
    }

    fn close_pid(&self, pid: u32, tracker_id: &str) -> std::io::Result<bool> {
        close_verified(pid, None, Some(tracker_id)).map_err(std::io::Error::other)
    }

    fn setup_window(
        &self,
        tracker_id: &str,
        _name: &str, // macOS can't retitle another app's window
        rect: Option<(i32, i32, i32, i32)>,
        skip: &[u32],
        minimize: bool,
    ) -> std::io::Result<()> {
        if !ax::trusted(false) {
            return Err(std::io::Error::other(ax::NOT_TRUSTED));
        }
        let end = Instant::now() + Duration::from_secs(45);
        while Instant::now() < end {
            for pid in tracked(tracker_id)
                .into_iter()
                .filter(|p| !skip.contains(p))
            {
                if let Some(w) = ax::windows(pid).into_iter().next() {
                    if let Some((x, y, w2, h)) = rect {
                        if !ax::set_frame(&w, [x, y, w2, h]) {
                            return Err(std::io::Error::other("Could not move the Roblox window."));
                        }
                    }
                    if minimize && !ax::set_minimized(&w, true) {
                        return Err(std::io::Error::other(
                            "Could not minimize the Roblox window.",
                        ));
                    }
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "The Roblox window did not appear.",
        ))
    }

    fn tracked_pids(&self, tracker_id: &str) -> Vec<u32> {
        tracked(tracker_id)
    }

    fn tracked_clients(&self, trackers: &[String]) -> Vec<(u32, String)> {
        players()
            .into_iter()
            .filter_map(|(pid, cmd, _)| {
                let t = crate::watcher::tracker_from_cmdline(&cmd)?;
                trackers.contains(&t).then_some((pid, t))
            })
            .collect()
    }

    fn minimize(&self, clients: &[(u32, String)]) -> usize {
        clients
            .iter()
            .filter(|(pid, t)| tracked(t).contains(pid))
            .flat_map(|(pid, _)| ax::windows(*pid))
            .filter(|w| ax::set_minimized(w, true))
            .count()
    }

    fn idle_ms(&self) -> Option<u64> {
        // SAFETY: plain query; 0 = combined session state, !0 = any input event type.
        let s = unsafe { keys::CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
        (s >= 0.0).then_some((s * 1000.0) as u64)
    }

    fn nudge(&self) -> usize {
        if !ax::trusted(false) {
            return 0;
        }
        let prev = ax::frontmost_pid();
        let mut n = 0;
        for (pid, _, _) in players() {
            if !ax::activate(pid) {
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(150)); // let it come forward
            if ax::frontmost_pid() == Some(pid) {
                keys::space();
                n += 1;
            }
        }
        if let Some(p) = prev {
            ax::activate(p);
        }
        n
    }
}

/// Synthetic Space for anti-AFK (needs the Accessibility permission).
mod keys {
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGEventSourceSecondsSinceLastEventType(state: i32, ty: u32) -> f64;
        fn CGEventCreateKeyboardEvent(src: *const c_void, key: u16, down: bool) -> *mut c_void;
        fn CGEventPost(tap: u32, e: *mut c_void);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(o: *const c_void);
    }

    const SPACE: u16 = 49; // kVK_Space

    pub fn space() {
        for down in [true, false] {
            // SAFETY: event created (+1), posted to the HID tap (0), released once.
            unsafe {
                let e = CGEventCreateKeyboardEvent(std::ptr::null(), SPACE, down);
                if !e.is_null() {
                    CGEventPost(0, e);
                    CFRelease(e);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(90));
        }
    }
}

/// Roblox game windows in slot order (see `grid::slots`), ranked by `order`; accounts ranked
/// `keep` or later are skipped. Same contract as the Windows version.
pub fn ordered_windows(order: &[(String, String)], keep: usize) -> Vec<(usize, u32, ax::Win)> {
    let wins = players()
        .into_iter()
        .flat_map(|(pid, cmd, _)| {
            let rank = order
                .iter()
                .position(|(t, _)| has_tracker(&cmd, t))
                .unwrap_or(usize::MAX);
            ax::windows(pid).into_iter().map(move |w| (rank, pid, w))
        })
        .filter(|w| w.0 < keep || w.0 == usize::MAX)
        .collect();
    crate::grid::slots(wins)
}

/// Tiles the Roblox windows on `work` (points, top-left origin) per `layout`; see the
/// Windows version. Returns windows handled.
pub fn arrange_grid(
    order: &[(String, String)],
    keep: usize,
    work: crate::accounts::Rect,
    layout: &crate::grid::Layout,
    main: Option<usize>,
) -> Result<usize, String> {
    if !ax::trusted(true) {
        return Err(ax::NOT_TRUSTED.into());
    }
    let wins = ordered_windows(order, keep);
    let main = main
        .and_then(|m| wins.iter().position(|w| w.0 == m))
        .unwrap_or(0);
    let plan = crate::grid::plan(wins.len(), work, layout, main);
    let mut failed = 0;
    for ((_, _, w), t) in wins.iter().zip(&plan) {
        if t.minimize {
            failed += usize::from(!ax::set_minimized(w, true));
            continue;
        }
        let (c, (cw, ch)) = (t.cell, t.size());
        // Roblox may refuse to go below its minimum size: place it by the size it took.
        let ok = (!ax::minimized(w) || ax::set_minimized(w, false))
            && ax::set_frame(w, [c.x, c.y, cw, ch])
            && ax::frame(w).is_some_and(|f| {
                let (x, y) = t.pos(work, (f[2], f[3]));
                ax::set_frame(w, [x, y, f[2], f[3]])
            });
        failed += usize::from(!ok);
    }
    // Raise back to front so the plan's z order holds.
    let mut z: Vec<usize> = (0..wins.len()).filter(|&i| !plan[i].minimize).collect();
    z.sort_by_key(|&i| plan[i].z);
    for i in z {
        ax::raise(&wins[i].2);
    }
    if failed > 0 {
        return Err(format!(
            "Could not arrange {failed} of {} Roblox windows.",
            wins.len()
        ));
    }
    Ok(wins.len())
}

/// Frames (x, y, w, h in points) of the visible Roblox windows in grid slot order.
pub fn identify_frames(order: &[(String, String)], keep: usize) -> Vec<[i32; 4]> {
    // Untrusted: no windows are visible; the system prompt is the only hint we can give.
    if !ax::trusted(true) {
        return vec![];
    }
    ordered_windows(order, keep)
        .iter()
        .filter(|w| !ax::minimized(&w.2))
        .filter_map(|w| ax::frame(&w.2))
        .collect()
}

/// Thin Accessibility (AXUIElement) wrapper.
pub mod ax {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;

    pub const NOT_TRUSTED: &str =
        "Allow RMI in System Settings > Privacy & Security > Accessibility, then try again.";

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Size {
        w: f64,
        h: f64,
    }
    const POINT: u32 = 1; // kAXValueCGPointType
    const SIZE: u32 = 2; // kAXValueCGSizeType

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
        fn AXUIElementCreateSystemWide() -> CFTypeRef;
        fn AXUIElementCopyAttributeValue(e: CFTypeRef, a: CFStringRef, v: *mut CFTypeRef) -> i32;
        fn AXUIElementSetAttributeValue(e: CFTypeRef, a: CFStringRef, v: CFTypeRef) -> i32;
        fn AXUIElementPerformAction(e: CFTypeRef, a: CFStringRef) -> i32;
        fn AXUIElementGetPid(e: CFTypeRef, pid: *mut i32) -> i32;
        fn AXValueCreate(t: u32, v: *const c_void) -> CFTypeRef;
        fn AXValueGetValue(v: CFTypeRef, t: u32, out: *mut c_void) -> u8;
        fn AXIsProcessTrustedWithOptions(o: CFTypeRef) -> u8;
        static kAXTrustedCheckOptionPrompt: CFStringRef;
    }

    /// One window of another app.
    pub struct Win(CFType);

    fn s(a: &'static str) -> CFString {
        CFString::from_static_string(a)
    }

    fn get(e: &CFType, attr: &'static str) -> Option<CFType> {
        let mut v: CFTypeRef = std::ptr::null();
        // SAFETY: e is a live AXUIElement; v is written only on success (+1 retained).
        let err = unsafe {
            AXUIElementCopyAttributeValue(e.as_CFTypeRef(), s(attr).as_concrete_TypeRef(), &mut v)
        };
        // SAFETY: owned reference from a Copy call.
        (err == 0 && !v.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(v) })
    }

    fn set(e: &CFType, attr: &'static str, v: &CFType) -> bool {
        // SAFETY: live element and value.
        unsafe {
            AXUIElementSetAttributeValue(
                e.as_CFTypeRef(),
                s(attr).as_concrete_TypeRef(),
                v.as_CFTypeRef(),
            ) == 0
        }
    }

    fn app(pid: i32) -> CFType {
        // SAFETY: returns a new (+1) element; never null.
        unsafe { CFType::wrap_under_create_rule(AXUIElementCreateApplication(pid)) }
    }

    /// RMI may use Accessibility. `prompt`: let macOS show its "allow" dialog once.
    pub fn trusted(prompt: bool) -> bool {
        // SAFETY: the extern static is a constant CFString.
        let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
        let opts = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::from(prompt))]);
        // SAFETY: a valid dictionary.
        unsafe { AXIsProcessTrustedWithOptions(opts.as_CFTypeRef()) != 0 }
    }

    /// Standard windows of `pid`, front to back.
    pub fn windows(pid: u32) -> Vec<Win> {
        let Some(list) = get(&app(pid as i32), "AXWindows") else {
            return vec![];
        };
        // SAFETY: AXWindows is a CFArray of AXUIElements.
        let arr: CFArray<CFType> =
            unsafe { CFArray::wrap_under_get_rule(list.as_CFTypeRef().cast()) };
        arr.iter()
            .map(|w| Win(w.clone()))
            .filter(|w| {
                get(&w.0, "AXSubrole")
                    .and_then(|r| r.downcast::<CFString>())
                    .is_some_and(|r| r == "AXStandardWindow")
            })
            .collect()
    }

    pub fn title(w: &Win) -> Option<String> {
        get(&w.0, "AXTitle")?
            .downcast::<CFString>()
            .map(|t| t.to_string())
    }

    fn value<T: Default>(w: &Win, attr: &'static str, kind: u32) -> Option<T> {
        let v = get(&w.0, attr)?;
        let mut out = T::default();
        // SAFETY: kind matches T's layout (CGPoint / CGSize).
        (unsafe { AXValueGetValue(v.as_CFTypeRef(), kind, (&raw mut out).cast()) } != 0)
            .then_some(out)
    }

    /// (x, y, w, h) in points.
    pub fn frame(w: &Win) -> Option<[i32; 4]> {
        let p: Point = value(w, "AXPosition", POINT)?;
        let z: Size = value(w, "AXSize", SIZE)?;
        Some([p.x as i32, p.y as i32, z.w as i32, z.h as i32])
    }

    pub fn set_frame(w: &Win, [x, y, cw, ch]: [i32; 4]) -> bool {
        let p = Point {
            x: x.into(),
            y: y.into(),
        };
        let z = Size {
            w: cw.into(),
            h: ch.into(),
        };
        // SAFETY: AXValueCreate copies the struct; +1 result.
        let (pv, zv) = unsafe {
            (
                CFType::wrap_under_create_rule(AXValueCreate(POINT, (&raw const p).cast())),
                CFType::wrap_under_create_rule(AXValueCreate(SIZE, (&raw const z).cast())),
            )
        };
        // Size, move, size again: a window resized near a screen edge can be pushed back.
        let a = set(&w.0, "AXSize", &zv);
        let b = set(&w.0, "AXPosition", &pv);
        set(&w.0, "AXSize", &zv) && (a || b)
    }

    pub fn minimized(w: &Win) -> bool {
        get(&w.0, "AXMinimized")
            .and_then(|v| v.downcast::<CFBoolean>())
            .is_some_and(bool::from)
    }

    pub fn set_minimized(w: &Win, on: bool) -> bool {
        set(&w.0, "AXMinimized", &CFBoolean::from(on).as_CFType())
    }

    /// Bring this window to the front of its app's windows without activating the app.
    pub fn raise(w: &Win) {
        // SAFETY: live element.
        unsafe { AXUIElementPerformAction(w.0.as_CFTypeRef(), s("AXRaise").as_concrete_TypeRef()) };
    }

    /// Pid of the frontmost app.
    pub fn frontmost_pid() -> Option<u32> {
        // SAFETY: new (+1) system-wide element.
        let sys = unsafe { CFType::wrap_under_create_rule(AXUIElementCreateSystemWide()) };
        let a = get(&sys, "AXFocusedApplication")?;
        let mut pid = 0;
        // SAFETY: a is an AXUIElement.
        (unsafe { AXUIElementGetPid(a.as_CFTypeRef(), &mut pid) } == 0).then_some(pid as u32)
    }

    /// Make `pid`'s app frontmost again.
    pub fn activate(pid: u32) -> bool {
        set(
            &app(pid as i32),
            "AXFrontmost",
            &CFBoolean::true_value().as_CFType(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_gets_uri_via_protocol_string() {
        let uri = "roblox-player:1+launchmode:play+browsertrackerid:42";
        assert_eq!(launch_args(uri), ["-protocolString", uri]);
        assert!(has_tracker(&launch_args(uri).join(" "), "42"));
    }
}
