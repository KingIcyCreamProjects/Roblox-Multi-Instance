//! Roblox's own user settings file, `GlobalBasicSettings_<n>.xml`: the in-game Graphics
//! Quality and Maximum Frame Rate. Every client reads it once at start and writes it back on
//! exit, so rewriting it right before each launch starts that client with our values.
//! (The FPS FFlag is off Roblox's FFlag allowlist since 2025-09-29; this file is the route.)

use regex::Regex;
use std::{io, path::PathBuf};

/// Tests never touch the real Roblox file.
#[cfg(test)]
fn dirs_to_try() -> Vec<PathBuf> {
    vec![]
}

/// Folders Roblox keeps the file in. macOS paths are from community docs, not yet seen live.
#[cfg(not(test))]
fn dirs_to_try() -> Vec<PathBuf> {
    if cfg!(windows) {
        dirs::data_local_dir()
            .map(|d| d.join("Roblox"))
            .into_iter()
            .collect()
    } else {
        let Some(h) = dirs::home_dir() else {
            return vec![];
        };
        [
            "Library/Roblox",
            "Library/Preferences/Roblox",
            "Library/Application Support/Roblox",
        ]
        .iter()
        .map(|p| h.join(p))
        .collect()
    }
}

/// The newest `GlobalBasicSettings_<n>.xml`. `None` until Roblox has run once.
fn find() -> Option<PathBuf> {
    let re = Regex::new(r"^GlobalBasicSettings_(\d+)\.xml$").unwrap();
    dirs_to_try()
        .into_iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let n: u32 = re.captures(&name)?[1].parse().ok()?;
            Some((n, e.path()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
}

const QUALITY: (&str, &str) = ("token", "SavedQualityLevel");
const FPS: (&str, &str) = ("int", "FramerateCap");

/// Per key (quality, fps): `(user's value before RMI's override, raw text RMI wrote)`;
/// inner `None` = the key was absent. Restored when a later launch has no override, unless
/// the file no longer holds RMI's value (the user changed it in game). Kept in the data dir
/// so a restart doesn't lose it.
type Orig = [Option<(Option<String>, String)>; 2];
const ORIG_FILE: &str = "basic-settings-orig.json";
/// Serializes writes.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Raw text of `<tag name="key">…</tag>`.
fn get(xml: &str, (tag, key): (&str, &str)) -> Option<String> {
    let re = Regex::new(&format!(r#"<{tag} name="{key}">([^<]*)</{tag}>"#)).unwrap();
    re.captures(xml).map(|c| c[1].to_owned())
}

/// Set `<tag name="key">value</tag>`, adding it to the first `<Properties>` when missing.
fn set(xml: &str, tag: &str, key: &str, value: impl std::fmt::Display) -> String {
    let re = Regex::new(&format!(r#"<{tag} name="{key}">[^<]*</{tag}>"#)).unwrap();
    let el = format!(r#"<{tag} name="{key}">{value}</{tag}>"#);
    if re.is_match(xml) {
        re.replace(xml, regex::NoExpand(&el)).into_owned()
    } else {
        xml.replacen("</Properties>", &format!("\t{el}\n\t\t</Properties>"), 1)
    }
}

/// Graphics level 1..=10 (`SavedQualityLevel`) and FPS cap (`FramerateCap`); `None` leaves
/// Roblox's own value. A missing file (Roblox never ran) is not an error.
pub fn apply(xml: &str, quality: Option<u8>, fps: Option<u32>) -> String {
    let mut x = xml.to_owned();
    if let Some(q) = quality {
        x = set(&x, "token", "SavedQualityLevel", u32::from(q.clamp(1, 10)));
    }
    if let Some(f) = fps {
        x = set(&x, "int", "FramerateCap", f.clamp(1, 240));
    }
    x
}

/// `apply`, plus: remember the user's value before an override of each key, and put it back
/// (or remove the key, if it was absent) for a key with no override (an earlier per-account
/// value would linger otherwise).
fn apply_restoring(xml: &str, orig: &mut Orig, quality: Option<u8>, fps: Option<u32>) -> String {
    let mut x = apply(xml, quality, fps);
    let [q, f] = orig;
    for (slot, k, over) in [(q, QUALITY, quality.is_some()), (f, FPS, fps.is_some())] {
        let cur = get(xml, k);
        // The file holds the user's value unless it is still the one RMI wrote.
        let prev = slot.take();
        let user = match &prev {
            Some((o, wrote)) if cur.as_deref() == Some(wrote.as_str()) => o.clone(),
            _ => cur.clone(),
        };
        if over {
            *slot = get(&x, k).map(|w| (user, w));
        } else {
            // Restored from RMI's value: kept, so a still-running override client writing
            // that value back on exit is recognized again on the next launch.
            *slot = prev.filter(|(_, w)| cur.as_deref() == Some(w.as_str()));
            x = match user {
                Some(v) => set(&x, k.0, k.1, v.as_str()),
                None => {
                    let re = Regex::new(&format!(r#"\s*<{0} name="{1}">[^<]*</{0}>"#, k.0, k.1))
                        .unwrap();
                    re.replace(&x, "").into_owned()
                }
            };
        }
    }
    x
}

pub fn write(root: &std::path::Path, quality: Option<u8>, fps: Option<u32>) -> io::Result<()> {
    let _g = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let orig_path = root.join(ORIG_FILE);
    let mut orig: Orig = std::fs::read(&orig_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if quality.is_none() && fps.is_none() && orig.iter().all(Option::is_none) {
        return Ok(());
    }
    let Some(path) = find() else {
        log::info!("no GlobalBasicSettings file yet; graphics and FPS left to Roblox");
        return Ok(());
    };
    let xml = std::fs::read_to_string(&path)?;
    let out = apply_restoring(&xml, &mut orig, quality, fps);
    if out == xml && !xml.contains("</Properties>") {
        log::warn!(
            "{} has no <Properties>; graphics and FPS not set",
            path.display()
        );
    }
    if out != xml {
        // tmp + rename: a client starting right now never reads a half-written file
        let tmp = path.with_extension("xml.rmi-tmp");
        std::fs::write(&tmp, out)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        log::info!(
            "set graphics {quality:?} and FPS {fps:?} in {}",
            path.display()
        );
    }
    let v = serde_json::to_value(&orig).map_err(io::Error::other)?;
    crate::settings::save(&orig_path, &v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = "<roblox><Item class=\"UserGameSettings\">\n\t\t<Properties>\n\t\t\t<int name=\"FramerateCap\">240</int>\n\t\t\t<token name=\"SavedQualityLevel\">10</token>\n\t\t</Properties>\n\t</Item></roblox>";

    #[test]
    fn sets_quality_and_fps() {
        let x = apply(XML, Some(1), Some(60));
        assert!(x.contains(r#"<int name="FramerateCap">60</int>"#));
        assert!(x.contains(r#"<token name="SavedQualityLevel">1</token>"#));
        assert!(!x.contains("240") && !x.contains(">10<"));
    }

    #[test]
    fn none_leaves_roblox_value() {
        assert_eq!(apply(XML, None, None), XML);
        assert!(apply(XML, Some(3), None).contains(r#"<int name="FramerateCap">240</int>"#));
    }

    #[test]
    fn missing_key_added_and_values_clamped() {
        let bare = "<roblox><Item><Properties>\n\t\t</Properties></Item></roblox>";
        let x = apply(bare, Some(0), Some(1000));
        assert!(x.contains(r#"<int name="FramerateCap">240</int>"#));
        assert!(x.contains(r#"<token name="SavedQualityLevel">1</token>"#));
        assert_eq!(x.matches("</Properties>").count(), 1);
    }

    #[test]
    fn absent_key_removed_again_and_user_change_kept() {
        let bare = "<roblox><Item><Properties>\n\t\t</Properties></Item></roblox>";
        let mut orig = Orig::default();
        let a = apply_restoring(bare, &mut orig, None, Some(30));
        assert!(a.contains(r#"<int name="FramerateCap">30</int>"#));
        // a second override must not take RMI's own 30 as the user's value
        let b = apply_restoring(&a, &mut orig, None, Some(60));
        let c = apply_restoring(&b, &mut orig, None, None);
        assert!(!c.contains("FramerateCap"), "{c}");
        assert_eq!(orig, [None, Some((None, "60".into()))]);
        // once the file no longer holds RMI's value, the slot is let go
        apply_restoring(&c, &mut orig, None, None);
        assert_eq!(orig, Orig::default());
        // user changed the value in game (file no longer holds RMI's): kept as theirs
        let mut orig = Orig::default();
        let a = apply_restoring(XML, &mut orig, None, Some(30));
        let changed = a.replace(">30<", ">45<");
        let b = apply_restoring(&changed, &mut orig, None, None);
        assert!(b.contains(r#"<int name="FramerateCap">45</int>"#));
    }

    #[test]
    fn none_after_override_restores_user_value() {
        let mut orig = Orig::default();
        let a = apply_restoring(XML, &mut orig, None, Some(30));
        assert!(a.contains(r#"<int name="FramerateCap">30</int>"#));
        // the client wrote 30 back on exit; a later override keeps the first original
        let b = apply_restoring(&a, &mut orig, None, Some(60));
        assert!(b.contains(r#"<int name="FramerateCap">60</int>"#));
        let c = apply_restoring(&b, &mut orig, None, None);
        assert!(c.contains(r#"<int name="FramerateCap">240</int>"#));
        // an override client still running exits after that and writes 60 back
        let d = apply_restoring(&b, &mut orig, None, None);
        assert!(d.contains(r#"<int name="FramerateCap">240</int>"#), "{d}");
        assert_eq!(apply_restoring(XML, &mut Orig::default(), None, None), XML);
    }

    #[test]
    fn quality_restored_and_raw_text_kept_literal() {
        let mut orig = Orig::default();
        let a = apply_restoring(XML, &mut orig, Some(3), None);
        assert!(a.contains(r#"<token name="SavedQualityLevel">3</token>"#));
        let b = apply_restoring(&a, &mut orig, None, None);
        assert!(b.contains(r#"<token name="SavedQualityLevel">10</token>"#));
        // restored raw text goes back as-is: no regex group expansion
        let mut o: Orig = [Some((Some("$1x".into()), "10".into())), None];
        let c = apply_restoring(XML, &mut o, None, None);
        assert!(
            c.contains(r#"<token name="SavedQualityLevel">$1x</token>"#),
            "{c}"
        );
    }

    #[test]
    fn no_properties_block_changes_nothing() {
        assert_eq!(apply("<roblox/>", Some(2), Some(30)), "<roblox/>");
    }
}
