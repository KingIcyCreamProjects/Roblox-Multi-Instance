//! `ClientSettings/ClientAppSettings.json` writer (spec §6.5): merges the user's custom flags.
//! Roblox only honours allowlisted flags and `DFIntTaskSchedulerTargetFps` is not one, so FPS
//! now goes through `basicsettings`; launches pass `fps: None`, which clears our old flag.

use serde_json::{Map, Value};
use std::{io, path::Path};

pub const FPS_FLAG: &str = "DFIntTaskSchedulerTargetFps";

/// Serialises writers (two launches can overlap).
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Written before every vanilla launch: drop our FPS flag, merge `custom` (top-level keys),
/// then set the flag if a cap applies (so a removed cap doesn't linger). Other keys are kept;
/// an unreadable or non-object file is backed up to `.bak`.
pub fn write_fps(version_dir: &Path, fps: Option<u32>, custom: Option<&Value>) -> io::Result<()> {
    let _g = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = version_dir.join("ClientSettings");
    let path = dir.join("ClientAppSettings.json");
    // What is on disk now (`None`: unreadable, must be replaced).
    let mut orig = Some(Map::new());
    let mut cur = match std::fs::read(&path) {
        Ok(b) => match serde_json::from_slice::<Value>(&b) {
            Ok(Value::Object(m)) => {
                orig = Some(m.clone());
                m
            }
            _ => {
                orig = None;
                // Keep the first backup: a later bad file must not overwrite the user's one.
                let bak = path.with_extension("json.bak");
                if !bak.exists() {
                    std::fs::write(&bak, &b)?;
                }
                log::warn!(
                    "{} was not a JSON object; backed up to .bak",
                    path.display()
                );
                Map::new()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e),
    };
    cur.remove(FPS_FLAG);
    // Keys merged from the custom file last time go first, so a flag removed from it (or
    // the whole file cleared) leaves Roblox's file too, or gets back the user's own value
    // it overwrote, unless the user changed it since RMI wrote it.
    let keys_path = dir.join(MERGED_KEYS);
    let (prev, wrote) = read_keys(&keys_path)?;
    for (k, v) in &prev {
        let disk = orig.as_ref().and_then(|m| m.get(k));
        if wrote.as_ref().is_some_and(|w| disk != w.get(k)) {
            continue;
        }
        match v {
            Value::Null => cur.remove(k),
            v => cur.insert(k.clone(), v.clone()),
        };
    }
    let mut merged = Map::new();
    match custom {
        Some(Value::Object(c)) => {
            merged = c
                .keys()
                .map(|k| (k.clone(), cur.get(k).cloned().unwrap_or(Value::Null)))
                .collect();
            cur.extend(c.clone());
        }
        Some(_) => log::warn!("custom client settings are not a JSON object; ignored"),
        None => {}
    }
    if let Some(f) = fps {
        cur.insert(FPS_FLAG.into(), f.into());
    }
    let now: Map<String, Value> = merged
        .keys()
        .map(|k| (k.clone(), cur.get(k).cloned().unwrap_or(Value::Null)))
        .collect();
    // Nothing changes: no write (macOS blocks writes into the signed Roblox.app bundle).
    if orig.as_ref() == Some(&cur) && prev == merged && wrote.as_ref() == Some(&now) {
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    // The restore record goes first, holding the old entries too: a failure from here on
    // still knows the user's values.
    let mut pre = merged.clone();
    pre.extend(prev.clone());
    let pre_w = wrote.clone().map(|w| {
        let mut m = now.clone();
        m.extend(w);
        m
    });
    if (&pre, &pre_w) != (&prev, &wrote) {
        crate::settings::save(&keys_path, &keys_json(pre, pre_w))?;
    }
    crate::settings::save(&path, &Value::Object(cur))?;
    crate::settings::save(&keys_path, &keys_json(merged, Some(now)))
}

/// Next to `ClientAppSettings.json`: `{"orig": custom key -> the value it replaced (null:
/// none), "wrote": key -> the value RMI wrote}`. Older versions stored the orig map alone, or
/// a plain key list; what RMI wrote is then unknown (`None`) and restored unconditionally.
const MERGED_KEYS: &str = "RMIMergedKeys.json";

type Keys = (Map<String, Value>, Option<Map<String, Value>>);

/// Missing = nothing merged. Unreadable or invalid is an error, never "empty": that would
/// leave merged flags in Roblox's file as if they were the user's.
fn read_keys(path: &Path) -> io::Result<Keys> {
    let b = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Map::new(), Some(Map::new()))),
        Err(e) => return Err(e),
    };
    let v: Value = serde_json::from_slice(&b).map_err(io::Error::other)?;
    if let (Some(Value::Object(o)), Some(Value::Object(w)), Some(2)) =
        (v.get("orig"), v.get("wrote"), v.as_object().map(Map::len))
    {
        return Ok((o.clone(), Some(w.clone())));
    }
    match v {
        Value::Object(p) => Ok((p, None)),
        Value::Array(p) => Ok((
            p.iter()
                .filter_map(Value::as_str)
                .map(|k| (k.to_owned(), Value::Null))
                .collect(),
            None,
        )),
        _ => Err(io::Error::other(format!(
            "{MERGED_KEYS} is not a JSON object"
        ))),
    }
}

fn keys_json(orig: Map<String, Value>, wrote: Option<Map<String, Value>>) -> Value {
    match wrote {
        Some(w) => serde_json::json!({ "orig": orig, "wrote": w }),
        None => Value::Object(orig),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(d: &Path) -> std::path::PathBuf {
        d.join("ClientSettings").join("ClientAppSettings.json")
    }

    fn read(d: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(file(d)).unwrap()).unwrap()
    }

    #[test]
    fn missing_file_created() {
        let d = tempfile::tempdir().unwrap();
        write_fps(d.path(), Some(144), None).unwrap();
        assert_eq!(read(d.path()), json!({ FPS_FLAG: 144 }));
    }

    #[test]
    fn nothing_to_change_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        write_fps(d.path(), None, None).unwrap();
        assert!(!d.path().join("ClientSettings").exists());
        write_fps(d.path(), None, Some(&json!({ "K": 1 }))).unwrap();
        let p = file(d.path());
        std::fs::remove_file(d.path().join("ClientSettings").join(MERGED_KEYS)).unwrap();
        std::fs::write(&p, r#"{"K":1}"#).unwrap();
        // Same content as on disk: the file is not rewritten (a save would reformat it).
        write_fps(d.path(), None, None).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"{"K":1}"#);
    }

    #[test]
    fn merge_preserves_keys_and_fps_wins_over_custom() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(file(d.path()).parent().unwrap()).unwrap();
        std::fs::write(
            file(d.path()),
            r#"{"Keep":1,"DFIntTaskSchedulerTargetFps":60}"#,
        )
        .unwrap();
        let custom = json!({ "Custom": "x", FPS_FLAG: 30 });
        write_fps(d.path(), Some(240), Some(&custom)).unwrap();
        assert_eq!(
            read(d.path()),
            json!({ "Keep": 1, "Custom": "x", FPS_FLAG: 240 })
        );
    }

    #[test]
    fn no_cap_removes_key_but_custom_may_set_it() {
        let d = tempfile::tempdir().unwrap();
        write_fps(d.path(), Some(240), None).unwrap();
        write_fps(d.path(), None, Some(&json!({ "K": 1 }))).unwrap();
        assert_eq!(read(d.path()), json!({ "K": 1 }));
        write_fps(d.path(), None, Some(&json!({ "K": 1, FPS_FLAG: 30 }))).unwrap();
        assert_eq!(read(d.path()), json!({ "K": 1, FPS_FLAG: 30 }));
    }

    #[test]
    fn removed_custom_keys_leave_the_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(file(d.path()).parent().unwrap()).unwrap();
        std::fs::write(file(d.path()), r#"{"Keep":1}"#).unwrap();
        write_fps(d.path(), None, Some(&json!({ "Foo": true, "Bar": 1 }))).unwrap();
        write_fps(d.path(), None, Some(&json!({ "Bar": 2 }))).unwrap();
        assert_eq!(read(d.path()), json!({ "Keep": 1, "Bar": 2 }));
        write_fps(d.path(), None, None).unwrap();
        assert_eq!(read(d.path()), json!({ "Keep": 1 }));
    }

    #[test]
    fn users_own_value_comes_back_when_custom_drops_the_key() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(file(d.path()).parent().unwrap()).unwrap();
        std::fs::write(file(d.path()), r#"{"X":true}"#).unwrap();
        write_fps(d.path(), None, Some(&json!({ "X": false }))).unwrap();
        write_fps(d.path(), None, Some(&json!({ "X": 2 }))).unwrap();
        assert_eq!(read(d.path()), json!({ "X": 2 }));
        write_fps(d.path(), None, None).unwrap();
        assert_eq!(read(d.path()), json!({ "X": true }));
        // the old key-list format still removes what it merged
        let keys = d.path().join("ClientSettings").join(MERGED_KEYS);
        write_fps(d.path(), None, Some(&json!({ "Y": 1 }))).unwrap();
        std::fs::write(&keys, r#"["Y"]"#).unwrap();
        write_fps(d.path(), None, None).unwrap();
        assert_eq!(read(d.path()), json!({ "X": true }));
    }

    #[test]
    fn user_edit_after_override_is_kept_and_bad_record_stops() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(file(d.path()).parent().unwrap()).unwrap();
        std::fs::write(file(d.path()), r#"{"X":"A"}"#).unwrap();
        write_fps(d.path(), None, Some(&json!({ "X": "B" }))).unwrap();
        std::fs::write(file(d.path()), r#"{"X":"C"}"#).unwrap();
        write_fps(d.path(), None, None).unwrap();
        assert_eq!(read(d.path()), json!({ "X": "C" }));
        // an unreadable record is not "nothing merged": Roblox's file is left alone
        let keys = d.path().join("ClientSettings").join(MERGED_KEYS);
        std::fs::write(&keys, "{bad").unwrap();
        assert!(write_fps(d.path(), None, Some(&json!({ "X": "D" }))).is_err());
        assert_eq!(read(d.path()), json!({ "X": "C" }));
    }

    #[test]
    fn invalid_json_backed_up_then_replaced() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(file(d.path()).parent().unwrap()).unwrap();
        std::fs::write(file(d.path()), "{not json").unwrap();
        write_fps(d.path(), Some(90), None).unwrap();
        assert_eq!(read(d.path()), json!({ FPS_FLAG: 90 }));
        let bak = file(d.path()).with_extension("json.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "{not json");
        // a later bad file keeps the first backup
        std::fs::write(file(d.path()), "[]").unwrap();
        write_fps(d.path(), None, None).unwrap();
        assert_eq!(std::fs::read_to_string(bak).unwrap(), "{not json");
    }

    #[test]
    fn non_object_custom_ignored_and_unreadable_file_is_error() {
        let d = tempfile::tempdir().unwrap();
        write_fps(d.path(), Some(90), Some(&json!([1, 2]))).unwrap();
        assert_eq!(read(d.path()), json!({ FPS_FLAG: 90 }));
        // a directory where the file should be: read fails, nothing is replaced
        let d2 = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(file(d2.path())).unwrap();
        assert!(write_fps(d2.path(), Some(90), None).is_err());
        assert!(file(d2.path()).is_dir());
    }
}
