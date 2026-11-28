//! Small Windows helpers shared by everything.

use std::path::Path;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The text up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())])
}

/// Writes a whole file or leaves the old one: a crash never leaves half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

pub fn log(message: &str) {
    use std::io::Write;
    let path = crate::settings::dir().join("clipvault.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 200_000) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {message}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}
