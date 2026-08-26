use std::collections::HashMap;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const SKIP_COMM: &[&str] = &[
    "apache2",
    "avahi-daemon",
    "caddy",
    "clash-verge",
    "cupsd",
    "dropbox",
    "httpd",
    "moshi",
    "moshi-hook",
    "nginx",
    "sshd",
    "systemd",
    "systemd-resolved",
];

#[derive(Clone, Debug)]
pub struct Listener {
    pub port: u16,
    pub pid: i32,
    pub bind: String,
    pub comm: String,
    pub cmd: String,
    pub cwd: String,
    pub starttime: u64,
}

pub fn parse_ipv4_port(addr: &str) -> Option<(String, u16)> {
    let (ip_hex, port_hex) = addr.split_once(':')?;
    let ip = u32::from_str_radix(ip_hex, 16).ok()?;
    let port = u16::from_str_radix(port_hex, 16).ok()?;
    Some((Ipv4Addr::from(ip.to_le_bytes()).to_string(), port))
}

pub fn parse_ipv6_port(addr: &str) -> Option<(String, u16)> {
    let (ip_hex, port_hex) = addr.split_once(':')?;
    if ip_hex.len() != 32 {
        return None;
    }
    let raw = hex_decode(ip_hex)?;
    let mut words = [0u8; 16];
    for i in 0..4 {
        let chunk = &raw[i * 4..i * 4 + 4];
        words[i * 4] = chunk[3];
        words[i * 4 + 1] = chunk[2];
        words[i * 4 + 2] = chunk[1];
        words[i * 4 + 3] = chunk[0];
    }
    let port = u16::from_str_radix(port_hex, 16).ok()?;
    Some((Ipv6Addr::from(words).to_string(), port))
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

pub fn is_local_bind(ip: &str) -> bool {
    matches!(ip, "0.0.0.0" | "::" | "::0" | "*")
        || ip.starts_with("127.")
        || matches!(ip, "::1" | "https://example.net/id/garnet")
}

fn inode_to_pid() -> HashMap<u64, i32> {
    let mut mapping = HashMap::new();
    let Ok(pids) = fs::read_dir("/proc") else {
        return mapping;
    };
    for entry in pids.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        let fd_dir = Path::new("/proc").join(pid.to_string()).join("fd");
        let Ok(fds) = fs::read_dir(&fd_dir) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            if let Some(rest) = target.to_string_lossy().strip_prefix("socket:[") {
                if let Ok(inode) = rest.trim_end_matches(']').parse::<u64>() {
                    mapping.insert(inode, pid);
                }
            }
        }
    }
    mapping
}

pub fn proc_field(pid: i32, name: &str) -> String {
    let path = Path::new("/proc").join(pid.to_string()).join(name);
    if name == "cwd" {
        return fs::read_link(&path)
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_default();
    }
    let Ok(raw) = fs::read(&path) else {
        return String::new();
    };
    if name == "cmdline" {
        return raw
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
    }
    String::from_utf8_lossy(&raw).trim().to_string()
}

fn proc_starttime(pid: i32) -> u64 {
    let path = format!("/proc/{pid}/stat");
    let Ok(stat) = fs::read_to_string(path) else {
        return 0;
    };
    let close = match stat.rfind(')') {
        Some(i) => i,
        None => return 0,
    };
    let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
    fields.get(19).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn proc_uid(pid: i32) -> Option<u32> {
    fs::metadata(format!("/proc/{pid}")).ok().map(|m| m.uid())
}

pub fn proc_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    let comm = proc_field(pid, "comm");
    let cmd = proc_field(pid, "cmdline");
    comm == "omaportless" || cmd.contains("omaportless")
}

pub fn scan_listeners(skip_ports: &[u16]) -> Vec<Listener> {
    let inodes = inode_to_pid();
    let uid = unsafe { libc::getuid() };
    let mut found: HashMap<u16, Listener> = HashMap::new();
    for (path, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 10 || parts[3] != "0A" {
                continue;
            }
            let parsed = if ipv6 {
                parse_ipv6_port(parts[1])
            } else {
                parse_ipv4_port(parts[1])
            };
            let Some((ip, port)) = parsed else {
                continue;
            };
            if port == 0 || skip_ports.contains(&port) {
                continue;
            }
            if !is_local_bind(&ip) {
                continue;
            }
            let Ok(inode) = parts[9].parse::<u64>() else {
                continue;
            };
            let Some(&pid) = inodes.get(&inode) else {
                continue;
            };
            if let Some(owner) = proc_uid(pid) {
                if owner != uid {
                    continue;
                }
            }
            let comm = proc_field(pid, "comm");
            if SKIP_COMM.contains(&comm.as_str()) {
                continue;
            }
            if found.get(&port).map(|e| e.pid) == Some(pid) {
                continue;
            }
            found.insert(
                port,
                Listener {
                    port,
                    pid,
                    bind: ip,
                    comm,
                    cmd: proc_field(pid, "cmdline"),
                    cwd: proc_field(pid, "cwd"),
                    starttime: proc_starttime(pid),
                },
            );
        }
    }
    let mut list: Vec<_> = found.into_values().collect();
    list.sort_by_key(|l| l.port);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_loopback_3000() {
        let (ip, port) = parse_ipv4_port("0100007F:0BB8").unwrap();
        assert_eq!(ip, "127.0.0.1");
        assert_eq!(port, 3000);
    }
}
