use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::config::{
    load_config, unit_path, DEFAULT_FALLBACK_PORT, UNIT_NAME,
};
use crate::status::{build_status, probe_public_port};

const REDIRECT_NFT: &str = "/etc/omaportless/redirect.nft";
const REDIRECT_UNIT: &str = "/etc/systemd/system/omaportless-redirect.service";

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

fn binary_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("omaportless"))
}

fn unit_contents(script: &str) -> String {
    format!(
        "[Unit]\nDescription=omaportless named .localhost reverse proxy\nAfter=default.target\n\n[Service]\nType=simple\nExecStart={script} daemon\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n"
    )
}

fn redirect_nft(port: u16) -> String {
    format!(
        "#!/usr/bin/nft -f\ndestroy table ip omaportless\ndestroy table ip6 omaportless\ntable ip omaportless {{\n  chain output {{\n    type nat hook output priority -100;\n    ip daddr 127.0.0.1 tcp dport 80 redirect to :{port}\n  }}\n}}\ntable ip6 omaportless {{\n  chain output {{\n    type nat hook output priority -100;\n    ip6 daddr ::1 tcp dport 80 redirect to :{port}\n  }}\n}}\n"
    )
}

fn redirect_unit_contents() -> String {
    format!(
        "[Unit]\nDescription=Redirect loopback TCP/80 to omaportless\nAfter=network.target\n\n[Service]\nType=oneshot\nExecStart=/usr/sbin/nft -f {REDIRECT_NFT}\nRemainAfterExit=yes\n\n[Install]\nWantedBy=multi-user.target\n"
    )
}

pub fn install_redirect(port: Option<u16>) -> i32 {
    if euid() != 0 {
        let bin = binary_path();
        let mut args = vec![
            "pkexec".to_string(),
            bin.display().to_string(),
            "install-redirect".to_string(),
        ];
        if let Some(p) = port {
            args.push(p.to_string());
        }
        return Command::new(&args[0])
            .args(&args[1..])
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or(1);
    }
    let cfg = load_config();
    let dest = port.unwrap_or(if cfg.fallback_port == 0 {
        DEFAULT_FALLBACK_PORT
    } else {
        cfg.fallback_port
    });
    if let Some(parent) = std::path::Path::new(REDIRECT_NFT).parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(REDIRECT_NFT, redirect_nft(dest)).is_err() {
        eprintln!("failed to write {REDIRECT_NFT}");
        return 1;
    }
    let _ = fs::set_permissions(REDIRECT_NFT, fs::Permissions::from_mode(0o644));
    if fs::write(REDIRECT_UNIT, redirect_unit_contents()).is_err() {
        eprintln!("failed to write {REDIRECT_UNIT}");
        return 1;
    }
    let loaded = Command::new("/usr/sbin/nft")
        .args(["-f", REDIRECT_NFT])
        .output();
    match loaded {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            eprintln!("{}", String::from_utf8_lossy(&out.stderr).trim());
            return 1;
        }
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    }
    let _ = Command::new("/usr/bin/systemctl").arg("daemon-reload").status();
    let enabled = Command::new("/usr/bin/systemctl")
        .args(["enable", "--now", "omaportless-redirect.service"])
        .output();
    if !enabled.map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("failed to enable omaportless-redirect.service");
        return 1;
    }
    println!("loopback :80 now redirects to :{dest}");
    0
}

pub fn uninstall_redirect() -> i32 {
    if euid() != 0 {
        let bin = binary_path();
        return Command::new("pkexec")
            .args([bin.to_str().unwrap_or("omaportless"), "uninstall-redirect"])
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or(1);
    }
    let _ = Command::new("/usr/bin/systemctl")
        .args(["disable", "--now", "omaportless-redirect.service"])
        .status();
    let _ = Command::new("/usr/sbin/nft")
        .args(["delete", "table", "ip", "omaportless"])
        .status();
    let _ = Command::new("/usr/sbin/nft")
        .args(["delete", "table", "ip6", "omaportless"])
        .status();
    let _ = fs::remove_file(REDIRECT_UNIT);
    let _ = fs::remove_file(REDIRECT_NFT);
    let _ = fs::remove_dir("/etc/omaportless");
    let _ = Command::new("/usr/bin/systemctl").arg("daemon-reload").status();
    println!("loopback :80 redirect removed");
    0
}

pub fn install() -> i32 {
    let script = binary_path();
    let path = unit_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(&path, unit_contents(&script.display().to_string())).is_err() {
        eprintln!("failed to write {}", path.display());
        return 1;
    }
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let result = Command::new("systemctl")
        .args(["--user", "enable", "--now", UNIT_NAME])
        .output();
    match result {
        Ok(out) if out.status.success() => println!("omaportless.service enabled"),
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            eprintln!("{}", err.trim());
            return 1;
        }
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    }
    let mut status = build_status();
    for _ in 0..20 {
        if status["proxy"]["port"].as_u64().unwrap_or(0) != 0 {
            break;
        }
        thread::sleep(Duration::from_millis(100));
        status = build_status();
    }
    let bind_port = status["proxy"]["port"].as_u64().unwrap_or(0) as u16;
    if bind_port != 0 && bind_port != 80 {
        println!("port 80 is taken; asking for permission to redirect loopback :80");
        let code = install_redirect(Some(bind_port));
        if code != 0 {
            eprintln!("Could not claim :80. URLs will keep using :{bind_port}");
            eprintln!("Retry with: {} enable-port80", script.display());
            return 0;
        }
        thread::sleep(Duration::from_millis(300));
        if probe_public_port(bind_port) == 80 {
            println!("named apps are now at http://name.localhost");
        } else {
            eprintln!("Redirect installed but probe failed; try opening http://name.localhost");
        }
    }
    0
}

pub fn uninstall() -> i32 {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", UNIT_NAME])
        .status();
    let _ = fs::remove_file(unit_path());
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    uninstall_redirect();
    println!("omaportless.service removed");
    0
}

pub fn start() -> i32 {
    if !unit_path().exists() {
        let code = install();
        if code != 0 {
            return code;
        }
        return 0;
    }
    let result = Command::new("systemctl")
        .args(["--user", "start", UNIT_NAME])
        .output();
    match result {
        Ok(out) if out.status.success() => {
            println!("started");
            0
        }
        Ok(out) => {
            eprintln!("{}", String::from_utf8_lossy(&out.stderr).trim());
            1
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

pub fn stop() -> i32 {
    let result = Command::new("systemctl")
        .args(["--user", "stop", UNIT_NAME])
        .output();
    match result {
        Ok(out) if out.status.success() => {
            println!("stopped");
            0
        }
        Ok(out) => {
            eprintln!("{}", String::from_utf8_lossy(&out.stderr).trim());
            1
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
