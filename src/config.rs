use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const DEFAULT_LISTEN_PORT: u16 = 80;
pub const DEFAULT_FALLBACK_PORT: u16 = 7777;
pub const DEFAULT_UPSTREAM: &str = "127.0.0.2:80";
pub const UNIT_NAME: &str = "omaportless.service";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen_port: u16,
    #[serde(default = "default_fallback")]
    pub fallback_port: u16,
    #[serde(default = "default_upstream")]
    pub upstream: String,
    #[serde(default)]
    pub names: HashMap<String, String>,
}

fn default_listen() -> u16 {
    DEFAULT_LISTEN_PORT
}
fn default_fallback() -> u16 {
    DEFAULT_FALLBACK_PORT
}
fn default_upstream() -> String {
    DEFAULT_UPSTREAM.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_port: DEFAULT_LISTEN_PORT,
            fallback_port: DEFAULT_FALLBACK_PORT,
            upstream: DEFAULT_UPSTREAM.to_string(),
            names: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub public_port: u16,
    #[serde(default)]
    pub fallback: bool,
    #[serde(default)]
    pub error: String,
    #[serde(default = "default_bind")]
    pub bind: String,
}

fn default_bind() -> String {
    "127.0.0.1".into()
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

pub fn config_dir() -> PathBuf {
    home_dir().join(".config/omaportless")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn state_path() -> PathBuf {
    config_dir().join("state.json")
}

pub fn unit_path() -> PathBuf {
    home_dir().join(".config/systemd/user").join(UNIT_NAME)
}

pub fn load_config() -> Config {
    let path = config_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Config::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_config(cfg: &Config) -> std::io::Result<()> {
    atomic_write(&config_path(), &serde_json::to_string_pretty(cfg).unwrap())
}

pub fn load_state() -> State {
    let Ok(text) = fs::read_to_string(state_path()) else {
        return State::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_state(state: &State) -> std::io::Result<()> {
    atomic_write(&state_path(), &serde_json::to_string_pretty(state).unwrap())
}

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;

/// Replaces `path` with `contents` atomically.
///
/// The temporary file is created inside the destination directory with an
/// unpredictable name and `O_CREAT | O_EXCL | O_NOFOLLOW`, so a symlink planted
/// at the temporary path cannot redirect the write to another file.
fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(parent)?;
    }

    // rename() replaces the destination without following it, so checking the
    // link itself here cannot be raced into a write through a symlink.
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("{} is not a regular file", path.display()),
            ))
        }
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let (mut file, tmp) = create_exclusive_temp(parent, path)?;
    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    // Make the new directory entry itself survive a crash.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn create_exclusive_temp(parent: &Path, path: &Path) -> std::io::Result<(fs::File, PathBuf)> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("omaportless");
    for _ in 0..16 {
        let tmp = parent.join(format!(".{name}.{}.tmp", unique_token()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)
        {
            Ok(file) => {
                // mode() above is masked by umask; pin it down on the open fd.
                file.set_permissions(fs::Permissions::from_mode(FILE_MODE))?;
                return Ok((file, tmp));
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        "could not create a unique temporary file",
    ))
}

fn unique_token() -> String {
    let mut bytes = [0u8; 8];
    let random = fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if random.is_ok() {
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{:x}-{:x}-{:x}",
        std::process::id(),
        nanos,
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

pub fn parse_upstream(value: &str) -> Option<(String, u16)> {
    let text = value.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':').and_then(|s| s.parse().ok()).unwrap_or(80);
        return Some((host.to_string(), port));
    }
    if let Some((host, port)) = text.rsplit_once(':') {
        if !host.contains(':') {
            return Some((host.to_string(), port.parse().ok()?));
        }
    }
    Some((text.to_string(), 80))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omaportless-test-{}", unique_token()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_contents_with_owner_only_permissions() {
        let dir = scratch_dir();
        let path = dir.join("config.json");
        atomic_write(&path, "{}").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, FILE_MODE);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn leaves_no_temporary_file_behind() {
        let dir = scratch_dir();
        let path = dir.join("state.json");
        atomic_write(&path, "{}").unwrap();
        atomic_write(&path, "{\"pid\":1}").unwrap();

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|name| name != "state.json")
            .collect();
        assert!(leftovers.is_empty(), "unexpected leftovers: {leftovers:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn does_not_write_through_a_planted_temporary_path() {
        let dir = scratch_dir();
        let path = dir.join("config.json");
        let victim = dir.join("victim");
        fs::write(&victim, "keep me\n").unwrap();
        // The old implementation used this exact predictable path.
        symlink(&victim, dir.join("config.tmp")).unwrap();

        atomic_write(&path, "{\"listen_port\":80}").unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me\n");
        assert!(fs::read_to_string(&path).unwrap().contains("listen_port"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_a_symlinked_destination() {
        let dir = scratch_dir();
        let victim = dir.join("victim");
        fs::write(&victim, "keep me\n").unwrap();
        let path = dir.join("config.json");
        symlink(&victim, &path).unwrap();

        let err = atomic_write(&path, "{}").unwrap_err();

        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
