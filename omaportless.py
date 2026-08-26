#!/usr/bin/env python3
"""omaportless — named .localhost URLs for local dev servers.

A loopback reverse proxy plus a CLI the Omarchy panel talks to.
Names are pinned by project directory (cwd) or by port; live listeners are
scanned from /proc. Existing localhost:PORT URLs are never taken away.
"""

from __future__ import annotations

import html
import json
import os
import re
import signal
import socket
import struct
import sys
import threading
import time
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

APP_NAME = "omaportless"
TLD = "localhost"
CONFIG_DIR = Path.home() / ".config" / "omaportless"
CONFIG_PATH = CONFIG_DIR / "config.json"
STATE_PATH = CONFIG_DIR / "state.json"
UNIT_NAME = "omaportless.service"
UNIT_PATH = Path.home() / ".config" / "systemd" / "user" / UNIT_NAME

DEFAULT_LISTEN_PORT = 80
DEFAULT_FALLBACK_PORT = 7777
DEFAULT_UPSTREAM = "127.0.0.2:80"
HEADER_LIMIT = 64 * 1024
BACKEND_CONNECT_TIMEOUT = 2.0
HOPS_HEADER = b"x-omaportless-hops"
MAX_HOPS = 3
REDIRECT_NFT = Path("/etc/omaportless/redirect.nft")
REDIRECT_UNIT = Path("/etc/systemd/system/omaportless-redirect.service")

SKIP_COMM = {
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
}

HOSTNAME_RE = re.compile(
    r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?"
    r"(?:\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$"
)

_inode_cache: dict[str, tuple[float, dict[int, int]]] = {}
_inode_lock = threading.Lock()


def eprint(*args: object) -> None:
    print(*args, file=sys.stderr)


def slug(value: str) -> str:
    text = str(value or "").strip().lower()
    text = re.sub(r"[^a-z0-9]+", "-", text)
    text = re.sub(r"-{2,}", "-", text).strip("-")
    return text[:63]


def valid_hostname(value: str) -> bool:
    name = str(value or "").strip().lower()
    if not name or not HOSTNAME_RE.match(name):
        return False
    labels = name.split(".")
    if "localhost" in labels or name == TLD:
        return False
    return True


def parse_host_header(blob: bytes) -> str:
    try:
        head = blob.split(b"\r\n\r\n", 1)[0].decode("iso-8859-1")
    except Exception:
        return ""
    for line in head.split("\r\n")[1:]:
        if line.lower().startswith("host:"):
            raw = line.split(":", 1)[1].strip()
            if raw.startswith("["):
                end = raw.find("]")
                host = raw[1:end] if end != -1 else raw
            else:
                host = raw.split(":", 1)[0]
            return host.strip().lower().rstrip(".")
    # HTTP/1.0 or absolute-form request line
    first = head.split("\r\n", 1)[0]
    parts = first.split()
    if len(parts) >= 2:
        target = parts[1]
        if target.startswith("http://") or target.startswith("https://"):
            return (urlsplit(target).hostname or "").lower()
    return ""


def short_name(host: str) -> str:
    host = (host or "").lower().rstrip(".")
    suffix = "." + TLD
    if host.endswith(suffix):
        return host[: -len(suffix)]
    if host == TLD:
        return ""
    return host


def is_index_host(host: str) -> bool:
    host = (host or "").lower().rstrip(".")
    return host in {"", TLD, "127.0.0.1", "::1", "https://example.net/id/garnet"}


def request_path(blob: bytes) -> str:
    try:
        first = blob.split(b"\r\n", 1)[0].decode("iso-8859-1")
    except Exception:
        return "/"
    parts = first.split()
    if len(parts) < 2:
        return "/"
    target = parts[1]
    if target.startswith("http://") or target.startswith("https://"):
        return urlsplit(target).path or "/"
    return target.split("?", 1)[0] or "/"


def hop_count(blob: bytes) -> int:
    try:
        head = blob.split(b"\r\n\r\n", 1)[0].decode("iso-8859-1")
    except Exception:
        return 0
    for line in head.split("\r\n")[1:]:
        if line.lower().startswith("x-omaportless-hops:"):
            try:
                return int(line.split(":", 1)[1].strip())
            except ValueError:
                return 0
    return 0


def with_hop(blob: bytes, hops: int) -> bytes:
    head, _, rest = blob.partition(b"\r\n\r\n")
    lines = head.split(b"\r\n")
    kept = [lines[0]] if lines else [b"GET / HTTP/1.1"]
    for line in lines[1:]:
        if not line.lower().startswith(HOPS_HEADER + b":"):
            kept.append(line)
    kept.append(b"X-Omaportless-Hops: " + str(hops).encode("ascii"))
    return b"\r\n".join(kept) + b"\r\n\r\n" + rest


def parse_upstream(value: str) -> tuple[str, int] | None:
    text = str(value or "").strip()
    if not text:
        return None
    if text.startswith("["):
        end = text.find("]")
        host = text[1:end]
        port = int(text.split("]:", 1)[1]) if "]:" in text else 80
        return host, port
    if text.count(":") == 1:
        host, port_s = text.split(":")
        return host, int(port_s)
    return text, 80


def service_url(hostname: str, proxy_port: int | None) -> str:
    host = f"{hostname}.{TLD}"
    if not proxy_port or int(proxy_port) == 80:
        return f"http://{host}"
    return f"http://{host}:{int(proxy_port)}"


def default_config() -> dict[str, Any]:
    return {
        "listen_port": DEFAULT_LISTEN_PORT,
        "fallback_port": DEFAULT_FALLBACK_PORT,
        "upstream": DEFAULT_UPSTREAM,
        "names": {},
    }


def load_config() -> dict[str, Any]:
    cfg = default_config()
    if CONFIG_PATH.exists():
        try:
            data = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
            if isinstance(data, dict):
                cfg["listen_port"] = int(data.get("listen_port") or DEFAULT_LISTEN_PORT)
                cfg["fallback_port"] = int(data.get("fallback_port") or DEFAULT_FALLBACK_PORT)
                if "upstream" in data:
                    cfg["upstream"] = str(data.get("upstream") or "")
                names = data.get("names") or {}
                if isinstance(names, dict):
                    cfg["names"] = {
                        str(k): str(v).strip().lower()
                        for k, v in names.items()
                        if str(v).strip()
                    }
        except (OSError, ValueError, TypeError):
            pass
    return cfg


def save_config(cfg: dict[str, Any]) -> None:
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    tmp = CONFIG_PATH.with_suffix(".tmp")
    tmp.write_text(json.dumps(cfg, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    tmp.replace(CONFIG_PATH)


def load_state() -> dict[str, Any]:
    if not STATE_PATH.exists():
        return {}
    try:
        data = json.loads(STATE_PATH.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def save_state(state: dict[str, Any]) -> None:
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    tmp = STATE_PATH.with_suffix(".tmp")
    tmp.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")
    tmp.replace(STATE_PATH)


def parse_ipv4_port(addr: str) -> tuple[str, int]:
    ip_hex, port_hex = addr.split(":")
    ip = socket.inet_ntoa(struct.pack("<I", int(ip_hex, 16)))
    return ip, int(port_hex, 16)


def parse_ipv6_port(addr: str) -> tuple[str, int]:
    ip_hex, port_hex = addr.split(":")
    raw = bytes.fromhex(ip_hex)
    words = b"".join(raw[i : i + 4][::-1] for i in range(0, 16, 4))
    ip = socket.inet_ntop(socket.AF_INET6, words)
    return ip, int(port_hex, 16)


def is_local_bind(ip: str) -> bool:
    if ip in {"0.0.0.0", "::", "::0", "*"}:
        return True
    if ip.startswith("127.") or ip in {"::1", "https://example.net/id/garnet"}:
        return True
    return False


def inode_to_pid() -> dict[int, int]:
    now = time.monotonic()
    with _inode_lock:
        cached = _inode_cache.get("map")
        if cached and now - cached[0] < 1.0:
            return cached[1]
    mapping: dict[int, int] = {}
    proc = Path("/proc")
    try:
        pids = [p for p in proc.iterdir() if p.name.isdigit()]
    except OSError:
        return {}
    for proc_dir in pids:
        fd_dir = proc_dir / "fd"
        try:
            for entry in fd_dir.iterdir():
                try:
                    target = os.readlink(entry)
                except OSError:
                    continue
                if target.startswith("socket:["):
                    try:
                        inode = int(target[8:-1])
                    except ValueError:
                        continue
                    mapping[inode] = int(proc_dir.name)
        except OSError:
            continue
    with _inode_lock:
        _inode_cache["map"] = (now, mapping)
    return mapping


def proc_field(pid: int, name: str) -> str:
    path = Path("/proc") / str(pid) / name
    try:
        if name == "cwd":
            return os.readlink(path)
        raw = path.read_bytes()
        if name == "cmdline":
            return raw.replace(b"\x00", b" ").decode("utf-8", "replace").strip()
        return raw.decode("utf-8", "replace").strip()
    except OSError:
        return ""


def proc_starttime(pid: int) -> int:
    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8", errors="replace")
        close = stat.rfind(")")
        fields = stat[close + 2 :].split()
        return int(fields[19])
    except (OSError, IndexError, ValueError):
        return 0


def proc_uid(pid: int) -> int | None:
    try:
        return Path(f"/proc/{pid}").stat().st_uid
    except OSError:
        return None


def scan_listeners(skip_ports: set[int] | None = None) -> list[dict[str, Any]]:
    skip_ports = skip_ports or set()
    inodes = inode_to_pid()
    found: dict[int, dict[str, Any]] = {}
    tables = [
        (Path("/proc/net/tcp"), False),
        (Path("/proc/net/tcp6"), True),
    ]
    uid = os.getuid()
    for path, ipv6 in tables:
        try:
            lines = path.read_text(encoding="utf-8").splitlines()[1:]
        except OSError:
            continue
        for line in lines:
            parts = line.split()
            if len(parts) < 10:
                continue
            if parts[3] != "0A":
                continue
            try:
                ip, port = parse_ipv6_port(parts[1]) if ipv6 else parse_ipv4_port(parts[1])
                inode = int(parts[9])
            except (ValueError, OSError):
                continue
            if port in skip_ports or port == 0:
                continue
            if not is_local_bind(ip):
                continue
            pid = inodes.get(inode)
            if pid is None:
                continue
            owner = proc_uid(pid)
            if owner is not None and owner != uid:
                continue
            comm = proc_field(pid, "comm")
            if comm in SKIP_COMM:
                continue
            existing = found.get(port)
            if existing and existing.get("pid") == pid:
                continue
            found[port] = {
                "port": port,
                "pid": pid,
                "bind": ip,
                "comm": comm,
                "cmd": proc_field(pid, "cmdline"),
                "cwd": proc_field(pid, "cwd"),
                "starttime": proc_starttime(pid),
            }
    return sorted(found.values(), key=lambda item: item["port"])


def service_id(item: dict[str, Any]) -> str:
    cwd = item.get("cwd") or ""
    if cwd and cwd not in {"/", str(Path.home())}:
        return f"cwd:{cwd}"
    return f"port:{item['port']}"


def suggested_name(item: dict[str, Any]) -> str:
    cwd = item.get("cwd") or ""
    base = Path(cwd).name if cwd else ""
    home_name = Path.home().name
    if slug(base) in {"", "home", "tmp", "src", "app", "apps", slug(home_name)}:
        base = item.get("comm") or ""
    name = slug(base)
    return name or f"port-{item['port']}"


def assign_names(listeners: list[dict[str, Any]], names: dict[str, str]) -> list[dict[str, Any]]:
    used: dict[str, int] = {}
    services: list[dict[str, Any]] = []
    for item in listeners:
        sid = service_id(item)
        pinned = names.get(sid) or names.get(f"port:{item['port']}") or ""
        if pinned and not valid_hostname(pinned):
            pinned = ""
        hostname = pinned or suggested_name(item)
        if not pinned:
            n = used.get(hostname, 0) + 1
            used[hostname] = n
            if n > 1:
                hostname = f"{hostname}-{item['port']}"
        services.append(
            {
                **item,
                "id": sid,
                "hostname": hostname,
                "pinned": bool(pinned),
                "alive": True,
            }
        )
    return services


def newest_for_hostname(services: list[dict[str, Any]], hostname: str) -> dict[str, Any] | None:
    matches = [s for s in services if s.get("hostname") == hostname and s.get("alive")]
    if not matches:
        return None
    matches.sort(key=lambda s: (int(s.get("starttime") or 0), int(s.get("pid") or 0)))
    return matches[-1]


def systemd_active() -> bool:
    try:
        import subprocess

        result = subprocess.run(
            ["systemctl", "--user", "is-active", "--quiet", UNIT_NAME],
            check=False,
        )
        return result.returncode == 0
    except OSError:
        return False


def probe_public_port(bind_port: int) -> int:
    if not bind_port:
        return 0
    if bind_port == 80:
        return 80
    req = (
        b"GET /__omaportless/ping HTTP/1.1\r\n"
        b"Host: omaportless.localhost\r\n"
        b"Connection: close\r\n\r\n"
    )
    for family, host in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")):
        sock = socket.socket(family, socket.SOCK_STREAM)
        try:
            sock.settimeout(0.5)
            sock.connect((host, 80))
            sock.sendall(req)
            data = sock.recv(256)
            if b"omaportless-ok" in data:
                return 80
        except OSError:
            continue
        finally:
            try:
                sock.close()
            except OSError:
                pass
    return bind_port


def proxy_alive(state: dict[str, Any]) -> bool:
    pid = int(state.get("pid") or 0)
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    comm = proc_field(pid, "comm")
    cmd = proc_field(pid, "cmdline")
    return "omaportless" in cmd or comm in {"python3", "python", "omaportless"}


def build_status() -> dict[str, Any]:
    cfg = load_config()
    state = load_state()
    listen_port = int(state.get("port") or 0)
    skip = {listen_port} if listen_port else set()
    skip.add(int(cfg.get("listen_port") or 0))
    skip.add(int(cfg.get("fallback_port") or 0))
    skip.discard(0)
    services = assign_names(scan_listeners(skip), cfg.get("names") or {})
    running = proxy_alive(state) or systemd_active()
    port = listen_port if running else 0
    public_port = int(state.get("public_port") or 0) if running else 0
    if running and port:
        public_port = probe_public_port(port)
    url_port = public_port or port or cfg.get("fallback_port") or DEFAULT_FALLBACK_PORT
    for svc in services:
        svc["url"] = service_url(svc["hostname"], url_port)
    named = sum(1 for s in services if s.get("pinned"))
    return {
        "ok": True,
        "proxy": {
            "running": running,
            "port": port,
            "public_port": public_port or port,
            "listen_port": int(cfg.get("listen_port") or DEFAULT_LISTEN_PORT),
            "fallback_port": int(cfg.get("fallback_port") or DEFAULT_FALLBACK_PORT),
            "fallback": bool(state.get("fallback")),
            "redirect": public_port == 80 and port not in (0, 80),
            "bind": state.get("bind") or "127.0.0.1",
            "pid": int(state.get("pid") or 0) if running else 0,
            "installed": UNIT_PATH.exists(),
            "error": state.get("error") or "",
        },
        "named": named,
        "listening": len(services),
        "services": services,
    }


def set_name(sid: str, hostname: str) -> dict[str, Any]:
    hostname = hostname.strip().lower()
    if hostname and not valid_hostname(hostname):
        raise ValueError("Name must be lowercase letters, digits, dots or hyphens")
    cfg = load_config()
    names = dict(cfg.get("names") or {})
    if hostname:
        for existing_id, existing_name in names.items():
            if existing_name == hostname and existing_id != sid:
                raise ValueError(f"Name {hostname!r} is already used by {existing_id}")
        names[sid] = hostname
    else:
        names.pop(sid, None)
    cfg["names"] = names
    save_config(cfg)
    return build_status()


def try_bind(port: int, host: str, family: int) -> socket.socket | None:
    sock = socket.socket(family, socket.SOCK_STREAM)
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if family == socket.AF_INET6:
            sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
        sock.bind((host, port))
        sock.listen(128)
        sock.settimeout(1.0)
        return sock
    except OSError:
        sock.close()
        return None


def bind_proxy(preferred: int, fallback: int) -> tuple[list[socket.socket], int, bool, str]:
    attempts = [preferred]
    if fallback and fallback != preferred:
        attempts.append(fallback)
    last_error = ""
    for port in attempts:
        sockets: list[socket.socket] = []
        for family, host in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")):
            sock = try_bind(port, host, family)
            if sock is not None:
                sockets.append(sock)
        if sockets:
            return sockets, port, port != preferred, ""
        last_error = f"Could not bind {port}"
    return [], 0, False, last_error or "Could not bind a proxy port"


def http_response(status: int, reason: str, body: str, content_type: str = "text/html; charset=utf-8") -> bytes:
    payload = body.encode("utf-8")
    headers = (
        f"HTTP/1.1 {status} {reason}\r\n"
        f"Content-Type: {content_type}\r\n"
        f"Content-Length: {len(payload)}\r\n"
        "Connection: close\r\n"
        "Cache-Control: no-store\r\n"
        "\r\n"
    )
    return headers.encode("ascii") + payload


def index_page(services: list[dict[str, Any]], proxy_port: int) -> str:
    rows = []
    for svc in services:
        url = html.escape(service_url(svc["hostname"], proxy_port), quote=True)
        name = html.escape(f"{svc['hostname']}.{TLD}")
        port = html.escape(str(svc["port"]))
        rows.append(
            f'<li><a href="{url}"><span class="name">{name}</span>'
            f'<span class="port">{port}</span></a></li>'
        )
    listing = "\n".join(rows) or '<li class="empty">Nothing listening</li>'
    return f"""<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>localhost</title>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{
    background: #fff;
    color: #111;
    font: 15px/1.45 ui-sans-serif, system-ui, sans-serif;
  }}
  main {{
    max-width: 36rem;
    margin: 0 auto;
    padding: 4rem 1.5rem;
  }}
  h1 {{
    font-size: 15px;
    font-weight: 600;
    letter-spacing: 0;
    margin: 0 0 1.5rem;
  }}
  ul {{ list-style: none; }}
  li + li a, li + li.empty {{ border-top: 1px solid #eee; }}
  a {{
    display: grid;
    grid-template-columns: minmax(0, 1fr) 4.5rem;
    align-items: baseline;
    gap: 1.5rem;
    padding: 0.7rem 0;
    color: inherit;
    text-decoration: none;
  }}
  a:hover .name {{ text-decoration: underline; }}
  .name {{ overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
  .port {{
    text-align: right;
    color: #666;
    font-variant-numeric: tabular-nums;
  }}
  .empty {{ color: #888; padding: 0.7rem 0; }}
</style>
<main>
  <h1>localhost</h1>
  <ul>
{listing}
  </ul>
</main>
"""


def not_found_page(host: str, services: list[dict[str, Any]], proxy_port: int) -> str:
    names = ", ".join(f"{s['hostname']}.{TLD}" for s in services) or "none"
    return (
        f"No app registered for {host or 'unknown host'}.\n"
        f"Listening: {names}\n"
        f"Proxy port: {proxy_port}\n"
    )


class ProxyCore:
    def __init__(self) -> None:
        self.cfg = load_config()
        self.services: list[dict[str, Any]] = []
        self.port = 0
        self.public_port = 0
        self.lock = threading.Lock()
        self.refresh()

    def refresh(self) -> None:
        cfg = load_config()
        skip = {self.port} if self.port else set()
        services = assign_names(scan_listeners(skip), cfg.get("names") or {})
        public = probe_public_port(self.port) if self.port else 0
        with self.lock:
            self.cfg = cfg
            self.services = services
            if public:
                self.public_port = public

    def lookup(self, host: str) -> dict[str, Any] | None:
        name = short_name(host)
        if not name or name == APP_NAME:
            return None
        with self.lock:
            services = list(self.services)
        return newest_for_hostname(services, name)

    def snapshot(self) -> list[dict[str, Any]]:
        with self.lock:
            return list(self.services)

    def url_port(self) -> int:
        with self.lock:
            return self.public_port or self.port or DEFAULT_FALLBACK_PORT

    def upstream(self) -> tuple[str, int] | None:
        with self.lock:
            if self.port in (0, 80):
                return None
            return parse_upstream(str(self.cfg.get("upstream") or DEFAULT_UPSTREAM))


def handle_client(client: socket.socket, core: ProxyCore, proxy_port: int) -> None:
    backend: socket.socket | None = None
    replied = False
    try:
        buf = b""
        client.settimeout(15)
        while b"\r\n\r\n" not in buf and len(buf) < HEADER_LIMIT:
            chunk = client.recv(4096)
            if not chunk:
                return
            buf += chunk
        host = parse_host_header(buf)
        path = request_path(buf)
        hops = hop_count(buf)
        if hops >= MAX_HOPS:
            client.sendall(http_response(508, "Loop Detected", "proxy loop\n", "text/plain; charset=utf-8"))
            replied = True
            return
        if path == "/__omaportless/ping" or short_name(host) == APP_NAME:
            client.sendall(http_response(200, "OK", "omaportless-ok\n", "text/plain; charset=utf-8"))
            replied = True
            return
        if is_index_host(host):
            client.sendall(http_response(200, "OK", index_page(core.snapshot(), core.url_port())))
            replied = True
            return
        target = core.lookup(host)
        if target:
            dest = ("127.0.0.1", int(target["port"]))
            payload = buf
        else:
            upstream = core.upstream()
            if not upstream:
                client.sendall(
                    http_response(
                        404,
                        "Not Found",
                        not_found_page(host, core.snapshot(), core.url_port()),
                        "text/plain; charset=utf-8",
                    )
                )
                replied = True
                return
            dest = upstream
            payload = with_hop(buf, hops + 1)
        backend = socket.create_connection(
            dest,
            timeout=BACKEND_CONNECT_TIMEOUT,
        )
        backend.settimeout(None)
        client.settimeout(None)
        backend.sendall(payload)
        splice(client, backend)
        replied = True
    except OSError:
        if not replied:
            try:
                client.sendall(
                    http_response(
                        502,
                        "Bad Gateway",
                        "Proxy error\n",
                        "text/plain; charset=utf-8",
                    )
                )
            except OSError:
                pass
    finally:
        if backend is not None:
            try:
                backend.close()
            except OSError:
                pass
        try:
            client.close()
        except OSError:
            pass


def splice(a: socket.socket, b: socket.socket) -> None:
    def pump(src: socket.socket, dst: socket.socket) -> None:
        try:
            while True:
                data = src.recv(65536)
                if not data:
                    break
                dst.sendall(data)
        except OSError:
            pass
        try:
            dst.shutdown(socket.SHUT_WR)
        except OSError:
            pass

    worker = threading.Thread(target=pump, args=(a, b), daemon=True)
    worker.start()
    pump(b, a)
    worker.join(timeout=1)


def accept_loop(sock: socket.socket, core: ProxyCore, proxy_port: int, stop: threading.Event) -> None:
    sock.settimeout(1.0)
    while not stop.is_set():
        try:
            conn, _addr = sock.accept()
        except TimeoutError:
            continue
        except OSError:
            if stop.is_set():
                return
            continue
        threading.Thread(target=handle_client, args=(conn, core, proxy_port), daemon=True).start()


def redirect_nft(port: int) -> str:
    return f"""#!/usr/bin/nft -f
destroy table ip omaportless
destroy table ip6 omaportless
table ip omaportless {{
  chain output {{
    type nat hook output priority -100;
    ip daddr 127.0.0.1 tcp dport 80 redirect to :{port}
  }}
}}
table ip6 omaportless {{
  chain output {{
    type nat hook output priority -100;
    ip6 daddr ::1 tcp dport 80 redirect to :{port}
  }}
}}
"""


def redirect_unit_contents() -> str:
    return f"""[Unit]
Description=Redirect loopback TCP/80 to omaportless
After=network.target

[Service]
Type=oneshot
ExecStart=/usr/sbin/nft -f {REDIRECT_NFT}
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
"""


def cmd_install_redirect(port: int | None = None) -> int:
    import subprocess

    if os.geteuid() != 0:
        script = str(Path(__file__).resolve())
        args = ["pkexec", "/usr/bin/python3", script, "install-redirect"]
        if port:
            args.append(str(port))
        result = subprocess.run(args, check=False)
        return result.returncode
    cfg = load_config()
    dest = int(port or cfg.get("fallback_port") or DEFAULT_FALLBACK_PORT)
    REDIRECT_NFT.parent.mkdir(parents=True, exist_ok=True)
    REDIRECT_NFT.write_text(redirect_nft(dest), encoding="utf-8")
    REDIRECT_UNIT.write_text(redirect_unit_contents(), encoding="utf-8")
    loaded = subprocess.run(["/usr/sbin/nft", "-f", str(REDIRECT_NFT)], check=False, capture_output=True, text=True)
    if loaded.returncode != 0:
        eprint(loaded.stderr.strip() or "nft failed")
        return 1
    subprocess.run(["/usr/bin/systemctl", "daemon-reload"], check=False)
    enabled = subprocess.run(
        ["/usr/bin/systemctl", "enable", "--now", "omaportless-redirect.service"],
        check=False,
        capture_output=True,
        text=True,
    )
    if enabled.returncode != 0:
        eprint(enabled.stderr.strip() or "failed to enable omaportless-redirect.service")
        return 1
    print(f"loopback :80 now redirects to :{dest}")
    return 0


def cmd_uninstall_redirect() -> int:
    import subprocess

    if os.geteuid() != 0:
        script = str(Path(__file__).resolve())
        return subprocess.run(["pkexec", "/usr/bin/python3", script, "uninstall-redirect"], check=False).returncode
    subprocess.run(["/usr/bin/systemctl", "disable", "--now", "omaportless-redirect.service"], check=False)
    subprocess.run(["/usr/sbin/nft", "delete", "table", "ip", "omaportless"], check=False)
    subprocess.run(["/usr/sbin/nft", "delete", "table", "ip6", "omaportless"], check=False)
    if REDIRECT_UNIT.exists():
        REDIRECT_UNIT.unlink()
    if REDIRECT_NFT.exists():
        REDIRECT_NFT.unlink()
        try:
            REDIRECT_NFT.parent.rmdir()
        except OSError:
            pass
    subprocess.run(["/usr/bin/systemctl", "daemon-reload"], check=False)
    print("loopback :80 redirect removed")
    return 0


def unit_contents(script: Path) -> str:
    return f"""[Unit]
Description=omaportless named .localhost reverse proxy
After=default.target

[Service]
Type=simple
ExecStart=/usr/bin/python3 {script} daemon
Restart=on-failure
RestartSec=2
Environment=PYTHONUNBUFFERED=1

[Install]
WantedBy=default.target
"""


def cmd_install() -> int:
    script = Path(__file__).resolve()
    UNIT_PATH.parent.mkdir(parents=True, exist_ok=True)
    UNIT_PATH.write_text(unit_contents(script), encoding="utf-8")
    import subprocess

    subprocess.run(["systemctl", "--user", "daemon-reload"], check=False)
    result = subprocess.run(
        ["systemctl", "--user", "enable", "--now", UNIT_NAME],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        eprint(result.stderr.strip() or result.stdout.strip() or "Failed to enable omaportless.service")
        return 1
    print("omaportless.service enabled")
    time.sleep(0.4)
    status = build_status()
    bind_port = int(status.get("proxy", {}).get("port") or 0)
    if bind_port and bind_port != 80:
        print("port 80 is taken; asking for permission to redirect loopback :80")
        code = cmd_install_redirect(bind_port)
        if code != 0:
            eprint("Could not claim :80. URLs will keep using :" + str(bind_port))
            eprint("Retry with: python3 " + str(Path(__file__).resolve()) + " enable-port80")
            return 0
        time.sleep(0.3)
        if probe_public_port(bind_port) == 80:
            print("named apps are now at http://name.localhost")
        else:
            eprint("Redirect installed but probe failed; try opening http://name.localhost")
    return 0


def cmd_uninstall() -> int:
    import subprocess

    subprocess.run(["systemctl", "--user", "disable", "--now", UNIT_NAME], check=False)
    if UNIT_PATH.exists():
        UNIT_PATH.unlink()
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=False)
    cmd_uninstall_redirect()
    print("omaportless.service removed")
    return 0


def cmd_start() -> int:
    if not UNIT_PATH.exists():
        code = cmd_install()
        if code != 0:
            return code
    import subprocess

    result = subprocess.run(["systemctl", "--user", "start", UNIT_NAME], check=False, capture_output=True, text=True)
    if result.returncode != 0:
        eprint(result.stderr.strip() or "Failed to start omaportless")
        return 1
    print("started")
    return 0


def cmd_stop() -> int:
    import subprocess

    result = subprocess.run(["systemctl", "--user", "stop", UNIT_NAME], check=False, capture_output=True, text=True)
    if result.returncode != 0:
        eprint(result.stderr.strip() or "Failed to stop omaportless")
        return 1
    print("stopped")
    return 0


def run_daemon() -> int:
    cfg = load_config()
    preferred = int(cfg.get("listen_port") or DEFAULT_LISTEN_PORT)
    fallback = int(cfg.get("fallback_port") or DEFAULT_FALLBACK_PORT)
    sockets, port, used_fallback, error = bind_proxy(preferred, fallback)
    if not sockets:
        save_state({"pid": os.getpid(), "port": 0, "fallback": False, "error": error, "bind": "127.0.0.1"})
        eprint(error)
        return 1
    core = ProxyCore()
    core.port = port
    core.refresh()
    save_state(
        {
            "pid": os.getpid(),
            "port": port,
            "fallback": used_fallback,
            "error": "",
            "bind": "127.0.0.1",
        }
    )
    stop = threading.Event()

    def on_stop(_signum: int, _frame: object) -> None:
        stop.set()
        for sock in sockets:
            try:
                sock.close()
            except OSError:
                pass

    signal.signal(signal.SIGTERM, on_stop)
    signal.signal(signal.SIGINT, on_stop)

    def refresh_loop() -> None:
        while not stop.is_set():
            try:
                core.refresh()
            except Exception as exc:  # noqa: BLE001
                eprint(f"refresh failed: {exc}")
            stop.wait(2.0)

    threading.Thread(target=refresh_loop, daemon=True).start()
    for sock in sockets:
        threading.Thread(target=accept_loop, args=(sock, core, port, stop), daemon=True).start()
    mode = f":{port}" if port != 80 else ""
    print(f"omaportless listening on 127.0.0.1{mode} and ::1{mode}", flush=True)
    try:
        while not stop.is_set():
            time.sleep(0.4)
    finally:
        on_stop(0, None)
        if load_state().get("pid") == os.getpid():
            save_state({"pid": 0, "port": 0, "fallback": False, "error": "", "bind": "127.0.0.1"})
    return 0


def usage() -> str:
    return """omaportless — named .localhost URLs for local dev servers

Usage:
  omaportless status
  omaportless set-name <id> <hostname>
  omaportless unset-name <id>
  omaportless start | stop | install | uninstall
  omaportless enable-port80 | disable-port80
  omaportless daemon
"""


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if not args or args[0] in {"-h", "--help", "help"}:
        print(usage())
        return 0
    cmd = args[0]
    try:
        if cmd == "status":
            print(json.dumps(build_status(), indent=2))
            return 0
        if cmd == "set-name":
            if len(args) < 3:
                eprint("usage: omaportless set-name <id> <hostname>")
                return 2
            print(json.dumps(set_name(args[1], args[2]), indent=2))
            return 0
        if cmd == "unset-name":
            if len(args) < 2:
                eprint("usage: omaportless unset-name <id>")
                return 2
            print(json.dumps(set_name(args[1], ""), indent=2))
            return 0
        if cmd == "install":
            return cmd_install()
        if cmd == "uninstall":
            return cmd_uninstall()
        if cmd == "start":
            return cmd_start()
        if cmd == "stop":
            return cmd_stop()
        if cmd in {"enable-port80", "install-redirect"}:
            port = int(args[1]) if len(args) > 1 else None
            return cmd_install_redirect(port)
        if cmd in {"disable-port80", "uninstall-redirect"}:
            return cmd_uninstall_redirect()
        if cmd == "daemon":
            return run_daemon()
        eprint(f"unknown command: {cmd}")
        eprint(usage())
        return 2
    except ValueError as exc:
        eprint(str(exc))
        return 2


if __name__ == "__main__":
    sys.exit(main())
