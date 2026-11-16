//! Viewer side: list the tailnet's other computers, pair with one, hand the webview what it
//! needs to open the session socket. Tokens are stored OS-wrapped (DPAPI / keychain) in
//! `remote-keys.bin`, keyed by the peer's Tailscale IPv4, like the friend secrets.

use super::{bind_ok, clean_name, valid_token};
use crate::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::Duration;
use tauri::Manager;

const KEYS_FILE: &str = "remote-keys.bin";
const PROBE: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub name: String,
    /// windows | macos | linux | ios | android | other
    pub os: String,
    pub ip: String,
    pub online: bool,
    /// The device name RMI answered `hello` with; `None`: no RMI remote there.
    pub rmi: Option<String>,
    pub paired: bool,
    /// Someone is connected to it right now.
    pub busy: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peers {
    pub installed: bool,
    pub running: bool,
    pub peers: Vec<Peer>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connect {
    pub url: String,
    /// Secret: sent as the socket's first message, never in the URL.
    pub token: String,
    pub name: String,
}

fn os_of(s: &str) -> &'static str {
    match s.to_ascii_lowercase().as_str() {
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        "ios" => "ios",
        "android" => "android",
        _ => "other",
    }
}

/// `tailscale status --json`: (running, peers with a 100.64/10 IPv4), sorted by name.
pub fn parse_peers(json: &str) -> (bool, Vec<Peer>) {
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    let running = v["BackendState"] == "Running";
    let mut peers: Vec<Peer> = v["Peer"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, p)| {
            let ip = p["TailscaleIPs"]
                .as_array()?
                .iter()
                .filter_map(|i| i.as_str()?.parse::<Ipv4Addr>().ok())
                .find(|i| bind_ok(*i))?;
            let dns_label = p["DNSName"].as_str().and_then(|d| d.split('.').next());
            let name = p["HostName"]
                .as_str()
                .filter(|h| !h.is_empty())
                .or(dns_label)
                .map(clean_name)
                .unwrap_or_else(|| ip.to_string());
            Some(Peer {
                name,
                os: os_of(p["OS"].as_str().unwrap_or_default()).into(),
                ip: ip.to_string(),
                online: p["Online"] == true,
                rmi: None,
                paired: false,
                busy: false,
            })
        })
        .collect();
    peers.sort_by_key(|p| p.name.to_lowercase());
    (running, peers)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Key {
    token: String,
    name: String,
}

type Keys = BTreeMap<String, Key>;

fn keys_path(st: &AppState) -> std::path::PathBuf {
    st.paths.root.join(KEYS_FILE)
}

/// Blocking (keyring / DPAPI).
fn load_keys(st: &AppState) -> Result<Keys, String> {
    let path = keys_path(st);
    let b = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Keys::new()),
        Err(e) => return Err(format!("Could not read the saved device keys: {e}")),
    };
    let os = st.os().map_err(|e| e.to_string())?;
    Ok(decode_keys(&path, os.unwrap(&b).map_err(|e| e.to_string())))
}

/// The keys in `plain`. Undecryptable (a Windows reset, another user profile) or unreadable:
/// those tokens are lost anyway, so the file is set aside as `.bad` and pairing starts
/// fresh instead of failing forever (and never dropping other devices on the next save).
fn decode_keys(path: &std::path::Path, plain: Result<zeroize::Zeroizing<Vec<u8>>, String>) -> Keys {
    // never the serde text: it can quote a token ("invalid type: string \"<token>\"")
    let parse = |p: zeroize::Zeroizing<Vec<u8>>| {
        serde_json::from_slice::<Keys>(&p).map_err(|_| "not valid device keys".to_owned())
    };
    match plain.and_then(parse) {
        Ok(keys) => {
            for k in keys.values() {
                crate::logging::register_secret(&k.token);
            }
            keys
        }
        Err(e) => {
            let bad = path.with_extension("bin.bad");
            match std::fs::rename(path, &bad) {
                Ok(()) => {
                    log::error!("remote: {KEYS_FILE} unreadable ({e}); kept as .bad, pair again")
                }
                Err(re) => {
                    log::error!("remote: {KEYS_FILE} unreadable ({e}), not set aside ({re})")
                }
            }
            Keys::new()
        }
    }
}

/// The keys as last decrypted, with the file's modified time: `push_running` runs every few
/// seconds and must not ask the OS secret store each time. Blocking on a changed file.
/// ponytail: keys that could not be read count as none until the file changes or the app
/// restarts (no asking the secret store again every round); pairing reads them afresh.
fn cached_keys(st: &AppState) -> Keys {
    static CACHE: std::sync::Mutex<Option<(std::time::SystemTime, Keys)>> =
        std::sync::Mutex::new(None);
    let Ok(at) = std::fs::metadata(keys_path(st)).and_then(|m| m.modified()) else {
        return Keys::new(); // no file: paired with nobody
    };
    let mut c = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, keys)) = c.as_ref().filter(|(t, _)| *t == at) {
        return keys.clone();
    }
    let keys = load_keys(st).unwrap_or_default();
    *c = Some((at, keys.clone()));
    keys
}

/// Tell every computer this one is paired with which Roblox accounts (user ids) have a client
/// here, so that computer's friend page can name this one. Sent only to computers the owner
/// paired this one with; a locked vault sends nothing.
/// ponytail: one small POST per paired computer every `PUSH_EVERY`, changed or not; send on
/// change only if that ever shows up as load.
pub async fn push_running(app: &tauri::AppHandle) {
    let found = blocking(app, |st| {
        // The vault first (a cheap lock): a locked app never asks the OS secret store.
        let accs: Vec<(u64, String)> = {
            let g = st
                .vault
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(v) = g.as_ref() else {
                return Ok(None);
            };
            let all = v.payload().accounts.iter();
            all.filter_map(|a| Some((a.user_id?, a.tracker_id.clone())))
                .collect()
        };
        let keys = cached_keys(st);
        if keys.is_empty() {
            return Ok(None);
        }
        let trackers: Vec<String> = accs.iter().map(|a| a.1.clone()).collect();
        let live = st.platform.tracked_clients(&trackers);
        let ids: Vec<u64> = accs
            .iter()
            .filter(|(_, t)| live.iter().any(|(_, l)| l == t))
            .map(|(u, _)| *u)
            .collect();
        let body = json!({ "name": st.remote.device_name(), "userIds": ids });
        Ok(Some((keys, body, st.remote.port())))
    })
    .await;
    let (Ok(Some((keys, body, port))), Ok(c)) = (found, client()) else {
        return;
    };
    // All at once: computers that are off must not delay the report to the ones that are on.
    // An older RMI (404), a computer that is off or no longer paired: try again next time.
    let sends = keys.iter().map(|(ip, k)| {
        c.post(format!("http://{ip}:{port}/v1/remote/running"))
            .bearer_auth(&k.token)
            .json(&body)
            .send()
    });
    futures_util::future::join_all(sends).await;
}

/// Blocking.
fn save_keys(st: &AppState, keys: &Keys) -> Result<(), String> {
    let os = st.os().map_err(|e| e.to_string())?;
    let plain = zeroize::Zeroizing::new(serde_json::to_vec(keys).map_err(|e| e.to_string())?);
    let blob = os.wrap(&plain).map_err(|e| e.to_string())?;
    crate::vault::store::write_atomic(&keys_path(st), &blob, crate::vault::store::Backup::Skip)
        .map_err(|e| e.to_string())
}

async fn blocking<T: Send + 'static>(
    app: &tauri::AppHandle,
    f: impl FnOnce(&AppState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let h = app.clone();
    tauri::async_runtime::spawn_blocking(move || f(h.state::<AppState>().inner()))
        .await
        .map_err(|e| e.to_string())?
}

fn peer_ip(ip: &str) -> Result<Ipv4Addr, String> {
    ip.parse()
        .ok()
        .filter(|i| bind_ok(*i))
        .ok_or_else(|| "That is not a Tailscale address.".into())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(PROBE)
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())
}

/// The name a peer's RMI answers `hello` with, and whether it is in a session, if it does.
async fn hello(c: &reqwest::Client, ip: &str, port: u16) -> Option<(String, bool)> {
    let v: Value = c
        .get(format!("http://{ip}:{port}/v1/remote/hello"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    (v["app"] == "rmi" && v["proto"] == 1).then(|| {
        (
            clean_name(v["name"].as_str().unwrap_or("RMI")),
            v["busy"] == true,
        )
    })
}

/// Paired computers no longer in the tailnet: listed offline so their token can be removed.
fn add_gone(peers: &mut Vec<Peer>, keys: &Keys) {
    for (ip, k) in keys {
        if !peers.iter().any(|p| p.ip == *ip) {
            peers.push(Peer {
                name: k.name.clone(),
                os: "other".into(),
                ip: ip.clone(),
                online: false,
                rmi: None,
                paired: true,
                busy: false,
            });
        }
    }
}

#[tauri::command]
pub async fn remote_peers(app: tauri::AppHandle) -> Result<Peers, String> {
    let (installed, status) = tauri::async_runtime::spawn_blocking(|| {
        (
            crate::friend::tailscale::installed(),
            crate::friend::tailscale::status_json(),
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    let (running, mut peers) = status.map(|j| parse_peers(&j)).unwrap_or_default();
    if !running {
        return Ok(Peers {
            installed,
            running,
            peers: Vec::new(),
        });
    }
    let keys = blocking(&app, load_keys).await.unwrap_or_else(|e| {
        log::warn!("remote: device keys not loaded: {e}");
        Keys::new()
    });
    let port = app.state::<AppState>().remote.port();
    let c = client()?;
    let mut set = tokio::task::JoinSet::new();
    for (i, p) in peers.iter().enumerate().filter(|(_, p)| p.online) {
        let (c, ip) = (c.clone(), p.ip.clone());
        set.spawn(async move { (i, hello(&c, &ip, port).await) });
    }
    while let Some(Ok((i, h))) = set.join_next().await {
        if let Some((name, busy)) = h {
            peers[i].rmi = Some(name);
            peers[i].busy = busy;
        }
    }
    for p in &mut peers {
        p.paired = keys.contains_key(&p.ip);
    }
    add_gone(&mut peers, &keys);
    Ok(Peers {
        installed,
        running,
        peers,
    })
}

/// Trades the code shown on `ip` for a token and saves it; returns the peer's name.
#[tauri::command]
pub async fn remote_pair(
    app: tauri::AppHandle,
    ip: String,
    code: String,
) -> Result<String, String> {
    let ip = peer_ip(&ip)?;
    let st = app.state::<AppState>();
    let (port, me) = (st.remote.port(), st.remote.device_name());
    // before the code is spent: a keys file that cannot be read must not cost a pairing
    blocking(&app, load_keys).await?;
    let r = client()?
        .post(format!("http://{ip}:{port}/v1/remote/pair"))
        .json(&json!({ "code": code, "name": me }))
        .send()
        .await
        .map_err(|_| {
            "Could not reach that computer. Check RMI is running there with remote access on."
                .to_owned()
        })?;
    let ok = r.status().is_success();
    let v: Value = r.json().await.unwrap_or_default();
    if !ok {
        let msg = v
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Pairing failed. Try again.");
        return Err(msg.chars().take(200).collect());
    }
    let token = v["token"]
        .as_str()
        .filter(|t| valid_token(t))
        .ok_or("That computer gave an invalid answer.")?
        .to_owned();
    crate::logging::register_secret(&token);
    let name = Some(clean_name(v["name"].as_str().unwrap_or_default()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| ip.to_string());
    let (k, n) = (ip.to_string(), name.clone());
    blocking(&app, move |st| {
        let mut keys = load_keys(st)?;
        keys.insert(k, Key { token, name: n });
        save_keys(st, &keys)
    })
    .await?;
    Ok(name)
}

/// macOS WebKit treats `tauri://localhost` as a secure page and blocks `ws://` to anything but
/// loopback (mixed content), so the viewer connects to a loopback relay (one per peer, kept for
/// the app's life) that forwards to the peer with the Host its DNS-rebinding guard expects.
#[cfg(target_os = "macos")]
async fn relay(to: std::net::SocketAddrV4) -> Result<u16, String> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    static RELAYS: Mutex<Option<HashMap<std::net::SocketAddrV4, u16>>> = Mutex::new(None);
    if let Some(p) = RELAYS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(&to)
    {
        return Ok(*p);
    }
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    RELAYS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(to, port);
    tokio::spawn(async move {
        while let Ok((mut a, _)) = l.accept().await {
            tokio::spawn(async move {
                // the HTTP upgrade request; the viewer sends nothing more before the 101
                let mut head = Vec::new();
                let mut b = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match a.read(&mut b).await {
                        Ok(n @ 1..) if head.len() < 16384 => head.extend_from_slice(&b[..n]),
                        _ => return,
                    }
                }
                let head = String::from_utf8_lossy(&head);
                // only the viewer's own upgrade: other loopback clients (a browser page) are
                // dropped before anything reaches the device
                let path = head
                    .strip_prefix("GET ")
                    .and_then(|r| r.split(' ').next())
                    .and_then(|p| p.split('?').next());
                let origin = head
                    .split("\r\n")
                    .filter_map(|l| l.split_once(':'))
                    .find(|(k, _)| k.eq_ignore_ascii_case("origin"))
                    .map(|(_, v)| v.trim());
                if path != Some("/v1/remote/ws")
                    || !origin.is_some_and(|o| super::origin_ok(Some(o)))
                {
                    return;
                }
                let head: String = head
                    .split_inclusive("\r\n")
                    .map(|l| {
                        if l.to_ascii_lowercase().starts_with("host:") {
                            format!("Host: {to}\r\n")
                        } else {
                            l.to_owned()
                        }
                    })
                    .collect();
                let Ok(mut c) = tokio::net::TcpStream::connect(to).await else {
                    log::warn!("remote: relay could not reach the device");
                    return;
                };
                let _ = (a.set_nodelay(true), c.set_nodelay(true)); // small input messages
                if c.write_all(head.as_bytes()).await.is_ok() {
                    let _ = tokio::io::copy_bidirectional(&mut a, &mut c).await;
                }
            });
        }
    });
    Ok(port)
}

/// What the viewer needs to open the session with `ip`.
#[tauri::command]
pub async fn remote_connect(app: tauri::AppHandle, ip: String) -> Result<Connect, String> {
    let ip = peer_ip(&ip)?;
    let port = app.state::<AppState>().remote.port();
    let key = blocking(&app, load_keys)
        .await?
        .remove(&ip.to_string())
        .ok_or("Pair with this computer first.")?;
    #[cfg(target_os = "macos")]
    let url = format!(
        "ws://127.0.0.1:{}/v1/remote/ws",
        relay(std::net::SocketAddrV4::new(ip, port)).await?
    );
    #[cfg(not(target_os = "macos"))]
    let url = format!("ws://{ip}:{port}/v1/remote/ws");
    Ok(Connect {
        url,
        token: key.token,
        name: key.name,
    })
}

/// Forget the token for `ip` (the other computer keeps its entry until removed there).
#[tauri::command]
pub async fn remote_forget(app: tauri::AppHandle, ip: String) -> Result<(), String> {
    let ip = peer_ip(&ip)?.to_string();
    blocking(&app, move |st| {
        let mut keys = load_keys(st)?;
        if keys.remove(&ip).is_some() {
            save_keys(st, &keys)?;
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peers_parsed_filtered_sorted() {
        let j = json!({
            "BackendState": "Running",
            "Self": {"TailscaleIPs": ["100.100.1.1", "fd7a::1"]},
            "Peer": {
                "a": {"HostName": "zeta", "OS": "windows", "TailscaleIPs": ["100.64.0.9", "fd7a::9"], "Online": true},
                "b": {"HostName": "", "DNSName": "alpha.tail.ts.net.", "OS": "macOS", "TailscaleIPs": ["100.127.3.4"], "Online": false},
                "c": {"HostName": "gamma", "OS": "linux", "TailscaleIPs": ["192.168.1.5"]},
                "d": {"HostName": "Phone\u{7}", "OS": "iOS", "TailscaleIPs": ["100.80.0.1"], "Online": true}
            }
        })
        .to_string();
        let (running, p) = parse_peers(&j);
        assert!(running);
        let names: Vec<_> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Phone", "zeta"]);
        assert_eq!(p[0].os, "macos");
        assert!(!p[0].online);
        assert_eq!(p[2].ip, "100.64.0.9");
        assert_eq!(p[1].os, "ios");
        assert_eq!(
            crate::friend::tailscale::self_ipv4(&j),
            Some(Ipv4Addr::new(100, 100, 1, 1))
        );
        let (running, p) = parse_peers(r#"{"BackendState":"Stopped"}"#);
        assert!(!running && p.is_empty());
        assert_eq!(parse_peers("not json").1, vec![]);
    }

    #[test]
    fn paired_but_gone_listed_offline() {
        let mut peers = parse_peers(
            &json!({"BackendState": "Running", "Peer": {"a": {"HostName": "here", "TailscaleIPs": ["100.64.0.9"], "Online": true}}})
                .to_string(),
        )
        .1;
        let key = |n: &str| Key {
            token: "t".into(),
            name: n.into(),
        };
        let keys = Keys::from([
            ("100.64.0.9".into(), key("here")),
            ("100.64.0.7".into(), key("Old PC")),
        ]);
        add_gone(&mut peers, &keys);
        assert_eq!(peers.len(), 2);
        assert_eq!(
            (peers[1].name.as_str(), peers[1].online, peers[1].paired),
            ("Old PC", false, true)
        );
    }

    #[test]
    fn unreadable_keys_set_aside_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEYS_FILE);
        let good = br#"{"100.64.0.9":{"token":"t","name":"PC"}}"#.to_vec();
        assert_eq!(decode_keys(&path, Ok(good.into())).len(), 1);
        for bad in [
            Err("DPAPI failed".to_owned()),
            Ok(b"not json".to_vec().into()),
        ] {
            std::fs::write(&path, b"blob").unwrap();
            assert!(decode_keys(&path, bad).is_empty());
            assert!(!path.exists(), "moved away");
            assert_eq!(
                std::fs::read(path.with_extension("bin.bad")).unwrap(),
                b"blob"
            );
        }
    }

    #[test]
    fn only_tailscale_peer_addresses() {
        assert!(peer_ip("100.64.0.1").is_ok());
        for bad in ["127.0.0.1", "192.168.0.2", "100.128.0.1", "example.com", ""] {
            assert!(peer_ip(bad).is_err(), "{bad}");
        }
    }
}
