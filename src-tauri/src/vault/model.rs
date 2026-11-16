use crate::accounts::{lenient, wipe_value, Account, AccountSecrets, LaunchTarget};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use zeroize::Zeroize;

/// Decrypted vault contents. Unknown keys (top level and per account) round-trip.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct VaultPayload {
    #[serde(deserialize_with = "de_accounts")]
    pub accounts: Vec<Account>,
    /// Keyed by `Account::id`. Zeroized on drop.
    #[serde(deserialize_with = "de_secrets")]
    pub secrets: BTreeMap<String, AccountSecrets>,
    /// Group display order.
    #[serde(deserialize_with = "de_groups")]
    pub groups: Vec<String>,
    /// Saved launch target per group name.
    #[serde(rename = "groupTargets", deserialize_with = "de_group_targets")]
    pub group_targets: BTreeMap<String, LaunchTarget>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// Lenient loaders: a malformed field resets to its default (logged), never "damaged".
fn de_accounts<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Account>, D::Error> {
    let mut v = Value::deserialize(d)?;
    let out = match &v {
        Value::Array(a) => a
            .iter()
            .enumerate()
            .filter_map(|(i, x)| lenient(x, &format!("account {i}")))
            .collect(),
        _ => Vec::new(),
    };
    wipe_value(&mut v);
    Ok(out)
}

fn de_secrets<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, AccountSecrets>, D::Error> {
    let mut v = Value::deserialize(d)?;
    let out = match &v {
        Value::Object(m) => m
            .iter()
            .filter_map(|(k, x)| Some((k.clone(), lenient(x, "secrets")?)))
            .collect(),
        _ => BTreeMap::new(),
    };
    wipe_value(&mut v);
    Ok(out)
}

fn de_groups<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(Vec::<String>::deserialize(&v).unwrap_or_default())
}

fn de_group_targets<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, LaunchTarget>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match &v {
        Value::Object(m) => m
            .iter()
            .filter_map(|(k, x)| Some((k.clone(), lenient(x, "group target")?)))
            .collect(),
        _ => BTreeMap::new(),
    })
}

impl VaultPayload {
    /// Best-effort wipe before the memory is freed (secrets zeroize on drop).
    pub fn wipe(&mut self) {
        fn w(v: &mut Value) {
            match v {
                Value::String(s) => s.zeroize(),
                Value::Array(a) => a.iter_mut().for_each(w),
                Value::Object(m) => m.values_mut().for_each(w),
                _ => {}
            }
        }
        self.secrets
            .values_mut()
            .for_each(|s| s.other.values_mut().for_each(w));
        self.secrets.clear();
        self.accounts
            .iter_mut()
            .for_each(|a| a.other.values_mut().for_each(w));
        self.extra.values_mut().for_each(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bad_fields_reset_and_bad_entries_drop() {
        let p: VaultPayload = serde_json::from_value(json!({
            "accounts": [{"id": "a1", "username": "ok", "pinned": "yes"}, 5, {"id": "a2"}],
            "secrets": {"a1": {"cookie": "c"}, "a2": 5},
            "groups": "nope",
            "groupTargets": {"g": {"placeId": 1}, "h": 5}
        }))
        .unwrap();
        assert_eq!(p.accounts.len(), 2);
        assert_eq!(p.accounts[0].username, "ok");
        assert!(!p.accounts[0].pinned);
        assert!(p.secrets.contains_key("a1") && p.secrets.len() == 1);
        assert!(p.groups.is_empty());
        assert_eq!(p.group_targets.len(), 1);
        assert_eq!(p.group_targets["g"].place_id, Some(1));
    }

    #[test]
    fn wipe_clears_secrets_and_zeroes_extra_strings() {
        let mut p: VaultPayload = serde_json::from_value(json!({
            "accounts": [{"id": "a1", "tok": "sekrit"}],
            "secrets": {"a1": {"cookie": "c"}},
            "future": "sekrit2"
        }))
        .unwrap();
        p.wipe();
        assert!(p.secrets.is_empty());
        assert_eq!(p.accounts[0].other["tok"], "");
        assert_eq!(p.extra["future"], "");
    }

    #[test]
    fn valid_groups_and_targets_load_non_array_accounts_drop() {
        let p: VaultPayload = serde_json::from_value(json!({
            "accounts": {"id": "a1"},
            "groups": ["main", "alts"],
            "groupTargets": {"main": {"placeId": 5}}
        }))
        .unwrap();
        assert!(p.accounts.is_empty());
        assert_eq!(p.groups, ["main", "alts"]);
        assert_eq!(p.group_targets["main"].place_id, Some(5));
    }

    #[test]
    fn wipe_recurses_into_nested_values() {
        let mut p: VaultPayload = serde_json::from_value(json!({
            "accounts": [{"id": "a1", "tok": {"x": ["sekrit"]}}],
            "future": {"deep": ["sekrit2"]}
        }))
        .unwrap();
        p.wipe();
        assert_eq!(p.accounts[0].other["tok"], json!({"x": [""]}));
        assert_eq!(p.extra["future"], json!({"deep": [""]}));
    }
}
