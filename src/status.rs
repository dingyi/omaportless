use std::process::Command;

use serde_json::{json, Value};

use crate::config::{
    load_config, load_state, unit_path, DEFAULT_FALLBACK_PORT, DEFAULT_LISTEN_PORT, UNIT_NAME,
};
use crate::names::{assign_names, service_url, valid_hostname};
use crate::scan::{proc_alive, scan_listeners};

fn systemd_active() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", UNIT_NAME])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn probe_public_port(bind_port: u16) -> u16 {
    if bind_port == 0 {
        return 0;
    }
    if bind_port == 80 {
        return 80;
    }
    let req = b"GET /__omaportless/ping HTTP/1.1\r\nHost: omaportless.localhost\r\nConnection: close\r\n\r\n";
    for addr in ["127.0.0.1:80", "[::1]:80"] {
        if let Ok(mut stream) = std::net::TcpStream::connect(addr) {
            use std::io::{Read, Write};
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(500)));
            if stream.write_all(req).is_ok() {
                let mut buf = [0u8; 256];
                if let Ok(n) = stream.read(&mut buf) {
                    if buf[..n].windows(b"omaportless-ok".len()).any(|w| w == b"omaportless-ok") {
                        return 80;
                    }
                }
            }
        }
    }
    bind_port
}

pub fn proxy_alive() -> bool {
    let state = load_state();
    if state.pid == 0 {
        return systemd_active();
    }
    proc_alive(state.pid as i32)
}

pub fn build_status() -> Value {
    let cfg = load_config();
    let state = load_state();
    let listen_port = state.port;
    let mut skip = Vec::new();
    if listen_port != 0 {
        skip.push(listen_port);
    }
    if cfg.listen_port != 0 {
        skip.push(cfg.listen_port);
    }
    if cfg.fallback_port != 0 {
        skip.push(cfg.fallback_port);
    }
    skip.sort();
    skip.dedup();
    let listeners = scan_listeners(&skip);
    let home = crate::config::home_dir();
    let mut services = assign_names(&listeners, &cfg.names, &home);
    let running = proxy_alive();
    let port = if running { listen_port } else { 0 };
    let mut public_port = if running { state.public_port } else { 0 };
    if running && port != 0 {
        public_port = probe_public_port(port);
    }
    let url_port = if public_port != 0 {
        public_port
    } else if port != 0 {
        port
    } else {
        cfg.fallback_port.max(DEFAULT_FALLBACK_PORT)
    };
    for svc in &mut services {
        svc.url = service_url(&svc.hostname, url_port);
    }
    let named = services.iter().filter(|s| s.pinned).count();
    json!({
        "ok": true,
        "proxy": {
            "running": running,
            "port": port,
            "public_port": if public_port != 0 { public_port } else { port },
            "listen_port": cfg.listen_port.max(DEFAULT_LISTEN_PORT),
            "fallback_port": cfg.fallback_port.max(DEFAULT_FALLBACK_PORT),
            "fallback": state.fallback,
            "redirect": public_port == 80 && port != 0 && port != 80,
            "bind": if state.bind.is_empty() { "127.0.0.1".into() } else { state.bind.clone() },
            "pid": if running { state.pid } else { 0 },
            "installed": unit_path().exists(),
            "error": state.error,
        },
        "named": named,
        "listening": services.len(),
        "services": services.iter().map(service_json).collect::<Vec<_>>(),
    })
}

fn service_json(svc: &crate::names::NamedService) -> Value {
    json!({
        "port": svc.listener.port,
        "pid": svc.listener.pid,
        "bind": svc.listener.bind,
        "comm": svc.listener.comm,
        "cmd": svc.listener.cmd,
        "cwd": svc.listener.cwd,
        "starttime": svc.listener.starttime,
        "id": svc.id,
        "hostname": svc.hostname,
        "pinned": svc.pinned,
        "alive": svc.alive,
        "url": svc.url,
    })
}

pub fn set_name(id: &str, hostname: &str) -> Result<Value, String> {
    let hostname = hostname.trim().to_ascii_lowercase();
    if !hostname.is_empty() && !valid_hostname(&hostname) {
        return Err("Name must be lowercase letters, digits, dots or hyphens".into());
    }
    let mut cfg = load_config();
    if hostname.is_empty() {
        cfg.names.remove(id);
    } else {
        if let Some((existing_id, _)) = cfg
            .names
            .iter()
            .find(|(k, v)| v.as_str() == hostname && k.as_str() != id)
        {
            return Err(format!("Name {hostname:?} is already used by {existing_id}"));
        }
        cfg.names.insert(id.to_string(), hostname);
    }
    crate::config::save_config(&cfg).map_err(|e| e.to_string())?;
    Ok(build_status())
}
