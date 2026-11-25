//! Legacy account-manager importer (spec §11). Read-only on source files, no UI.
//! Format interop only; written from the spec's data description.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use zeroize::Zeroizing;

/// Fixed 64-byte ASCII header of the password-locked encoding (§11.2).
pub const MAGIC: &[u8; 64] = b"Roblox Account Manager created by ic3w0lf22 @ github.com .......";
/// DPAPI optional entropy (§11.2), exactly as written.
#[cfg(windows)]
const ENTROPY: &[u8] = b"ROBLOX ACCOUNT MANAGER | :) | BROUGHT TO YOU BUY ic3w0lf";
/// Cap on any source file / text we parse.
pub const MAX_FILE: u64 = 32 * 1024 * 1024;
/// Cap on accounts in one file.
const MAX_ENTRIES: usize = 5000;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const MAC_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Locked,
    Dpapi,
    Plain,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ImportError {
    #[error("This file is password-locked. Enter the old manager's password.")]
    PasswordRequired,
    #[error("Wrong old password, or the file is damaged.")]
    WrongPassword,
    #[error("The file is truncated or damaged.")]
    Truncated,
    #[error(
        "Could not decrypt this file. It only works on the same Windows user and PC that created it."
    )]
    Dpapi,
    #[error("Only readable on the Windows PC that created it.")]
    DpapiUnsupported,
    #[error("This file is too large to be an account list.")]
    TooLarge,
    /// Coded: the catalog text (src/lib/ipc.ts) states the `MAX_ENTRIES` limit.
    #[error("IMPORT_TOO_MANY")]
    TooMany,
    #[error("Could not read the file.")]
    Unreadable,
    #[error("Not a recognised account file.")]
    Unrecognised,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchTarget {
    pub place_id: Option<u64>,
    pub job_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRect {
    pub x: i64,
    pub y: i64,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Default)]
pub struct ImportedAccount {
    pub user_id: Option<u64>,
    pub username: String,
    pub alias: Option<String>,
    pub note: Option<String>,
    pub group: String,
    pub cookie: Option<Zeroizing<String>>,
    pub password: Option<Zeroizing<String>>,
    pub launch_target: Option<LaunchTarget>,
    pub fps_cap: Option<u32>,
    pub window_rect: Option<WindowRect>,
    pub last_launched_at: Option<String>,
    pub tracker_id: Option<String>,
    pub extra: BTreeMap<String, String>,
}

impl fmt::Debug for ImportedAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mask = |o: &Option<Zeroizing<String>>| if o.is_some() { "<redacted>" } else { "None" };
        f.debug_struct("ImportedAccount")
            .field("user_id", &self.user_id)
            .field("username", &self.username)
            .field("alias", &self.alias)
            .field("group", &self.group)
            .field("cookie", &format_args!("{}", mask(&self.cookie)))
            .field("password", &format_args!("{}", mask(&self.password)))
            .field("launch_target", &self.launch_target)
            .field("fps_cap", &self.fps_cap)
            .field("window_rect", &self.window_rect)
            .field("last_launched_at", &self.last_launched_at)
            .field("tracker_id", &self.tracker_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub struct Detected {
    pub account_data: Option<(PathBuf, Result<Encoding, ImportError>)>,
    pub settings: Option<PathBuf>,
    pub recents: Option<PathBuf>,
    pub favorites: Option<PathBuf>,
}

fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)
}

/// Cheap sniff (spec order): magic header, DPAPI version bytes, leading `[`.
fn sniff(bytes: &[u8]) -> Result<Encoding, ImportError> {
    if bytes.len() as u64 > MAX_FILE {
        Err(ImportError::TooLarge)
    } else if bytes.starts_with(MAGIC) {
        Ok(Encoding::Locked)
    } else if bytes.starts_with(&[1, 0, 0, 0]) {
        Ok(Encoding::Dpapi)
    } else if strip_bom(bytes).trim_ascii_start().first() == Some(&b'[') {
        Ok(Encoding::Plain)
    } else {
        Err(ImportError::Unrecognised)
    }
}

/// Read a source file read-only, never more than `MAX_FILE` bytes.
pub fn read_capped(path: &Path) -> Result<Vec<u8>, ImportError> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE + 1).read_to_end(&mut buf))
        .map_err(|_| ImportError::Unreadable)?;
    if buf.len() as u64 > MAX_FILE {
        return Err(ImportError::TooLarge);
    }
    Ok(buf)
}

/// Enough of a file to sniff it (the size check uses its length, not a full read). Wiped on
/// drop: a plain file starts with cookies.
fn head(path: &Path) -> Result<Zeroizing<Vec<u8>>, ImportError> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|_| ImportError::Unreadable)?;
    if f.metadata().map_err(|_| ImportError::Unreadable)?.len() > MAX_FILE {
        return Err(ImportError::TooLarge);
    }
    let mut buf = Zeroizing::new(Vec::with_capacity(4096));
    f.take(4096)
        .read_to_end(&mut buf)
        .map_err(|_| ImportError::Unreadable)?;
    Ok(buf)
}

/// Look for the known files in `dir` (case-sensitive names, read-only).
/// `account_data` is reported whenever the file exists; its encoding may be an error
/// (too large, unreadable, unrecognised).
pub fn detect(dir: &Path) -> Detected {
    let find = |n: &str| Some(dir.join(n)).filter(|p| p.is_file());
    Detected {
        account_data: find("AccountData.json").map(|p| {
            let enc = head(&p).and_then(|b| sniff(&b));
            (p, enc)
        }),
        settings: find("RAMSettings.ini"),
        recents: find("RecentGames.json"),
        favorites: find("FavoriteGames.json"),
    }
}

/// The accounts, and how many entries were skipped (no username).
pub fn decrypt(
    bytes: &[u8],
    password: Option<&str>,
) -> Result<(Vec<ImportedAccount>, usize), ImportError> {
    let payload: Zeroizing<Vec<u8>> = match sniff(bytes)? {
        Encoding::Locked => unlock(
            &bytes[MAGIC.len()..],
            password.ok_or(ImportError::PasswordRequired)?,
        )?,
        Encoding::Dpapi => dpapi_unprotect(bytes)?,
        Encoding::Plain => Zeroizing::new(strip_bom(bytes).to_vec()),
    };
    // BOM inside a decrypted file too, not only a plain one.
    let mut items: Vec<Value> =
        serde_json::from_slice(strip_bom(&payload)).map_err(|_| ImportError::Unrecognised)?;
    // A real account file holds far fewer; a huge one would make the commit quadratic.
    if items.len() > MAX_ENTRIES {
        items.iter_mut().for_each(crate::accounts::wipe_value);
        return Err(ImportError::TooMany);
    }
    let out: Vec<ImportedAccount> = items.iter().filter_map(map_account).collect();
    let skipped = items.len() - out.len();
    // the parsed copies hold every cookie/password: wipe before drop
    items.iter_mut().for_each(crate::accounts::wipe_value);
    Ok((out, skipped))
}

/// `rest` = salt | nonce | MAC | ciphertext.
fn unlock(rest: &[u8], password: &str) -> Result<Zeroizing<Vec<u8>>, ImportError> {
    use crypto_secretbox::aead::{Aead, KeyInit};
    use crypto_secretbox::{Nonce, XSalsa20Poly1305};
    use sha2::{Digest, Sha512};

    if rest.len() < SALT_LEN + NONCE_LEN + MAC_LEN {
        return Err(ImportError::Truncated);
    }
    let (salt, rest) = rest.split_at(SALT_LEN);
    let (nonce, ct) = rest.split_at(NONCE_LEN);
    let mut pre = Zeroizing::new([0u8; 64]);
    pre.copy_from_slice(&Sha512::digest(password.as_bytes()));
    let mut key = Zeroizing::new([0u8; 32]);
    // libsodium "moderate": t=3, m=256 MiB, p=1, Argon2id v1.3.
    let params =
        argon2::Params::new(262_144, 3, 1, Some(32)).map_err(|_| ImportError::Unrecognised)?;
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(pre.as_slice(), salt, key.as_mut_slice())
        .map_err(|_| ImportError::Unrecognised)?;
    XSalsa20Poly1305::new(key.as_slice().into())
        .decrypt(Nonce::from_slice(nonce), ct)
        .map(Zeroizing::new)
        .map_err(|_| ImportError::WrongPassword)
}

#[cfg(windows)]
fn dpapi_unprotect(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, ImportError> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    if bytes.is_empty() {
        return Err(ImportError::Dpapi);
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let ent = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: blobs point at live slices; API only reads input/entropy.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&ent),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|_| ImportError::Dpapi)?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize).fill(0);
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        Ok(Zeroizing::new(v))
    }
}

#[cfg(not(windows))]
fn dpapi_unprotect(_: &[u8]) -> Result<Zeroizing<Vec<u8>>, ImportError> {
    Err(ImportError::DpapiUnsupported)
}

// ---- mapping (§11.3) ----

fn s(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(x) if !x.is_empty() => Some(x.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn secret(v: &Value, k: &str) -> Option<Zeroizing<String>> {
    s(v, k).map(Zeroizing::new)
}

/// Names from the old file: no control or invisible format characters (bidi overrides,
/// zero-width marks), trimmed, at most `max` characters.
fn clean(s: &str, max: usize) -> String {
    let s: String = s
        .chars()
        .filter(|c| !c.is_control() && !crate::remote::invisible(*c))
        .collect();
    s.trim().chars().take(max).collect()
}

fn map_account(v: &Value) -> Option<ImportedAccount> {
    let username = Some(clean(&s(v, "Username")?, 40)).filter(|u| !u.is_empty())?;
    let mut a = ImportedAccount {
        user_id: s(v, "UserID").and_then(|x| x.parse().ok()),
        username,
        alias: s(v, "Alias").map(|x| clean(&x, 50)),
        note: s(v, "Description").map(|x| x.chars().take(5000).collect()),
        group: s(v, "Group")
            .map(|g| clean(&g, 64))
            .filter(|g| !g.is_empty() && g != "Default")
            .unwrap_or_else(|| "Unsorted".into()),
        // Same shape check as a pasted cookie; a malformed one is dropped.
        cookie: secret(v, "SecurityToken")
            .and_then(|c| crate::accounts::normalize_cookie(&c))
            .map(Zeroizing::new),
        password: secret(v, "Password"),
        last_launched_at: s(v, "LastUse").and_then(|d| parse_date(&d)),
        tracker_id: s(v, "BrowserTrackerID"),
        ..Default::default()
    };
    let mut f: BTreeMap<String, String> = v
        .get("Fields")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| match v {
                    Value::String(x) => Some((k.clone(), x.clone())),
                    Value::Number(n) => Some((k.clone(), n.to_string())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let place = f.remove("SavedPlaceId").and_then(|x| x.parse().ok());
    let job = f.remove("SavedJobId").filter(|x| !x.is_empty());
    // a job id alone is no target: launch ignores it, so don't show it as an override
    if place.is_some() {
        a.launch_target = Some(LaunchTarget {
            place_id: place,
            job_id: job,
        });
    }
    a.fps_cap = f
        .remove("MaxFPS")
        .and_then(|x| x.trim().parse().ok())
        .filter(|n: &u32| (1..=1000).contains(n))
        .map(|n| n.min(240)); // the app's range (the Inspector refuses more)
                              // Window keys: X, Y, Width|W, Height|H. Only a complete, parsable set becomes a rect;
                              // otherwise every Window_* key stays in `extra`.
    let num = |f: &BTreeMap<String, String>, names: &[&str]| {
        names
            .iter()
            .find_map(|n| f.get(*n))
            .and_then(|x| x.trim().parse::<i64>().ok())
    };
    if let (Some(x), Some(y), Some(w), Some(h)) = (
        num(&f, &["Window_X"]),
        num(&f, &["Window_Y"]),
        num(&f, &["Window_Width", "Window_W"]),
        num(&f, &["Window_Height", "Window_H"]),
    ) {
        if let (Ok(w), Ok(h)) = (u32::try_from(w), u32::try_from(h)) {
            for k in [
                "Window_X",
                "Window_Y",
                "Window_Width",
                "Window_W",
                "Window_Height",
                "Window_H",
            ] {
                f.remove(k);
            }
            a.window_rect = Some(WindowRect { x, y, w, h });
        }
    }
    a.extra = f;
    Some(a)
}

/// ISO/RFC3339, naive ISO, or .NET-style "M/d/yyyy h:mm:ss AM" -> UTC ISO string.
fn parse_date(d: &str) -> Option<String> {
    use chrono::{DateTime, NaiveDateTime, SecondsFormat};
    let d = d.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(d) {
        return Some(t.to_utc().to_rfc3339_opts(SecondsFormat::Secs, true));
    }
    [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%m/%d/%Y %I:%M:%S %p",
        "%m/%d/%Y %H:%M:%S",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(d, f).ok())
    .map(|n| n.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true))
}

// ---- RAMSettings.ini (§11.1) ----

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyPrefs {
    pub launch_delay_ms: Option<u64>,
    pub fps_unlock: Option<bool>,
    pub fps_value: Option<u32>,
    pub multi_client: Option<bool>,
    pub recents_max: Option<u32>,
}

/// Heuristics (key names are not specified; case-insensitive substring match, first hit wins):
/// "delay" -> launch delay; seconds (x1000) unless key also contains "ms".
/// "unlock" -> fps_unlock; other "fps" -> fps_value; "multi" -> multi_client;
/// "recent" -> recents_max. Sections, comments and everything else ignored.
pub fn parse_ini(text: &str) -> LegacyPrefs {
    let mut p = LegacyPrefs::default();
    if text.len() as u64 > MAX_FILE {
        return p;
    }
    let b = |v: &str| match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with([';', '#', '[']) {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k.contains("delay") {
            if let Ok(n) = v.parse::<f64>() {
                if n >= 0.0 && p.launch_delay_ms.is_none() {
                    let ms = if k.contains("ms") { n } else { n * 1000.0 };
                    p.launch_delay_ms = Some(ms.round() as u64);
                }
            }
        } else if k.contains("fps") && k.contains("unlock") {
            p.fps_unlock = p.fps_unlock.or_else(|| b(v));
        } else if k.contains("fps") {
            p.fps_value = p.fps_value.or_else(|| v.parse().ok());
        } else if k.contains("multi") {
            p.multi_client = p.multi_client.or_else(|| b(v));
        } else if k.contains("recent") {
            p.recents_max = p.recents_max.or_else(|| v.parse().ok());
        }
    }
    p
}

// ---- RecentGames.json / FavoriteGames.json ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyGame {
    pub place_id: u64,
    pub name: String,
    pub image_url: Option<String>,
}

pub fn parse_games_json(text: &str) -> Vec<LegacyGame> {
    if text.len() as u64 > MAX_FILE {
        return vec![];
    }
    let Ok(Value::Array(items)) = serde_json::from_str(text.trim_start_matches('\u{feff}')) else {
        return vec![];
    };
    let pick = |v: &Value, keys: &[&str]| keys.iter().find_map(|k| s(v, k));
    items
        .iter()
        .filter_map(|v| {
            Some(LegacyGame {
                place_id: pick(v, &["PlaceId", "placeId", "Id", "id"])?.parse().ok()?,
                name: pick(v, &["Name", "name"]).unwrap_or_default(),
                image_url: pick(v, &["ImageUrl", "imageUrl", "Thumbnail", "thumbnail"]),
            })
        })
        .collect()
}

pub mod cmds;

#[cfg(test)]
mod tests;
