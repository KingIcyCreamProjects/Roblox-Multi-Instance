//! Favorites in `library.json` next to M4's recents (docs/contracts/discover.md `Favorite`).

use crate::launch::recents::{edit_key, read_key};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub id: String,
    pub place_id: u64,
    pub name: String,
    pub private_link: Option<String>,
    pub thumbnail: Option<String>,
    pub order: u32,
}

const KEY: &str = "favorites";

/// By `order`. A read error is returned, not shown as an empty list.
pub fn list(path: &Path) -> std::io::Result<Vec<Favorite>> {
    let mut v: Vec<Favorite> = read_key(path, KEY)?;
    v.sort_by_key(|f| f.order);
    Ok(v)
}

/// Same placeId + privateLink returns the existing one and `false`.
fn push(v: &mut Vec<Favorite>, new: Favorite) -> (Favorite, bool) {
    if let Some(d) = v
        .iter()
        .find(|f| f.place_id == new.place_id && f.private_link == new.private_link)
    {
        return (d.clone(), false);
    }
    let order = v.iter().map(|f| f.order + 1).max().unwrap_or(0);
    let f = Favorite { order, ..new };
    v.push(f.clone());
    (f, true)
}

/// Same placeId + privateLink returns the existing one.
pub fn add(path: &Path, new: Favorite) -> io::Result<Favorite> {
    edit_key(path, KEY, |v| push(v, new).0)
}

/// [`add`] for many in one write (import). Returns how many were new.
pub fn add_all(path: &Path, new: Vec<Favorite>) -> io::Result<usize> {
    edit_key(path, KEY, |v| {
        new.into_iter().filter(|f| push(v, f.clone()).1).count()
    })
}

/// `private_link`: None = keep, Some(None) = clear. Ok(None) = no such id. A link that
/// would duplicate another favorite (same placeId + privateLink, as `push`) is an error.
pub fn update(
    path: &Path,
    id: &str,
    name: Option<String>,
    private_link: Option<Option<String>>,
) -> io::Result<Option<Favorite>> {
    edit_key(path, KEY, |v: &mut Vec<Favorite>| {
        let Some(i) = v.iter().position(|f| f.id == id) else {
            return Ok(None);
        };
        if let Some(l) = &private_link {
            let place = v[i].place_id;
            if v.iter()
                .any(|o| o.id != id && o.place_id == place && o.private_link == *l)
            {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "that game and server are already a favorite",
                ));
            }
        }
        let f = &mut v[i];
        if let Some(n) = name {
            f.name = n;
        }
        if let Some(l) = private_link {
            f.private_link = l;
        }
        Ok(Some(f.clone()))
    })?
}

/// Sets the image of favorites that have none, by placeId. Returns how many changed.
pub fn fill_thumbnails(path: &Path, found: &HashMap<u64, String>) -> io::Result<usize> {
    edit_key(path, KEY, |v: &mut Vec<Favorite>| {
        let mut n = 0;
        for f in v.iter_mut().filter(|f| f.thumbnail.is_none()) {
            if let Some(t) = found.get(&f.place_id) {
                f.thumbnail = Some(t.clone());
                n += 1;
            }
        }
        n
    })
}

pub fn remove(path: &Path, id: &str) -> io::Result<()> {
    edit_key(path, KEY, |v: &mut Vec<Favorite>| v.retain(|f| f.id != id))
}

/// `ids` first in that order, unlisted ones after, keeping their relative order.
pub fn reorder(path: &Path, ids: &[String]) -> io::Result<()> {
    edit_key(path, KEY, |v: &mut Vec<Favorite>| {
        let rank = |f: &Favorite| ids.iter().position(|i| *i == f.id).unwrap_or(usize::MAX);
        v.sort_by_key(|f| (rank(f), f.order));
        for (i, f) in v.iter_mut().enumerate() {
            f.order = i as u32;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::recents::{self, Recent};
    use serde_json::Value;

    fn fav(id: &str, place: u64, link: Option<&str>) -> Favorite {
        Favorite {
            id: id.into(),
            place_id: place,
            name: format!("g{place}"),
            private_link: link.map(Into::into),
            thumbnail: None,
            order: 0,
        }
    }
    fn ids(p: &Path) -> Vec<String> {
        list(p).unwrap().into_iter().map(|f| f.id).collect()
    }

    #[test]
    fn fill_thumbnails_sets_only_missing_ones() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        add(&p, fav("a", 1, None)).unwrap();
        add(&p, fav("b", 2, None)).unwrap();
        add(&p, fav("c", 3, None)).unwrap();
        let mut have = fav("d", 4, None);
        have.thumbnail = Some("keep".into());
        add(&p, have).unwrap();
        let found: HashMap<u64, String> = [(1, "u1"), (4, "new")]
            .iter()
            .map(|(k, v)| (*k, v.to_string()))
            .collect();
        assert_eq!(fill_thumbnails(&p, &found).unwrap(), 1);
        let got: Vec<Option<String>> = list(&p).unwrap().into_iter().map(|f| f.thumbnail).collect();
        assert_eq!(got, [Some("u1".into()), None, None, Some("keep".into())]);
    }

    #[test]
    fn crud_reorder_keep_recents_and_other_keys() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        std::fs::write(&p, r#"{"folders":[1]}"#).unwrap();
        let r = Recent {
            place_id: 9,
            name: "r".into(),
            thumbnail: None,
            last_played_at: "t".into(),
        };
        recents::push(&p, r.clone(), 20).unwrap();

        let a = add(&p, fav("a", 1, None)).unwrap();
        assert_eq!(a.order, 0);
        add(&p, fav("b", 2, None)).unwrap();
        add(&p, fav("c", 1, Some("L"))).unwrap();
        // dedupe on placeId + privateLink
        assert_eq!(add(&p, fav("dup", 1, None)).unwrap().id, "a");
        assert_eq!(ids(&p), ["a", "b", "c"]);

        let u = update(&p, "b", Some("Renamed".into()), Some(Some("L2".into())))
            .unwrap()
            .unwrap();
        assert_eq!(
            (u.name.as_str(), u.private_link.as_deref()),
            ("Renamed", Some("L2"))
        );
        let u = update(&p, "b", None, Some(None)).unwrap().unwrap();
        assert_eq!((u.name.as_str(), u.private_link), ("Renamed", None));
        assert_eq!(update(&p, "zz", None, None).unwrap(), None);
        // a link that duplicates "c" (place 1 + L) is refused; "a" keeps its own
        assert!(update(&p, "a", None, Some(Some("L".into()))).is_err());
        assert_eq!(list(&p).unwrap()[0].private_link, None);

        reorder(&p, &["c".into(), "a".into(), "ghost".into()]).unwrap();
        assert_eq!(ids(&p), ["c", "a", "b"]);
        assert_eq!(
            list(&p)
                .unwrap()
                .iter()
                .map(|f| f.order)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );

        remove(&p, "a").unwrap();
        assert_eq!(ids(&p), ["c", "b"]);
        // new favorites go last
        add(&p, fav("d", 3, None)).unwrap();
        assert_eq!(ids(&p), ["c", "b", "d"]);

        recents::clear(&p).unwrap();
        assert_eq!(ids(&p), ["c", "b", "d"]);
        recents::push(&p, r.clone(), 20).unwrap();
        assert_eq!(recents::list(&p, 20), vec![r]);
        let v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["folders"][0], 1);
        assert_eq!(read_key::<Favorite>(&p, "favorites").unwrap().len(), 3);
    }

    #[test]
    fn add_all_counts_only_new_and_appends_in_order() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        add(&p, fav("a", 1, None)).unwrap();
        let n = add_all(
            &p,
            vec![
                fav("x", 1, None),
                fav("b", 2, None),
                fav("y", 2, None),
                fav("c", 3, None),
            ],
        )
        .unwrap();
        assert_eq!(n, 2, "dupes of existing and of each other are skipped");
        assert_eq!(ids(&p), ["a", "b", "c"]);
        assert_eq!(list(&p).unwrap()[2].order, 2);
        assert_eq!(add_all(&p, vec![]).unwrap(), 0);
    }

    #[test]
    fn list_missing_file_is_empty_and_bad_key_is_error() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        assert!(list(&p).unwrap().is_empty());
        std::fs::write(&p, r#"{"favorites": 5}"#).unwrap();
        assert_eq!(
            list(&p).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        // an edit must not overwrite the unreadable key
        assert!(add(&p, fav("a", 1, None)).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"{"favorites": 5}"#);
    }

    #[test]
    fn list_sorts_by_stored_order() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("library.json");
        std::fs::write(
            &p,
            r#"{"favorites":[
            {"id":"b","placeId":2,"name":"B","privateLink":null,"thumbnail":null,"order":5},
            {"id":"a","placeId":1,"name":"A","privateLink":null,"thumbnail":null,"order":1}]}"#,
        )
        .unwrap();
        assert_eq!(ids(&p), ["a", "b"]);
    }
}
