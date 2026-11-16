//! macOS: VideoToolbox (`VTCompressionSession`), hardware on every Mac that has it. Real time,
//! no frame reordering, low-latency rate control where the OS offers it. BGRA in (VideoToolbox
//! converts on the GPU), AVCC out, turned into Annex B with SPS and PPS before each IDR.

#![allow(non_upper_case_globals, non_snake_case)]

use image::RgbaImage;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::time::Instant;

type CFTypeRef = *const c_void;
type CFStringRef = CFTypeRef;
type CFDictionaryRef = CFTypeRef;
type OSStatus = i32;
type Session = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}
const CMTIME_VALID: u32 = 1;
const INVALID: CMTime = CMTime {
    value: 0,
    timescale: 0,
    flags: 0,
    epoch: 0,
};

type Callback = extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CFTypeRef);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFRelease(cf: CFTypeRef);
    fn CFNumberCreate(alloc: CFTypeRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        n: isize,
        kcb: *const c_void,
        vcb: *const c_void,
    ) -> CFDictionaryRef;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetDataBuffer(sb: CFTypeRef) -> CFTypeRef;
    fn CMSampleBufferGetFormatDescription(sb: CFTypeRef) -> CFTypeRef;
    fn CMBlockBufferGetDataLength(bb: CFTypeRef) -> usize;
    fn CMBlockBufferCopyDataBytes(
        bb: CFTypeRef,
        off: usize,
        len: usize,
        dst: *mut c_void,
    ) -> OSStatus;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        desc: CFTypeRef,
        index: usize,
        ptr: *mut *const u8,
        size: *mut usize,
        count: *mut usize,
        header_len: *mut i32,
    ) -> OSStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferCreate(
        alloc: CFTypeRef,
        w: usize,
        h: usize,
        format: u32,
        attrs: CFDictionaryRef,
        out: *mut CFTypeRef,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(pb: CFTypeRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: CFTypeRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: CFTypeRef) -> *mut u8;
    fn CVPixelBufferGetBytesPerRow(pb: CFTypeRef) -> usize;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    static kVTCompressionPropertyKey_RealTime: CFStringRef;
    static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
    static kVTProfileLevel_H264_Baseline_AutoLevel: CFStringRef;
    static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    static kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder: CFStringRef;
    static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;
    fn VTCompressionSessionCreate(
        alloc: CFTypeRef,
        w: i32,
        h: i32,
        codec: u32,
        spec: CFDictionaryRef,
        src_attrs: CFDictionaryRef,
        data_alloc: CFTypeRef,
        cb: Callback,
        refcon: *mut c_void,
        out: *mut Session,
    ) -> OSStatus;
    fn VTSessionSetProperty(s: Session, key: CFStringRef, value: CFTypeRef) -> OSStatus;
    fn VTSessionCopyProperty(
        s: Session,
        key: CFStringRef,
        alloc: CFTypeRef,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(
        s: Session,
        image: CFTypeRef,
        pts: CMTime,
        duration: CMTime,
        props: CFDictionaryRef,
        frame_refcon: *mut c_void,
        flags_out: *mut u32,
    ) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(s: Session, until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(s: Session);
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
}
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

/// A VideoToolbox constant only newer macOS has, looked up at run time (linking it would stop
/// the app from starting on older macOS).
fn newer_key(name: &std::ffi::CStr) -> Option<CFStringRef> {
    // SAFETY: dlsym of a data symbol; a found one is a valid CFStringRef
    unsafe {
        let p = dlsym(RTLD_DEFAULT, name.as_ptr()) as *const CFStringRef;
        (!p.is_null()).then(|| *p)
    }
}

const AVC1: u32 = u32::from_be_bytes(*b"avc1");
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const SINT32: isize = 3;

/// A CF object released on drop.
struct Cf(CFTypeRef);
impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: an owned (+1) reference
            unsafe { CFRelease(self.0) }
        }
    }
}

fn num(v: i32) -> Cf {
    // SAFETY: plain CF call; `v` lives across it
    Cf(unsafe { CFNumberCreate(null(), SINT32, (&raw const v).cast()) })
}

fn dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> Cf {
    let (k, v): (Vec<_>, Vec<_>) = pairs.iter().copied().unzip();
    // SAFETY: keys and values are live CF objects; the dictionary retains them
    Cf(unsafe {
        CFDictionaryCreate(
            null(),
            k.as_ptr(),
            v.as_ptr(),
            k.len() as isize,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    })
}

/// Encoded access units (Annex B), or the OSStatus of a failed frame, written by VideoToolbox's
/// callback.
type Out = Mutex<Vec<Result<Vec<u8>, OSStatus>>>;

extern "C" fn on_frame(
    refcon: *mut c_void,
    _: *mut c_void,
    status: OSStatus,
    _: u32,
    sb: CFTypeRef,
) {
    // SAFETY: `refcon` is the `Out` boxed in `Enc`, alive until the session is invalidated
    let out = unsafe { &*(refcon as *const Out) };
    let r = if status != 0 {
        Err(status)
    } else if sb.is_null() {
        Ok(Vec::new()) // dropped by the encoder
    } else {
        // SAFETY: a valid sample buffer for the duration of the callback
        unsafe { annex_b(sb) }.ok_or(-1)
    };
    out.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(r);
}

/// The sample's AVCC data as Annex B, SPS and PPS before the first IDR slice.
unsafe fn annex_b(sb: CFTypeRef) -> Option<Vec<u8>> {
    unsafe {
        let bb = CMSampleBufferGetDataBuffer(sb);
        let len = CMBlockBufferGetDataLength(bb);
        let mut avcc = vec![0u8; len];
        if CMBlockBufferCopyDataBytes(bb, 0, len, avcc.as_mut_ptr().cast()) != 0 {
            return None;
        }
        let desc = CMSampleBufferGetFormatDescription(sb);
        let (mut count, mut hl) = (0usize, 4i32);
        let mut out = Vec::with_capacity(len + 64);
        let mut i = 0;
        let mut params = false;
        while i + 4 <= avcc.len() {
            let n = u32::from_be_bytes(avcc[i..i + 4].try_into().ok()?) as usize;
            let nal = avcc.get(i + 4..i + 4 + n)?;
            if !params && nal.first().is_some_and(|b| b & 0x1f == 5) {
                params = true;
                // an IDR: the parameter sets live in the format description
                let mut k = 0;
                loop {
                    let (mut p, mut sz) = (null(), 0usize);
                    if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                        desc, k, &mut p, &mut sz, &mut count, &mut hl,
                    ) != 0
                    {
                        break;
                    }
                    out.extend([0, 0, 0, 1]);
                    out.extend(std::slice::from_raw_parts(p, sz));
                    k += 1;
                    if k >= count {
                        break;
                    }
                }
            }
            out.extend([0, 0, 0, 1]);
            out.extend(nal);
            i += 4 + n;
        }
        // VideoToolbox's NAL length size is 4 for H.264; anything else would have failed above
        let _ = hl;
        Some(out)
    }
}

pub struct Enc {
    s: Session,
    out: Box<Out>,
    name: String,
    t0: Instant,
    /// The next frame is to be a keyframe.
    key: bool,
}

impl Drop for Enc {
    fn drop(&mut self) {
        // SAFETY: invalidate waits for callbacks to finish; then nothing touches `out`
        unsafe {
            VTCompressionSessionInvalidate(self.s);
            CFRelease(self.s.cast_const());
        }
    }
}

pub fn encoders() -> Vec<String> {
    Enc::new(1920, 1080, 16_000, 60, &[])
        .map(|e| vec![e.name.clone()])
        .unwrap_or_default()
}

impl Enc {
    /// `skip`: encoders that failed this session (here: VideoToolbox itself).
    pub fn new(w: u32, h: u32, kbps: u32, fps: u32, skip: &[String]) -> Result<Enc, String> {
        if skip.iter().any(|s| s == "vt") {
            return Err("VideoToolbox failed this session".into());
        }
        let out: Box<Out> = Box::default();
        let refcon = (&raw const *out).cast_mut().cast();
        let mut s: Session = null_mut();
        // low-latency rate control (macOS 11.3+, Apple silicon and recent Intel); else plain
        // real time
        // SAFETY: a static CF constant
        let low = newer_key(c"kVTVideoEncoderSpecification_EnableLowLatencyRateControl")
            .map(|k| dict(&[(k, unsafe { kCFBooleanTrue })]));
        let mut st = -1;
        for spec in [low.as_ref().map_or(null(), |d| d.0), null()] {
            // SAFETY: valid arguments; `refcon` outlives the session (dropped after Invalidate)
            st = unsafe {
                VTCompressionSessionCreate(
                    null(),
                    w as i32,
                    h as i32,
                    AVC1,
                    spec,
                    null(),
                    null(),
                    on_frame,
                    refcon,
                    &mut s,
                )
            };
            if st == 0 {
                break;
            }
        }
        if st != 0 {
            return Err(format!("VTCompressionSessionCreate: {st}"));
        }
        let mut e = Enc {
            s,
            out,
            name: String::new(),
            t0: Instant::now(),
            key: false,
        };
        let (fps_n, key_every) = (num(fps as i32), num(i32::MAX));
        // SAFETY: static CF constants, live values; refused properties are logged, not fatal
        unsafe {
            for (k, v) in [
                (kVTCompressionPropertyKey_RealTime, kCFBooleanTrue),
                (
                    kVTCompressionPropertyKey_AllowFrameReordering,
                    kCFBooleanFalse,
                ),
                (
                    kVTCompressionPropertyKey_ProfileLevel,
                    kVTProfileLevel_H264_Baseline_AutoLevel,
                ),
                (kVTCompressionPropertyKey_ExpectedFrameRate, fps_n.0),
                (kVTCompressionPropertyKey_MaxKeyFrameInterval, key_every.0),
            ] {
                let r = VTSessionSetProperty(e.s, k, v);
                if r != 0 {
                    log::debug!("remote: VideoToolbox property refused: {r}");
                }
            }
            let mut hw: CFTypeRef = null();
            let _ = VTSessionCopyProperty(
                e.s,
                kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                null(),
                &mut hw,
            );
            let hw = Cf(hw);
            // the low-latency (hardware) encoder does not answer this: only an explicit
            // "no" means software
            e.name = if hw.0 == kCFBooleanFalse {
                "VideoToolbox (software)"
            } else {
                "VideoToolbox"
            }
            .into();
        }
        if !e.set_kbps(kbps) {
            return Err("VideoToolbox refused the bitrate".into());
        }
        Ok(e)
    }

    pub fn id(&self) -> &str {
        "vt"
    }

    /// The next frame is a keyframe.
    pub fn force_key(&mut self) -> bool {
        self.key = true;
        true
    }

    /// VideoToolbox's IDRs always carry SPS and PPS (`annex_b`).
    pub fn params(&self) -> Option<Vec<u8>> {
        None
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_kbps(&mut self, kbps: u32) -> bool {
        let bps = num(kbps.saturating_mul(1000).min(i32::MAX as u32) as i32);
        // SAFETY: live session and value
        unsafe {
            VTSessionSetProperty(self.s, kVTCompressionPropertyKey_AverageBitRate, bps.0) == 0
        }
    }

    pub fn encode(&mut self, img: &RgbaImage) -> Result<Vec<Vec<u8>>, String> {
        let key = std::mem::take(&mut self.key);
        let (w, h) = img.dimensions();
        let mut pb: CFTypeRef = null();
        // SAFETY: plain CoreVideo calls; rows are written within the locked buffer's bounds
        unsafe {
            if CVPixelBufferCreate(null(), w as usize, h as usize, BGRA, null(), &mut pb) != 0 {
                return Err("CVPixelBufferCreate failed".into());
            }
            let pb = Cf(pb);
            CVPixelBufferLockBaseAddress(pb.0, 0);
            let (base, stride) = (
                CVPixelBufferGetBaseAddress(pb.0),
                CVPixelBufferGetBytesPerRow(pb.0),
            );
            for (y, row) in img.as_raw().chunks_exact(w as usize * 4).enumerate() {
                let dst = std::slice::from_raw_parts_mut(base.add(y * stride), row.len());
                for (d, s) in dst
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0)
                {
                    *d = [s[2], s[1], s[0], 255];
                }
            }
            CVPixelBufferUnlockBaseAddress(pb.0, 0);
            let pts = CMTime {
                value: self.t0.elapsed().as_micros() as i64,
                timescale: 1_000_000,
                flags: CMTIME_VALID,
                epoch: 0,
            };
            let props =
                key.then(|| dict(&[(kVTEncodeFrameOptionKey_ForceKeyFrame, kCFBooleanTrue)]));
            let props = props.as_ref().map_or(null(), |p| p.0);
            let r = VTCompressionSessionEncodeFrame(
                self.s,
                pb.0,
                pts,
                INVALID,
                props,
                null_mut(),
                null_mut(),
            );
            if r != 0 {
                return Err(format!("VTCompressionSessionEncodeFrame: {r}"));
            }
            // real time: the frame comes out now; waiting for it keeps one frame per call
            let r = VTCompressionSessionCompleteFrames(self.s, INVALID);
            if r != 0 {
                return Err(format!("VTCompressionSessionCompleteFrames: {r}"));
            }
        }
        let got = std::mem::take(
            &mut *self
                .out
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        got.into_iter()
            .map(|r| r.map_err(|st| format!("VideoToolbox frame: {st}")))
            .collect()
    }
}
