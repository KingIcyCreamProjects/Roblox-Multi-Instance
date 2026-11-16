//! Recent games in `library.json` (no secrets; docs/contracts/discover.md `Recent`).
//! Other top-level keys (favorites, discover/favorites.rs) round-trip untouched.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{path::Path, sync::Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub place_id: u64,
    pub name: String,
    pub thumbnail: Option<String>,
    pub last_played_at: String,
}

/// Serialises read-modify-write of the file.
static LOCK: Mutex<()> = Mutex::new(());

/// Newest first by `last_played_at` (ISO-8601, compared as strings), one per placeId, at
/// most `max`. Pushes finish out of launch order, so an older one never jumps ahead or
/// replaces a newer entry for its place.
pub fn push_into(list: &mut Vec<Recent>, r: Recent, max: usize) {
    if list
        .iter()
        .any(|x| x.place_id == r.place_id && x.last_played_at > r.last_played_at)
    {
        return;
    }
    list.retain(|x| x.place_id != r.place_id);
    let i = list
        .iter()
        .position(|x| x.last_played_at <= r.last_played_at)
        .unwrap_or(list.len());
    list.insert(i, r);
    list.truncate(max);
}

/// Missing = empty. Any other read error is returned so an edit never overwrites keys it
/// couldn't see. A non-object file is only replaced once its `.bak` copy is written.
fn read(path: &Path) -> std::io::Result<Map<String, Value>> {
    match std::fs::read(path) {
        Ok(b) => match serde_json::from_slice::<Value>(&b) {
            Ok(Value::Object(m)) => Ok(m),
            _ => {
                // Keep the first backup: every read of a still-bad file must not rewrite it.
                let bak = path.with_extension("json.bak");
                if !bak.exists() {
                    std::fs::write(&bak, &b)?;
                }
                log::warn!(
                    "{} was not a JSON object; backed up to .bak",
                    path.display()
                );
                Ok(Map::new())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e),
    }
}

/// Missing key = empty. A present key that doesn't decode is InvalidData, so an edit never
/// writes `[]` over entries it couldn't read.
fn list_of<T: DeserializeOwned>(m: &Map<String, Value>, key: &str) -> std::io::Result<Vec<T>> {
    m.get(key).map_or(Ok(vec![]), |v| {
        serde_json::from_value(v.clone())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })
}

/// Read-modify-write one list key of the file; every other key round-trips untouched.
pub(crate) fn edit_key<T: Serialize + DeserializeOwned, R>(
    path: &Path,
    key: &str,
    f: impl FnOnce(&mut Vec<T>) -> R,
) -> std::io::Result<R> {
    let _g = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut m = read(path)?;
    let mut v = list_of(&m, key)?;
    let r = f(&mut v);
    m.insert(key.into(), serde_json::to_value(v)?);
    crate::settings::save(path, &Value::Object(m))?;
    Ok(r)
}

pub(crate) fn read_key<T: DeserializeOwned>(path: &Path, key: &str) -> std::io::Result<Vec<T>> {
    let _g = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    read(path).and_then(|m| list_of(&m, key))
}

pub fn list(path: &Path, max: usize) -> Vec<Recent> {
    let mut v: Vec<Recent> = read_key(path, "recents").unwrap_or_else(|e| {
        log::warn!("could not read {} key recents: {e}", path.display());
        vec![]
    });
    v.truncate(max);
    v
}

pub fn push(path: &Path, r: Recent, max: usize) -> std::io::Result<()> {
    edit_key(path, "recents", |v| push_into(v, r, max))
}

pub fn clear(path: &Path) -> std::io::Result<()> {
    edit_key(path, "recents", Vec::<Recent>::clear)
}

/// Import old-manager recents after existing ones (deduped, capped at `max`).
/// Returns how many survived the cap. Favorites: discover::favorites::add_all.
pub fn import(path: &Path, recents: Vec<Recent>, max: usize) -> std::io::Result<usize> {
    edit_key(path, "recents", |v: &mut Vec<Recent>| {
        let had = v.len();
        for r in recents {
            if !v.iter().any(|x| x.place_id == r.place_id) {
                v.push(r);
            }
        }
        v.truncate(max);
        v.len().saturating_sub(had)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: u64, at: &str) -> Recent {
        Recent {
            place_id: id,
            name: format!("g{id}"),
            thumbnail: None,
            last_played_at: at.into(),
        }
    }

    #[test]
    fn dedupes_newest_first_and_caps() {
        let mut v = vec![];
        push_into(&mut v, r(1, "a"), 2);
        push_into(&mut v, r(2, "b"), 2);
        push_into(&mut v, r(1, "c"), 2);
        assert_eq!(v, vec![r(1, "c"), r(2, "b")]);
        push_into(&mut v, r(3, "d"), 2);
        assert_eq!(v, vec![r(3, "d"), r(1, "c")]);
    }

    #[test]
    fn late_finishing_older_push_stays_behind() {
        let (t1, t2) = ("2026-01-01T00:00:01Z", "2026-01-01T00:00:02Z");
        let mut v = vec![];
        push_into(&mut v, r(2, t2), 5);
        push_into(&mut v, r(1, t1), 5);
        assert_eq!(v, vec![r(2, t2), r(1, t1)]);
        // same place: the older launch never replaces the newer entry
        push_into(&mut v, r(2, t1), 5);
        assert_eq!(v, vec![r(2, t2), r(1, t1)]);
    }

    #[test]
    fn unreadable_file_aborts_edit() {
        let d = tempfile::tempdir().unwrap();
        // a directory: read fails with something other than NotFound
        let p = d.path().join("library.json");
        std::fs::create_dir(&p).unwrap();
        assert!(
            read(&p).is_err(),
            "read error is reported, not an empty library"
        );
        assert!(push(&p, r(1, "a"), 20).is_err());
        assert!(clear(&p).is_err());
        assert!(list(&p, 20).is_empty());
        assert!(p.is_dir(), "untouched");
    }

    #[test]
    fn undecodable_key_aborts_edit() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        let bad = r#"{"recents":[{"placeId":"nope"}]}"#;
        std::fs::write(&p, bad).unwrap();
        let e = push(&p, r(1, "a"), 20).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(clear(&p).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), bad, "untouched");
        assert!(list(&p, 20).is_empty(), "read stays lenient");
    }

    #[test]
    fn file_roundtrip_keeps_other_keys() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        std::fs::write(&p, r#"{"favorites":[{"id":"x"}]}"#).unwrap();
        push(&p, r(5, "t"), 20).unwrap();
        push(&p, r(6, "u"), 20).unwrap();
        assert_eq!(list(&p, 1), vec![r(6, "u")]);
        clear(&p).unwrap();
        assert!(list(&p, 20).is_empty());
        let v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["favorites"][0]["id"], "x");
    }

    #[test]
    fn import_appends_dedupes_and_keeps_other_keys() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        std::fs::write(&p, r#"{"favorites":[{"id":"x"}]}"#).unwrap();
        push(&p, r(1, "a"), 20).unwrap();
        let n = import(&p, vec![r(1, "x"), r(2, "y"), r(3, "z")], 2).unwrap();
        assert_eq!(n, 1, "counts what survived the cap");
        assert_eq!(list(&p, 20), vec![r(1, "a"), r(2, "y")]);
        assert_eq!(import(&p, vec![r(4, "w")], 2).unwrap(), 0);
        let v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["favorites"][0]["id"], "x");
    }

    #[test]
    fn non_object_file_backed_up_once_then_replaced() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        std::fs::write(&p, "[1,2]").unwrap();
        assert!(list(&p, 20).is_empty());
        push(&p, r(1, "a"), 20).unwrap();
        assert_eq!(list(&p, 20), vec![r(1, "a")]);
        let bak = d.path().join("library.json.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "[1,2]");
        // a second bad file keeps the first backup
        std::fs::write(&p, "7").unwrap();
        push(&p, r(2, "b"), 20).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "[1,2]");
    }

    #[test]
    fn middle_insert_keeps_newest_first() {
        let mut v = vec![r(1, "c"), r(2, "a")];
        push_into(&mut v, r(3, "b"), 5);
        assert_eq!(v, vec![r(1, "c"), r(3, "b"), r(2, "a")]);
    }
}
