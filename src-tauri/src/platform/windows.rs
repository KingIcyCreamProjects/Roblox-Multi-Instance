use super::{Capabilities, Guard, Platform, RobloxInstall};
use std::{
    collections::HashMap,
    io,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, LazyLock, Mutex, PoisonError,
    },
};
use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, FILETIME, HANDLE, HWND,
        INVALID_HANDLE_VALUE, LPARAM, LRESULT, RECT, UNICODE_STRING, WAIT_OBJECT_0, WAIT_TIMEOUT,
        WPARAM,
    },
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Gdi::{
            BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
            MonitorFromRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_SINGLELINE,
            DT_VCENTER, FW_BOLD, MONITOR_DEFAULTTONULL, PAINTSTRUCT, TRANSPARENT,
        },
    },
    System::{
        Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE},
        Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_REG_SZ},
        Threading::{
            CreateEventW, CreateMutexW, GetProcessTimes, OpenEventW, OpenMutexW, OpenProcess,
            QueryFullProcessImageNameW, ReleaseMutex, SetEvent, TerminateProcess,
            WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, DETACHED_PROCESS,
            EVENT_MODIFY_STATE, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, SYNCHRONIZATION_SYNCHRONIZE,
        },
    },
    UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
            GetClassNameW, GetClientRect, GetForegroundWindow, GetMessageW, GetWindow,
            GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow, IsIconic,
            IsWindow, IsWindowVisible, IsZoomed, PostMessageW, RegisterClassW, SendMessageTimeoutW,
            SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, ShowWindowAsync,
            SystemParametersInfoW, GW_OWNER, HWND_TOP, LWA_ALPHA, MSG, SMTO_ABORTIFHUNG,
            SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MINIMIZE,
            SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, WM_CLOSE, WM_PAINT, WM_SETTEXT, WM_TIMER,
            WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
            WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

/// Anti-AFK focus swap (see `Platform::nudge`).
mod nudge {
    use std::{thread::sleep, time::Duration};
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MOVE, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindow, SetCursorPos, SetForegroundWindow,
        ShowWindow, ShowWindowAsync, WindowFromPoint, GA_ROOT, SW_SHOWMINNOACTIVE,
        SW_SHOWNOACTIVATE,
    };

    /// Chat button in Roblox's top bar, client px at 96 DPI (scales with the window's DPI).
    const CHAT: (i32, i32) = (138, 33);

    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK,
        KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
        WH_MOUSE_LL, WM_MOUSEMOVE, WM_TIMER,
    };

    static LAST: AtomicU64 = AtomicU64::new(0);
    /// Last real key or mouse button/wheel (not moves: our own SetCursorPos may show up as
    /// one). `run` stops when the user comes back mid-run.
    static PRESS: AtomicU64 = AtomicU64::new(0);
    /// The hook thread is up (it exits once anti-AFK stops asking).
    static RUNNING: AtomicBool = AtomicBool::new(false);
    /// Last `idle_ms` call (anti-AFK asks every 2 s while it is on).
    static ASKED: AtomicU64 = AtomicU64::new(0);

    /// Anti-AFK stopped asking (turned off): the system-wide hooks should go.
    pub(super) fn hooks_unwanted(now: u64, asked: u64) -> bool {
        now.saturating_sub(asked) > 30_000
    }

    unsafe extern "system" fn on_key(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
        if code >= 0 && (*(l as *const KBDLLHOOKSTRUCT)).flags & LLKHF_INJECTED == 0 {
            let t = GetTickCount64();
            LAST.store(t, Relaxed);
            PRESS.store(t, Relaxed);
        }
        CallNextHookEx(std::ptr::null_mut(), code, w, l)
    }

    unsafe extern "system" fn on_mouse(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
        if code >= 0 && (*(l as *const MSLLHOOKSTRUCT)).flags & LLMHF_INJECTED == 0 {
            let t = GetTickCount64();
            LAST.store(t, Relaxed);
            if w as u32 != WM_MOUSEMOVE {
                PRESS.store(t, Relaxed);
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, w, l)
    }

    /// Both hooks are installed (optimistic until the hook thread says otherwise).
    static HOOKED: AtomicBool = AtomicBool::new(true);

    /// Since the user last touched a real keyboard or mouse. Low-level hooks, because
    /// GetLastInputInfo is also reset by injected input (other tools, our own clicks) and by
    /// some devices, so it can read 0 forever. Only timestamps are kept, never keys.
    /// `None` (anti-AFK paused) when a hook could not be installed.
    pub fn idle_ms() -> Option<u64> {
        // SAFETY: plain call.
        let now = unsafe { GetTickCount64() };
        ASKED.store(now, Relaxed);
        if !RUNNING.swap(true, Relaxed) {
            LAST.store(now, Relaxed);
            HOOKED.store(true, Relaxed);
            // SAFETY: hooks live on a thread that only pumps messages; handles unhooked once.
            std::thread::spawn(|| unsafe {
                // Every system-wide key/mouse event waits on this thread: never let a busy
                // PC starve it.
                SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
                let arm = || {
                    let k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), null_mut(), 0);
                    let m = SetWindowsHookExW(WH_MOUSE_LL, Some(on_mouse), null_mut(), 0);
                    HOOKED.store(!k.is_null() && !m.is_null(), Relaxed);
                    [k, m]
                };
                let unhook = |hooks: [_; 2]| {
                    for h in hooks.into_iter().filter(|h: &HHOOK| !h.is_null()) {
                        UnhookWindowsHookEx(h);
                    }
                };
                let mut hooks = arm();
                // Windows silently drops an LL hook whose callback ran too long (pegged
                // CPU): re-install both every minute, or remove them once anti-AFK is off.
                SetTimer(null_mut(), 0, 60_000, None);
                let mut m: MSG = std::mem::zeroed();
                while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
                    if m.message == WM_TIMER {
                        unhook(hooks);
                        if hooks_unwanted(GetTickCount64(), ASKED.load(Relaxed)) {
                            HOOKED.store(false, Relaxed); // stale LAST: never read as idle
                            RUNNING.store(false, Relaxed);
                            return;
                        }
                        hooks = arm();
                    }
                }
            });
        }
        // SAFETY: plain call.
        HOOKED
            .load(Relaxed)
            .then(|| unsafe { GetTickCount64() }.saturating_sub(LAST.load(Relaxed)))
    }

    unsafe fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32) {
        let i = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        SendInput(1, &i, std::mem::size_of::<INPUT>() as i32);
    }

    /// Click `w`'s chat button twice (open, close: chat ends as it was). Roblox only sees a
    /// click after the cursor moves onto the button, so it slides in over 3 px first. Stops
    /// before a click once the user is back (a real key or click since `t0`), so their input
    /// goes back to their own window at once (Roblox's chat may be left open).
    unsafe fn click_chat(w: HWND, t0: u64) -> bool {
        let dpi = GetDpiForWindow(w).max(96) as i32;
        let mut p = POINT {
            x: CHAT.0 * dpi / 96,
            y: CHAT.1 * dpi / 96,
        };
        if ClientToScreen(w, &mut p) == 0 || GetAncestor(WindowFromPoint(p), GA_ROOT) != w {
            return false; // something covers the button: never click another window
        }
        SetCursorPos(p.x - 3, p.y);
        for _ in 0..3 {
            sleep(Duration::from_millis(30));
            mouse(MOUSEEVENTF_MOVE, 1);
        }
        sleep(Duration::from_millis(120));
        for i in 0..2 {
            if i > 0 {
                sleep(Duration::from_millis(300));
            }
            if PRESS.load(Relaxed) > t0 {
                return false;
            }
            mouse(MOUSEEVENTF_LEFTDOWN, 0);
            sleep(Duration::from_millis(50));
            mouse(MOUSEEVENTF_LEFTUP, 0);
        }
        sleep(Duration::from_millis(30));
        true
    }

    /// Foreground `w`, borrowing the current foreground thread's input state so Windows
    /// lets a background process do it.
    unsafe fn focus(w: HWND) -> bool {
        let fg = GetForegroundWindow();
        if fg != w {
            let me = GetCurrentThreadId();
            let other = GetWindowThreadProcessId(fg, std::ptr::null_mut());
            let attached = other != 0 && other != me && AttachThreadInput(me, other, 1) != 0;
            SetForegroundWindow(w);
            if attached {
                AttachThreadInput(me, other, 0);
            }
            sleep(Duration::from_millis(60)); // a frame for Roblox to see the focus
        }
        GetForegroundWindow() == w
    }

    /// A non-Roblox app covering its whole monitor (game, video): don't pull it out.
    pub(super) unsafe fn fullscreen_app(w: HWND) -> bool {
        let mut class = [0u16; 32];
        let n = GetClassNameW(w, class.as_mut_ptr(), class.len() as i32);
        let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        if w.is_null() || matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return false; // desktop / taskbar
        }
        let mut r: RECT = std::mem::zeroed();
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetWindowRect(w, &mut r) != 0
            && GetMonitorInfoW(MonitorFromWindow(w, MONITOR_DEFAULTTONEAREST), &mut mi) != 0
            && r.left <= mi.rcMonitor.left
            && r.top <= mi.rcMonitor.top
            && r.right >= mi.rcMonitor.right
            && r.bottom >= mi.rcMonitor.bottom
    }

    pub unsafe fn run(wins: &[HWND]) -> usize {
        let prev = GetForegroundWindow();
        // `fullscreen` also rules out a maximized captioned window, which covers its monitor
        // when the taskbar auto-hides or sits on another screen.
        if wins.is_empty() || (!wins.contains(&prev) && super::fullscreen(prev as isize)) {
            return 0;
        }
        let mut cur: POINT = std::mem::zeroed();
        let had_cur = GetCursorPos(&mut cur) != 0;
        let mut n = 0;
        let t0 = GetTickCount64();
        for &w in wins {
            // The user is back (key, click): stop; cursor and focus still go back below.
            if PRESS.load(Relaxed) > t0 {
                break;
            }
            let min = IsIconic(w) != 0;
            if min {
                ShowWindow(w, SW_SHOWNOACTIVATE);
            }
            if focus(w) && click_chat(w, t0) {
                n += 1;
            }
            if min {
                ShowWindowAsync(w, SW_SHOWMINNOACTIVE);
            }
        }
        if had_cur {
            SetCursorPos(cur.x, cur.y);
        }
        if IsWindow(prev) != 0 {
            focus(prev);
        }
        n
    }
}

/// Window `h` is a borderless non-shell app covering its whole monitor (a game, a video).
/// A title bar rules it out: with an auto-hide taskbar a maximized window covers it too.
pub fn fullscreen(h: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GWL_STYLE, WS_CAPTION};
    // SAFETY: plain queries; a null or stale handle fails them.
    unsafe {
        GetWindowLongW(h as _, GWL_STYLE) as u32 & WS_CAPTION != WS_CAPTION
            && nudge::fullscreen_app(h as _)
    }
}

pub const ROBLOX_MUTEX: &str = "ROBLOX_singletonMutex";
const PLAYER_EXE: &str = "RobloxPlayerBeta.exe";
/// Window class of the Roblox game window (the title can be an account name).
const GAME_CLASS: &str = "WINDOWSCLIENT";

/// Retitle another process's window; gives up after 1 s if it hangs (WM_SETTEXT is a send).
/// False when the send timed out or failed (title not applied).
pub fn set_title(hw: HWND, title: &str) -> bool {
    let t = wide(title);
    // SAFETY: hw is a top-level window (a stale one just fails); NUL-terminated title that
    // outlives the call.
    unsafe {
        SendMessageTimeoutW(
            hw,
            WM_SETTEXT,
            0,
            t.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            1000,
            std::ptr::null_mut(),
        ) != 0
    }
}

/// Screen rect `(x, y, w, h)` overlaps at least one connected monitor.
fn on_a_monitor((x, y, w, h): (i32, i32, i32, i32)) -> bool {
    let r = RECT {
        left: x,
        top: y,
        right: x.saturating_add(w),
        bottom: y.saturating_add(h),
    };
    // SAFETY: plain query on a stack RECT.
    !unsafe { MonitorFromRect(&r, MONITOR_DEFAULTTONULL) }.is_null()
}

/// Visible bounds (no invisible resize border) and that border: left, top, right, bottom.
fn frame(hw: HWND) -> Option<([i32; 4], [i32; 4])> {
    let r = rect(hw)?;
    // SAFETY: zeroed RECT is a valid out param sized as passed.
    let f = unsafe {
        let mut f: RECT = std::mem::zeroed();
        (DwmGetWindowAttribute(
            hw,
            DWMWA_EXTENDED_FRAME_BOUNDS as _,
            (&raw mut f).cast(),
            std::mem::size_of::<RECT>() as u32,
        ) >= 0)
            .then_some([f.left, f.top, f.right, f.bottom])?
    };
    Some((f, [f[0] - r[0], f[1] - r[1], r[2] - f[2], r[3] - f[3]]))
}

/// Tiles the Roblox game windows on `work` (None: the primary monitor's work area) per
/// `layout`, in `order` ((tracker id, title), wanted order; clients we didn't launch go
/// last; accounts ranked `keep` or later are left alone) and titles each window. `main`:
/// index into `order` of the focus account. Returns windows handled.
pub fn arrange_grid(
    order: &[(String, String)],
    keep: usize,
    work: Option<crate::accounts::Rect>,
    layout: &crate::grid::Layout,
    main: Option<usize>,
) -> Result<usize, String> {
    let work = match work {
        Some(w) => w,
        None => unsafe {
            let mut r: RECT = std::mem::zeroed();
            // SAFETY: SPI_GETWORKAREA writes one RECT.
            if SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut r).cast(), 0) == 0 {
                return Err(format!(
                    "Could not get the primary monitor work area: {}",
                    io::Error::last_os_error()
                ));
            }
            crate::accounts::Rect {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
            }
        },
    };
    let wins = ordered_windows(order, keep);
    let main = main
        .and_then(|m| wins.iter().position(|w| w.0 == m))
        .unwrap_or(0);
    let plan = crate::grid::plan(wins.len(), work, layout, main);
    let mut failed = 0;
    for (&(rank, _, hw), t) in wins.iter().zip(&plan) {
        if let Some((_, name)) = order.get(rank) {
            set_title(hw, name);
        }
        if t.minimize {
            // SAFETY: as below.
            unsafe { ShowWindow(hw, SW_MINIMIZE) };
            continue;
        }
        let c = t.cell;
        // SAFETY: hw is a live top-level window from EnumWindows (a stale one just fails).
        let placed = unsafe {
            if IsIconic(hw) != 0 || IsZoomed(hw) != 0 {
                ShowWindow(hw, SW_SHOWNOACTIVATE); // SW_RESTORE would activate it
            }
            // Ask for the target's size; Roblox may grow it to its minimum, so place it by
            // the size it actually took.
            let (w, h) = t.size();
            let ask = |b: [i32; 4]| {
                SetWindowPos(
                    hw,
                    std::ptr::null_mut(),
                    c.x - b[0],
                    c.y - b[1],
                    w + b[0] + b[2],
                    h + b[1] + b[3],
                    SWP_NOZORDER | SWP_NOACTIVATE,
                ) != 0
            };
            ask(frame(hw).map_or([0; 4], |f| f.1))
                // A move onto a monitor with another DPI rescales the window (and its
                // border): size a big one again, now that it is on the target monitor.
                && (t.small
                    || frame(hw).is_none_or(|(f, b)| (f[2] - f[0], f[3] - f[1]) == (w, h) || ask(b)))
                && frame(hw).is_some_and(|(f, b)| {
                    let (x, y) = t.pos(work, (f[2] - f[0], f[3] - f[1]));
                    SetWindowPos(
                        hw,
                        std::ptr::null_mut(),
                        x - b[0],
                        y - b[1],
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    ) != 0
                })
        };
        failed += usize::from(!placed);
    }
    // Stack front to back by the plan's z (grids: middle column over its neighbors, lower
    // rows over upper ones). Each goes right under the previous (relative moves aren't
    // blocked by focus rules).
    let mut z: Vec<usize> = (0..wins.len()).filter(|&i| !plan[i].minimize).collect();
    z.sort_by_key(|&i| std::cmp::Reverse(plan[i].z));
    let mut above = HWND_TOP;
    for i in z {
        let hw = wins[i].2;
        // SAFETY: live top-level windows; only the z-order changes.
        unsafe {
            SetWindowPos(
                hw,
                above,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        above = hw;
    }
    if failed > 0 {
        return Err(format!(
            "Could not arrange {failed} of {} Roblox windows.",
            wins.len()
        ));
    }
    Ok(wins.len())
}

/// Roblox game windows in slot order (see `grid::slots`), ranked by `order`. Skipped: hung
/// windows (moving them would block) and accounts ranked `keep` or later (not picked).
pub fn ordered_windows(order: &[(String, String)], keep: usize) -> Vec<(usize, u32, HWND)> {
    crate::grid::slots(
        game_windows(&process_ids(PLAYER_EXE))
            .into_iter()
            // SAFETY: hw from EnumWindows; a stale one reads as not hung.
            .filter(|&(_, hw)| unsafe { IsHungAppWindow(hw) } == 0)
            .map(|(pid, hw)| {
                let rank = cmdline(pid)
                    .and_then(|c| order.iter().position(|(t, _)| has_tracker(&c, t)))
                    .unwrap_or(usize::MAX);
                (rank, pid, hw)
            })
            .filter(|w| w.0 < keep || w.0 == usize::MAX)
            .collect(),
    )
}

/// Shows a big slot number (1..N, same order as `arrange_grid`) centered on each visible
/// Roblox window for ~3 s: layered, topmost, click-through, never activated, so focus and
/// the cursor stay put. Returns windows marked.
pub fn identify(order: &[(String, String)], keep: usize) -> usize {
    let badges: Vec<(usize, [i32; 4])> = ordered_windows(order, keep)
        .iter()
        .enumerate()
        // SAFETY: hw from EnumWindows; a stale one just reads as not iconic / no frame.
        .filter(|(_, w)| unsafe { IsIconic(w.2) } == 0)
        .filter_map(|(i, w)| Some((i + 1, frame(w.2)?.0)))
        .collect();
    let n = badges.len();
    if n > 0 {
        std::thread::spawn(move || show_badges(&badges));
    }
    n
}

fn show_badges(badges: &[(usize, [i32; 4])]) {
    let class = wide("RmiIdentifyBadge");
    // SAFETY: plain Win32 window lifecycle on this thread; every pointer passed outlives
    // its call; the windows are destroyed before the thread ends.
    unsafe {
        let brush = CreateSolidBrush(0x0030_2018);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(badge_proc),
            lpszClassName: class.as_ptr(),
            hbrBackground: brush,
            ..std::mem::zeroed()
        };
        // Already registered by an earlier call: keep that one.
        if RegisterClassW(&wc) == 0 {
            DeleteObject(brush);
        }
        let wins: Vec<HWND> = badges
            .iter()
            .map(|&(num, f)| {
                let side = ((f[2] - f[0]).min(f[3] - f[1]) / 3).max(80);
                let hw = CreateWindowExW(
                    WS_EX_LAYERED
                        | WS_EX_TRANSPARENT
                        | WS_EX_TOPMOST
                        | WS_EX_NOACTIVATE
                        | WS_EX_TOOLWINDOW,
                    class.as_ptr(),
                    wide(&num.to_string()).as_ptr(),
                    WS_POPUP,
                    (f[0] + f[2] - side) / 2,
                    (f[1] + f[3] - side) / 2,
                    side,
                    side,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                SetLayeredWindowAttributes(hw, 0, 225, LWA_ALPHA);
                ShowWindow(hw, SW_SHOWNOACTIVATE);
                hw
            })
            .collect();
        // Thread timer (no window): its WM_TIMER ends the loop.
        SetTimer(std::ptr::null_mut(), 0, 3000, None);
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0
            && !(m.message == WM_TIMER && m.hwnd.is_null())
        {
            DispatchMessageW(&m);
        }
        for hw in wins {
            DestroyWindow(hw);
        }
    }
}

unsafe extern "system" fn badge_proc(hw: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != WM_PAINT {
        // SAFETY: default handling for our own window.
        return unsafe { DefWindowProcW(hw, msg, wp, lp) };
    }
    // SAFETY: standard BeginPaint/EndPaint pair; buffers sized as passed; the font is
    // deselected before it's deleted.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hw, &mut ps);
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(hw, &mut r);
        let mut t = [0u16; 8];
        let n = GetWindowTextW(hw, t.as_mut_ptr(), t.len() as i32);
        let face = wide("Segoe UI");
        let font = CreateFontW(
            r.bottom * 3 / 4,
            0,
            0,
            0,
            FW_BOLD as _,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            face.as_ptr(),
        );
        let old = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, 0x00FF_FFFF);
        DrawTextW(
            dc,
            t.as_mut_ptr(),
            n,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(dc, old);
        DeleteObject(font);
        EndPaint(hw, &ps);
    }
    0
}

fn class(hw: HWND) -> String {
    let mut b = [0u16; 64];
    // SAFETY: buffer length passed matches the buffer.
    let n = unsafe { GetClassNameW(hw, b.as_mut_ptr(), b.len() as i32) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

/// Game windows (not the launcher splash) of `pids`: (pid, hwnd).
pub fn game_windows(pids: &[u32]) -> Vec<(u32, HWND)> {
    let mut w = windows_of(pids);
    w.retain(|&(_, hw)| class(hw) == GAME_CLASS);
    w
}

pub struct Windows;

impl Platform for Windows {
    fn capabilities(&self) -> Capabilities {
        let launch = self.roblox_install().is_some();
        Capabilities {
            os: "windows",
            watcher: true,
            launch,
            launch_reason: (!launch).then(|| "Install Roblox to launch.".into()),
            multi_instance: true,
            cookie_lock: true,
            window_rect: true,
            client_helpers: true,
            fullscreen_wait: true,
            audio: true,
        }
    }

    fn roblox_install(&self) -> Option<RobloxInstall> {
        // Versions fallback only when the handler is missing/unparsable/points nowhere, so a
        // bootstrapper never comes with a stale vanilla version_dir.
        let (version_dir, bootstrapper) = match reg_handler_command()
            .as_deref()
            .and_then(parse_handler_command)
            .filter(|exe| exe.is_file())
        {
            Some(exe) => classify(&exe),
            None => (
                dirs::data_local_dir()
                    .and_then(|d| newest_version_dir(&d.join("Roblox").join("Versions"))),
                None,
            ),
        };
        (version_dir.is_some() || bootstrapper.is_some()).then_some(RobloxInstall {
            version_dir,
            bootstrapper,
        })
    }

    /// Blocking (ShellExecute can stall on a slow handler) and initialises COM (STA) on the
    /// calling thread: call from `spawn_blocking`, never the UI thread.
    fn open_uri(&self, uri: &str) -> io::Result<()> {
        // Also before the launch's mutex take (launcher); this catches a tray started since.
        close_idle_trays();
        // The URI carries the auth ticket: wipe our UTF-16 copy afterwards.
        let (op, file) = (wide("open"), zeroize::Zeroizing::new(wide(uri)));
        // SAFETY: COM init paired with CoUninitialize on success; valid NUL-terminated wide
        // strings that outlive the call.
        let r = unsafe {
            let com = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as _);
            let r = ShellExecuteW(
                std::ptr::null_mut(),
                op.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWMINNOACTIVE,
            );
            // S_OK / S_FALSE both need a matching uninit; RPC_E_CHANGED_MODE (< 0) does not.
            if com >= 0 {
                CoUninitialize();
            }
            r
        };
        // > 32 = success; 2 (ERROR_FILE_NOT_FOUND) / 31 (SE_ERR_NOASSOC) = no handler.
        match r as isize {
            n if n > 32 => Ok(()),
            2 | 31 => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Roblox is not installed (no roblox-player handler)",
            )),
            n => Err(io::Error::other(format!("ShellExecuteW failed ({n})"))),
        }
    }

    fn roblox_running(&self) -> bool {
        !process_ids(PLAYER_EXE).is_empty()
    }

    fn close_tracked(&self, tracker_id: &str) -> io::Result<usize> {
        let mut n = 0;
        for pid in process_ids(PLAYER_EXE) {
            match close_verified(pid, None, Some(tracker_id)) {
                Ok(true) => n += 1,
                Ok(false) => log::warn!("Roblox pid {pid} survived the close"),
                // "Not ours" (another tracker id, pid reused, not the player).
                Err(e) => log::debug!("close pid {pid}: {e}"),
            }
        }
        Ok(n)
    }

    fn close_pid(&self, pid: u32, tracker_id: &str) -> io::Result<bool> {
        close_verified(pid, None, Some(tracker_id)).map_err(io::Error::other)
    }

    fn tracked_pids(&self, tracker_id: &str) -> Vec<u32> {
        tracked_pids(tracker_id)
    }

    fn tracked_clients(&self, trackers: &[String]) -> Vec<(u32, String)> {
        process_ids(PLAYER_EXE)
            .into_iter()
            .filter_map(|p| {
                let c = cmdline(p)?;
                let t = trackers.iter().find(|t| has_tracker(&c, t))?;
                Some((p, t.clone()))
            })
            .collect()
    }

    fn minimize(&self, clients: &[(u32, String)]) -> usize {
        let mut n = 0;
        for (pid, t) in clients {
            // SAFETY: the handle is checked and closed once; holding it open while we
            // minimize keeps the pid from being reused by another process.
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, *pid);
                if h.is_null() {
                    continue;
                }
                if cmdline_of(h).is_some_and(|c| has_tracker(&c, t)) && image_is_player(h) {
                    for (_, w) in windows_of(&[*pid]) {
                        // w is a live top-level window from EnumWindows; ShowWindowAsync
                        // tolerates a window that closed meanwhile and never blocks on a
                        // hung one.
                        ShowWindowAsync(w, SW_MINIMIZE);
                        n += 1;
                    }
                }
                CloseHandle(h);
            }
        }
        n
    }

    fn idle_ms(&self) -> Option<u64> {
        nudge::idle_ms()
    }

    fn nudge(&self) -> usize {
        // Hung windows skipped (as Grid Arrange does): a sync ShowWindow on one would block
        // the anti-AFK thread for every client.
        let wins: Vec<HWND> = game_windows(&process_ids(PLAYER_EXE))
            .into_iter()
            .map(|(_, w)| w)
            // SAFETY: plain query on a window handle from EnumWindows.
            .filter(|&w| unsafe { IsHungAppWindow(w) } == 0)
            .collect();
        // SAFETY: handles from EnumWindows / GetForegroundWindow; every call tolerates a
        // window that closed meanwhile.
        unsafe { nudge::run(&wins) }
    }

    fn setup_window(
        &self,
        tracker_id: &str,
        name: &str,
        rect: Option<(i32, i32, i32, i32)>,
        skip: &[u32],
        minimize: bool,
    ) -> io::Result<()> {
        // A newer poll for the same tracker (quick relaunch) supersedes this one.
        static POLLS: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Default::default);
        let poll = |bump: bool| {
            let mut g = POLLS.lock().unwrap_or_else(PoisonError::into_inner);
            let n = g.entry(tracker_id.to_owned()).or_default();
            *n += u64::from(bump);
            *n
        };
        let me = poll(true);
        let t0 = std::time::Instant::now();
        while t0.elapsed() < setup_deadline(minimize) {
            if poll(false) != me {
                return Err(io::Error::other("superseded by a newer launch"));
            }
            // Windows first, then command lines of their pids only: no process snapshot.
            let found = all_game_windows().into_iter().find(|&(p, _)| {
                !skip.contains(&p) && cmdline(p).is_some_and(|c| has_tracker(&c, tracker_id))
            });
            if let Some((_, hw)) = found {
                // A busy window (still loading) can time the send out: try again.
                if !set_title(hw, name) {
                    std::thread::sleep(poll_interval(t0.elapsed()));
                    continue;
                }
                // A saved rect on a monitor that is gone would open the window off-screen.
                let rect = rect.filter(|&r| on_a_monitor(r));
                // SAFETY: hw is a live top-level window from EnumWindows.
                let ok = unsafe {
                    rect.is_none_or(|(x, y, w, h)| {
                        // SetWindowPos on a minimized window moves its icon, not the rect it
                        // restores to: restore first (the minimize below re-minimizes it).
                        if IsIconic(hw) != 0 {
                            ShowWindow(hw, SW_SHOWNOACTIVATE);
                        }
                        SetWindowPos(
                            hw,
                            std::ptr::null_mut(),
                            x,
                            y,
                            w,
                            h,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        ) != 0
                    })
                };
                if minimize {
                    // Roblox re-shows its window while loading: keep it minimized for a while,
                    // unless a newer launch took over or the window closed.
                    let shown = std::time::Instant::now();
                    while shown.elapsed() < REMINIMIZE_FOR && poll(false) == me {
                        // SAFETY: plain queries on hw; ShowWindowAsync never blocks on a hung
                        // window and tolerates one that closed meanwhile.
                        unsafe {
                            if IsWindow(hw) == 0 {
                                break;
                            }
                            if IsIconic(hw) == 0 {
                                // Already foreground: SW_MINIMIZE hands activation to the
                                // next window; SW_SHOWMINNOACTIVE would keep it.
                                let fg = GetForegroundWindow() == hw;
                                ShowWindowAsync(
                                    hw,
                                    if fg { SW_MINIMIZE } else { SW_SHOWMINNOACTIVE },
                                );
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                } else {
                    // open_uri hands Roblox SW_SHOWMINNOACTIVE: undo it if Roblox honoured it.
                    // SAFETY: plain query on hw; ShowWindowAsync never blocks on a hung window.
                    unsafe {
                        if IsIconic(hw) != 0 {
                            ShowWindowAsync(hw, SW_SHOWNOACTIVATE);
                        }
                    }
                }
                return if !ok {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                };
            }
            std::thread::sleep(poll_interval(t0.elapsed()));
        }
        Err(io::ErrorKind::TimedOut.into())
    }

    fn close_idle_trays(&self) {
        close_idle_trays();
    }

    fn hold_multi(&self) -> io::Result<Guard> {
        let r = match ensure_holder() {
            Ok(()) => return Ok(Box::new(HolderGuard)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
            Err(e) => {
                // Fallback: released when RMI exits, which closes waiting Roblox clients.
                log::warn!("mutex holder process failed ({e}); holding the mutex in-process");
                MultiInstance::acquire(ROBLOX_MUTEX).map(|m| Box::new(m) as Guard)
            }
        };
        r.map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => {
                io::Error::new(io::ErrorKind::AlreadyExists, "Roblox is already running")
            }
            _ => e,
        })
    }

    fn hold_cookie_lock(&self) -> io::Result<Guard> {
        let p = dirs::data_local_dir()
            .ok_or_else(|| io::Error::other("no LOCALAPPDATA"))?
            .join(r"Roblox\LocalStorage\RobloxCookies.dat");
        let f = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(p)?;
        Ok(Box::new(f))
    }
}

/// How long setup_window waits for the window. Minimized launches may sit behind a slow
/// bootstrapper update, and nothing else would minimize the window later.
fn setup_deadline(minimize: bool) -> std::time::Duration {
    std::time::Duration::from_secs(if minimize { 180 } else { 45 })
}

/// After the window first appears, re-minimize any re-show for this long.
const REMINIMIZE_FOR: std::time::Duration = std::time::Duration::from_secs(15);

/// Window-poll cadence: fast at first (catch the window before it paints), then relaxed.
fn poll_interval(elapsed: std::time::Duration) -> std::time::Duration {
    std::time::Duration::from_millis(if elapsed < std::time::Duration::from_secs(10) {
        100
    } else {
        500
    })
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain([0]).collect()
}

/// Hidden arg: `--hold-roblox-mutex=<RMI exe file name>` runs `run_holder`, not the app.
pub const HOLDER_ARG: &str = "--hold-roblox-mutex";
/// Named event the holder process waits on; while it exists, a holder is up.
const HOLDER_STOP: &str = "RMI_roblox_mutex_holder_stop";

/// The RMI exe name from `--hold-roblox-mutex=<name>`, if this is a holder process.
pub fn holder_arg(mut args: impl Iterator<Item = String>) -> Option<String> {
    args.find_map(|a| {
        a.strip_prefix(HOLDER_ARG)?
            .strip_prefix('=')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}

/// The holder may let go once nothing can be hurt by the release: Roblox clients waiting on
/// the mutex close themselves when it is released, and a running RMI wants it held.
fn holder_done(roblox_clients: usize, rmi_processes: usize) -> bool {
    roblox_clients == 0 && rmi_processes == 0
}

/// Checks in a row that found nothing left (the holder lets go at two). A process list that
/// does not show the holder itself is not believed: a failed snapshot reads as "nothing
/// runs", and letting go then makes Roblox close every client but one.
fn holder_idle(idle: u32, seen_self: bool, roblox_clients: usize, rmi_processes: usize) -> u32 {
    if seen_self && holder_done(roblox_clients, rmi_processes) {
        idle + 1
    } else {
        0
    }
}

/// Body of the holder process: owns `ROBLOX_singletonMutex` outside RMI, so closing or
/// crashing RMI (or updating it) never releases it. Prints `ok` / `busy` for the spawner.
/// Exits once no Roblox client and no `main_exe` remain (polled every 15 s), or when RMI
/// sets the stop event (multi-instance turned off).
pub fn run_holder(main_exe: &str) -> i32 {
    use std::io::Write;
    let say = |s: &str| {
        let _ = writeln!(io::stdout(), "{s}");
    };
    let me = std::process::id();
    // SAFETY: valid NUL-terminated names; GetLastError read right after each create; the
    // handles live until process exit (which releases and closes them anyway).
    unsafe {
        let stop = CreateEventW(std::ptr::null(), 1, 0, wide(HOLDER_STOP).as_ptr());
        if stop.is_null() {
            return 1;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            say("ok"); // another holder has it
            return 0;
        }
        let m = CreateMutexW(std::ptr::null(), 1, wide(ROBLOX_MUTEX).as_ptr());
        if m.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            // Gone before the spawner reads: the stop event must not look like a holder.
            CloseHandle(stop);
            say("busy");
            return 2;
        }
        say("ok");
        // One process snapshot per check, all night: slow polls (the stop event is instant).
        let mut idle = 0;
        while WaitForSingleObject(stop, 15_000) == WAIT_TIMEOUT {
            let all = processes().unwrap_or_default();
            let count = |exe: &str| {
                let named = all.iter().filter(|(_, n)| n.eq_ignore_ascii_case(exe));
                named.filter(|(p, _)| *p != me).count()
            };
            let seen_self = all.iter().any(|(p, _)| *p == me);
            idle = holder_idle(idle, seen_self, count(PLAYER_EXE), count(main_exe));
            if idle >= 2 {
                break;
            }
        }
        ReleaseMutex(m);
    }
    0
}

/// Make sure a holder process owns the Roblox mutex, spawning one if none runs.
/// `AlreadyExists` = someone else (a Roblox client) has it.
fn ensure_holder() -> io::Result<()> {
    use std::io::BufRead;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    // SAFETY: valid NUL-terminated names; handles closed once.
    unsafe {
        let h = OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide(HOLDER_STOP).as_ptr());
        if !h.is_null() {
            CloseHandle(h);
            return Ok(());
        }
        // Cheap check before spawning (roblox_status polls this while Roblox has it).
        let m = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide(ROBLOX_MUTEX).as_ptr());
        if !m.is_null() {
            CloseHandle(m);
            return Err(io::ErrorKind::AlreadyExists.into());
        }
    }
    let cur = std::env::current_exe()?;
    let main = cur
        .file_name()
        .ok_or_else(|| io::Error::other("no exe name"))?
        .to_string_lossy()
        .into_owned();
    // Run a copy: the updater replaces (and may kill by name) RMI's exe while the holder runs.
    // Next to RMI, not in %TEMP%: an exe copied to TEMP and run hidden is what Defender's
    // heuristics flag, and the install folder is the one users exclude.
    let copy = cur.with_file_name("rmi-mutex-holder.exe");
    let exe = match std::fs::copy(&cur, &copy) {
        Ok(_) => copy,
        Err(e) => {
            log::warn!("holder copy failed ({e}); running RMI's own exe as the holder");
            cur
        }
    };
    let mut cmd = Command::new(&exe);
    cmd.arg(format!("{HOLDER_ARG}={main}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let base = DETACHED_PROCESS | CREATE_NO_WINDOW;
    // Breakaway so a kill-on-close job around RMI doesn't take the holder down; not every
    // job allows it.
    let mut child = cmd
        .creation_flags(base | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| cmd.creation_flags(base).spawn())?;
    let mut line = String::new();
    if let Some(out) = child.stdout.take() {
        io::BufReader::new(out).read_line(&mut line)?;
    }
    match line.trim() {
        "ok" => {
            log::info!("mutex holder process {} started", child.id());
            Ok(())
        }
        "busy" => Err(io::ErrorKind::AlreadyExists.into()),
        _ => Err(io::Error::other("mutex holder process exited")),
    }
}

/// Dropped only when `/multi/enabled` is turned off (never on exit): tell the holder to
/// release the mutex and wait briefly so a re-enable right after spawns a fresh one.
struct HolderGuard;

impl super::Held for std::fs::File {}
impl super::Held for MultiInstance {
    fn in_process(&self) -> bool {
        true
    }
}

impl super::Held for HolderGuard {
    /// A holder is up while its stop event exists (killed / crashed holder: gone).
    fn alive(&self) -> bool {
        // SAFETY: valid NUL-terminated name; handle closed once.
        unsafe {
            let h = OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide(HOLDER_STOP).as_ptr());
            if h.is_null() {
                return false;
            }
            CloseHandle(h);
            true
        }
    }
}

impl Drop for HolderGuard {
    fn drop(&mut self) {
        let name = wide(HOLDER_STOP);
        // SAFETY: valid NUL-terminated name; handles closed once.
        unsafe {
            let h = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if h.is_null() {
                return;
            }
            SetEvent(h);
            CloseHandle(h);
            for _ in 0..40 {
                let h = OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
                if h.is_null() {
                    return;
                }
                CloseHandle(h);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// A named mutex created *and owned* by one long-lived owner thread, held for as long as
/// this value lives; drop = ReleaseMutex + CloseHandle on that same thread. Mutex ownership
/// is per-thread, so it must not live on a tokio worker that may exit (abandoned mutex).
/// Roblox's single-instance check is assumed to be existence/ownership of
/// `ROBLOX_singletonMutex`; to be verified live with two clients (M4 live test).
pub struct MultiInstance(usize);

enum Req {
    Acquire(Vec<u16>, mpsc::Sender<io::Result<usize>>),
    Release(usize, mpsc::Sender<()>),
}

/// Sends to the mutex owner thread, (re)starting it if it never started or is gone (its
/// handles went with it, so a fresh thread loses nothing).
fn owner_send(mut req: Req) -> io::Result<()> {
    static TX: Mutex<Option<mpsc::Sender<Req>>> = Mutex::new(None);
    let mut g = TX.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(tx) = g.as_ref() {
        match tx.send(req) {
            Ok(()) => return Ok(()),
            Err(mpsc::SendError(r)) => req = r,
        }
    }
    let tx = spawn_owner()?;
    let r = tx
        .send(req)
        .map_err(|_| io::Error::other("mutex owner thread gone"));
    *g = Some(tx);
    r
}

fn spawn_owner() -> io::Result<mpsc::Sender<Req>> {
    {
        let (tx, rx) = mpsc::channel::<Req>();
        let spawned = std::thread::Builder::new()
            .name("rmi-mutex-owner".into())
            .spawn(move || {
                let mut held: HashMap<usize, HANDLE> = HashMap::new();
                // Global ids: a restarted owner thread never reuses an old guard's id.
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                for req in rx {
                    match req {
                        Req::Acquire(name, reply) => {
                            // SAFETY: valid NUL-terminated name; null attrs = default security;
                            // GetLastError read right after the call; handle closed once.
                            let r = unsafe {
                                let h = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
                                if h.is_null() {
                                    Err(io::Error::last_os_error())
                                } else if GetLastError() == ERROR_ALREADY_EXISTS {
                                    // Not ours (bInitialOwner ignored): don't keep it alive.
                                    CloseHandle(h);
                                    Err(io::ErrorKind::AlreadyExists.into())
                                } else {
                                    let next = NEXT.fetch_add(1, Ordering::Relaxed) + 1;
                                    held.insert(next, h);
                                    Ok(next)
                                }
                            };
                            let _ = reply.send(r);
                        }
                        Req::Release(id, reply) => {
                            if let Some(h) = held.remove(&id) {
                                // SAFETY: owned by this thread; released and closed once.
                                unsafe {
                                    ReleaseMutex(h);
                                    CloseHandle(h);
                                }
                            }
                            let _ = reply.send(());
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            log::error!("could not start the mutex owner thread: {e}");
            return Err(e);
        }
        Ok(tx)
    }
}

impl MultiInstance {
    /// `AlreadyExists` if someone else (e.g. a running Roblox) already has the mutex.
    pub fn acquire(name: &str) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        owner_send(Req::Acquire(wide(name), tx))?;
        rx.recv()
            .map_err(|_| io::Error::other("mutex owner thread gone"))?
            .map(Self)
    }
}

impl Drop for MultiInstance {
    fn drop(&mut self) {
        // Synchronous so a re-acquire right after sees the mutex gone.
        let (tx, rx) = mpsc::channel();
        if owner_send(Req::Release(self.0, tx)).is_ok() {
            let _ = rx.recv();
        }
    }
}

fn utf16z(b: &[u16]) -> String {
    let len = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..len])
}

/// Every running process as (pid, exe file name). `None`: the snapshot could not be taken
/// (it can fail when the system is short on memory), which is not the same as "none run".
fn processes() -> Option<Vec<(u32, String)>> {
    let mut out = vec![];
    // SAFETY: snapshot handle checked and closed; entry sized per API contract.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            out.push((e.th32ProcessID, utf16z(&e.szExeFile)));
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    (!out.is_empty()).then_some(out)
}

fn process_ids(exe: &str) -> Vec<u32> {
    let all = processes().unwrap_or_default().into_iter();
    all.filter(|(_, n)| n.eq_ignore_ascii_case(exe))
        .map(|(p, _)| p)
        .collect()
}

/// Another process's command line (`ProcessCommandLineInformation`, Win 8.1+).
/// Wiped on drop: a Roblox command line carries the auth ticket.
pub fn cmdline(pid: u32) -> Option<zeroize::Zeroizing<String>> {
    // SAFETY: handle checked and closed once.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let c = cmdline_of(h);
        CloseHandle(h);
        c
    }
}

/// SAFETY: `h` is a process handle with QUERY_LIMITED_INFORMATION.
unsafe fn cmdline_of(h: HANDLE) -> Option<zeroize::Zeroizing<String>> {
    // SAFETY: buffer is u64-aligned for UNICODE_STRING and sized from the first call; Buffer
    // points inside our allocation, Length is in bytes.
    unsafe {
        let mut len = 0u32;
        NtQueryInformationProcess(
            h,
            ProcessCommandLineInformation,
            std::ptr::null_mut(),
            0,
            &mut len,
        );
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(2)];
        let st = NtQueryInformationProcess(
            h,
            ProcessCommandLineInformation,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut len,
        );
        let buf = zeroize::Zeroizing::new(buf);
        if st < 0 {
            return None;
        }
        let us = &*(buf.as_ptr() as *const UNICODE_STRING);
        if us.Buffer.is_null() {
            return None;
        }
        let s = std::slice::from_raw_parts(us.Buffer, us.Length as usize / 2);
        Some(zeroize::Zeroizing::new(String::from_utf16_lossy(s)))
    }
}

/// `cmd`'s tracker field (`browsertrackerid:` / `-b`) is exactly `t`; a digit run elsewhere
/// (`roblox-player:1`, launchtime) never matches.
pub fn has_tracker(cmd: &str, t: &str) -> bool {
    crate::watcher::tracker_from_cmdline(cmd).as_deref() == Some(t)
}

/// Roblox's tray mode: an idle `--launch-to-tray` player with no game window takes over
/// the next roblox-player launch ("WebPlay NoReload with Tray only"), so the game runs in a
/// process without our tracker in its command line: never retitled, closed or tracked.
fn idle_tray(cmd: &str, game_window: bool) -> bool {
    !game_window
        && cmd.contains("--launch-to-tray")
        && crate::watcher::tracker_from_cmdline(cmd).is_none()
}

/// Close idle tray players so the launch starts a fresh, tracked client. One that already
/// hosts a game (game window up) is left alone.
fn close_idle_trays() {
    for pid in process_ids(PLAYER_EXE) {
        let (Some(c), Some(ms)) = (cmdline(pid), start_ms(pid)) else {
            continue;
        };
        if !idle_tray(&c, !game_windows(&[pid]).is_empty()) {
            continue;
        }
        match close_verified(pid, Some(ms.div_euclid(1000)), None) {
            Ok(true) => log::info!("closed idle Roblox tray process {pid} before launching"),
            Ok(false) => log::warn!("idle Roblox tray process {pid} survived the close"),
            Err(e) => log::debug!("close tray pid {pid}: {e}"),
        }
    }
    // After the closes, and with no tray too: an earlier one may have left it stale.
    let mut exes: Vec<String> = process_ids(PLAYER_EXE)
        .into_iter()
        .filter_map(image)
        .collect();
    exes.sort();
    exes.dedup();
    exes.iter().for_each(|e| clear_stale_guard(e));
}

/// Roblox's SingleInstanceGuard section of a player exe: its path with `\` as `_`, + `.shm`.
fn guard_section(exe: &str) -> String {
    format!("{}.shm", exe.replace('\\', "_"))
}

/// A tray that was closed leaves its window handle in Roblox's SingleInstanceGuard section
/// (the other clients keep the section alive). The next roblox-player launch sends to that
/// dead window and crashes at start ("Invalid window handle"), every time, until all of
/// Roblox is closed. Zero a dead handle: the launch then starts as if no tray had run. A
/// live window is left alone. No section (no client of this exe running): nothing to do.
fn clear_stale_guard(exe: &str) {
    use std::sync::atomic::Ordering::SeqCst;
    // the guard holds an 8-byte handle; `hw >> 32` below needs a 64-bit usize
    const _: () = assert!(std::mem::size_of::<usize>() == 8);
    let name = wide(&guard_section(exe));
    // SAFETY: handle and view checked; the view covers the 8 bytes read and written (the
    // section is at least a page); unmapped and closed once.
    unsafe {
        let m = OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, 0, name.as_ptr());
        if m.is_null() {
            return;
        }
        let m = Handle(m);
        let v = MapViewOfFile(m.0, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 8);
        if v.Value.is_null() {
            return;
        }
        // the view is page-aligned, so 8-byte aligned as AtomicUsize needs
        let a = std::sync::atomic::AtomicUsize::from_ptr(v.Value.cast::<usize>());
        let hw = a.load(SeqCst);
        // HWNDs are 32-bit values: high bits set means this is not a window handle, leave it
        if hw != 0 && hw >> 32 == 0 && IsWindow(hw as HWND) == 0 {
            // only if Roblox did not write a new handle meanwhile
            if a.compare_exchange(hw, 0, SeqCst, SeqCst).is_ok() {
                log::info!("cleared a closed Roblox tray's window from the single-instance guard");
            }
        }
        UnmapViewOfFile(v);
    }
}

/// Full image path of `pid`.
fn image(pid: u32) -> Option<String> {
    // SAFETY: handle checked, closed by `Handle`; buffer and its length match.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let h = Handle(h);
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        (QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n) != 0)
            .then(|| String::from_utf16_lossy(&buf[..n as usize]))
    }
}

fn tracked_pids(t: &str) -> Vec<u32> {
    process_ids(PLAYER_EXE)
        .into_iter()
        .filter(|&p| cmdline(p).is_some_and(|c| has_tracker(&c, t)))
        .collect()
}

/// SAFETY: `h` is a process handle with QUERY_LIMITED_INFORMATION.
unsafe fn image_is_player(h: HANDLE) -> bool {
    let mut buf = [0u16; 1024];
    let mut n = buf.len() as u32;
    // SAFETY: buffer and its length match.
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n) };
    ok != 0
        && Path::new(&String::from_utf16_lossy(&buf[..n as usize]))
            .file_name()
            .is_some_and(|f| f.eq_ignore_ascii_case(PLAYER_EXE))
}

/// Visible, unowned top-level windows of `pids`: (pid, hwnd).
pub fn windows_of(pids: &[u32]) -> Vec<(u32, HWND)> {
    visible_windows(&|p| pids.contains(&p))
}

/// Game windows of any process: no process snapshot (setup_window polls this fast).
pub fn all_game_windows() -> Vec<(u32, HWND)> {
    let mut w = visible_windows(&|_| true);
    w.retain(|&(_, hw)| class(hw) == GAME_CLASS);
    w
}

/// Visible, unowned top-level windows whose pid passes `keep`: (pid, hwnd).
fn visible_windows(keep: &dyn Fn(u32) -> bool) -> Vec<(u32, HWND)> {
    type St<'a> = (&'a dyn Fn(u32) -> bool, Vec<(u32, HWND)>);
    unsafe extern "system" fn cb(hw: HWND, lp: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: lp is the &mut St passed below, alive for the EnumWindows call; hw comes
        // from EnumWindows.
        unsafe {
            let (keep, out) = &mut *(lp as *mut St);
            let mut p = 0u32;
            GetWindowThreadProcessId(hw, &mut p);
            if keep(p) && IsWindowVisible(hw) != 0 && GetWindow(hw, GW_OWNER).is_null() {
                out.push((p, hw));
            }
        }
        1
    }
    let mut st: St = (keep, vec![]);
    // SAFETY: callback only touches `st` during the synchronous enumeration.
    unsafe { EnumWindows(Some(cb), &mut st as *mut _ as LPARAM) };
    st.1
}

pub fn title(hw: HWND) -> String {
    let mut b = [0u16; 256];
    // SAFETY: buffer length passed matches the buffer.
    let n = unsafe { GetWindowTextW(hw, b.as_mut_ptr(), b.len() as i32) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

/// left, top, right, bottom; `None` while minimized (that rect is the off-screen
/// -32000 icon) or if the window is gone.
pub fn rect(hw: HWND) -> Option<[i32; 4]> {
    // SAFETY: zeroed RECT is a valid out param; a stale hwnd just fails.
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        (IsIconic(hw) == 0 && GetWindowRect(hw, &mut r) != 0)
            .then_some([r.left, r.top, r.right, r.bottom])
    }
}

/// Closes the process handle once.
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: handle came from OpenProcess and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Process creation time, unix ms. SAFETY: `h` has QUERY_LIMITED_INFORMATION.
unsafe fn start_unix_ms(h: HANDLE) -> Option<i64> {
    // SAFETY: zeroed FILETIMEs are valid out params for a valid handle.
    unsafe {
        let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) =
            std::mem::zeroed();
        if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 {
            return None;
        }
        let ft = (u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime);
        Some((ft / 10_000) as i64 - 11_644_473_600_000)
    }
}

/// SAFETY: `h` is a process handle with QUERY_LIMITED_INFORMATION.
unsafe fn start_unix(h: HANDLE) -> Option<i64> {
    // SAFETY: same contract.
    unsafe { start_unix_ms(h) }.map(|ms| ms.div_euclid(1000))
}

/// Precise creation time (unix ms) of `pid`.
pub fn start_ms(pid: u32) -> Option<i64> {
    // SAFETY: handle checked, then owned and closed once by `Handle`.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let h = Handle(h);
        start_unix_ms(h.0)
    }
}

/// Close one RobloxPlayerBeta.exe, verified on the same handle so a reused pid is never
/// touched: image name, plus start time (unix secs, +-2 s) and/or tracker id in the
/// command line when given. WM_CLOSE its windows (owner re-checked), wait 1 s, then
/// TerminateProcess. `Ok(true)` = the process is gone; `Ok(false)` = verified but it
/// survived (logged); `Err` = not verified (not ours).
pub fn close_verified(
    pid: u32,
    started: Option<i64>,
    tracker: Option<&str>,
) -> Result<bool, String> {
    // SAFETY: handle checked, then owned by `Handle`; every call below gets that handle
    // with the rights it was opened with; stale hwnds just fail.
    unsafe {
        let h = OpenProcess(
            PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if h.is_null() {
            return Err(format!(
                "open process {pid}: {}",
                io::Error::last_os_error()
            ));
        }
        let h = Handle(h);
        if !image_is_player(h.0) {
            return Err(format!("pid {pid} is not {PLAYER_EXE}"));
        }
        if let Some(s) = started {
            if start_unix(h.0).is_none_or(|t| (t - s).abs() > 2) {
                return Err(format!("pid {pid} start time changed (pid reused)"));
            }
        }
        if let Some(t) = tracker {
            if !cmdline_of(h.0).is_some_and(|c| has_tracker(&c, t)) {
                return Err(format!("pid {pid} is not tracker {t}"));
            }
        }
        for (_, hw) in windows_of(&[pid]) {
            let mut owner = 0u32;
            GetWindowThreadProcessId(hw, &mut owner);
            if owner == pid {
                PostMessageW(hw, WM_CLOSE, 0, 0);
            }
        }
        if WaitForSingleObject(h.0, 1000) == WAIT_OBJECT_0 {
            return Ok(true);
        }
        if TerminateProcess(h.0, 1) == 0 {
            // It may have exited between the wait and the terminate.
            let e = io::Error::last_os_error();
            let gone = WaitForSingleObject(h.0, 0) == WAIT_OBJECT_0;
            if !gone {
                log::warn!("terminate pid {pid}: {e}");
            }
            return Ok(gone);
        }
        Ok(WaitForSingleObject(h.0, 2000) == WAIT_OBJECT_0)
    }
}

fn reg_handler_command() -> Option<String> {
    let key = wide(r"roblox-player\shell\open\command");
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: buffer and size describe the same allocation; null value name = default value.
    let r = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            key.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if r != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Settings > Personalization > Colors > Transparency effects (missing value = on).
pub fn transparency_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("EnableTransparency");
    let mut v = 1u32;
    let mut size = 4u32;
    // SAFETY: `v`/`size` describe one DWORD; key and name are NUL-terminated.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut v as *mut u32).cast(),
            &mut size,
        )
    };
    r != 0 || v != 0
}

/// Exe path from a shell `open\command` value: `"C:\x\y.exe" %1` or `C:\x\y.exe %1`.
pub fn parse_handler_command(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    let exe = if let Some(rest) = cmd.strip_prefix('"') {
        &rest[..rest.find('"')?]
    } else {
        let end = cmd
            .to_ascii_lowercase()
            .find(".exe")
            .map(|i| i + 4)
            .unwrap_or_else(|| cmd.find(' ').unwrap_or(cmd.len()));
        &cmd[..end]
    };
    (!exe.is_empty()).then(|| PathBuf::from(exe))
}

/// Vanilla player under `...\Roblox\Versions\version-*\` gives its folder; anything else is
/// a bootstrapper: the folder above `Versions` for a foreign player exe (e.g. "Fishstrap"),
/// else the exe stem (e.g. "Bloxstrap").
pub fn classify(exe: &Path) -> (Option<PathBuf>, Option<String>) {
    fn name(p: Option<&Path>) -> Option<std::borrow::Cow<'_, str>> {
        p.and_then(Path::file_name).map(|f| f.to_string_lossy())
    }
    let name_is = |p: Option<&Path>, n: &str| name(p).is_some_and(|f| f.eq_ignore_ascii_case(n));
    let dir = exe.parent();
    let versions = dir.and_then(Path::parent);
    let owner = versions.and_then(Path::parent);
    if name_is(Some(exe), PLAYER_EXE) && name_is(versions, "Versions") {
        if name_is(owner, "Roblox") {
            return (dir.map(Path::to_path_buf), None);
        }
        if let Some(o) = name(owner) {
            return (None, Some(o.into_owned()));
        }
    }
    let stem = exe.file_stem().map(|s| s.to_string_lossy().into_owned());
    (None, stem)
}

/// Newest (by modified time) `version-*` folder containing the player exe.
fn newest_version_dir(versions: &Path) -> Option<PathBuf> {
    std::fs::read_dir(versions)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("version-"))
        .filter(|e| e.path().join(PLAYER_EXE).is_file())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_interval_fast_then_slow() {
        use std::time::Duration as D;
        assert_eq!(poll_interval(D::from_secs(0)), D::from_millis(100));
        assert_eq!(poll_interval(D::from_millis(9_999)), D::from_millis(100));
        assert_eq!(poll_interval(D::from_secs(10)), D::from_millis(500));
    }

    #[test]
    fn input_hooks_go_once_anti_afk_stops_asking() {
        assert!(!nudge::hooks_unwanted(100_000, 98_000), "asked 2 s ago");
        assert!(!nudge::hooks_unwanted(100_000, 100_500), "asked just now");
        assert!(nudge::hooks_unwanted(100_000, 60_000), "silent 40 s: off");
    }

    #[test]
    fn game_windows_scan_is_read_only_and_runs() {
        // This test process has no game window; the scan must still enumerate cleanly.
        let me = std::process::id();
        assert!(all_game_windows().iter().all(|&(p, _)| p != me));
    }

    #[test]
    fn minimized_setup_waits_out_slow_bootstrappers() {
        use std::time::Duration as D;
        assert_eq!(setup_deadline(false), D::from_secs(45));
        assert_eq!(setup_deadline(true), D::from_secs(180));
    }

    #[test]
    fn parses_handler_commands() {
        let p = |s| parse_handler_command(s).map(|p| p.display().to_string());
        assert_eq!(
            p(r#""C:\R\Versions\version-1\RobloxPlayerBeta.exe" %1"#).as_deref(),
            Some(r"C:\R\Versions\version-1\RobloxPlayerBeta.exe")
        );
        assert_eq!(
            p(r"C:\Program Files\Bloxstrap\Bloxstrap.EXE -player %1").as_deref(),
            Some(r"C:\Program Files\Bloxstrap\Bloxstrap.EXE")
        );
        assert_eq!(p(""), None);
        assert_eq!(p(r#""unterminated"#), None);
    }

    #[test]
    fn classifies_vanilla_and_bootstrapper() {
        let v = Path::new(r"C:\U\AppData\Local\Roblox\Versions\version-abc\RobloxPlayerBeta.exe");
        assert_eq!(
            classify(v),
            (
                Some(PathBuf::from(
                    r"C:\U\AppData\Local\Roblox\Versions\version-abc"
                )),
                None
            )
        );
        let b = Path::new(r"C:\U\AppData\Local\Bloxstrap\Bloxstrap.exe");
        assert_eq!(classify(b), (None, Some("Bloxstrap".into())));
        // player exe outside Roblox\Versions = managed by someone else
        let o = Path::new(r"C:\Fishstrap\Versions\version-x\RobloxPlayerBeta.exe");
        assert_eq!(classify(o), (None, Some("Fishstrap".into())));
    }

    #[test]
    fn newest_version_dir_needs_player_exe() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("version-empty")).unwrap();
        let good = d.path().join("version-good");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join(PLAYER_EXE), "").unwrap();
        assert_eq!(newest_version_dir(d.path()), Some(good));
    }

    #[test]
    fn tracker_matches_whole_number_only() {
        let c = r#""C:\R\RobloxPlayerBeta.exe" roblox-player:1+launchtime:1700123456789012+browsertrackerid:123456789012+x"#;
        assert!(has_tracker(c, "123456789012"));
        assert!(!has_tracker(c, "23456789012"));
        assert!(!has_tracker(c, "456789"));
        assert!(
            !has_tracker(c, "1"),
            "roblox-player:1 is not the tracker field"
        );
        assert!(has_tracker("--app -b 555 -t x", "555"));
        assert!(!has_tracker(c, ""));
    }

    #[test]
    fn idle_tray_only_without_game_or_tracker() {
        let tray = r#""C:\R\RobloxPlayerBeta.exe" --launch-to-tray"#;
        assert!(idle_tray(tray, false));
        assert!(!idle_tray(tray, true), "tray hosting a game is left alone");
        let c = r#""C:\R\RobloxPlayerBeta.exe" roblox-player:1+launchtime:1+browsertrackerid:123456789012"#;
        assert!(!idle_tray(c, false), "tracked client is not a tray");
        assert!(!idle_tray(&format!("{tray} {c}"), false));
    }

    #[test]
    fn close_verified_refuses_non_player() {
        // This test binary is not RobloxPlayerBeta.exe: refused before any close.
        let me = std::process::id();
        assert!(close_verified(me, None, None)
            .unwrap_err()
            .contains("is not"));
        assert!(windows_of(&[me]).is_empty());
    }

    #[test]
    fn stale_guard_window_is_cleared_live_one_kept() {
        use windows_sys::Win32::System::Memory::{CreateFileMappingW, PAGE_READWRITE};
        use windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
        let exe = format!(r"C:\RMI test\{}\RobloxPlayerBeta.exe", std::process::id());
        assert!(guard_section(&exe).starts_with("C:_RMI test_"));
        assert!(guard_section(&exe).ends_with("_RobloxPlayerBeta.exe.shm"));
        clear_stale_guard(&exe); // no section: no-op
        let name = wide(&guard_section(&exe));
        // SAFETY: page-file section of one page, mapped whole; closed at the end.
        unsafe {
            let m = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                4096,
                name.as_ptr(),
            );
            assert!(!m.is_null());
            let m = Handle(m);
            let v = MapViewOfFile(m.0, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 16);
            let p = v.Value.cast::<usize>();
            // the pid field after the handle stays
            p.add(1).write_volatile(26768);
            let live = GetDesktopWindow() as usize;
            p.write_volatile(live);
            clear_stale_guard(&exe);
            assert_eq!(p.read_volatile(), live, "a live tray window is kept");
            p.write_volatile(0x7FFF_FFF0); // no such window
            clear_stale_guard(&exe);
            assert_eq!(p.read_volatile(), 0, "a dead window is cleared");
            let odd = 0x1_0000_1234usize; // high bits set: not a window handle
            p.write_volatile(odd);
            clear_stale_guard(&exe);
            assert_eq!(p.read_volatile(), odd, "a non-handle value is left alone");
            assert_eq!(p.add(1).read_volatile(), 26768);
            UnmapViewOfFile(v);
        }
    }

    #[test]
    fn own_cmdline_readable() {
        assert!(cmdline(std::process::id()).is_some_and(|c| !c.is_empty()));
    }

    #[test]
    fn holder_arg_parses() {
        let a = |v: &[&str]| holder_arg(v.iter().map(|s| s.to_string()));
        assert_eq!(
            a(&["rmi.exe", "--hold-roblox-mutex=RMI.exe"]).as_deref(),
            Some("RMI.exe")
        );
        assert_eq!(a(&["rmi.exe"]), None);
        assert_eq!(a(&["rmi.exe", "--hold-roblox-mutex"]), None);
        assert_eq!(a(&["rmi.exe", "--hold-roblox-mutex="]), None);
        assert_eq!(a(&["rmi.exe", "--hold-roblox-mutexX=a"]), None);
    }

    #[test]
    fn saved_rect_off_every_monitor_is_rejected() {
        assert!(!on_a_monitor((-1_000_000, -1_000_000, 800, 600)));
        assert!(!on_a_monitor((i32::MAX - 10, 0, 800, 600)));
    }

    #[test]
    fn holder_exits_only_when_nothing_left() {
        assert!(holder_done(0, 0));
        assert!(!holder_done(1, 0), "a client would close itself on release");
        assert!(!holder_done(0, 1), "RMI still wants it held");
        assert!(!holder_done(3, 1));
        // Lets go only after two believed checks in a row; a process list without the holder
        // itself (a failed snapshot) never counts.
        assert_eq!(holder_idle(0, true, 0, 0), 1);
        assert_eq!(holder_idle(1, true, 0, 0), 2);
        assert_eq!(holder_idle(1, false, 0, 0), 0);
        assert_eq!(holder_idle(1, true, 1, 0), 0);
        assert!(processes().is_some_and(|all| all.iter().any(|(p, _)| *p == std::process::id())));
    }

    #[test]
    fn multi_instance_acquire_and_release() {
        let name = format!("RMI_test_mutex_{}", std::process::id());
        let a = MultiInstance::acquire(&name).unwrap();
        let e = MultiInstance::acquire(&name)
            .err()
            .expect("second acquire must fail");
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        // acquired from another thread than the drop: ownership stays on the owner thread
        std::thread::spawn(move || drop(a)).join().unwrap();
        assert!(MultiInstance::acquire(&name).is_ok(), "drop released it");
    }
}
