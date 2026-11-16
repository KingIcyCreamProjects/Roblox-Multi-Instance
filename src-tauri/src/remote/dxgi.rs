//! Windows screen capture with DXGI Desktop Duplication: the GPU hands over the desktop
//! picture only when it changed; one copy into a CPU-readable texture per new frame. Kept
//! per capture thread (COM objects are not `Send`) and dropped with it. `None` means "use
//! xcap": no duplication here (RDP, secure desktop, rotated output) or it was just lost.

use image::RgbaImage;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_FLAG, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

/// After a failed open xcap is used this long before DXGI is tried again, doubling per
/// failure up to `RETRY_MAX` (a session taking over waits briefly for the old one's
/// duplication to go).
const RETRY: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(5);

struct Dup {
    /// Output's top-left in virtual-desktop pixels (what xcap's `x()`/`y()` report).
    at: (i32, i32),
    dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    stage: Option<ID3D11Texture2D>,
    img: Option<RgbaImage>,
}

/// A monitor (top-left) where DXGI last failed to open: when, and the wait before the next
/// try.
type Failed = ((i32, i32), Instant, Duration);

thread_local! {
    static DUPS: RefCell<Vec<Dup>> = const { RefCell::new(Vec::new()) };
    static FAILED: RefCell<Vec<Failed>> = const { RefCell::new(Vec::new()) };
}

/// The monitor whose top-left is `at`, physical pixels; the last picture again if nothing
/// changed since.
pub fn grab(at: (i32, i32)) -> Option<RgbaImage> {
    DUPS.with_borrow_mut(|dups| {
        let i = match dups.iter().position(|d| d.at == at) {
            Some(i) => i,
            None => {
                let failed = FAILED.with_borrow(|f| f.iter().find(|f| f.0 == at).copied());
                if failed.is_some_and(|(_, t, wait)| t.elapsed() < wait) {
                    return None;
                }
                match open(at) {
                    Ok(d) => {
                        FAILED.with_borrow_mut(|f| f.retain(|f| f.0 != at));
                        dups.push(d);
                        dups.len() - 1
                    }
                    Err(e) => {
                        log::debug!("remote: DXGI capture unavailable, using GDI: {e}");
                        let wait = failed.map_or(RETRY, |f| (f.2 * 2).min(RETRY_MAX));
                        FAILED.with_borrow_mut(|f| {
                            f.retain(|f| f.0 != at);
                            f.push((at, Instant::now(), wait));
                        });
                        return None;
                    }
                }
            }
        };
        next(dups, i).ok()?;
        dups[i].img.clone()
    })
}

/// Nothing new on the open duplication at `at` since the last `grab`/`unchanged` (also
/// when only the pointer moved): the caller can reuse its last picture without a copy.
/// False when there is no duplication there (xcap in use).
pub fn unchanged(at: (i32, i32)) -> bool {
    DUPS.with_borrow_mut(|dups| {
        let Some(i) = dups.iter().position(|d| d.at == at) else {
            return false;
        };
        dups[i].img.is_some() && matches!(next(dups, i), Ok(false))
    })
}

/// `dups[i]` takes in the newest picture (true if there was one); access lost (mode
/// change, desktop switch), device removed, ...: dropped, reopened by the next `grab`.
fn next(dups: &mut Vec<Dup>, i: usize) -> Result<bool> {
    dups[i].frame().inspect_err(|e| {
        log::debug!("remote: DXGI capture lost: {e}");
        dups.swap_remove(i);
    })
}

#[cfg(test)]
pub fn reset() {
    DUPS.with_borrow_mut(Vec::clear);
    FAILED.with_borrow_mut(Vec::clear);
}

fn open(at: (i32, i32)) -> Result<Dup> {
    // SAFETY: plain COM calls; every out pointer is a live local.
    unsafe {
        let f: IDXGIFactory1 = CreateDXGIFactory1()?;
        for a in (0..).map_while(|i| f.EnumAdapters1(i).ok()) {
            for o in (0..).map_while(|i| a.EnumOutputs(i).ok()) {
                let d = o.GetDesc()?;
                if (d.DesktopCoordinates.left, d.DesktopCoordinates.top) != at {
                    continue;
                }
                // ponytail: rotated outputs come unrotated from DXGI; xcap handles them
                if d.Rotation != DXGI_MODE_ROTATION_IDENTITY
                    && d.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED
                {
                    return Err(E_FAIL.into());
                }
                let (mut dev, mut ctx) = (None, None);
                D3D11CreateDevice(
                    &a,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_FLAG::default(),
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut dev),
                    None,
                    Some(&mut ctx),
                )?;
                let dev = dev.ok_or(E_FAIL)?;
                let dup = o.cast::<IDXGIOutput1>()?.DuplicateOutput(&dev)?;
                return Ok(Dup {
                    at,
                    dev,
                    ctx: ctx.ok_or(E_FAIL)?,
                    dup,
                    stage: None,
                    img: None,
                });
            }
        }
        Err(DXGI_ERROR_NOT_FOUND.into())
    }
}

impl Dup {
    /// Copies the newest picture into `img`; false if none came in (or only the pointer
    /// moved).
    fn frame(&mut self) -> Result<bool> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res = None;
        // no wait once there is a picture: the capture loop paces itself
        let wait = if self.img.is_some() { 0 } else { 100 };
        // SAFETY: out pointers are live locals.
        match unsafe { self.dup.AcquireNextFrame(wait, &mut info, &mut res) } {
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
            r => r?,
        }
        let r = self.copy(&info, res);
        // SAFETY: a frame is held (acquired above).
        unsafe { self.dup.ReleaseFrame() }?;
        r
    }

    fn copy(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO, res: Option<IDXGIResource>) -> Result<bool> {
        if info.LastPresentTime == 0 && self.img.is_some() {
            return Ok(false); // only the pointer moved
        }
        let tex: ID3D11Texture2D = res.ok_or(E_FAIL)?.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: writes one desc.
        unsafe { tex.GetDesc(&mut desc) };
        let (w, h) = (desc.Width, desc.Height);
        if w == 0 || h == 0 {
            return Err(E_FAIL.into());
        }
        if self.img.as_ref().is_none_or(|i| i.dimensions() != (w, h)) {
            let sd = D3D11_TEXTURE2D_DESC {
                MipLevels: 1,
                ArraySize: 1,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..desc
            };
            let mut t = None;
            // SAFETY: desc and out pointer are live locals.
            unsafe { self.dev.CreateTexture2D(&sd, None, Some(&mut t)) }?;
            self.stage = Some(t.ok_or(E_FAIL)?);
            self.img = Some(RgbaImage::new(w, h));
        }
        let (Some(stage), Some(img)) = (&self.stage, &mut self.img) else {
            return Err(E_FAIL.into());
        };
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: same-size textures; the map is undone below.
        unsafe {
            self.ctx.CopyResource(stage, &tex);
            self.ctx.Map(stage, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        }
        let (pitch, row) = (m.RowPitch as usize, w as usize * 4);
        // SAFETY: a mapped BGRA texture: `h` rows `pitch` bytes apart, `row` bytes each.
        let src = unsafe {
            std::slice::from_raw_parts(m.pData as *const u8, pitch * (h as usize - 1) + row)
        };
        for (d, s) in img.chunks_exact_mut(row).zip(src.chunks(pitch)) {
            for (d, s) in d
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(s[..row].as_chunks::<4>().0)
            {
                *d = [s[2], s[1], s[0], 255];
            }
        }
        // SAFETY: mapped above.
        unsafe { self.ctx.Unmap(stage, 0) };
        Ok(true)
    }
}
