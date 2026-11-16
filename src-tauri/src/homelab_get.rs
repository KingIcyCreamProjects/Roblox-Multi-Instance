//! HomeLab installer: finds the newest GitHub release and downloads its installer to the
//! Downloads folder, checked against the release's SHA256SUMS. Windows then opens it; macOS
//! and Linux only reveal the file, nothing runs.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;
use tauri_plugin_opener::OpenerExt;

const LATEST: &str = "https://api.github.com/repos/KingIcyCreamProjects/HomeLab/releases/latest";
/// Where the first download URL must point (host and path).
const RELEASE_DL: &str = "/KingIcyCreamProjects/HomeLab/releases/download/";
/// Hosts every redirect hop may come from.
const HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const UA: &str = concat!("Roblox-Multi-Instance/", env!("CARGO_PKG_VERSION"));
const SUMS: &str = "SHA256SUMS";
const SUMS_MAX: u64 = 64 * 1024;
/// No installer for this OS and CPU (or the release has none yet).
const NO_BUILD: &str = "HOMELAB_NO_BUILD";

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HomeLabRelease {
    pub version: String,
    pub asset: String,
    pub size: u64,
    /// The release page on GitHub.
    pub page: String,
}

/// Installer file name per `std::env::consts::OS` and `ARCH`.
fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("HomeLabSetup.exe"),
        ("linux", "x86_64") => Some("homelab-linux-amd64"),
        ("macos", "aarch64") => Some("homelab-darwin-arm64"),
        _ => None,
    }
}

#[derive(Debug)]
struct Picked {
    rel: HomeLabRelease,
    url: String,
    /// The release's SHA256SUMS download URL, if it has one.
    sums: Option<(String, u64)>,
}

/// The release, the asset's download URL and the SHA256SUMS one. Both URLs must be this
/// repo's release downloads on github.com.
fn pick(release_json: &str, os: &str, arch: &str) -> Result<Picked, &'static str> {
    const BAD: &str = "Could not read the answer.";
    let v: Value = serde_json::from_str(release_json).map_err(|_| BAD)?;
    let assets = v["assets"].as_array().ok_or(BAD)?;
    let name = asset_name(os, arch).ok_or(NO_BUILD)?;
    let find = |n: &str| assets.iter().find(|a| a["name"] == n);
    let a = find(name).ok_or(NO_BUILD)?;
    let url = a["browser_download_url"].as_str().ok_or(BAD)?;
    if !release_url(url) {
        return Err("The download address is not on GitHub.");
    }
    let sums = find(SUMS).and_then(|s| {
        let u = s["browser_download_url"]
            .as_str()
            .filter(|u| release_url(u))?;
        Some((u.to_owned(), s["size"].as_u64()?))
    });
    let rel = HomeLabRelease {
        version: v["tag_name"]
            .as_str()
            .ok_or(BAD)?
            .trim_start_matches('v')
            .to_owned(),
        asset: name.to_owned(),
        size: a["size"].as_u64().ok_or(BAD)?,
        page: v["html_url"].as_str().ok_or(BAD)?.to_owned(),
    };
    Ok(Picked {
        rel,
        url: url.to_owned(),
        sums,
    })
}

/// The release for this OS and CPU from a GitHub "latest release" JSON.
pub fn pick_asset(
    release_json: &str,
    os: &str,
    arch: &str,
) -> Result<HomeLabRelease, &'static str> {
    pick(release_json, os, arch).map(|p| p.rel)
}

/// https on an allowed host (redirect hops).
fn host_ok(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .is_some_and(|u| u.scheme() == "https" && u.host_str().is_some_and(|h| HOSTS.contains(&h)))
}

/// The first download URL: https://github.com/KingIcyCreamProjects/HomeLab/releases/download/...
fn release_url(url: &str) -> bool {
    url::Url::parse(url).ok().is_some_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("github.com")
            && u.port().is_none()
            && u.username().is_empty()
            && u.path().starts_with(RELEASE_DL)
    })
}

/// The lowercase SHA-256 hex for `file` in a SHA256SUMS text (`<hex>  <name>`, or `<hex> *<name>`).
fn sum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hex, name) = l.trim_end().split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*');
        (name == file && hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hex.to_ascii_lowercase())
    })
}

/// GitHub's "latest release" JSON.
async fn latest_body() -> Result<String, String> {
    static C: LazyLock<Option<reqwest::Client>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(10))
            .build()
            .ok()
    });
    let c = C.as_ref().ok_or("No network client.")?;
    let r = c
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| "Could not reach GitHub.")?;
    // 404 until the first HomeLab release exists; 403 when rate limited
    if !r.status().is_success() {
        return Err(format!("GitHub answered {}.", r.status().as_u16()));
    }
    r.text()
        .await
        .map_err(|_| "Could not read the answer.".into())
}

fn current() -> (&'static str, &'static str) {
    (std::env::consts::OS, std::env::consts::ARCH)
}

#[tauri::command]
pub async fn homelab_latest() -> Result<HomeLabRelease, String> {
    let (os, arch) = current();
    if asset_name(os, arch).is_none() {
        return Err(NO_BUILD.into()); // no request needed
    }
    pick_asset(&latest_body().await?, os, arch).map_err(Into::into)
}

/// Download client: every redirect hop stays on the allowed hosts.
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(900))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 5 {
                a.error("too many redirects")
            } else if host_ok(a.url().as_str()) {
                a.follow()
            } else {
                a.error("download moved to an untrusted host")
            }
        }))
        .build()
        .map_err(|e| e.to_string())
}

async fn get(c: &reqwest::Client, url: &str) -> Result<reqwest::Response, String> {
    let r = c
        .get(url)
        .send()
        .await
        .map_err(|_| "The download was refused or failed.")?;
    if !r.status().is_success() || !host_ok(r.url().as_str()) {
        return Err("The download was refused.".into());
    }
    Ok(r)
}

/// The release's SHA256SUMS text (at most 64 KB).
async fn fetch_sums(c: &reqwest::Client, url: &str, size: u64) -> Result<String, String> {
    const BAD: &str = "Could not read the release's SHA256SUMS.";
    if size > SUMS_MAX {
        return Err(BAD.into());
    }
    let mut r = get(c, url).await?;
    let mut b = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|_| BAD)? {
        b.extend_from_slice(&chunk);
        if b.len() as u64 > SUMS_MAX {
            return Err(BAD.into());
        }
    }
    String::from_utf8(b).map_err(|_| BAD.into())
}

/// Streams `url` into `part`; refuses other hosts and any size but `size`. Returns the SHA-256 hex.
async fn fetch(
    c: &reqwest::Client,
    url: &str,
    size: u64,
    part: &std::path::Path,
) -> Result<String, String> {
    let mut r = get(c, url).await?;
    let mut f = std::fs::File::create(part)
        .map_err(|e| format!("Could not write to the Downloads folder: {e}"))?;
    let mut h = Sha256::new();
    let mut got = 0u64;
    while let Some(chunk) = r
        .chunk()
        .await
        .map_err(|_| "The download was interrupted.")?
    {
        got += chunk.len() as u64;
        if got > size {
            return Err("The download is bigger than expected.".into());
        }
        h.update(&chunk);
        f.write_all(&chunk)
            .map_err(|e| format!("Could not write to the Downloads folder: {e}"))?;
    }
    f.flush().map_err(|e| e.to_string())?;
    if got != size {
        return Err("The download is incomplete.".into());
    }
    Ok(hex(&h.finalize()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `Ok` only when `got` is the release's published hash for the file.
fn check_sum(want: Option<String>, got: &str) -> Result<(), &'static str> {
    match want {
        None => Err("The release has no checksum for this installer, so it was not kept."),
        Some(w) if w == got => Ok(()),
        Some(_) => Err("The download does not match the release's checksum, so it was deleted."),
    }
}

/// One download at a time.
static BUSY: AtomicBool = AtomicBool::new(false);
struct Busy;
impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

/// Saves the installer to Downloads under its own file name (an older copy is replaced) once
/// its SHA-256 matches the release's SHA256SUMS, and returns the path. Windows marks it as
/// downloaded from the internet and opens it; elsewhere it is shown in the file manager.
#[tauri::command]
pub async fn homelab_download(app: tauri::AppHandle) -> Result<String, String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("A HomeLab download is already running.".into());
    }
    let _busy = Busy;
    let (os, arch) = current();
    let p = pick(&latest_body().await?, os, arch)?;
    let (sums_url, sums_size) = p
        .sums
        .clone()
        .ok_or("The release has no SHA256SUMS, so the installer was not downloaded.")?;
    let c = client()?;
    let want = sum_for(&fetch_sums(&c, &sums_url, sums_size).await?, &p.rel.asset);
    let dir = dirs::download_dir().ok_or("Could not find your Downloads folder.")?;
    let dest = dir.join(&p.rel.asset);
    let part = dir.join(format!("{}.part", p.rel.asset));
    let checked = match fetch(&c, &p.url, p.rel.size, &part).await {
        Ok(got) => check_sum(want, &got).map_err(String::from),
        Err(e) => Err(e),
    };
    if let Err(e) = checked {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&part, &dest) {
        let _ = std::fs::remove_file(&part);
        return Err(format!("Could not replace the older download: {e}"));
    }
    let path = dest.display().to_string();
    let opened = if cfg!(windows) {
        // Mark of the Web: SmartScreen and the UAC prompt treat it as from the internet
        let zone = format!("{path}:Zone.Identifier");
        if let Err(e) = std::fs::write(&zone, "[ZoneTransfer]\r\nZoneId=3\r\n") {
            log::warn!("could not mark the HomeLab installer as downloaded: {e}");
        }
        app.opener().open_path(&path, None::<&str>)
    } else {
        app.opener().reveal_item_in_dir(&dest)
    };
    opened.map_err(|e| format!("Downloaded to {path}, but it could not be opened: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DL: &str = "https://github.com/KingIcyCreamProjects/HomeLab/releases/download/v1.4.0";
    fn rel() -> String {
        format!(
            r#"{{
      "tag_name": "v1.4.0",
      "html_url": "https://github.com/KingIcyCreamProjects/HomeLab/releases/tag/v1.4.0",
      "assets": [
        {{"name":"SHA256SUMS","size":300,"browser_download_url":"{DL}/SHA256SUMS"}},
        {{"name":"HomeLabSetup.exe","size":123456,"browser_download_url":"{DL}/HomeLabSetup.exe"}},
        {{"name":"homelab-darwin-arm64","size":222,"browser_download_url":"{DL}/homelab-darwin-arm64"}},
        {{"name":"homelab-linux-amd64","size":333,"browser_download_url":"{DL}/homelab-linux-amd64"}}
      ]
    }}"#
        )
    }

    #[test]
    fn picks_asset_per_os_and_cpu() {
        let w = pick_asset(&rel(), "windows", "x86_64").unwrap();
        assert_eq!(
            (w.version.as_str(), w.asset.as_str(), w.size),
            ("1.4.0", "HomeLabSetup.exe", 123_456)
        );
        assert!(w.page.ends_with("/tag/v1.4.0"));
        assert_eq!(
            pick_asset(&rel(), "macos", "aarch64").unwrap().asset,
            "homelab-darwin-arm64"
        );
        assert_eq!(pick_asset(&rel(), "linux", "x86_64").unwrap().size, 333);
        for (os, arch) in [
            ("freebsd", "x86_64"),
            ("windows", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "aarch64"),
        ] {
            assert_eq!(pick_asset(&rel(), os, arch), Err(NO_BUILD), "{os} {arch}");
        }
    }

    #[test]
    fn missing_asset_or_bad_json() {
        let no_exe = rel().replace("HomeLabSetup.exe", "other.exe");
        assert_eq!(pick_asset(&no_exe, "windows", "x86_64"), Err(NO_BUILD));
        assert!(pick_asset(r#"{"message":"Not Found"}"#, "windows", "x86_64").is_err());
        assert!(pick_asset("not json", "windows", "x86_64").is_err());
    }

    #[test]
    fn urls_come_from_this_repos_release() {
        let p = pick(&rel(), "windows", "x86_64").unwrap();
        assert!(release_url(&p.url));
        assert_eq!(p.sums.unwrap().0, format!("{DL}/SHA256SUMS"));
        // another repo or host for the asset: refused; for SHA256SUMS: treated as missing
        let other = rel().replace(
            &format!("{DL}/HomeLabSetup.exe"),
            "https://github.com/someone/else/releases/download/v1/HomeLabSetup.exe",
        );
        assert!(pick(&other, "windows", "x86_64").is_err());
        let sums = rel().replace(
            &format!("{DL}/SHA256SUMS"),
            "https://objects.githubusercontent.com/SHA256SUMS",
        );
        assert!(pick(&sums, "windows", "x86_64").unwrap().sums.is_none());
    }

    #[test]
    fn release_url_needs_github_and_repo_path() {
        assert!(release_url(&format!("{DL}/HomeLabSetup.exe")));
        assert!(!release_url("https://objects.githubusercontent.com/KingIcyCreamProjects/HomeLab/releases/download/x"));
        assert!(!release_url(
            "https://github.com/KingIcyCreamProjects/HomeLab/archive/x.zip"
        ));
        assert!(!release_url(
            "http://github.com/KingIcyCreamProjects/HomeLab/releases/download/x"
        ));
        assert!(!release_url(
            "https://github.com:444/KingIcyCreamProjects/HomeLab/releases/download/x"
        ));
        assert!(!release_url("not a url"));
    }

    #[test]
    fn host_check_refuses_other_hosts() {
        assert!(host_ok("https://github.com/a/b/releases/download/v1/x.exe"));
        assert!(host_ok("https://objects.githubusercontent.com/abc"));
        assert!(!host_ok("http://github.com/x"));
        assert!(!host_ok("https://evil.example/github.com"));
        assert!(!host_ok("https://github.com.evil.example/x"));
        assert!(!host_ok("https://user@evil.example/x"));
        assert!(!host_ok("not a url"));
    }

    const A: &str = "a3f1c2d4e5b6a7980123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn sums_parser() {
        let sums = format!(
            "{A}  HomeLabSetup.exe\n{}  homelab-linux-amd64\n",
            "0".repeat(64)
        );
        assert_eq!(sum_for(&sums, "HomeLabSetup.exe").as_deref(), Some(A));
        assert_eq!(sum_for(&sums, "homelab-darwin-arm64"), None, "missing file");
        let star = format!("{}  *HomeLabSetup.exe\r\n", A.to_uppercase());
        assert_eq!(sum_for(&star, "HomeLabSetup.exe").as_deref(), Some(A));
        // malformed: short hash, non-hex, no name
        assert_eq!(
            sum_for("abc123  HomeLabSetup.exe", "HomeLabSetup.exe"),
            None
        );
        let nonhex = format!("{}  HomeLabSetup.exe", "z".repeat(64));
        assert_eq!(sum_for(&nonhex, "HomeLabSetup.exe"), None);
        assert_eq!(sum_for(A, "HomeLabSetup.exe"), None);
        assert_eq!(sum_for("", "HomeLabSetup.exe"), None);
    }

    #[test]
    fn checksum_mismatch_or_missing_refuses() {
        let got = hex(&Sha256::digest(b"installer bytes"));
        assert_eq!(check_sum(Some(got.clone()), &got), Ok(()));
        assert!(check_sum(Some(A.into()), &got).is_err());
        assert!(check_sum(None, &got).is_err());
        // known vector
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn busy_guard_releases_on_drop() {
        assert!(!BUSY.swap(true, Ordering::SeqCst));
        drop(Busy);
        assert!(!BUSY.load(Ordering::SeqCst));
    }
}
