use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io::{Error, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
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
/// The destination directory is opened once with `O_NOFOLLOW`, and every later
/// step acts on that descriptor rather than on a pathname. A symlink planted at
/// the config directory or at the temporary file therefore cannot redirect the
/// write outside the directory this resolved to.
fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let dest = entry_name(path)?;

    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(parent)?;
    let dir = open_dir(parent)?;

    if entry_mode(&dir, &dest)?.is_some_and(|mode| mode != libc::S_IFREG) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }

    let (mut file, tmp) = create_temp(&dir, path)?;
    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written.and_then(|()| rename_at(&dir, &tmp, &dest)) {
        let _ = unlink_at(&dir, &tmp);
        return Err(e);
    }

    // Make the new directory entry itself survive a crash.
    let _ = dir.sync_all();
    Ok(())
}

/// Opens `dir` refusing to traverse a final symlink, then confirms the opened
/// directory belongs to this user. The first check stops the config directory
/// itself from being swapped for a link; the second stops a redirect through
/// any earlier path component, which `O_NOFOLLOW` does not cover.
fn open_dir(dir: &Path) -> std::io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) | Some(libc::ENOTDIR) => Error::new(
                ErrorKind::InvalidInput,
                format!("{} is not a directory", dir.display()),
            ),
            _ => e,
        })?;

    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(file.as_raw_fd(), st.as_mut_ptr()) } != 0 {
        return Err(Error::last_os_error());
    }
    let owner = unsafe { st.assume_init() }.st_uid;
    if owner != unsafe { libc::geteuid() } {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            format!("{} is not owned by this user", dir.display()),
        ));
    }
    Ok(file)
}

fn create_temp(dir: &fs::File, path: &Path) -> std::io::Result<(fs::File, CString)> {
    let base = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("omaportless");
    for _ in 0..16 {
        let name = to_cstring(format!(".{base}.{}.tmp", unique_token()).as_bytes())?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                FILE_MODE as libc::c_uint,
            )
        };
        if fd >= 0 {
            let file = unsafe { fs::File::from_raw_fd(fd) };
            // The mode passed to openat is masked by umask; pin it on the fd.
            file.set_permissions(fs::Permissions::from_mode(FILE_MODE))?;
            return Ok((file, name));
        }
        let e = Error::last_os_error();
        if e.kind() != ErrorKind::AlreadyExists {
            return Err(e);
        }
    }
    Err(Error::new(
        ErrorKind::AlreadyExists,
        "could not create a unique temporary file",
    ))
}

/// Returns the `S_IFMT` bits of `name` inside `dir`, or `None` when absent.
/// The link itself is inspected, never its target.
fn entry_mode(dir: &fs::File, name: &CString) -> std::io::Result<Option<u32>> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    let rc = unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            st.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc != 0 {
        let e = Error::last_os_error();
        return if e.kind() == ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(e)
        };
    }
    Ok(Some(unsafe { st.assume_init() }.st_mode & libc::S_IFMT))
}

fn rename_at(dir: &fs::File, from: &CString, to: &CString) -> std::io::Result<()> {
    let fd = dir.as_raw_fd();
    let rc = unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) };
    if rc != 0 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

fn unlink_at(dir: &fs::File, name: &CString) -> std::io::Result<()> {
    let rc = unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) };
    if rc != 0 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

fn entry_name(path: &Path) -> std::io::Result<CString> {
    let name = path.file_name().ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        )
    })?;
    to_cstring(name.as_bytes())
}

fn to_cstring(bytes: &[u8]) -> std::io::Result<CString> {
    CString::new(bytes).map_err(|_| {
        Error::new(
            ErrorKind::InvalidInput,
            "file name contains an interior NUL byte",
        )
    })
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
    fn does_not_write_through_a_symlinked_config_directory() {
        let dir = scratch_dir();
        let elsewhere = dir.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let config_dir = dir.join("omaportless");
        symlink(&elsewhere, &config_dir).unwrap();

        let err = atomic_write(&config_dir.join("config.json"), "{}").unwrap_err();

        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn creates_a_missing_config_directory() {
        let dir = scratch_dir();
        let path = dir.join("nested/omaportless/config.json");
        atomic_write(&path, "{}").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
        let mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, DIR_MODE);
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
