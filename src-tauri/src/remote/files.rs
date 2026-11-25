//! `files` cap (docs/remote-protocol.md): browse folders, send files to the viewer, take
//! uploads into the signed-in user's Downloads folder. File IO runs on blocking threads;
//! every message leaves through the session's writer task (`bulk` for downloads, so file
//! chunks never crowd out control messages, sound or frames).

use super::{ctl_msg, lock};
use axum::body::Bytes;
use axum::extract::ws::Message;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Largest data part of one `0x04` / `0x10` message.
pub const CHUNK: usize = 256 * 1024;
/// Bytes a download may have unacknowledged (and an upload may run ahead).
const WINDOW: u64 = 4 * 1024 * 1024;
/// An upload reports `got` at least this often.
const GOT_EVERY: u64 = 1024 * 1024;
pub const MAX_SIZE: u64 = 4 * 1024 * 1024 * 1024;
const MAX_XFERS: usize = 2;
const MAX_ENTRIES: usize = 2000;
/// UTF-8 bytes kept of an uploaded file's name (extension included). File systems cap a name
/// at 255 bytes (Linux, macOS) or 255 UTF-16 units (Windows, never more than the UTF-8 bytes),
/// so this leaves room for " (n)" and ".part".
const NAME_MAX: usize = 150;
const LOG_FILES: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// From this computer to the viewer.
    Sent,
    /// From the viewer into Downloads.
    Received,
}

/// One transfer in the session log (written when it starts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileLog {
    pub name: String,
    pub size: u64,
    pub dir: Dir,
}

enum Up {
    Data(Bytes),
    End,
}

struct Xfer {
    cancel: Arc<AtomicBool>,
    /// Download: bytes the viewer acknowledged.
    acked: Arc<AtomicU64>,
    /// Upload: its chunks, and the bytes queued for it.
    up: Option<(mpsc::UnboundedSender<Up>, Arc<AtomicU64>)>,
}

/// A session's transfers.
pub struct Files {
    xfers: Arc<Mutex<HashMap<u32, Xfer>>>,
    pub log: Arc<Mutex<Vec<FileLog>>>,
    /// Control messages (listings, upload progress).
    out: mpsc::Sender<Message>,
    /// Download header, chunks and end, in order.
    bulk: mpsc::Sender<Message>,
    downloads: Option<PathBuf>,
}

fn file_msg(id: u32, v: Value) -> Message {
    let mut m = json!({ "t": "file", "id": id });
    if let (Some(m), Value::Object(v)) = (m.as_object_mut(), v) {
        m.extend(v);
    }
    ctl_msg(&m)
}

/// Session log entry; at most `LOG_FILES` per session (`remote.json` stays small).
fn note(log: &Mutex<Vec<FileLog>>, f: FileLog) {
    let mut l = lock(log);
    if l.len() < LOG_FILES {
        l.push(f);
    }
}

/// A full path on this computer. Network (`\\host\share`) and device (`\\.\`) paths are
/// refused: opening one would make this PC sign in to whatever server the viewer names.
/// On Windows only drive paths (`C:\`, `\\?\C:\`) pass: any other verbatim form, such as
/// `\\?\GLOBALROOT\Device\Mup\...`, can reach the network or a device.
pub fn local_abs(p: &Path) -> bool {
    use std::path::{Component, Prefix};
    p.is_absolute()
        && match p.components().next() {
            Some(Component::Prefix(x)) => {
                matches!(x.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
            }
            _ => true,
        }
}

fn io_err(e: &std::io::Error) -> String {
    match e.kind() {
        ErrorKind::NotFound => "Not found.".into(),
        ErrorKind::PermissionDenied => "Access denied.".into(),
        ErrorKind::NotADirectory => "That is not a folder.".into(),
        ErrorKind::StorageFull => "The disk is full.".into(),
        _ => e.to_string(),
    }
}

/// Windows device names, refused on every OS (a file may move to Windows later).
fn reserved(stem: &str) -> bool {
    let s = stem.trim_end_matches([' ', '.']).to_ascii_uppercase();
    matches!(
        s.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || (s.len() == 4
        && (s.starts_with("COM") || s.starts_with("LPT"))
        && s.as_bytes()[3].is_ascii_digit())
        || ((s.starts_with("COM") || s.starts_with("LPT"))
            && matches!(s.get(3..), Some("¹" | "²" | "³")))
}

/// (stem, ".ext"); a leading dot (`.bashrc`) is not an extension.
fn split_ext(s: &str) -> (&str, &str) {
    match s.rfind('.') {
        Some(i) if i > 0 && s.len() - i <= 16 => s.split_at(i),
        _ => (s, ""),
    }
}

/// An uploaded file's name as saved: base name only, no separators or characters Windows
/// refuses, no device names, no trailing dots or spaces, at most `NAME_MAX` bytes.
pub fn safe_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let s: String = base
        .chars()
        .filter(|c| !c.is_control() && !super::invisible(*c) && !"<>:\"|?*".contains(*c))
        .collect();
    let s = s.trim().trim_end_matches(['.', ' ']);
    let (stem, ext) = split_ext(s);
    let mut keep = NAME_MAX.saturating_sub(ext.len()).min(stem.len());
    while !stem.is_char_boundary(keep) {
        keep -= 1;
    }
    let stem = &stem[..keep];
    let stem = stem.trim_end_matches(['.', ' ']);
    let first = stem.split('.').next().unwrap_or_default();
    let name = if reserved(first) {
        format!("_{stem}{ext}")
    } else {
        format!("{stem}{ext}")
    };
    if name.trim_matches(['.', ' ']).is_empty() {
        "file".into()
    } else {
        name
    }
}

/// `p` + `.part`: an upload until it is complete.
pub fn part_of(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".part");
    s.into()
}

/// Creates `name` in `dir`, or `name (1)`, `name (2)` ... if taken. `create_new` never
/// overwrites and never follows a link or reparse point already at that name. `part`:
/// creates `<free name>.part` instead (the name itself also free); returns the name.
pub fn create_unique(dir: &Path, name: &str, part: bool) -> Result<(File, PathBuf), String> {
    let (stem, ext) = split_ext(name);
    for n in 0..1000u32 {
        let cand = if n == 0 {
            name.to_owned()
        } else {
            format!("{stem} ({n}){ext}")
        };
        let p = dir.join(cand);
        if part && fs::symlink_metadata(&p).is_ok() {
            continue;
        }
        let at = if part { part_of(&p) } else { p.clone() };
        match OpenOptions::new().write(true).create_new(true).open(&at) {
            Ok(f) => return Ok((f, p)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_err(&e)),
        }
    }
    Err("Too many files with that name in Downloads.".into())
}

/// The `fs` reply for `path` (`""`: home): folders first, then by name, at most 2000.
pub fn list(path: &str) -> Value {
    let p = if path.is_empty() {
        dirs::home_dir()
    } else {
        Some(PathBuf::from(path))
    };
    let shown = p
        .as_ref()
        .map_or(path.to_owned(), |p| p.to_string_lossy().into_owned());
    let err = |e: String| json!({ "t": "fs", "path": shown, "err": e });
    let Some(p) = p else {
        return err("This computer has no home folder.".into());
    };
    if !local_abs(&p) {
        return err("Use a full path.".into());
    }
    let rd = match fs::read_dir(&p) {
        Ok(rd) => rd,
        Err(e) => return err(io_err(&e)),
    };
    let mut entries: Vec<(bool, String, u64, u64)> = rd
        .flatten()
        .filter_map(|e| {
            // a link shows what it points at; a broken one as itself
            let md = fs::metadata(e.path()).or_else(|_| e.metadata()).ok()?;
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let dir = md.is_dir();
            let name = e.file_name().to_string_lossy().into_owned();
            Some((dir, name, if dir { 0 } else { md.len() }, mtime))
        })
        .collect();
    entries.sort_by_cached_key(|(dir, name, ..)| (!dir, name.to_lowercase()));
    entries.truncate(MAX_ENTRIES);
    let entries: Vec<Value> = entries
        .into_iter()
        .map(|(dir, name, size, mtime)| json!({ "name": name, "dir": dir, "size": size, "mtime": mtime }))
        .collect();
    json!({
        "t": "fs", "path": shown,
        "parent": p.parent().map(|q| q.to_string_lossy().into_owned()),
        "entries": entries,
    })
}

impl Files {
    pub fn new(
        out: mpsc::Sender<Message>,
        bulk: mpsc::Sender<Message>,
        downloads: Option<PathBuf>,
    ) -> Self {
        Files {
            xfers: Arc::default(),
            log: Arc::default(),
            out,
            bulk,
            downloads,
        }
    }

    fn say(&self, m: Message) {
        // a full queue (slow link) must not lose a transfer's end: wait for room off this task,
        // a few seconds at most (the session's own send limit). Each `say` is the last word on
        // one transfer, so their order between transfers does not matter.
        if let Err(mpsc::error::TrySendError::Full(m)) = self.out.try_send(m) {
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                let out = self.out.clone();
                rt.spawn(async move {
                    let _ = tokio::time::timeout(super::SEND_WAIT, out.send(m)).await;
                });
            }
        }
    }

    /// A text message `t` = `fs` or `file`. `refuse`: why new listings and transfers are
    /// refused (view only, switched off); acks, ends and cancels always pass. True when a
    /// listing or transfer started.
    pub fn handle(&self, v: &Value, refuse: Option<&str>) -> bool {
        let op = v["op"].as_str().unwrap_or_default();
        if v["t"] == "fs" {
            let path = v["path"].as_str().unwrap_or_default().to_owned();
            if let Some(why) = refuse {
                self.say(ctl_msg(&json!({ "t": "fs", "path": path, "err": why })));
                return false;
            }
            if op != "list" {
                return false;
            }
            // `bulk`: a large listing gets the frame-class send time, not the control one
            let bulk = self.bulk.clone();
            tokio::task::spawn_blocking(move || {
                let _ = bulk.blocking_send(ctl_msg(&list(&path)));
            });
            return true;
        }
        let Some(id) = v["id"].as_u64().and_then(|i| u32::try_from(i).ok()) else {
            return false;
        };
        let err = |e: &str| {
            self.say(file_msg(id, json!({ "err": e })));
            false
        };
        match op {
            "ack" => {
                if let (Some(x), Some(got)) = (lock(&self.xfers).get(&id), v["got"].as_u64()) {
                    x.acked.fetch_max(got, SeqCst);
                }
                false
            }
            "cancel" => {
                self.cancel(id);
                false
            }
            "end" => {
                let x = lock(&self.xfers);
                if let Some((tx, _)) = x.get(&id).and_then(|x| x.up.as_ref()) {
                    let _ = tx.send(Up::End);
                }
                false
            }
            "get" | "put" => {
                if let Some(why) = refuse {
                    return err(why);
                }
                let mut x = lock(&self.xfers);
                if x.contains_key(&id) {
                    return err("That transfer id is in use.");
                }
                if x.len() >= MAX_XFERS {
                    return err("Two transfers are running already. Wait for one to finish.");
                }
                let cancel = Arc::new(AtomicBool::new(false));
                let acked = Arc::new(AtomicU64::new(0));
                let done = {
                    let (xfers, cancel) = (self.xfers.clone(), cancel.clone());
                    // removes this transfer only (the id may be reused once it is gone)
                    move || {
                        let mut x = lock(&xfers);
                        if x.get(&id).is_some_and(|t| Arc::ptr_eq(&t.cancel, &cancel)) {
                            x.remove(&id);
                        }
                    }
                };
                let log = self.log.clone();
                if op == "get" {
                    let path = v["path"].as_str().unwrap_or_default().to_owned();
                    let (bulk, c, a) = (self.bulk.clone(), cancel.clone(), acked.clone());
                    x.insert(
                        id,
                        Xfer {
                            cancel,
                            acked,
                            up: None,
                        },
                    );
                    tokio::task::spawn_blocking(move || {
                        let r = download(&path, id, &c, &a, &bulk, &log);
                        // the slot is free before the last message: the viewer may start
                        // its next transfer as soon as it reads it
                        done();
                        let last = match r {
                            Ok(()) => json!({ "done": true }),
                            Err(_) if c.load(SeqCst) => return,
                            Err(e) => json!({ "err": e }),
                        };
                        let _ = bulk.blocking_send(file_msg(id, last));
                    });
                } else {
                    let size = v["size"].as_u64().unwrap_or(u64::MAX);
                    if size > MAX_SIZE {
                        return err("Files over 4 GB cannot be sent.");
                    }
                    let name = safe_name(v["name"].as_str().unwrap_or_default());
                    let (tx, rx) = mpsc::unbounded_channel();
                    let queued = Arc::new(AtomicU64::new(0));
                    let (out, dir, q) = (self.out.clone(), self.downloads.clone(), queued.clone());
                    x.insert(
                        id,
                        Xfer {
                            cancel,
                            acked,
                            up: Some((tx, queued)),
                        },
                    );
                    tokio::task::spawn_blocking(move || {
                        let r = upload(dir, &name, size, id, rx, &q, &out, &log);
                        // the slot is free before the last message (see the download)
                        done();
                        match r {
                            Ok((got, shown)) => {
                                let _ = out.blocking_send(file_msg(id, json!({ "got": got })));
                                let _ = out.blocking_send(file_msg(
                                    id,
                                    json!({ "done": true, "path": shown }),
                                ));
                            }
                            Err(Some(e)) => {
                                let _ = out.blocking_send(file_msg(id, json!({ "err": e })));
                            }
                            // a cancel or a dropped session says nothing more
                            Err(None) => {}
                        }
                    });
                }
                true
            }
            _ => false,
        }
    }

    /// A viewer binary message: `0x10`, `u32` BE id, data (at most `CHUNK`, the socket's
    /// limit). Unknown ids are ignored (chunks still in flight after a cancel).
    pub fn chunk(&self, b: &[u8]) {
        let Some((&0x10, rest)) = b.split_first() else {
            return;
        };
        let (Some(id), data) = (rest.get(..4), rest.get(4..).unwrap_or_default()) else {
            return;
        };
        let id = u32::from_be_bytes([id[0], id[1], id[2], id[3]]);
        if data.is_empty() {
            return;
        }
        let (over, gone) = {
            let x = lock(&self.xfers);
            let Some((tx, queued)) = x.get(&id).and_then(|x| x.up.as_ref()) else {
                return;
            };
            // the viewer stays at most 4 MB ahead of `got`, which comes every 1 MB
            let q = queued.fetch_add(data.len() as u64, SeqCst) + data.len() as u64;
            let over = q > WINDOW + 2 * GOT_EVERY;
            // gone: the upload already failed and said why
            (
                over,
                !over && tx.send(Up::Data(Bytes::copy_from_slice(data))).is_err(),
            )
        };
        if over || gone {
            self.cancel(id);
        }
        if over {
            self.say(file_msg(
                id,
                json!({ "err": "The file came in faster than allowed." }),
            ));
        }
    }

    /// Ends every transfer, telling the viewer `why` (file transfer switched off).
    pub fn stop_all(&self, why: &str) {
        for (id, x) in lock(&self.xfers).drain() {
            x.cancel.store(true, SeqCst);
            self.say(file_msg(id, json!({ "err": why })));
        }
    }

    /// Stops a transfer; a partial upload is deleted by its thread.
    pub fn cancel(&self, id: u32) {
        if let Some(x) = lock(&self.xfers).remove(&id) {
            x.cancel.store(true, SeqCst);
        }
    }

    /// Session end: everything stops, partial uploads are deleted.
    pub fn end(&self) {
        for (_, x) in lock(&self.xfers).drain() {
            x.cancel.store(true, SeqCst);
        }
    }
}

fn download(
    path: &str,
    id: u32,
    cancel: &AtomicBool,
    acked: &AtomicU64,
    bulk: &mpsc::Sender<Message>,
    log: &Mutex<Vec<FileLog>>,
) -> Result<(), String> {
    let p = Path::new(path);
    if !local_abs(p) {
        return Err("Use a full path.".into());
    }
    // a folder first: Windows refuses to open one as "access denied"
    if fs::metadata(p).is_ok_and(|m| !m.is_file()) {
        return Err("That is not a file.".into());
    }
    let f = File::open(p).map_err(|e| io_err(&e))?;
    let md = f.metadata().map_err(|e| io_err(&e))?;
    if !md.is_file() {
        return Err("That is not a file.".into());
    }
    let size = md.len();
    if size > MAX_SIZE {
        return Err("Files over 4 GB cannot be sent.".into());
    }
    let name = p
        .file_name()
        .map_or("file".into(), |n| n.to_string_lossy().into_owned());
    note(
        log,
        FileLog {
            name: name.clone(),
            size,
            dir: Dir::Sent,
        },
    );
    let gone = |_| "The session ended.".to_owned();
    let msg = |v| file_msg(id, v);
    bulk.blocking_send(msg(json!({ "name": name, "size": size })))
        .map_err(gone)?;
    // a file that grows meanwhile is sent as it was announced
    let mut f = f.take(size);
    let (mut sent, mut buf) = (0u64, vec![0u8; CHUNK]);
    loop {
        if cancel.load(SeqCst) {
            return Err("Cancelled.".into());
        }
        // ponytail: polled window (10 ms), a condvar if this ever shows up in a profile
        if sent.saturating_sub(acked.load(SeqCst)) >= WINDOW {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let n = f.read(&mut buf).map_err(|e| io_err(&e))?;
        if n == 0 {
            break;
        }
        let mut m = Vec::with_capacity(5 + n);
        m.push(0x04);
        m.extend(id.to_be_bytes());
        m.extend(&buf[..n]);
        bulk.blocking_send(Message::Binary(m.into()))
            .map_err(gone)?;
        sent += n as u64;
    }
    if sent < size {
        return Err("The file got shorter while it was being sent.".into());
    }
    // the caller sends `done` once the slot is free
    Ok(())
}

/// `Err(None)`: cancelled or the session ended (nothing to say). The partial file is
/// deleted on every error before the last byte (a complete one is kept). `Ok`: (bytes received, saved path) for the final `got` and
/// `done` the caller sends once the slot is free.
#[allow(clippy::too_many_arguments)]
fn upload(
    dir: Option<PathBuf>,
    name: &str,
    size: u64,
    id: u32,
    mut rx: mpsc::UnboundedReceiver<Up>,
    queued: &AtomicU64,
    out: &mpsc::Sender<Message>,
    log: &Mutex<Vec<FileLog>>,
) -> Result<(u64, String), Option<String>> {
    let dir = dir.ok_or_else(|| Some("This computer has no Downloads folder.".to_owned()))?;
    fs::create_dir_all(&dir).map_err(|e| Some(io_err(&e)))?;
    // written as `<name>.part`: a quit or crash never leaves a partial file under the real name
    let (mut f, path) = create_unique(&dir, name, true).map_err(Some)?;
    let part = part_of(&path);
    let file_name = |p: &Path| {
        p.file_name()
            .map_or(name.into(), |n| n.to_string_lossy().into_owned())
    };
    let logged = file_name(&path);
    note(
        log,
        FileLog {
            name: logged.clone(),
            size,
            dir: Dir::Received,
        },
    );
    let (mut got, mut told) = (0u64, 0u64);
    let r = loop {
        match rx.blocking_recv() {
            Some(Up::Data(b)) => {
                queued.fetch_sub(b.len() as u64, SeqCst);
                got += b.len() as u64;
                if got > size {
                    break Err(Some("More data arrived than the file's size.".into()));
                }
                if let Err(e) = f.write_all(&b) {
                    break Err(Some(io_err(&e)));
                }
                if got - told >= GOT_EVERY {
                    told = got;
                    let _ = out.blocking_send(file_msg(id, json!({ "got": got })));
                }
            }
            Some(Up::End) if got == size => break f.sync_all().map_err(|e| Some(io_err(&e))),
            Some(Up::End) => break Err(Some("The file arrived incomplete.".into())),
            None => break Err(None),
        }
    };
    drop(f);
    if let Err(e) = r {
        if let Err(re) = fs::remove_file(&part) {
            log::warn!("remote: could not delete a partial upload: {re}");
        }
        return Err(e);
    }
    // the name may have been taken meanwhile: claim a free one (never overwrites), then
    // move the finished file onto that empty claim. Complete: a failure here keeps the
    // `.part` (a scanner holding the claim must not cost the whole transfer).
    let kept = |e: String| Some(format!("{e} The file was kept as {}.", part.display()));
    let (claim, path) = create_unique(&dir, name, false).map_err(kept)?;
    drop(claim);
    if let Err(e) = fs::rename(&part, &path) {
        let _ = fs::remove_file(&path);
        return Err(kept(io_err(&e)));
    }
    let final_name = file_name(&path);
    if final_name != logged {
        let mut l = lock(log);
        if let Some(e) = l
            .iter_mut()
            .rev()
            .find(|e| e.dir == Dir::Received && e.name == logged)
        {
            e.name = final_name;
        }
    }
    Ok((got, path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xfer(tx: mpsc::UnboundedSender<Up>, queued: u64) -> Xfer {
        Xfer {
            cancel: Arc::default(),
            acked: Arc::default(),
            up: Some((tx, Arc::new(AtomicU64::new(queued)))),
        }
    }

    #[test]
    fn only_an_overrun_is_called_too_fast() {
        let (out, mut said) = mpsc::channel(8);
        let (bulk, _bulk) = mpsc::channel(8);
        let f = Files::new(out, bulk, None);
        // the upload already failed (and said why): its chunks only cancel
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        lock(&f.xfers).insert(1, xfer(tx, 0));
        f.chunk(&[0x10, 0, 0, 0, 1, 7]);
        assert!(lock(&f.xfers).is_empty());
        assert!(said.try_recv().is_err(), "nothing said");
        // past the window
        let (tx, _rx) = mpsc::unbounded_channel();
        lock(&f.xfers).insert(2, xfer(tx, WINDOW + 2 * GOT_EVERY));
        f.chunk(&[0x10, 0, 0, 0, 2, 7]);
        assert!(lock(&f.xfers).is_empty());
        assert!(said.try_recv().is_ok(), "the overrun error");
    }
}
