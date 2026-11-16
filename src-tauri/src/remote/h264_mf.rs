//! Windows: Media Foundation H.264 encoder MFTs. Hardware first (one path for NVIDIA NVENC,
//! AMD AMF and Intel Quick Sync, whichever GPUs are in the machine; NVIDIA, then AMD, then
//! Intel), then Microsoft's software MFT that ships with Windows. Hardware MFTs are
//! asynchronous (they ask for input and announce output through events); the software one is
//! synchronous. Low latency, CBR, no B-frames, no periodic keyframes, live bitrate changes.
//!
//! ponytail: frames reach the encoder as CPU NV12 (converted here from the captured RGBA).
//! Zero copy (the DXGI duplication texture through an `IMFDXGIDeviceManager` and a D3D11 video
//! processor for BGRA to NV12) is the upgrade if conversion and upload show up in `enc`.

use image::RgbaImage;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Foundation::VARIANT_TRUE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};

type R<T> = windows::core::Result<T>;

/// COM on this thread and Media Foundation in the process (both kept for the process's life).
fn init() -> bool {
    // SAFETY: plain initialisation calls; an already initialised thread just says so
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    static STARTED: OnceLock<bool> = OnceLock::new();
    // SAFETY: as above; never shut down (sessions come and go)
    *STARTED.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.is_ok())
}

fn attr_str(a: &IMFAttributes, key: &GUID) -> Option<String> {
    let (mut p, mut n) = (PWSTR::null(), 0u32);
    // SAFETY: the string is ours to free once copied
    unsafe {
        a.GetAllocatedString(key, &mut p, &mut n).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

/// An encoder MFT that takes NV12 and makes H.264: (activation, label for `stats`, its own name).
struct Cand {
    /// Unique in the list (two GPUs can have encoders of the same name).
    id: String,
    act: IMFActivate,
    label: String,
    name: String,
}

/// Every H.264 encoder MFT, hardware (NVIDIA, AMD, Intel, others) before software.
fn candidates() -> Vec<Cand> {
    if !init() {
        return Vec::new();
    }
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut all = Vec::new();
    for hw in [true, false] {
        let flags = MFT_ENUM_FLAG_SORTANDFILTER
            | if hw {
                MFT_ENUM_FLAG_HARDWARE
            } else {
                MFT_ENUM_FLAG_SYNCMFT
            };
        let (mut pp, mut n) = (null_mut(), 0u32);
        // SAFETY: MF fills an array of `n` activations that we take and then free
        let mut found: Vec<Cand> = unsafe {
            if MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                flags,
                Some(&input),
                Some(&output),
                &mut pp,
                &mut n,
            )
            .is_err()
                || pp.is_null()
            {
                continue;
            }
            let list = std::slice::from_raw_parts_mut(pp, n as usize);
            let found = list
                .iter_mut()
                .filter_map(Option::take)
                .map(|act| {
                    let name = attr_str(&act, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_default();
                    let vendor =
                        attr_str(&act, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).unwrap_or_default();
                    let label = match (hw, vendor.to_ascii_uppercase().as_str()) {
                        (false, _) => "software MF".to_owned(),
                        (_, "VEN_10DE") => "NVENC via MF".to_owned(),
                        (_, "VEN_1002" | "VEN_1022") => "AMD AMF via MF".to_owned(),
                        (_, "VEN_8086") => "Intel Quick Sync via MF".to_owned(),
                        _ => format!("{name} via MF"),
                    };
                    Cand {
                        id: String::new(),
                        act,
                        label,
                        name,
                    }
                })
                .collect();
            CoTaskMemFree(Some(pp as *const c_void));
            found
        };
        // the discrete NVIDIA card before an AMD or Intel iGPU (stable: MF's order otherwise)
        found.sort_by_key(|c| {
            ["NVENC", "AMD", "Intel"]
                .iter()
                .position(|v| c.label.starts_with(v))
                .unwrap_or(3)
        });
        all.extend(found);
    }
    for (i, c) in all.iter_mut().enumerate() {
        c.id = format!("{i}:{}", c.name);
    }
    all
}

pub fn encoders() -> Vec<String> {
    candidates()
        .into_iter()
        .map(|c| format!("{} ({})", c.label, c.name))
        .collect()
}

fn var_u4(v: u32) -> VARIANT {
    let mut x = VARIANT::default();
    // SAFETY: plain fields of a zeroed VARIANT
    unsafe {
        let x = &mut *x.Anonymous.Anonymous;
        x.vt = VT_UI4;
        x.Anonymous.ulVal = v;
    }
    x
}

fn var_true() -> VARIANT {
    let mut x = VARIANT::default();
    // SAFETY: plain fields of a zeroed VARIANT
    unsafe {
        let x = &mut *x.Anonymous.Anonymous;
        x.vt = VT_BOOL;
        x.Anonymous.boolVal = VARIANT_TRUE;
    }
    x
}

pub struct Enc {
    t: IMFTransform,
    act: IMFActivate,
    codec: Option<ICodecAPI>,
    /// Hardware MFTs are asynchronous: their events say when they want input or have output.
    events: Option<IMFMediaEventGenerator>,
    /// Input the async MFT asked for and has not had yet.
    need: u32,
    /// Frames in that have not come out yet.
    held: u32,
    /// Which candidate this is (`State::bad`).
    id: String,
    /// The MFT allocates its output samples (else `out_size` bytes each).
    provides: bool,
    out_size: u32,
    label: String,
    w: u32,
    h: u32,
    fps: u32,
    t0: Instant,
}

impl Drop for Enc {
    fn drop(&mut self) {
        // SAFETY: plain calls on live objects; errors do not matter any more
        unsafe {
            let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.act.ShutdownObject();
        }
    }
}

impl Enc {
    /// The first encoder that opens, leaving out `skip` (failed this session).
    pub fn new(w: u32, h: u32, kbps: u32, fps: u32, skip: &[String]) -> Result<Enc, String> {
        let mut why = Vec::new();
        for c in candidates().iter().filter(|c| !skip.contains(&c.id)) {
            match Enc::open(c, w, h, kbps, fps) {
                Ok(e) => return Ok(e),
                Err(e) => why.push(format!("{}: {e}", c.name)),
            }
        }
        if why.is_empty() {
            why.push("none installed or all failed".into());
        }
        Err(format!("no H.264 encoder MFT started ({})", why.join("; ")))
    }

    fn open(c: &Cand, w: u32, h: u32, kbps: u32, fps: u32) -> R<Enc> {
        // SAFETY: Media Foundation calls on objects created here, on this thread
        unsafe {
            let t: IMFTransform = c.act.ActivateObject()?;
            let attrs = t.GetAttributes().ok();
            let is_async = attrs
                .as_ref()
                .and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok())
                == Some(1);
            if let Some(a) = attrs.as_ref().filter(|_| is_async) {
                a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            }
            let mut e = Enc {
                codec: t.cast().ok(),
                t,
                act: c.act.clone(),
                events: None,
                need: 0,
                held: 0,
                id: c.id.clone(),
                provides: false,
                out_size: 0,
                label: c.label.clone(),
                w,
                h,
                fps,
                t0: Instant::now(),
            };
            e.settings(kbps);
            let size = (u64::from(w) << 32) | u64::from(h);
            let rate = (u64::from(fps) << 32) | 1;
            let mut set = Err(windows::core::Error::empty());
            // baseline where the encoder has it; main has no B-frames either (count set to 0)
            for profile in [eAVEncH264VProfile_Base, eAVEncH264VProfile_Main] {
                let o = MFCreateMediaType()?;
                o.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                o.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
                o.SetUINT32(&MF_MT_AVG_BITRATE, kbps.saturating_mul(1000))?;
                o.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
                o.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
                o.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
                o.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                o.SetUINT32(&MF_MT_MPEG2_PROFILE, profile.0 as u32)?;
                set = e.t.SetOutputType(0, &o, 0);
                if set.is_ok() {
                    break;
                }
            }
            set?;
            let i = MFCreateMediaType()?;
            i.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            i.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            i.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            i.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            i.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
            i.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            i.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)?;
            i.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            i.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            e.t.SetInputType(0, &i, 0)?;
            // some encoders only take these once the types are set
            e.settings(kbps);
            let info = e.t.GetOutputStreamInfo(0)?;
            e.provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
            e.out_size = info.cbSize.max(w * h * 3 / 2);
            e.t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            e.t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            if is_async {
                e.events = Some(e.t.cast()?);
            }
            Ok(e)
        }
    }

    /// Low latency, CBR at `kbps`, no B-frames, keyframes only when asked. Best effort: an
    /// encoder that refuses one keeps its default.
    fn settings(&self, kbps: u32) {
        let Some(c) = &self.codec else { return };
        let set: [(&GUID, VARIANT); 5] = [
            (&CODECAPI_AVLowLatencyMode, var_true()),
            (
                &CODECAPI_AVEncCommonRateControlMode,
                var_u4(eAVEncCommonRateControlMode_CBR.0 as u32),
            ),
            (
                &CODECAPI_AVEncCommonMeanBitRate,
                var_u4(kbps.saturating_mul(1000)),
            ),
            (&CODECAPI_AVEncMPVDefaultBPictureCount, var_u4(0)),
            (&CODECAPI_AVEncMPVGOPSize, var_u4(u32::MAX)),
        ];
        for (k, v) in &set {
            // SAFETY: a live ICodecAPI and VARIANTs that outlive the call
            if let Err(e) = unsafe { c.SetValue(*k, v) } {
                log::debug!("remote: {} refused a setting: {e}", self.label);
            }
        }
    }

    pub fn name(&self) -> &str {
        &self.label
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn set_kbps(&mut self, kbps: u32) -> bool {
        let Some(c) = &self.codec else { return false };
        // SAFETY: as in `settings`
        unsafe {
            c.SetValue(
                &CODECAPI_AVEncCommonMeanBitRate,
                &var_u4(kbps.saturating_mul(1000)),
            )
        }
        .is_ok()
    }

    /// The next event of an async MFT, waiting at most `wait` (`None`: none came).
    fn event(&self, wait: Duration) -> R<Option<u32>> {
        let Some(ev) = &self.events else {
            return Ok(None);
        };
        let until = Instant::now() + wait;
        loop {
            // SAFETY: a live event generator
            match unsafe { ev.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(e) => {
                    // SAFETY: a live event
                    let (t, st) = unsafe { (e.GetType()?, e.GetStatus()?) };
                    if t == MEError.0 as u32 || st.is_err() {
                        return Err(st.into());
                    }
                    return Ok(Some(t));
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= until {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_micros(500));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// One encoded access unit, `None` when the MFT has nothing (more) right now.
    fn pull(&mut self) -> R<Option<Vec<u8>>> {
        // a format change renegotiates the output type once, then tries again
        for _ in 0..2 {
            // SAFETY: Media Foundation calls on our own objects; the sample comes back owned
            unsafe {
                let sample = if self.provides {
                    None
                } else {
                    let s = MFCreateSample()?;
                    s.AddBuffer(&MFCreateMemoryBuffer(self.out_size)?)?;
                    Some(s)
                };
                let mut b = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(sample),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0;
                let r = self.t.ProcessOutput(0, &mut b, &mut status);
                let [b] = b;
                let sample = ManuallyDrop::into_inner(b.pSample);
                drop(ManuallyDrop::into_inner(b.pEvents));
                match r {
                    Ok(()) => {
                        let Some(s) = sample else { return Ok(None) };
                        let buf = s.ConvertToContiguousBuffer()?;
                        let (mut p, mut len) = (null_mut(), 0u32);
                        buf.Lock(&mut p, None, Some(&mut len))?;
                        let v = if len == 0 {
                            Vec::new()
                        } else {
                            std::slice::from_raw_parts(p, len as usize).to_vec()
                        };
                        buf.Unlock()?;
                        return Ok(Some(v));
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        let ty = self.t.GetOutputAvailableType(0, 0)?;
                        self.t.SetOutputType(0, &ty, 0)?;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(None)
    }

    /// `img` (even size, as the encoder was opened) as NV12 in a sample.
    fn sample(&self, img: &RgbaImage) -> R<IMFSample> {
        let (w, h) = (self.w as usize, self.h as usize);
        let len = w * h * 3 / 2;
        // SAFETY: the locked buffer is `len` bytes long
        unsafe {
            let buf = MFCreateMemoryBuffer(len as u32)?;
            let mut p = null_mut();
            buf.Lock(&mut p, None, None)?;
            let (y, uv) = std::slice::from_raw_parts_mut(p, len).split_at_mut(w * h);
            nv12(img, y, uv);
            buf.Unlock()?;
            buf.SetCurrentLength(len as u32)?;
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            // 100 ns units
            s.SetSampleTime((self.t0.elapsed().as_nanos() / 100) as i64)?;
            s.SetSampleDuration(10_000_000 / i64::from(self.fps.max(1)))?;
            Ok(s)
        }
    }

    pub fn encode(&mut self, img: &RgbaImage) -> Result<Vec<Vec<u8>>, String> {
        self.encode_mf(img).map_err(|e| e.to_string())
    }

    /// The next frame is a keyframe (`false`: this encoder cannot be told).
    pub fn force_key(&mut self) -> bool {
        // SAFETY: as in `settings`
        self.codec.as_ref().is_some_and(|c| unsafe {
            c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &var_u4(1))
                .is_ok()
        })
    }

    /// SPS and PPS from the output type (for an encoder whose IDRs come without them).
    pub fn params(&self) -> Option<Vec<u8>> {
        // SAFETY: a blob MF owns until we free it
        unsafe {
            let ty = self.t.GetOutputCurrentType(0).ok()?;
            let n = ty.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
            let mut v = vec![0u8; n as usize];
            ty.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut v, None).ok()?;
            Some(v)
        }
    }

    fn encode_mf(&mut self, img: &RgbaImage) -> R<Vec<Vec<u8>>> {
        let sample = self.sample(img)?;
        let mut outs = Vec::new();
        if self.events.is_none() {
            // synchronous (software): in, then everything it has
            // SAFETY: a live MFT and sample
            unsafe { self.t.ProcessInput(0, &sample, 0)? };
            while let Some(au) = self.pull()? {
                outs.push(au);
            }
            return Ok(outs);
        }
        // asynchronous (hardware): input only when asked; output when announced. Waits are
        // short: a session ending (or taken over) never waits long on this thread
        let take = |e: &mut Self, outs: &mut Vec<Vec<u8>>| -> R<()> {
            if let Some(au) = e.pull()? {
                e.held = e.held.saturating_sub(1);
                outs.push(au);
            }
            Ok(())
        };
        while self.need == 0 {
            match self.event(Duration::from_millis(500))? {
                Some(t) if t == METransformNeedInput.0 as u32 => self.need += 1,
                Some(t) if t == METransformHaveOutput.0 as u32 => take(self, &mut outs)?,
                Some(_) => {}
                None => {
                    return Err(windows::core::Error::new(
                        MF_E_NO_EVENTS_AVAILABLE,
                        "the encoder stopped asking for input",
                    ))
                }
            }
        }
        // SAFETY: a live MFT and sample
        unsafe { self.t.ProcessInput(0, &sample, 0)? };
        self.need -= 1;
        self.held += 1;
        // in low-latency mode every frame comes out now: wait until none is held (a slow
        // frame does not leave one behind for good). An encoder that asks for more input
        // while still holding one gets a short grace; the rest comes with the next call.
        let mut until = Instant::now() + Duration::from_millis(200);
        while self.held > 0 {
            match self.event(until.saturating_duration_since(Instant::now()))? {
                Some(t) if t == METransformHaveOutput.0 as u32 => take(self, &mut outs)?,
                Some(t) if t == METransformNeedInput.0 as u32 => {
                    self.need += 1;
                    until = until.min(Instant::now() + Duration::from_millis(30));
                }
                Some(_) => {}
                None => break,
            }
        }
        Ok(outs)
    }
}

/// RGBA to NV12, BT.709 limited range (what decoders assume for HD without colour info), in
/// horizontal bands on several threads. `img` has even width and height.
fn nv12(img: &RgbaImage, y: &mut [u8], uv: &mut [u8]) {
    let w = img.width() as usize;
    let px = img.as_raw();
    let bands = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let pairs = (img.height() as usize / 2).div_ceil(bands).max(1); // row pairs per band
    std::thread::scope(|sc| {
        for (b, (ys, uvs)) in y
            .chunks_mut(2 * pairs * w)
            .zip(uv.chunks_mut(pairs * w))
            .enumerate()
        {
            sc.spawn(move || {
                for (r, (y2, uvr)) in ys.chunks_mut(2 * w).zip(uvs.chunks_mut(w)).enumerate() {
                    let row = (b * pairs + r) * 2;
                    let (top, bot) = (
                        &px[row * w * 4..][..w * 4],
                        &px[(row + 1) * w * 4..][..w * 4],
                    );
                    let (yt, yb) = y2.split_at_mut(w);
                    let luma = |p: &[u8]| {
                        let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
                        (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8
                    };
                    for x in (0..w).step_by(2) {
                        let q = |s: &[u8], i: usize| {
                            i32::from(s[x * 4 + i]) + i32::from(s[x * 4 + 4 + i])
                        };
                        yt[x] = luma(&top[x * 4..]);
                        yt[x + 1] = luma(&top[x * 4 + 4..]);
                        yb[x] = luma(&bot[x * 4..]);
                        yb[x + 1] = luma(&bot[x * 4 + 4..]);
                        // sums of 4 pixels
                        let (r, g, b) = (
                            q(top, 0) + q(bot, 0),
                            q(top, 1) + q(bot, 1),
                            q(top, 2) + q(bot, 2),
                        );
                        uvr[x] =
                            (((-26 * r - 87 * g + 112 * b + 512) >> 10) + 128).clamp(0, 255) as u8;
                        uvr[x + 1] =
                            (((112 * r - 102 * g - 10 * b + 512) >> 10) + 128).clamp(0, 255) as u8;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::h264::{nals, sps_info};

    /// Every encoder MFT on this machine that opens also encodes: IDR first (with SPS and
    /// PPS), deltas, an IDR on request. Each on its own, so the fallbacks (another GPU, the
    /// software MFT) are known to work too. Prints each one's SPS (reordering).
    #[test]
    fn every_mft_encodes() {
        let img = |n: u32| {
            RgbaImage::from_fn(640, 360, |x, y| {
                let v = if (x / 16, y / 16) == (n, 3) { 20 } else { 128 };
                image::Rgba([v, v, v, 255])
            })
        };
        for c in candidates() {
            let mut e = match Enc::open(&c, 640, 360, 6_000, 60) {
                Ok(e) => e,
                Err(err) => {
                    println!(
                        "{} ({}): does not open here, skipped: {err}",
                        c.label, c.name
                    );
                    continue;
                }
            };
            let mut aus = Vec::new();
            for n in 0..8 {
                if n == 4 {
                    assert!(e.force_key(), "{}: no keyframe on request", c.name);
                }
                aus.extend(
                    e.encode_mf(&img(n))
                        .unwrap_or_else(|err| panic!("{}: {err}", c.name)),
                );
            }
            let idr: Vec<bool> = aus.iter().map(|a| nals(a).any(|(t, _)| t == 5)).collect();
            let sps = aus
                .iter()
                .flat_map(|a| nals(a))
                .find(|(t, _)| *t == 7)
                .map(|(_, n)| n.to_vec());
            let sps = sps.or_else(|| {
                e.params()
                    .and_then(|p| nals(&p).find(|(t, _)| *t == 7).map(|(_, n)| n.to_vec()))
            });
            println!(
                "{} ({}): {} of 8 out, IDR at {:?}; SPS: {}",
                c.label,
                c.name,
                aus.len(),
                idr.iter()
                    .enumerate()
                    .filter(|(_, k)| **k)
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>(),
                sps.as_deref().map_or("none".into(), sps_info)
            );
            assert!(aus.len() >= 6, "{}: frames held back", c.name);
            assert!(
                idr[0] && !idr[1] && idr[2..].contains(&true),
                "{}: {idr:?}",
                c.name
            );
            assert!(sps.is_some(), "{}: no SPS", c.name);
        }
    }
}
