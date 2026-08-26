use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

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

fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.write_all(b"\n")?;
    }
    fs::rename(tmp, path)
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
