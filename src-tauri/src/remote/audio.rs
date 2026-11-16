//! `audio` cap (docs/remote-protocol.md): this computer's sound output, as 16-bit PCM in
//! about 20 ms messages. Never queued: more than 200 ms waiting drops the oldest.

use super::desk::Desk;
use super::{ctl_msg, lock};
use axum::body::Bytes;
use axum::extract::ws::Message;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Notify};

/// 20 ms messages waiting at most (200 ms).
const KEEP: usize = 10;

/// What a desk's sound capture hands over.
pub enum Sound {
    /// (rate, channels): at the start and after every device change.
    Format(u32, u16),
    /// Interleaved samples.
    Pcm(Vec<i16>),
    /// Capture cannot run (a plain sentence for the viewer); the capture returns after it.
    Fail(&'static str),
}

pub fn f32_to_i16(s: f32) -> i16 {
    // NaN casts to 0
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

pub fn i32_to_i16(s: i32) -> i16 {
    (s >> 16) as i16
}

/// Sound messages waiting for the writer task.
#[derive(Default)]
pub struct AudioQ {
    q: Mutex<VecDeque<Bytes>>,
    pub ready: Notify,
}

impl AudioQ {
    pub fn push(&self, b: Bytes) {
        {
            let mut q = lock(&self.q);
            if q.len() >= KEEP {
                q.pop_front();
            }
            q.push_back(b);
        }
        self.ready.notify_one();
    }

    pub fn pop(&self) -> Option<Bytes> {
        lock(&self.q).pop_front()
    }

    pub fn waiting(&self) -> bool {
        !lock(&self.q).is_empty()
    }

    fn clear(&self) {
        lock(&self.q).clear();
    }
}

/// The session's sound thread: streams while `on` and `alive` hold.
pub fn run(
    desk: &dyn Desk,
    on: &AtomicBool,
    alive: &AtomicBool,
    out: &mpsc::Sender<Message>,
    q: &AudioQ,
) {
    let (mut chunk, mut chans, mut buf) = (0usize, 0usize, Vec::<i16>::new());
    desk.audio(
        &|| on.load(SeqCst) && alive.load(SeqCst),
        &mut |s| match s {
            Sound::Format(rate, ch) => {
                // old-format sound must not play at the new rate
                q.clear();
                buf.clear();
                // the viewer plays stereo at most: surround keeps its first two channels
                chans = usize::from(ch);
                let ch = ch.min(2);
                chunk = (rate / 50) as usize * usize::from(ch);
                let m = json!({ "t": "audio", "rate": rate, "ch": ch });
                let _ = out.blocking_send(ctl_msg(&m));
            }
            Sound::Pcm(p) if chunk > 0 => {
                if chans > 2 {
                    buf.extend(p.chunks_exact(chans).flat_map(|f| [f[0], f[1]]));
                } else {
                    buf.extend(p);
                }
                while buf.len() >= chunk {
                    let mut m = Vec::with_capacity(1 + chunk * 2);
                    m.push(0x05);
                    for s in buf.drain(..chunk) {
                        m.extend(s.to_le_bytes());
                    }
                    q.push(m.into());
                }
            }
            Sound::Pcm(_) => {}
            Sound::Fail(why) => {
                log::warn!("remote: sound capture unavailable: {why}");
                on.store(false, SeqCst);
                let m = json!({ "t": "audio", "on": false, "err": why });
                let _ = out.blocking_send(ctl_msg(&m));
            }
        },
    );
}

/// Starts the sound thread; storing false in the returned flag stops it.
pub fn start(
    desk: Arc<dyn Desk>,
    alive: Arc<AtomicBool>,
    out: mpsc::Sender<Message>,
    q: Arc<AudioQ>,
) -> Arc<AtomicBool> {
    let on = Arc::new(AtomicBool::new(true));
    let flag = on.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("remote-audio".into())
        .spawn(move || run(&*desk, &flag, &alive, &out, &q))
    {
        log::error!("remote: sound thread failed to start: {e}");
    }
    on
}

/// Waits `d` in small steps while `run()` holds.
#[cfg(windows)]
fn nap(run: &dyn Fn() -> bool, d: std::time::Duration) {
    let until = std::time::Instant::now() + d;
    while run() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// WASAPI loopback of the default output device (cpal: an input stream on an output
/// device). Rebuilt when the default device changes or the stream fails.
#[cfg(windows)]
pub fn loopback(run: &dyn Fn() -> bool, sink: &mut dyn FnMut(Sound)) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::SampleFormat;
    use std::time::{Duration, Instant};
    let host = cpal::default_host();
    while run() {
        let Some(dev) = host.default_output_device() else {
            return sink(Sound::Fail("That computer has no sound output device."));
        };
        let id = dev.id().ok();
        let mut unsupported = false;
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<i16>>(64);
        let failed = Arc::new(AtomicBool::new(false));
        let built = dev
            .default_output_config()
            .map_err(|e| e.to_string())
            .and_then(|cfg| {
                let sc = cfg.config();
                let f = failed.clone();
                let on_err = move |e| {
                    log::warn!("remote: sound capture stopped: {e}");
                    f.store(true, SeqCst);
                };
                // a full channel (a stalled session) drops sound, never blocks the device
                let s = match cfg.sample_format() {
                    SampleFormat::F32 => dev.build_input_stream(
                        sc,
                        move |d: &[f32], _: &_| {
                            let _ = tx.try_send(d.iter().map(|&s| f32_to_i16(s)).collect());
                        },
                        on_err,
                        None,
                    ),
                    SampleFormat::I16 => dev.build_input_stream(
                        sc,
                        move |d: &[i16], _: &_| {
                            let _ = tx.try_send(d.to_vec());
                        },
                        on_err,
                        None,
                    ),
                    SampleFormat::I32 => dev.build_input_stream(
                        sc,
                        move |d: &[i32], _: &_| {
                            let _ = tx.try_send(d.iter().map(|&s| i32_to_i16(s)).collect());
                        },
                        on_err,
                        None,
                    ),
                    f => {
                        unsupported = true;
                        return Err(format!("unsupported sample format {f:?}"));
                    }
                }
                .map_err(|e| e.to_string())?;
                s.play().map_err(|e| e.to_string())?;
                Ok((s, cfg.sample_rate(), cfg.channels()))
            });
        let (stream, rate, ch) = match built {
            Ok(b) => b,
            // logged once by `run`
            Err(_) if unsupported => {
                return sink(Sound::Fail(
                    "That computer's sound device uses a format that cannot be sent.",
                ));
            }
            Err(e) => {
                log::warn!("remote: sound capture failed: {e}");
                nap(run, Duration::from_secs(2));
                continue;
            }
        };
        sink(Sound::Format(rate, ch));
        let mut checked = Instant::now();
        while run() && !failed.load(SeqCst) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(p) => sink(Sound::Pcm(p)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
            if checked.elapsed() >= Duration::from_secs(1) {
                checked = Instant::now();
                if host.default_output_device().and_then(|d| d.id().ok()) != id {
                    break; // another output device: follow it
                }
            }
        }
        drop(stream);
        if failed.load(SeqCst) {
            nap(run, Duration::from_secs(1)); // no tight loop on a device that keeps failing
        }
    }
}
