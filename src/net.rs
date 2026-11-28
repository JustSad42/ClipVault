//! The only network use: website icons for link cards (straight from the site), and updates from
//! this app's GitHub releases.

use crate::images::Bitmap;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const REPO: &str = "JustSad42/ClipVault";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(seconds)))
        // Windows' own TLS, so no TLS library is bundled in the exe
        .tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build())
        .build()
        .into()
}

fn get(url: &str, agent_name: &str, limit: u64, seconds: u64) -> Result<Vec<u8>, ureq::Error> {
    let mut response = agent(seconds).get(url).header("User-Agent", agent_name).call()?;
    let mut out = Vec::new();
    response.body_mut().as_reader().take(limit + 1).read_to_end(&mut out)?;
    Ok(out)
}

// ---------- site icons ----------

pub fn favicon_file(host: &str) -> Option<PathBuf> {
    // a host name only: never let it name a path
    let safe = !host.is_empty() && host.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '-')) && !host.starts_with('.');
    safe.then(|| crate::settings::dir().join("icons").join(format!("{host}.ico")))
}

/// Downloads https://site/favicon.ico once and caches it; a failure is remembered too.
pub fn fetch_favicon(scheme: &str, host: &str) -> Option<Bitmap> {
    let file = favicon_file(host)?;
    let bytes = get(&format!("{scheme}://{host}/favicon.ico"), "Mozilla/5.0 ClipVault", 512_000, 5).ok();
    let icon = bytes.as_deref().filter(|b| !b.is_empty() && b.len() <= 512_000).and_then(|b| Bitmap::decode_icon(b, 32).ok());
    let _ = std::fs::create_dir_all(file.parent()?);
    match (&icon, &bytes) {
        (Some(_), Some(b)) => {
            let _ = crate::win::write_atomic(&file, b);
        }
        _ => {
            let _ = std::fs::write(format!("{}.none", file.display()), "");
        }
    }
    icon
}

// ---------- updates ----------

#[derive(Default)]
pub struct Update {
    pub status: String,
    /// A verified new version, downloaded next to the exe and waiting to be swapped in.
    pub ready: Option<String>,
    busy: bool,
}

pub static UPDATE: Mutex<Update> = Mutex::new(Update { status: String::new(), ready: None, busy: false });

pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.trim().trim_start_matches(['v', 'V']).split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Dev builds (run from a target folder) never update themselves.
pub fn can_self_update(exe: &Path) -> bool {
    exe.file_name().is_some_and(|n| n.eq_ignore_ascii_case("ClipVault.exe"))
        && !exe.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target"))
        && !crate::settings::is_test()
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

fn set_status(text: impl Into<String>) {
    UPDATE.lock().unwrap().status = text.into();
    crate::app::post(crate::app::Event::UpdateStatus);
}

/// Looks for a newer release and downloads it. Runs on its own thread; safe to call often.
pub fn check_in_background(manual: bool) {
    {
        let mut u = UPDATE.lock().unwrap();
        if u.busy || u.ready.is_some() {
            return;
        }
        u.busy = true;
    }
    std::thread::spawn(move || {
        set_status("Checking for updates…");
        let outcome = check().unwrap_or_else(|e| if manual { format!("Couldn't check for updates: {e}") } else { "Couldn't check for updates.".into() });
        UPDATE.lock().unwrap().busy = false;
        set_status(outcome);
    });
}

/// `Ok`: what to tell the user.
fn check() -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = match get(&url, "ClipVault-updater", 2 << 20, 60) {
        Ok(b) => b,
        Err(ureq::Error::StatusCode(404)) => return Ok("No releases published yet.".into()),
        Err(e) => return Err(e.to_string()),
    };
    let release: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let tag = release["tag_name"].as_str().unwrap_or("");
    let latest = parse_version(tag).ok_or("couldn't read the latest version")?;
    if latest <= parse_version(VERSION).unwrap_or_default() {
        return Ok(format!("You have the latest version ({VERSION})."));
    }
    let version = format!("{}.{}.{}", latest.0, latest.1, latest.2);
    let assets = release["assets"].as_array().cloned().unwrap_or_default();
    let asset = |name: &str| assets.iter().find(|a| a["name"] == name);
    let Some(exe_asset) = asset("ClipVault.exe") else { return Ok(format!("Version {version} is out, but has no download yet.")) };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if !can_self_update(&exe) {
        return Ok(format!("Version {version} is available on GitHub."));
    }
    // the expected hash: the .sha256 file, else GitHub's own digest. Never install an unverified file.
    let from_file = match asset("ClipVault.exe.sha256").and_then(|a| a["browser_download_url"].as_str()) {
        Some(u) => Some(String::from_utf8_lossy(&get(u, "ClipVault-updater", 4096, 60).map_err(|e| e.to_string())?).split_whitespace().next().unwrap_or("").to_string()),
        None => None,
    };
    let expected = from_file.or_else(|| exe_asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(String::from)).unwrap_or_default();
    if expected.len() != 64 {
        return Ok(format!("Version {version} has no checksum, so it wasn't installed."));
    }
    let download = exe_asset["browser_download_url"].as_str().ok_or("no download address")?;
    set_status(format!("Downloading version {version}…"));
    let bytes = get(download, "ClipVault-updater", 200 << 20, 600).map_err(|e| e.to_string())?;
    let actual: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if !actual.eq_ignore_ascii_case(&expected) {
        return Ok("The download didn't match its checksum, so it wasn't installed.".into());
    }
    std::fs::write(sibling(&exe, ".new"), bytes).map_err(|e| e.to_string())?;
    UPDATE.lock().unwrap().ready = Some(version.clone());
    Ok(format!("Version {version} is ready. It installs when the popup closes."))
}

/// Puts the downloaded version where `exe` is, keeping the old one beside it.
pub fn swap(exe: &Path) -> std::io::Result<()> {
    let (new, old) = (sibling(exe, ".new"), sibling(exe, ".old"));
    if !new.exists() {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    }
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)?; // Windows allows renaming a running exe
    std::fs::rename(&new, exe).inspect_err(|_| {
        let _ = std::fs::rename(&old, exe);
    })
}

/// Swaps in the downloaded version and starts it. `true`: the caller should now quit.
pub fn apply_and_restart() -> bool {
    if UPDATE.lock().unwrap().ready.is_none() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    if let Err(e) = swap(&exe) {
        crate::win::log(&format!("update: {e}"));
        UPDATE.lock().unwrap().ready = None;
        set_status("Couldn't install the update (is the folder read-only?).");
        return false;
    }
    let pid = std::process::id().to_string();
    std::process::Command::new(&exe).args(["--background", "--updated", "--wait-for", &pid]).spawn().is_ok()
}

/// At start-up: removes what an update left behind (off the UI thread; the old exe may still be closing).
pub fn clean_up() {
    std::thread::spawn(|| {
        let Ok(exe) = std::env::current_exe() else { return };
        for _ in 0..20 {
            let old = std::fs::remove_file(sibling(&exe, ".old"));
            if UPDATE.lock().unwrap().ready.is_none() {
                let _ = std::fs::remove_file(sibling(&exe, ".new"));
            }
            match old {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => std::thread::sleep(Duration::from_millis(500)),
                _ => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_names_and_swapping() {
        assert!(parse_version("v2.0.0") > parse_version("1.9.12"));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert!(favicon_file("example.com").is_some());
        assert!(favicon_file(r"..\..\x").is_none() && favicon_file("a/b").is_none());
        assert!(!can_self_update(Path::new(r"C:\x\target\release\ClipVault.exe")));
        let d = std::env::temp_dir().join(format!("clipvault-swap-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let exe = d.join("ClipVault.exe");
        std::fs::write(&exe, "old").unwrap();
        assert!(swap(&exe).is_err(), "nothing downloaded");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        std::fs::write(d.join("ClipVault.exe.new"), "new").unwrap();
        swap(&exe).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(d.join("ClipVault.exe.old")).unwrap(), "old");
        let _ = std::fs::remove_dir_all(d);
    }
}
