//! The screen and input of this computer, behind two small traits so tests use fakes and
//! never touch the real mouse. Windows: DXGI Desktop Duplication (xcap fallback) + SendInput.
//! macOS: xcap + enigo (compile-checked only). Linux: nothing (`no_display`; hosting stays off there).

use image::RgbaImage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub id: u8,
    /// Position and size in the coordinates input uses (virtual desktop pixels on Windows).
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub primary: bool,
}

/// What a session injects. Coordinates are absolute, in `Screen` coordinates.
pub trait Input {
    fn move_to(&mut self, x: i32, y: i32);
    /// 0 left, 1 middle, 2 right
    fn button(&mut self, b: u8, down: bool);
    /// 120 per notch, positive `dy` scrolls down
    fn wheel(&mut self, dx: i32, dy: i32);
    /// `KeyboardEvent.code`; unknown codes are ignored
    fn key(&mut self, code: &str, down: bool);
    fn text(&mut self, s: &str);
    fn lock(&mut self);
}

pub trait Desk: Send + Sync + 'static {
    fn os(&self) -> &'static str;
    fn screens(&self) -> Vec<Screen>;
    fn grab(&self, id: u8) -> Option<RgbaImage>;
    /// Monitor `id` surely shows what the last `grab` of it returned (Windows DXGI: no new
    /// frame), so that picture can be reused without a capture; false when unsure.
    fn unchanged(&self, _id: u8) -> bool {
        false
    }
    /// Pointer position in `Screen` coordinates, if known.
    fn cursor(&self) -> Option<(i32, i32)>;
    /// The pointer's CSS cursor name (`cursor` cap); `None` while it is hidden.
    fn cursor_shape(&self) -> Option<&'static str> {
        Some("default")
    }
    /// (`ready` | `locked` | `needs_permission` | `no_display`, can inject input)
    fn state(&self) -> (&'static str, bool);
    /// Called on the session's input thread (macOS input is not `Send`).
    fn input(&self) -> Box<dyn Input>;
    /// Sound output capture (`audio` cap); without it the cap is not listed.
    fn has_audio(&self) -> bool {
        false
    }
    /// Streams the sound output into `sink` while `run()` holds (on the caller's thread).
    fn audio(&self, _run: &dyn Fn() -> bool, _sink: &mut dyn FnMut(super::audio::Sound)) {}
}

/// The real desktop of this computer.
pub fn system() -> std::sync::Arc<dyn Desk> {
    std::sync::Arc::new(System)
}

struct System;

#[cfg(any(windows, target_os = "macos"))]
fn monitors() -> Vec<(Screen, xcap::Monitor)> {
    let all = xcap::Monitor::all().unwrap_or_default();
    all.into_iter()
        .take(16)
        .enumerate()
        .map(|(i, m)| {
            let s = Screen {
                id: i as u8,
                x: m.x().unwrap_or(0),
                y: m.y().unwrap_or(0),
                w: m.width().unwrap_or(0),
                h: m.height().unwrap_or(0),
                primary: m.is_primary().unwrap_or(false),
            };
            (s, m)
        })
        .collect()
}

#[cfg(windows)]
#[path = "dxgi.rs"]
mod dxgi;

#[cfg(any(windows, target_os = "macos"))]
type Mons = (
    Option<std::time::Instant>,
    Vec<(Option<(i32, i32)>, xcap::Monitor)>,
);

#[cfg(any(windows, target_os = "macos"))]
thread_local! {
    /// The capture thread's monitor list (xcap monitors are not `Send` on Windows), with
    /// positions (`None` if unreadable: DXGI is not used there): re-read every second (like
    /// the screen list) or after a failed capture, never per frame (each read costs
    /// display-settings calls on Windows).
    static MONS: std::cell::RefCell<Mons> = const { std::cell::RefCell::new((None, Vec::new())) };
}

#[cfg(any(windows, target_os = "macos"))]
fn grab_monitor(id: u8) -> Option<RgbaImage> {
    MONS.with_borrow_mut(|(at, list)| {
        let stale = at.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1));
        if stale || list.len() <= id.into() {
            *list = xcap::Monitor::all()
                .unwrap_or_default()
                .into_iter()
                .map(|m| (m.x().ok().zip(m.y().ok()), m))
                .collect();
            *at = Some(std::time::Instant::now());
        }
        let (_pos, m) = list.get(usize::from(id))?;
        #[cfg(windows)]
        if let Some(img) = _pos.and_then(dxgi::grab) {
            return Some(img);
        }
        let img = m
            .capture_image()
            .map_err(|e| log::debug!("remote: capture failed: {e}"))
            .ok();
        if img.is_none() {
            *at = None;
        }
        img
    })
}

#[cfg(windows)]
impl Desk for System {
    fn os(&self) -> &'static str {
        "windows"
    }
    fn screens(&self) -> Vec<Screen> {
        monitors().into_iter().map(|(s, _)| s).collect()
    }
    fn grab(&self, id: u8) -> Option<RgbaImage> {
        grab_monitor(id)
    }
    fn unchanged(&self, id: u8) -> bool {
        MONS.with_borrow(|(_, l)| l.get(usize::from(id)).and_then(|m| m.0))
            .is_some_and(dxgi::unchanged)
    }
    fn cursor(&self) -> Option<(i32, i32)> {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: writes one POINT.
        (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
    }
    fn cursor_shape(&self) -> Option<&'static str> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetCursorInfo, LoadCursorW, CURSORINFO, CURSOR_SHOWING, IDC_APPSTARTING, IDC_ARROW,
            IDC_CROSS, IDC_HAND, IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS,
            IDC_SIZENWSE, IDC_SIZEWE, IDC_WAIT,
        };
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            flags: 0,
            hCursor: std::ptr::null_mut(),
            ptScreenPos: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
        };
        // SAFETY: writes one CURSORINFO with cbSize set.
        if unsafe { GetCursorInfo(&mut ci) } == 0 {
            return Some("default");
        }
        if ci.flags & CURSOR_SHOWING == 0 {
            return None;
        }
        // system cursors are shared handles: compare with the stock ones
        [
            (IDC_ARROW, "default"),
            (IDC_IBEAM, "text"),
            (IDC_HAND, "pointer"),
            (IDC_SIZEALL, "move"),
            (IDC_SIZEWE, "ew-resize"),
            (IDC_SIZENS, "ns-resize"),
            (IDC_SIZENWSE, "nwse-resize"),
            (IDC_SIZENESW, "nesw-resize"),
            (IDC_WAIT, "wait"),
            (IDC_APPSTARTING, "wait"),
            (IDC_CROSS, "crosshair"),
            (IDC_NO, "not-allowed"),
        ]
        .into_iter()
        // SAFETY: loads a stock cursor (no instance), nothing to free.
        .find(|(id, _)| unsafe { LoadCursorW(std::ptr::null_mut(), *id) } == ci.hCursor)
        .map_or(Some("default"), |(_, n)| Some(n))
    }
    fn state(&self) -> (&'static str, bool) {
        use windows_sys::Win32::System::StationsAndDesktops::{
            CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS,
        };
        // The lock screen, UAC and sign-in run on the secure desktop: a normal process
        // cannot open the input desktop then.
        // SAFETY: handle closed right away.
        let h = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
        if h.is_null() {
            return ("locked", false);
        }
        unsafe { CloseDesktop(h) };
        ("ready", true)
    }
    fn input(&self) -> Box<dyn Input> {
        Box::new(win::Win)
    }
    fn has_audio(&self) -> bool {
        true
    }
    fn audio(&self, run: &dyn Fn() -> bool, sink: &mut dyn FnMut(super::audio::Sound)) {
        super::audio::loopback(run, sink);
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE,
        MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    pub struct Win;

    fn send(i: &[INPUT]) {
        // SAFETY: a slice of fully initialised INPUTs.
        unsafe {
            SendInput(
                i.len() as u32,
                i.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
    }

    fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn kb(scan: u16, flags: u32) -> INPUT {
        kb_vk(0, scan, flags)
    }

    fn kb_vk(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    /// `p` in `[origin, origin + size)` to SendInput's 0..65535 virtual-desktop scale.
    pub(super) fn norm(p: i32, origin: i32, size: i32) -> i32 {
        let span = i64::from((size - 1).max(1));
        (i64::from(p - origin).clamp(0, span) * 65535 / span) as i32
    }

    impl super::Input for Win {
        fn move_to(&mut self, x: i32, y: i32) {
            // SAFETY: plain metric reads.
            let (vx, vy, vw, vh) = unsafe {
                (
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    GetSystemMetrics(SM_CXVIRTUALSCREEN),
                    GetSystemMetrics(SM_CYVIRTUALSCREEN),
                )
            };
            let f = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
            send(&[mouse(norm(x, vx, vw), norm(y, vy, vh), 0, f)]);
        }
        fn button(&mut self, b: u8, down: bool) {
            let f = match (b, down) {
                (0, true) => MOUSEEVENTF_LEFTDOWN,
                (0, false) => MOUSEEVENTF_LEFTUP,
                (1, true) => MOUSEEVENTF_MIDDLEDOWN,
                (1, false) => MOUSEEVENTF_MIDDLEUP,
                (2, true) => MOUSEEVENTF_RIGHTDOWN,
                (2, false) => MOUSEEVENTF_RIGHTUP,
                _ => return,
            };
            send(&[mouse(0, 0, 0, f)]);
        }
        fn wheel(&mut self, dx: i32, dy: i32) {
            // Windows: positive wheel data scrolls up, so flip dy
            if dy != 0 {
                send(&[mouse(0, 0, -dy, MOUSEEVENTF_WHEEL)]);
            }
            if dx != 0 {
                send(&[mouse(0, 0, dx, MOUSEEVENTF_HWHEEL)]);
            }
        }
        fn key(&mut self, code: &str, down: bool) {
            if let Some(vk) = super::super::keys::win_vk(code) {
                send(&[kb_vk(vk, 0, if down { 0 } else { KEYEVENTF_KEYUP })]);
                return;
            }
            let Some(sc) = super::super::keys::scancode(code) else {
                return;
            };
            let mut f = KEYEVENTF_SCANCODE;
            if sc & 0xFF00 == 0xE000 {
                f |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                f |= KEYEVENTF_KEYUP;
            }
            send(&[kb(sc & 0xFF, f)]);
        }
        fn text(&mut self, s: &str) {
            let ev: Vec<INPUT> = s
                .encode_utf16()
                .flat_map(|u| {
                    [
                        kb(u, KEYEVENTF_UNICODE),
                        kb(u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                    ]
                })
                .collect();
            send(&ev);
        }
        fn lock(&mut self) {
            // SAFETY: plain call.
            unsafe { windows_sys::Win32::System::Shutdown::LockWorkStation() };
        }
    }
}

#[cfg(target_os = "macos")]
impl Desk for System {
    fn os(&self) -> &'static str {
        "macos"
    }
    fn screens(&self) -> Vec<Screen> {
        monitors().into_iter().map(|(s, _)| s).collect()
    }
    fn grab(&self, id: u8) -> Option<RgbaImage> {
        grab_monitor(id)
    }
    fn cursor(&self) -> Option<(i32, i32)> {
        mac::pointer()
    }
    // shape: "default" (the trait's), position only
    fn state(&self) -> (&'static str, bool) {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
            fn CGRequestScreenCaptureAccess() -> bool;
        }
        static ASKED: std::sync::Once = std::sync::Once::new();
        // SAFETY: no arguments, reads the TCC state.
        if !unsafe { CGPreflightScreenCaptureAccess() } {
            // once per run: puts RMI in the Screen Recording list (macOS asks, a restart applies it)
            // SAFETY: no arguments.
            ASKED.call_once(|| unsafe {
                CGRequestScreenCaptureAccess();
            });
            return ("needs_permission", false);
        }
        ("ready", crate::platform::macos::ax::trusted(false))
    }
    fn input(&self) -> Box<dyn Input> {
        // the first try may show the Accessibility prompt; later ones (mac::Mac::get) do not
        let e = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| log::warn!("remote: input not allowed yet: {e}"))
            .ok();
        Box::new(mac::Mac(e, [0; 2]))
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use enigo::{Axis, Button, Coordinate, Direction, Key, Keyboard, Mouse};

    /// `None` until Accessibility is granted; retried on the next event. Then the wheel
    /// deltas (y, x) not yet a whole notch.
    pub struct Mac(pub Option<enigo::Enigo>, pub [i32; 2]);

    impl Mac {
        fn get(&mut self) -> Option<&mut enigo::Enigo> {
            if self.0.is_none() {
                let s = enigo::Settings {
                    open_prompt_to_get_permissions: false,
                    ..Default::default()
                };
                self.0 = enigo::Enigo::new(&s).ok();
            }
            self.0.as_mut()
        }
    }

    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    /// Pointer position in global display points (what xcap's monitor rects use).
    pub fn pointer() -> Option<(i32, i32)> {
        // SAFETY: a null-source event only reads the current location; released right after.
        unsafe {
            let e = CGEventCreate(std::ptr::null());
            if e.is_null() {
                return None;
            }
            let p = CGEventGetLocation(e);
            CFRelease(e);
            Some((p.x as i32, p.y as i32))
        }
    }

    /// `media` cap keys macOS has (sent as system events by enigo). Pause and Stop: none.
    fn media(code: &str) -> Option<Key> {
        Some(match code {
            "AudioVolumeUp" => Key::VolumeUp,
            "AudioVolumeDown" => Key::VolumeDown,
            "AudioVolumeMute" => Key::VolumeMute,
            "MediaPlayPause" => Key::MediaPlayPause,
            "MediaTrackNext" => Key::MediaNextTrack,
            "MediaTrackPrevious" => Key::MediaPrevTrack,
            _ => return None,
        })
    }

    fn dir(down: bool) -> Direction {
        if down {
            Direction::Press
        } else {
            Direction::Release
        }
    }

    impl super::Input for Mac {
        fn move_to(&mut self, x: i32, y: i32) {
            if let Some(e) = self.get() {
                let _ = e.move_mouse(x, y, Coordinate::Abs);
            }
        }
        fn button(&mut self, b: u8, down: bool) {
            let b = match b {
                0 => Button::Left,
                1 => Button::Middle,
                2 => Button::Right,
                _ => return,
            };
            if let Some(e) = self.get() {
                let _ = e.button(b, dir(down));
            }
        }
        fn wheel(&mut self, dx: i32, dy: i32) {
            // small deltas (a trackpad, every frame) add up to whole notches, not one each
            let mut notches = [0; 2];
            for (i, d) in [dy, dx].into_iter().enumerate() {
                let r = self.1[i].saturating_add(d);
                notches[i] = r / 120;
                self.1[i] = r % 120;
            }
            let Some(e) = self.get() else { return };
            if notches[0] != 0 {
                let _ = e.scroll(notches[0], Axis::Vertical);
            }
            if notches[1] != 0 {
                let _ = e.scroll(notches[1], Axis::Horizontal);
            }
        }
        fn key(&mut self, code: &str, down: bool) {
            let Some(e) = self.get() else { return };
            if let Some(k) = media(code) {
                let _ = e.key(k, dir(down));
            } else if let Some(k) = super::super::keys::mac_keycode(code) {
                let _ = e.raw(k, dir(down));
            }
        }
        fn text(&mut self, s: &str) {
            if let Some(e) = self.get() {
                let _ = e.text(s);
            }
        }
        fn lock(&mut self) {
            let Some(e) = self.get() else { return };
            // Ctrl+Cmd+Q
            for (k, d) in [(0x3B, true), (0x37, true), (0x0C, true)] {
                let _ = e.raw(k, dir(d));
            }
            for k in [0x0C, 0x37, 0x3B] {
                let _ = e.raw(k, Direction::Release);
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
impl Desk for System {
    fn os(&self) -> &'static str {
        "linux"
    }
    fn screens(&self) -> Vec<Screen> {
        Vec::new()
    }
    fn grab(&self, _: u8) -> Option<RgbaImage> {
        None
    }
    fn cursor(&self) -> Option<(i32, i32)> {
        None
    }
    fn state(&self) -> (&'static str, bool) {
        ("no_display", false)
    }
    fn input(&self) -> Box<dyn Input> {
        Box::new(NoInput)
    }
}

/// No screen, no input: app tests (never the real desktop, like `platform::Null`).
pub struct Null;

impl Desk for Null {
    fn os(&self) -> &'static str {
        "linux"
    }
    fn screens(&self) -> Vec<Screen> {
        Vec::new()
    }
    fn grab(&self, _: u8) -> Option<RgbaImage> {
        None
    }
    fn cursor(&self) -> Option<(i32, i32)> {
        None
    }
    fn state(&self) -> (&'static str, bool) {
        ("no_display", false)
    }
    fn input(&self) -> Box<dyn Input> {
        Box::new(NoInput)
    }
}

/// Input that does nothing (no permission, no platform support).
pub struct NoInput;

impl Input for NoInput {
    fn move_to(&mut self, _: i32, _: i32) {}
    fn button(&mut self, _: u8, _: bool) {}
    fn wheel(&mut self, _: i32, _: i32) {}
    fn key(&mut self, _: &str, _: bool) {}
    fn text(&mut self, _: &str) {}
    fn lock(&mut self) {}
}

/// Classic arrow, `X` outline and `.` fill, tip at (0, 0).
const ARROW: [&str; 17] = [
    "X",
    "XX",
    "X.X",
    "X..X",
    "X...X",
    "X....X",
    "X.....X",
    "X......X",
    "X.......X",
    "X........X",
    "X.....XXXXX",
    "X..X..X",
    "X.X X..X",
    "XX  X..X",
    "X    X..X",
    "     X..X",
    "      XX",
];

/// Draws the pointer with its tip at (`x`, `y`) image pixels; parts outside are clipped.
pub fn draw_cursor(img: &mut RgbaImage, x: i64, y: i64) {
    for (dy, row) in ARROW.iter().enumerate() {
        for (dx, c) in row.bytes().enumerate() {
            let px = match c {
                b'X' => [0, 0, 0, 255],
                b'.' => [255, 255, 255, 255],
                _ => continue,
            };
            let (px_x, px_y) = (x + dx as i64, y + dy as i64);
            if (0..i64::from(img.width())).contains(&px_x)
                && (0..i64::from(img.height())).contains(&px_y)
            {
                img.put_pixel(px_x as u32, px_y as u32, image::Rgba(px));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_clips_at_edges() {
        let mut img = RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255]));
        draw_cursor(&mut img, 6, 6);
        draw_cursor(&mut img, -50, -50);
        assert_eq!(img.get_pixel(6, 6).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(0, 0).0, [9, 9, 9, 255]);
    }

    #[cfg(windows)]
    #[test]
    fn virtual_desktop_scale() {
        assert_eq!(win::norm(0, 0, 1920), 0);
        assert_eq!(win::norm(1919, 0, 1920), 65535);
        assert_eq!(win::norm(-1920, -1920, 3840), 0);
        assert_eq!(win::norm(5000, 0, 1920), 65535, "clamped");
    }

    /// Needs the interactive desktop (not an SSH session):
    /// `cargo test --release --lib capture_speed -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn capture_speed() {
        let m = xcap::Monitor::all().unwrap().remove(0);
        let at = (m.x().unwrap(), m.y().unwrap());
        let time = |name: &str, f: &dyn Fn() -> Option<RgbaImage>| {
            let t = std::time::Instant::now();
            let n = (0..60).filter(|_| f().is_some()).count();
            println!(
                "{name}: {n}/60 ok, {:.1} ms/frame",
                t.elapsed().as_secs_f64() * 1000.0 / 60.0
            );
        };
        assert!(dxgi::grab(at).is_some(), "DXGI unavailable here");
        time("dxgi", &|| dxgi::grab(at));
        // open + first full copy each time: upper bound for a changed frame
        time("dxgi reopen", &|| {
            dxgi::reset();
            dxgi::grab(at)
        });
        time("xcap", &|| m.capture_image().ok());
        time("grab_monitor", &|| grab_monitor(0));
    }
}
