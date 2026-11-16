//! H.264 frames (`0x06`) for viewers that decode them (WebCodecs), from the platform's own
//! encoder: Media Foundation on Windows (the GPU vendor's hardware MFT: NVENC, AMF, Quick
//! Sync; else Microsoft's software MFT), VideoToolbox on macOS. Low latency, no B-frames,
//! rate control by bitrate, no periodic keyframes (TCP loses nothing): an IDR comes first,
//! then only on a size change, a viewer request or after an encoder error. No encoder (or
//! one that fails to start): the stream stays JPEG.

use axum::body::Bytes;
use image::RgbaImage;

#[cfg(target_os = "macos")]
#[path = "h264_vt.rs"]
mod imp;
#[cfg(windows)]
#[path = "h264_mf.rs"]
mod imp;

/// Encoders take the longer side up to 3840 and the shorter up to 2160; tiny pictures are
/// scaled up (hardware encoders refuse very small frames).
const LONG: u32 = 3840;
const SHORT: u32 = 2160;
const MIN_W: u32 = 256;
const MIN_H: u32 = 128;

/// This host can encode H.264 (checked once per process).
pub fn available() -> bool {
    #[cfg(any(windows, target_os = "macos"))]
    {
        static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        // its own thread: Windows initialises COM for the probe
        *OK.get_or_init(|| {
            let list = std::thread::spawn(imp::encoders).join().unwrap_or_default();
            log::info!("remote: H.264 encoders: {list:?}");
            !list.is_empty()
        })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    false
}

/// Every encoder this host could use, best first (bench).
#[cfg(all(test, any(windows, target_os = "macos")))]
pub fn encoders() -> Vec<String> {
    imp::encoders()
}

/// Frame size for a `w` x `h` picture: at most `max_w` wide, within the encoders' limits,
/// both even (4:2:0).
pub fn size(w: u32, h: u32, max_w: u32) -> (u32, u32) {
    let (w, h) = (w.max(2), h.max(2));
    let f = (f64::from(max_w.max(2)) / f64::from(w))
        .min(f64::from(LONG) / f64::from(w.max(h)))
        .min(f64::from(SHORT) / f64::from(w.min(h)))
        .min(1.0)
        .max(f64::from(MIN_W) / f64::from(w))
        .max(f64::from(MIN_H) / f64::from(h));
    let even = |v: u32| ((f64::from(v) * f).round() as u32 & !1).max(2);
    (even(w), even(h))
}

/// The encoder kept between frames of one stream.
struct H264 {
    #[cfg(any(windows, target_os = "macos"))]
    enc: imp::Enc,
    dims: (u32, u32),
    kbps: u32,
    fps: u32,
    /// Access units it made.
    made: usize,
}

/// The encoder thread's H.264 state for one session.
#[derive(Default)]
pub struct State {
    cur: Option<H264>,
    /// Encoders that failed this session (`Enc::id`); never tried again.
    bad: Vec<String>,
    /// Encode failures in a row (across restarts).
    fails: u32,
    /// SPS and PPS (Annex B) of the stream, put before an IDR that comes without them.
    params: Vec<u8>,
    /// Tests: every encode fails.
    #[cfg(test)]
    fail: bool,
}

impl State {
    /// Which encoder runs (`stats.encoder`), if one does.
    pub fn name(&self) -> Option<&str> {
        #[cfg(any(windows, target_os = "macos"))]
        return self.cur.as_ref().map(|s| s.enc.name());
        #[cfg(not(any(windows, target_os = "macos")))]
        None
    }

    /// Tests: a state whose every encode fails.
    #[cfg(test)]
    pub fn failing() -> State {
        State {
            fail: true,
            ..Default::default()
        }
    }

    /// Drops the encoder (the next frame starts a new one, with an IDR).
    pub fn reset(&mut self) {
        self.cur = None;
    }
}

/// NAL unit types and bodies (start code stripped) of an Annex B stream.
pub fn nals(d: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut i = 0;
    std::iter::from_fn(move || {
        // the next start code (00 00 01; a 4-byte one ends the same way)
        let start = (i..d.len().saturating_sub(2))
            .find(|&k| d[k] == 0 && d[k + 1] == 0 && d[k + 2] == 1)?
            + 3;
        let end = (start..d.len().saturating_sub(2))
            .find(|&k| d[k] == 0 && d[k + 1] == 0 && d[k + 2] <= 1)
            .unwrap_or(d.len());
        i = end;
        Some((d.get(start).map_or(0, |b| b & 0x1f), &d[start..end]))
    })
}

/// `0x06` frames of `img`: screen id, flags (bit 0: IDR), then one Annex B access unit (SPS
/// and PPS in band before every IDR). Usually one; none while a hardware encoder still holds
/// the frame (it comes with the next call), more if it held some. On an encode error the
/// encoder is dropped (the next frame starts again with an IDR); one that fails before its
/// first frame, or twice in a row, is not used again this session (the next one is tried).
/// `Err`: no encoder (left) works here: stream JPEG instead.
pub fn frame(
    st: &mut State,
    img: &RgbaImage,
    screen: u8,
    kbps: u32,
    fps: u32,
    max_w: u32,
    key: bool,
) -> Result<Vec<Bytes>, String> {
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (st, img, screen, kbps, fps, max_w, key);
        Err("no H.264 encoder on this platform".into())
    }
    #[cfg(any(windows, target_os = "macos"))]
    {
        let (w, h) = size(img.width(), img.height(), max_w);
        let small;
        let img = if (w, h) == img.dimensions() {
            img
        } else {
            small = super::scale(img, w, h);
            &small
        };
        // a new size or frame rate (both come with a new `start`, which asks for an IDR
        // anyway) starts a new encoder
        if st
            .cur
            .as_ref()
            .is_some_and(|s| s.dims != (w, h) || s.fps != fps)
        {
            st.cur = None;
        }
        if let Some(s) = st.cur.as_mut().filter(|s| s.kbps != kbps) {
            if s.enc.set_kbps(kbps) {
                s.kbps = kbps;
            } else {
                st.cur = None;
            }
        }
        // a keyframe the encoder cannot be told to make: a new one starts with it
        if key && st.cur.as_mut().is_some_and(|s| !s.enc.force_key()) {
            st.cur = None;
        }
        let s = match &mut st.cur {
            Some(s) => s,
            None => st.cur.insert(H264 {
                enc: imp::Enc::new(w, h, kbps, fps, &st.bad)?,
                dims: (w, h),
                kbps,
                fps,
                made: 0,
            }),
        };
        #[cfg(test)]
        let r = if st.fail {
            Err("failing on purpose".to_owned())
        } else {
            s.enc.encode(img)
        };
        #[cfg(not(test))]
        let r = s.enc.encode(img);
        let aus = match r {
            Ok(a) => a,
            Err(e) => {
                st.fails += 1;
                log::warn!("remote: H.264 encode failed ({}): {e}", s.enc.name());
                if s.made == 0 || st.fails >= 2 {
                    log::warn!("remote: not using {} again this session", s.enc.name());
                    st.bad.push(s.enc.id().to_owned());
                }
                st.cur = None;
                return Ok(Vec::new());
            }
        };
        let aus: Vec<_> = aus.into_iter().filter(|au| !au.is_empty()).collect();
        if !aus.is_empty() {
            st.fails = 0;
            s.made += aus.len();
        }
        Ok(aus
            .into_iter()
            .map(|au| {
                let idr = nals(&au).any(|(t, _)| t == 5);
                let mut out = Vec::with_capacity(au.len() + st.params.len() + 3);
                out.extend([0x06, screen, u8::from(idr)]);
                if idr {
                    let ps: Vec<u8> = nals(&au)
                        .filter(|(t, _)| matches!(t, 7 | 8))
                        .flat_map(|(_, n)| [&[0, 0, 0, 1][..], n].concat())
                        .collect();
                    if !ps.is_empty() {
                        st.params = ps;
                    } else {
                        if st.params.is_empty() {
                            st.params = s.enc.params().unwrap_or_default();
                        }
                        out.extend(&st.params);
                    }
                }
                out.extend(au);
                out.into()
            })
            .collect())
    }
}

/// Test aid: what an SPS says about decoding order (profile, level, and whether its VUI
/// promises no reordering, `max_num_reorder_frames`).
#[cfg(test)]
pub fn sps_info(sps: &[u8]) -> String {
    // emulation prevention bytes out (00 00 03 -> 00 00)
    let mut b = Vec::with_capacity(sps.len());
    for &x in sps.iter().skip(1) {
        if x == 3 && b.len() >= 2 && b[b.len() - 1] == 0 && b[b.len() - 2] == 0 {
            continue;
        }
        b.push(x);
    }
    let mut pos = 0usize;
    let mut bit = |n: u32| -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            let byte = *b.get(pos / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - pos % 8)) & 1);
            pos += 1;
        }
        Some(v)
    };
    let mut parse = || -> Option<String> {
        let ue = |bit: &mut dyn FnMut(u32) -> Option<u32>| -> Option<u32> {
            let mut z = 0;
            while bit(1)? == 0 {
                z += 1;
            }
            Some((1 << z) - 1 + bit(z)?)
        };
        let (profile, cons, level) = (bit(8)?, bit(8)?, bit(8)?);
        let head = format!("profile {profile}, constraints {cons:08b}, level {level}");
        ue(&mut bit)?; // sps id
        if [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135].contains(&profile) {
            if ue(&mut bit)? == 3 {
                bit(1)?;
            }
            ue(&mut bit)?;
            ue(&mut bit)?;
            bit(1)?;
            if bit(1)? == 1 {
                return Some(format!("{head}, scaling matrices (not parsed further)"));
            }
        }
        ue(&mut bit)?; // log2_max_frame_num
        match ue(&mut bit)? {
            0 => {
                ue(&mut bit)?;
            }
            1 => {
                bit(1)?;
                ue(&mut bit)?;
                ue(&mut bit)?;
                for _ in 0..ue(&mut bit)? {
                    ue(&mut bit)?;
                }
            }
            _ => {}
        }
        let refs = ue(&mut bit)?;
        bit(1)?;
        ue(&mut bit)?;
        ue(&mut bit)?;
        if bit(1)? == 0 {
            bit(1)?;
        }
        bit(1)?;
        if bit(1)? == 1 {
            for _ in 0..4 {
                ue(&mut bit)?;
            }
        }
        if bit(1)? == 0 {
            return Some(format!("{head}, {refs} ref frames, no VUI"));
        }
        if bit(1)? == 1 && bit(8)? == 255 {
            bit(32)?;
        }
        if bit(1)? == 1 {
            bit(1)?;
        }
        if bit(1)? == 1 {
            bit(4)?; // video_format, full range
            if bit(1)? == 1 {
                bit(24)?;
            }
        }
        if bit(1)? == 1 {
            ue(&mut bit)?;
            ue(&mut bit)?;
        }
        if bit(1)? == 1 {
            bit(32)?;
            bit(32)?;
            bit(1)?;
        }
        let mut hrd = false;
        for _ in 0..2 {
            if bit(1)? == 1 {
                hrd = true;
                let n = ue(&mut bit)?;
                bit(8)?;
                for _ in 0..=n {
                    ue(&mut bit)?;
                    ue(&mut bit)?;
                    bit(1)?;
                }
                bit(20)?;
            }
        }
        if hrd {
            bit(1)?;
        }
        bit(1)?;
        if bit(1)? == 0 {
            return Some(format!(
                "{head}, {refs} ref frames, VUI without bitstream_restriction"
            ));
        }
        bit(1)?;
        for _ in 0..4 {
            ue(&mut bit)?;
        }
        let reorder = ue(&mut bit)?;
        Some(format!(
            "{head}, {refs} ref frames, max_num_reorder_frames {reorder}"
        ))
    };
    parse().unwrap_or_else(|| "unreadable SPS".into())
}
