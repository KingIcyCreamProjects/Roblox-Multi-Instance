//! Turns Tailscale Funnel on or off for the loopback friend server by calling the user's
//! own `tailscale` CLI. Never replaces or removes a Serve/Funnel entry that points somewhere
//! else: only one whose sole handler is `/` proxying the friend server counts as RMI's.

use crate::AppState;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::Manager;

const TIMEOUT: Duration = Duration::from_secs(20);
const PORTS: [u16; 3] = [443, 8443, 10000];

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TsStatus {
    pub installed: bool,
    pub running: bool,
    pub dns_name: Option<String>,
    /// Funnel port that already proxies the friend server.
    pub funnel_port: Option<u16>,
}

#[derive(Debug, Serialize)]
pub struct TsError {
    /// not_installed | not_running | needs_admin | failed
    pub code: &'static str,
    pub url: Option<String>,
    pub message: String,
}

fn err(code: &'static str, message: impl Into<String>) -> TsError {
    TsError {
        code,
        url: None,
        message: message.into(),
    }
}

/// UI text for a failed CLI call. The raw output never reaches the UI.
fn fail_message(cli_output: &str) -> &'static str {
    let t = cli_output.to_ascii_lowercase();
    if t.contains("flag provided but not defined") {
        "Update Tailscale to the latest version, then try again."
    } else if t.contains("access denied") || t.contains("permission") {
        "Tailscale refused the change. Open Tailscale and check you are signed in as the owner of this device."
    } else {
        "Tailscale could not turn on the link. Try again."
    }
}

/// `failed` with a mapped message; the raw CLI text goes to the log only.
fn fail(cli_output: &str) -> TsError {
    log::warn!("tailscale failed: {}", cli_output.trim());
    err("failed", fail_message(cli_output))
}

/// `tailscale status --json`: (BackendState == "Running", Self.DNSName without trailing dot).
pub fn parse_status(json: &str) -> (bool, Option<String>) {
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    let dns = v
        .pointer("/Self/DNSName")
        .and_then(Value::as_str)
        .map(|s| s.trim_end_matches('.'))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    (v["BackendState"] == "Running", dns)
}

/// This device's Tailscale IPv4 (`Self.TailscaleIPs`), if any.
pub fn self_ipv4(status_json: &str) -> Option<std::net::Ipv4Addr> {
    let v: Value = serde_json::from_str(status_json).ok()?;
    v.pointer("/Self/TailscaleIPs")?
        .as_array()?
        .iter()
        .find_map(|ip| ip.as_str()?.parse().ok())
}

/// Blocking: `tailscale status --json`, `None` when Tailscale is missing or the call failed.
pub(crate) fn status_json() -> Option<String> {
    let bin = find()?;
    run(&bin, &["status", "--json"], never)
        .ok()
        .map(|o| o.stdout)
}

/// Whether the Tailscale CLI is installed.
pub(crate) fn installed() -> bool {
    find().is_some()
}

/// `tailscale serve status --json`; an empty output means no config. `None` if unreadable.
fn serve_config(serve_json: &str) -> Option<Value> {
    if serve_json.trim().is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str(serve_json).ok()
}

/// `Web` entries (`host:port`) on `port`.
fn web_on(v: &Value, port: u16) -> impl Iterator<Item = (&String, &Value)> {
    let suffix = format!(":{port}");
    v["Web"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(move |(k, _)| k.ends_with(&suffix))
}

/// RMI's entry: exactly one handler, `/`, proxying `target`. Anything more is someone else's.
fn proxies(web: &Value, target: &str) -> bool {
    web["Handlers"].as_object().is_some_and(|h| h.len() == 1)
        && web.pointer("/Handlers/~1/Proxy").and_then(Value::as_str) == Some(target)
}

/// First of 443, 8443, 10000 that is free or already proxies `target`.
pub fn pick_port(serve_json: &str, target: &str) -> Option<u16> {
    let v = serve_config(serve_json)?;
    PORTS.into_iter().find(|&p| {
        let mut web = web_on(&v, p).peekable();
        let free = v["TCP"].get(p.to_string()).is_none() && web.peek().is_none();
        free || web.any(|(_, w)| proxies(w, target))
    })
}

/// Funnel port whose only handler `/` proxies `target` with Funnel allowed.
fn funnel_port(serve_json: &str, target: &str) -> Option<u16> {
    let v = serve_config(serve_json)?;
    v["Web"].as_object()?.iter().find_map(|(k, w)| {
        let on = proxies(w, target) && v["AllowFunnel"][k] == true;
        on.then(|| k.rsplit(':').next()?.parse().ok()).flatten()
    })
}

/// The https://login.tailscale.com/... or https://tailscale.com/s/... link the CLI prints when
/// Funnel/HTTPS is off (the latter: the tailnet lacks the Funnel attribute or HTTPS certs).
pub fn parse_enable_url(cli_output: &str) -> Option<String> {
    cli_output
        .split_whitespace()
        .find(|w| {
            w.starts_with("https://login.tailscale.com/")
                || w.starts_with("https://tailscale.com/s/")
        })
        .map(|w| w.trim_end_matches(['.', ',', ')', '"', '\'']).to_owned())
}

fn public_url(dns: &str, port: u16) -> String {
    match port {
        443 => format!("https://{dns}"),
        p => format!("https://{dns}:{p}"),
    }
}

/// A public address `friend_ts_enable` could have made: https `*.ts.net` on 443, 8443 or 10000.
pub fn rmi_made(public_url: &str) -> bool {
    url::Url::parse(public_url.trim()).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some_and(|h| h.ends_with(".ts.net"))
            && matches!(u.port(), None | Some(8443) | Some(10000))
    })
}

/// The old friend port whose Funnel entry must go after a settings save: the remote was on an
/// RMI-made address and then was turned off, moved to another port, or lost its address.
pub fn stale_port(old: &Value, new: &Value) -> Option<u16> {
    let f = |v: &Value, k: &str| v.pointer(&format!("/friend/{k}")).cloned();
    let url = |v: &Value| f(v, "publicUrl").and_then(|u| u.as_str().map(str::to_owned));
    url(old).filter(|u| rmi_made(u))?;
    let port = u16::try_from(f(old, "port")?.as_u64()?).ok()?;
    let off = f(old, "enabled") == Some(Value::Bool(true))
        && f(new, "enabled") != Some(Value::Bool(true));
    let moved = f(new, "port") != Some(port.into());
    let cleared = url(new).is_none_or(|u| u.trim().is_empty());
    (off || moved || cleared).then_some(port)
}

fn target_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The default Windows / macOS install path first, then absolute PATH entries only.
fn find() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "tailscale.exe"
    } else {
        "tailscale"
    };
    let fixed = if cfg!(windows) {
        Some(r"C:\Program Files\Tailscale\tailscale.exe")
    } else if cfg!(target_os = "macos") {
        Some("/Applications/Tailscale.app/Contents/MacOS/Tailscale")
    } else {
        None
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    fixed
        .map(PathBuf::from)
        .into_iter()
        .chain(
            std::env::split_paths(&path)
                .filter(|d| d.is_absolute())
                .map(|d| d.join(exe)),
        )
        .find(|p| p.is_file())
}

struct Out {
    /// `None` = killed (timeout, or `stop` matched).
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Sends `r`'s bytes as they arrive, tagged stdout (`true`) or stderr.
fn pump(r: Option<impl Read + Send + 'static>, out: bool, tx: mpsc::Sender<(bool, Vec<u8>)>) {
    std::thread::spawn(move || {
        let Some(mut r) = r else { return };
        let mut b = [0u8; 4096];
        while let Ok(n @ 1..) = r.read(&mut b) {
            if tx.send((out, b[..n].to_vec())).is_err() {
                break;
            }
        }
    });
}

fn never(_: &str) -> bool {
    false
}

/// Blocking: runs the CLI with no console window; killed after 20 s, or as soon as `stop`
/// matches its stdout or stderr so far.
fn run(bin: &Path, args: &[&str], stop: fn(&str) -> bool) -> Result<Out, TsError> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Tailscale.app's binary only acts as the CLI with a TERM or this set; a GUI app has neither.
    #[cfg(target_os = "macos")]
    cmd.env("TAILSCALE_BE_CLI", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        log::warn!("could not run tailscale: {e}");
        err("failed", "Could not run Tailscale.")
    })?;
    let (tx, rx) = mpsc::channel();
    pump(child.stdout.take(), true, tx.clone());
    pump(child.stderr.take(), false, tx);
    // [stderr, stdout]
    let mut buf = [Vec::new(), Vec::new()];
    let text = |b: &Vec<u8>| String::from_utf8_lossy(b).into_owned();
    let deadline = Instant::now() + TIMEOUT;
    let code = loop {
        while let Ok((out, b)) = rx.try_recv() {
            buf[usize::from(out)].extend(b);
        }
        let stopped = buf.iter().any(|b| stop(&text(b)));
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) if !stopped && Instant::now() < deadline => {
                if let Ok((out, b)) = rx.recv_timeout(Duration::from_millis(100)) {
                    buf[usize::from(out)].extend(b);
                }
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // the rest of the output: ends when both pipes close (or 2 s of silence)
    while let Ok((out, b)) = rx.recv_timeout(Duration::from_secs(2)) {
        buf[usize::from(out)].extend(b);
    }
    Ok(Out {
        code,
        stdout: text(&buf[1]),
        stderr: text(&buf[0]),
    })
}

/// The friend port in the saved settings.
fn saved_port(app: &tauri::AppHandle) -> u16 {
    let s = app.state::<AppState>();
    let port = s
        .settings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pointer("/friend/port")
        .and_then(Value::as_u64)
        .and_then(|p| u16::try_from(p).ok());
    port.unwrap_or(7812)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, TsError> + Send + 'static,
) -> Result<T, TsError> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| err("failed", e.to_string()))?
}

fn status_blocking(target: &str) -> TsStatus {
    let Some(bin) = find() else {
        return TsStatus::default();
    };
    let (running, dns_name) = run(&bin, &["status", "--json"], never)
        .map(|o| parse_status(&o.stdout))
        .unwrap_or_default();
    let funnel_port = running
        .then(|| run(&bin, &["serve", "status", "--json"], never).ok())
        .flatten()
        .and_then(|o| funnel_port(&o.stdout, target));
    TsStatus {
        installed: true,
        running,
        dns_name,
        funnel_port,
    }
}

const NOT_RUNNING: &str =
    "Tailscale is not running or not signed in. Open Tailscale, sign in, then try again.";

/// The DNS name when Tailscale is running and signed in, else `not_running`.
fn running_dns(status_json: &str) -> Result<String, TsError> {
    let (running, dns) = parse_status(status_json);
    dns.filter(|_| running)
        .ok_or_else(|| err("not_running", NOT_RUNNING))
}

fn read_serve(bin: &Path) -> Result<String, TsError> {
    let serve = run(bin, &["serve", "status", "--json"], never)?;
    if serve.code != Some(0) {
        log::warn!("tailscale serve status failed: {}", serve.stderr.trim());
        return Err(err(
            "failed",
            "Could not read the Tailscale Serve settings.",
        ));
    }
    Ok(serve.stdout)
}

fn enable_blocking(target: &str) -> Result<String, TsError> {
    let bin = find().ok_or_else(|| err("not_installed", "Tailscale is not installed."))?;
    let dns = running_dns(&run(&bin, &["status", "--json"], never)?.stdout)?;
    let port = pick_port(&read_serve(&bin)?, target).ok_or_else(|| {
        err(
            "failed",
            "Ports 443, 8443 and 10000 are already used by other Tailscale Serve or Funnel entries.",
        )
    })?;
    let https = format!("--https={port}");
    // the CLI may wait for the admin page to be visited: stop as soon as it prints the link
    let o = run(&bin, &["funnel", "--bg", "--yes", &https, target], |s| {
        parse_enable_url(s).is_some()
    })?;
    if o.code == Some(0) {
        return Ok(public_url(&dns, port));
    }
    if let Some(url) = parse_enable_url(&o.stdout).or_else(|| parse_enable_url(&o.stderr)) {
        return Err(TsError {
            code: "needs_admin",
            url: Some(url),
            message: "Turn on HTTPS and Funnel for your tailnet in the Tailscale admin page, then try again.".into(),
        });
    }
    Err(match o.code {
        None => err("failed", "Tailscale did not answer in time."),
        Some(_) => fail(&format!("{}\n{}", o.stderr.trim(), o.stdout.trim())),
    })
}

/// Removes RMI's Funnel entry for the friend server on `port` (one `/` handler proxying
/// `http://127.0.0.1:<port>`). Nothing else is touched; no entry, or no Tailscale: Ok.
pub fn disable_target(port: u16) -> Result<(), TsError> {
    let Some(bin) = find() else { return Ok(()) };
    let Some(fp) = funnel_port(&read_serve(&bin)?, &target_url(port)) else {
        return Ok(());
    };
    let https = format!("--https={fp}");
    let o = run(&bin, &["funnel", &https, "off"], never)?;
    if o.code == Some(0) {
        return Ok(());
    }
    log::warn!("tailscale funnel off failed: {}", o.stderr.trim());
    Err(err("failed", "Tailscale could not turn off the link."))
}

#[tauri::command]
pub async fn friend_ts_status(app: tauri::AppHandle) -> Result<TsStatus, String> {
    let t = target_url(saved_port(&app));
    tauri::async_runtime::spawn_blocking(move || status_blocking(&t))
        .await
        .map_err(|e| e.to_string())
}

/// Funnel on for the friend server; saves `friend.publicUrl` + `friend.enabled` the way the
/// settings UI does (which also starts the friend server). Returns the public URL.
#[tauri::command]
pub async fn friend_ts_enable(app: tauri::AppHandle) -> Result<String, TsError> {
    let t = target_url(saved_port(&app));
    let url = blocking(move || enable_blocking(&t)).await?;
    let patch = json!({ "friend": { "publicUrl": url, "enabled": true } });
    crate::commands::settings_set(app.clone(), app.state::<AppState>(), patch)
        .await
        .map_err(|m| err("failed", m))?;
    Ok(url)
}

/// Funnel off for the port that proxies the friend server (nothing else is touched).
#[tauri::command]
pub async fn friend_ts_disable(app: tauri::AppHandle) -> Result<(), String> {
    let port = saved_port(&app);
    blocking(move || disable_target(port))
        .await
        .map_err(|e| e.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "http://127.0.0.1:7812";

    fn serve(entries: &[(u16, &str, bool)]) -> String {
        let mut v = json!({ "TCP": {}, "Web": {}, "AllowFunnel": {} });
        for &(p, proxy, funnel) in entries {
            let k = format!("rmi.example.ts.net:{p}");
            v["TCP"][p.to_string()] = json!({ "HTTPS": true });
            v["Web"][&k] = json!({ "Handlers": { "/": { "Proxy": proxy } } });
            if funnel {
                v["AllowFunnel"][&k] = json!(true);
            }
        }
        v.to_string()
    }

    #[test]
    fn status_running_stopped_no_self() {
        let run = r#"{"BackendState":"Running","Self":{"DNSName":"rmi.example.ts.net."}}"#;
        assert_eq!(parse_status(run), (true, Some("rmi.example.ts.net".into())));
        let stop = r#"{"BackendState":"NeedsLogin","Self":{"DNSName":""}}"#;
        assert_eq!(parse_status(stop), (false, None));
        assert_eq!(parse_status(r#"{"BackendState":"Running"}"#), (true, None));
        assert_eq!(parse_status("not json"), (false, None));
    }

    #[test]
    fn signed_out_maps_to_not_running() {
        for state in ["NeedsLogin", "Stopped"] {
            let j = format!(
                r#"{{"BackendState":"{state}","Self":{{"DNSName":"rmi.example.ts.net."}}}}"#
            );
            assert_eq!(running_dns(&j).unwrap_err().code, "not_running", "{state}");
        }
        let ok = r#"{"BackendState":"Running","Self":{"DNSName":"rmi.example.ts.net."}}"#;
        assert_eq!(running_dns(ok).unwrap(), "rmi.example.ts.net");
    }

    #[test]
    fn picks_free_or_own_port() {
        assert_eq!(pick_port("", T), Some(443));
        assert_eq!(pick_port("{}", T), Some(443));
        let other = "http://127.0.0.1:9000";
        assert_eq!(pick_port(&serve(&[(443, other, true)]), T), Some(8443));
        let all = serve(&[
            (443, other, true),
            (8443, other, false),
            (10000, other, true),
        ]);
        assert_eq!(pick_port(&all, T), None);
        let ours = serve(&[(443, other, true), (8443, T, true)]);
        assert_eq!(pick_port(&ours, T), Some(8443));
        // A raw TCP forwarder on 443 counts as taken too.
        let tcp = r#"{"TCP":{"443":{"TCPForward":"127.0.0.1:22"}}}"#;
        assert_eq!(pick_port(tcp, T), Some(8443));
        assert_eq!(pick_port("garbage", T), None);
    }

    fn multi_handler(port: u16) -> String {
        let k = format!("rmi.example.ts.net:{port}");
        let mut v = json!({ "TCP": {}, "Web": {}, "AllowFunnel": {} });
        v["TCP"][port.to_string()] = json!({ "HTTPS": true });
        v["Web"][&k] = json!({ "Handlers": {
            "/": { "Proxy": T },
            "/admin": { "Proxy": "http://127.0.0.1:9000" }
        } });
        v["AllowFunnel"][&k] = json!(true);
        v.to_string()
    }

    #[test]
    fn multi_handler_port_is_not_ours() {
        // `/` proxies the friend server, but a second handler makes it someone else's
        assert_eq!(pick_port(&multi_handler(443), T), Some(8443));
        assert_eq!(funnel_port(&multi_handler(443), T), None);
    }

    #[test]
    fn funnel_port_needs_funnel_and_target() {
        let other = "http://127.0.0.1:9000";
        let s = serve(&[(443, other, true), (8443, T, true)]);
        assert_eq!(funnel_port(&s, T), Some(8443));
        assert_eq!(funnel_port(&serve(&[(443, T, false)]), T), None);
        assert_eq!(funnel_port("", T), None);
        // finder for an old port ignores the current one and foreign entries
        let s = serve(&[(443, other, true), (8443, T, true)]);
        assert_eq!(funnel_port(&s, &target_url(7900)), None);
        assert_eq!(funnel_port(&s, &target_url(9000)), Some(443));
    }

    #[test]
    fn stale_port_on_off_move_or_clear() {
        let s = |on: bool, port: u16, url: &str| json!({ "friend": { "enabled": on, "port": port, "publicUrl": url } });
        let ts = "https://rmi.example.ts.net";
        // turned off, port moved, address cleared: old port
        assert_eq!(
            stale_port(&s(true, 7812, ts), &s(false, 7812, ts)),
            Some(7812)
        );
        assert_eq!(
            stale_port(&s(true, 7812, ts), &s(true, 7900, ts)),
            Some(7812)
        );
        assert_eq!(
            stale_port(&s(false, 7812, ts), &s(false, 7900, ts)),
            Some(7812)
        );
        assert_eq!(
            stale_port(&s(true, 7812, ts), &s(true, 7812, "")),
            Some(7812)
        );
        // nothing relevant changed, or turned on
        assert_eq!(stale_port(&s(true, 7812, ts), &s(true, 7812, ts)), None);
        assert_eq!(stale_port(&s(false, 7812, ts), &s(true, 7812, ts)), None);
        assert_eq!(stale_port(&s(true, 7812, ""), &s(true, 7812, ts)), None);
        // not an RMI-made address: never ours to remove
        let own = "https://friend.example.com";
        assert_eq!(stale_port(&s(true, 7812, own), &s(false, 7812, own)), None);
        let odd = "https://rmi.example.ts.net:4443";
        assert_eq!(stale_port(&s(true, 7812, odd), &s(false, 7812, odd)), None);
        assert_eq!(stale_port(&json!({}), &s(false, 7812, "")), None);
    }

    #[test]
    fn rmi_made_addresses() {
        assert!(rmi_made("https://rmi.example.ts.net"));
        assert!(rmi_made("https://rmi.example.ts.net/"));
        assert!(rmi_made("https://rmi.example.ts.net:8443"));
        assert!(rmi_made("https://rmi.example.ts.net:10000"));
        assert!(!rmi_made("http://rmi.example.ts.net"));
        assert!(!rmi_made("https://rmi.example.ts.net.evil.example"));
        assert!(!rmi_made("https://friend.example.com"));
        assert!(!rmi_made(""));
    }

    #[test]
    fn cli_errors_map_to_plain_text() {
        let m = fail_message("flag provided but not defined: -yes");
        assert!(m.starts_with("Update Tailscale"));
        assert!(
            fail_message("Access denied: serve config denied").contains("signed in as the owner")
        );
        assert!(fail_message("needs root permission").contains("signed in as the owner"));
        let other = fail_message("500 internal: some backend text");
        assert_eq!(other, "Tailscale could not turn on the link. Try again.");
        assert!(!other.contains("backend"));
    }

    #[test]
    fn enable_url_found_other_text_ignored() {
        let out = "Funnel is not enabled on your tailnet.\nTo enable, visit:\n\n         https://login.tailscale.com/f/funnel?node=nEXAMPLE\n";
        assert_eq!(
            parse_enable_url(out).as_deref(),
            Some("https://login.tailscale.com/f/funnel?node=nEXAMPLE")
        );
        assert_eq!(
            parse_enable_url("see https://tailscale.com/kb/1223/funnel."),
            None
        );
        assert_eq!(
            parse_enable_url(
                "Funnel not available; visit:
  https://tailscale.com/s/funnel-nodeattr)."
            )
            .as_deref(),
            Some("https://tailscale.com/s/funnel-nodeattr")
        );
        assert_eq!(parse_enable_url(""), None);
    }

    #[test]
    fn public_url_drops_443() {
        assert_eq!(
            public_url("rmi.example.ts.net", 443),
            "https://rmi.example.ts.net"
        );
        assert_eq!(
            public_url("rmi.example.ts.net", 8443),
            "https://rmi.example.ts.net:8443"
        );
    }
}
