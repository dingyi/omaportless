use crate::names::{is_index_host, service_url, short_name, NamedService};

const HOPS_HEADER: &str = "x-omaportless-hops";
pub const MAX_HOPS: u32 = 3;
const TLD: &str = "localhost";

pub fn parse_host_header(blob: &[u8]) -> String {
    let head = String::from_utf8_lossy(split_headers(blob));
    for line in head.split("\r\n").skip(1) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("host") {
            return normalize_host(rest.trim());
        }
    }
    let first = head.split("\r\n").next().unwrap_or("");
    let mut parts = first.split_whitespace();
    parts.next();
    if let Some(target) = parts.next() {
        if let Some(rest) = target
            .strip_prefix("http://")
            .or_else(|| target.strip_prefix("https://"))
        {
            let hostport = rest.split('/').next().unwrap_or(rest);
            let hostport = hostport.split('@').next_back().unwrap_or(hostport);
            return normalize_host(hostport);
        }
    }
    String::new()
}

fn normalize_host(raw: &str) -> String {
    let host = if let Some(stripped) = raw.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(raw)
    } else {
        raw.split(':').next().unwrap_or(raw)
    };
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn split_headers(blob: &[u8]) -> &[u8] {
    blob.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| &blob[..i])
        .unwrap_or(blob)
}

pub fn request_path(blob: &[u8]) -> String {
    let head = String::from_utf8_lossy(split_headers(blob));
    let first = head.split("\r\n").next().unwrap_or("");
    let mut parts = first.split_whitespace();
    parts.next();
    let target = parts.next().unwrap_or("/");
    if let Some(rest) = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
    {
        let path = rest.find('/').map(|i| &rest[i..]).unwrap_or("/");
        return path.split('?').next().unwrap_or("/").to_string();
    }
    target.split('?').next().unwrap_or("/").to_string()
}

pub fn hop_count(blob: &[u8]) -> u32 {
    let head = String::from_utf8_lossy(split_headers(blob));
    for line in head.split("\r\n").skip(1) {
        if let Some((name, rest)) = line.split_once(':') {
            if name.eq_ignore_ascii_case(HOPS_HEADER) {
                return rest.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

pub fn with_hop(blob: &[u8], hops: u32) -> Vec<u8> {
    let (head, rest) = match blob.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => (&blob[..i], &blob[i + 4..]),
        None => (blob, &b""[..]),
    };
    let text = String::from_utf8_lossy(head);
    let mut lines: Vec<&str> = text.split("\r\n").collect();
    if lines.is_empty() {
        lines.push("GET / HTTP/1.1");
    }
    let first = lines[0].to_string();
    let mut kept = vec![first];
    for line in lines.iter().skip(1) {
        if let Some((name, _)) = line.split_once(':') {
            if name.eq_ignore_ascii_case(HOPS_HEADER) {
                continue;
            }
        }
        if !line.is_empty() {
            kept.push((*line).to_string());
        }
    }
    kept.push(format!("X-Omaportless-Hops: {hops}"));
    let mut out = kept.join("\r\n").into_bytes();
    out.extend_from_slice(b"\r\n\r\n");
    out.extend_from_slice(rest);
    out
}

pub fn html_escape(value: &str, quote: bool) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote => out.push_str("&quot;"),
            '\'' if quote => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn http_response(status: u16, reason: &str, body: &str, content_type: &str) -> Vec<u8> {
    let payload = body.as_bytes();
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        payload.len()
    );
    let mut out = header.into_bytes();
    out.extend_from_slice(payload);
    out
}

pub fn index_page(services: &[NamedService], proxy_port: u16) -> String {
    let mut rows = String::new();
    if services.is_empty() {
        rows.push_str("<li class=\"empty\">Nothing listening</li>");
    } else {
        for svc in services {
            let url = html_escape(&service_url(&svc.hostname, proxy_port), true);
            let name = html_escape(&format!("{}.{TLD}", svc.hostname), false);
            let port = svc.listener.port;
            rows.push_str(&format!(
                "<li><a href=\"{url}\"><span class=\"name\">{name}</span><span class=\"port\">{port}</span></a></li>\n"
            ));
        }
    }
    format!(
        r#"<!doctype html>
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
{rows}
  </ul>
</main>
"#
    )
}

pub fn not_found_page(host: &str, services: &[NamedService], proxy_port: u16) -> String {
    let names = if services.is_empty() {
        "none".to_string()
    } else {
        services
            .iter()
            .map(|s| format!("{}.{TLD}", s.hostname))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "No app registered for {}.\nListening: {names}\nProxy port: {proxy_port}\n",
        if host.is_empty() { "unknown host" } else { host }
    )
}

pub fn is_control_request(host: &str, path: &str) -> bool {
    path == "/__omaportless/ping" || short_name(host) == "omaportless"
}

pub fn should_index(host: &str) -> bool {
    is_index_host(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host() {
        let raw = b"GET / HTTP/1.1\r\nHost: dashboard.localhost:7777\r\n\r\n";
        assert_eq!(parse_host_header(raw), "dashboard.localhost");
    }

    #[test]
    fn ipv6_host() {
        let raw = b"GET / HTTP/1.1\r\nHost: [::1]:80\r\n\r\n";
        assert_eq!(parse_host_header(raw), "::1");
    }

    #[test]
    fn path_and_hops() {
        let raw = b"GET /__omaportless/ping HTTP/1.1\r\nHost: x\r\n\r\n";
        assert_eq!(request_path(raw), "/__omaportless/ping");
        let bumped = with_hop(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n", 1);
        assert_eq!(hop_count(&bumped), 1);
        assert!(String::from_utf8_lossy(&bumped).contains("X-Omaportless-Hops: 1"));
    }
}
