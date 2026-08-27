use std::sync::Arc;
use std::time::Duration;

use std::sync::RwLock;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::config::{load_config, load_state, parse_upstream, save_state, Config, State};
use crate::http::{
    hop_count, http_response, index_page, is_control_request, not_found_page, parse_host_header,
    request_path, should_index, with_hop, MAX_HOPS,
};
use crate::names::{assign_names, newest_for_hostname, short_name, NamedService};
use crate::scan::scan_listeners;
use crate::status::probe_public_port;

const HEADER_LIMIT: usize = 64 * 1024;

pub struct ProxyCore {
    pub port: u16,
    pub public_port: u16,
    cfg: Config,
    services: Vec<NamedService>,
}

impl ProxyCore {
    fn new(port: u16) -> Self {
        let mut core = Self {
            port,
            public_port: 0,
            cfg: load_config(),
            services: Vec::new(),
        };
        core.refresh();
        core
    }

    fn refresh(&mut self) {
        self.cfg = load_config();
        let skip = [self.port, self.cfg.listen_port, self.cfg.fallback_port];
        let listeners = scan_listeners(&skip);
        let home = crate::config::home_dir();
        self.services = assign_names(&listeners, &self.cfg.names, &home);
    }

    fn url_port(&self) -> u16 {
        if self.public_port != 0 {
            self.public_port
        } else {
            self.port
        }
    }

    fn lookup(&self, host: &str) -> Option<NamedService> {
        let name = short_name(host);
        if name.is_empty() || name == "omaportless" {
            return None;
        }
        newest_for_hostname(&self.services, &name).cloned()
    }

    fn upstream(&self) -> Option<(String, u16)> {
        if self.port == 0 || self.port == 80 {
            return None;
        }
        parse_upstream(&self.cfg.upstream)
    }
}

async fn read_headers(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = timeout(Duration::from_secs(15), stream.read(&mut tmp)).await??;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > HEADER_LIMIT {
            break;
        }
    }
    Ok(buf)
}

async fn handle_client(mut client: TcpStream, core: Arc<RwLock<ProxyCore>>) {
    let buf = match read_headers(&mut client).await {
        Ok(b) if !b.is_empty() => b,
        _ => return,
    };
    let host = parse_host_header(&buf);
    let path = request_path(&buf);
    let hops = hop_count(&buf);
    let snapshot = {
        let g = core.read().unwrap();
        (g.url_port(), g.services.clone(), g.lookup(&host), g.upstream(), g.port)
    };

    let reply = async |client: &mut TcpStream, bytes: Vec<u8>| {
        let _ = client.write_all(&bytes).await;
    };

    if hops >= MAX_HOPS {
        reply(
            &mut client,
            http_response(508, "Loop Detected", "proxy loop\n", "text/plain; charset=utf-8"),
        )
        .await;
        return;
    }
    if is_control_request(&host, &path) {
        reply(
            &mut client,
            http_response(200, "OK", "omaportless-ok\n", "text/plain; charset=utf-8"),
        )
        .await;
        return;
    }
    if should_index(&host) {
        reply(
            &mut client,
            http_response(
                200,
                "OK",
                &index_page(&snapshot.1, snapshot.0),
                "text/html; charset=utf-8",
            ),
        )
        .await;
        return;
    }

    let (addrs, payload) = if let Some(target) = snapshot.2 {
        (
            backend_addrs(&target.listener.bind, target.listener.port),
            buf,
        )
    } else if let Some(up) = snapshot.3 {
        (vec![format_addr(&up.0, up.1)], with_hop(&buf, hops + 1))
    } else {
        reply(
            &mut client,
            http_response(
                404,
                "Not Found",
                &not_found_page(&host, &snapshot.1, snapshot.0),
                "text/plain; charset=utf-8",
            ),
        )
        .await;
        return;
    };

    match connect_backend(&addrs).await {
        Ok(mut backend) => {
            if backend.write_all(&payload).await.is_ok() {
                let _ = tokio::io::copy_bidirectional(&mut client, &mut backend).await;
            }
        }
        Err(_) => {
            reply(
                &mut client,
                http_response(502, "Bad Gateway", "Proxy error\n", "text/plain; charset=utf-8"),
            )
            .await;
        }
    }
}

fn format_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub fn backend_addrs(bind: &str, port: u16) -> Vec<String> {
    let v4 = format!("127.0.0.1:{port}");
    let v6 = format!("[::1]:{port}");
    match bind {
        "::1" | "::" | "::0" | "https://example.net/id/garnet" => vec![v6, v4],
        ip if ip.starts_with("127.") || ip == "0.0.0.0" => vec![v4, v6],
        _ => vec![v4, v6],
    }
}

async fn connect_backend(addrs: &[String]) -> std::io::Result<TcpStream> {
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no backend address");
    for addr in addrs {
        match timeout(Duration::from_secs(2), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(err)) => last = err,
            Err(_) => {
                last = std::io::Error::new(std::io::ErrorKind::TimedOut, format!("connect {addr}"))
            }
        }
    }
    Err(last)
}

fn try_listen(addr: std::net::SocketAddr) -> std::io::Result<TcpListener> {
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    TcpListener::from_std(listener)
}

fn bind_proxy(preferred: u16, fallback: u16) -> Result<(Vec<TcpListener>, u16, bool), String> {
    let mut attempts = vec![preferred];
    if fallback != 0 && fallback != preferred {
        attempts.push(fallback);
    }
    let mut last = "Could not bind a proxy port".to_string();
    for port in attempts {
        let mut listeners = Vec::new();
        let v4 = "127.0.0.1".parse().ok().map(|ip| std::net::SocketAddr::new(ip, port));
        let v6 = "::1".parse().ok().map(|ip| std::net::SocketAddr::new(ip, port));
        for addr in [v4, v6].into_iter().flatten() {
            if let Ok(l) = try_listen(addr) {
                listeners.push(l);
            }
        }
        if !listeners.is_empty() {
            return Ok((listeners, port, port != preferred));
        }
        last = format!("Could not bind {port}");
    }
    Err(last)
}

async fn accept_loop(listener: TcpListener, core: Arc<RwLock<ProxyCore>>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let core = Arc::clone(&core);
                tokio::spawn(async move { handle_client(stream, core).await });
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

pub async fn run_daemon() -> i32 {
    let cfg = load_config();
    let preferred = if cfg.listen_port == 0 { 80 } else { cfg.listen_port };
    let fallback = if cfg.fallback_port == 0 { 7777 } else { cfg.fallback_port };
    let (listeners, port, used_fallback) = match bind_proxy(preferred, fallback) {
        Ok(v) => v,
        Err(error) => {
            let _ = save_state(&State {
                pid: std::process::id(),
                error,
                ..State::default()
            });
            eprintln!("{}", load_state().error);
            return 1;
        }
    };
    let core = Arc::new(RwLock::new(ProxyCore::new(port)));
    for listener in listeners {
        let core = Arc::clone(&core);
        tokio::spawn(accept_loop(listener, core));
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let public_port = tokio::task::spawn_blocking(move || probe_public_port(port))
        .await
        .unwrap_or(port);
    core.write().unwrap().public_port = public_port;
    let _ = save_state(&State {
        pid: std::process::id(),
        port,
        public_port,
        fallback: used_fallback,
        error: String::new(),
        bind: "127.0.0.1".into(),
    });
    let mode = if port == 80 {
        String::new()
    } else {
        format!(":{port}")
    };
    println!("omaportless listening on 127.0.0.1{mode} and ::1{mode}");

    let refresher = {
        let core = Arc::clone(&core);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tick.tick().await;
                let port = core.read().unwrap().port;
                let public = tokio::task::spawn_blocking(move || probe_public_port(port))
                    .await
                    .unwrap_or(port);
                let mut g = core.write().unwrap();
                g.refresh();
                g.public_port = public;
            }
        })
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = async {
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("sigterm");
            sigterm.recv().await;
        } => {}
    }
    refresher.abort();
    0
}

#[cfg(test)]
mod tests {
    use super::backend_addrs;

    #[test]
    fn ipv6_only_prefers_loopback_v6() {
        assert_eq!(
            backend_addrs("::1", 4322),
            vec!["[::1]:4322".to_string(), "127.0.0.1:4322".to_string()]
        );
    }

    #[test]
    fn ipv4_only_prefers_loopback_v4() {
        assert_eq!(
            backend_addrs("127.0.0.1", 4321),
            vec!["127.0.0.1:4321".to_string(), "[::1]:4321".to_string()]
        );
    }
}
