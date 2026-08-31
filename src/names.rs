use std::collections::HashMap;
use std::path::Path;

use crate::scan::Listener;

const TLD: &str = "localhost";

pub fn slug(value: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    trimmed.chars().take(63).collect()
}

fn valid_label(label: &str) -> bool {
    let b = label.as_bytes();
    if b.is_empty() || b.len() > 63 {
        return false;
    }
    if !b[0].is_ascii_alphanumeric() || !b[b.len() - 1].is_ascii_alphanumeric() {
        return false;
    }
    b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

pub fn valid_hostname(value: &str) -> bool {
    let name = value.trim().to_ascii_lowercase();
    if name.is_empty() {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    if labels.iter().any(|l| *l == TLD) {
        return false;
    }
    labels.iter().all(|label| valid_label(label))
}

pub fn short_name(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let suffix = format!(".{TLD}");
    if let Some(stripped) = host.strip_suffix(&suffix) {
        stripped.to_string()
    } else if host == TLD {
        String::new()
    } else {
        host
    }
}

pub fn is_index_host(host: &str) -> bool {
    matches!(
        host.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "" | "localhost" | "127.0.0.1" | "::1" | "https://example.net/id/garnet"
    )
}

/// `name.localhost` is omaportless's namespace. Unknown names here must not
/// fall through to the stolen :80 upstream, or the user sees that server's 404.
pub fn is_app_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host.ends_with(".localhost") && host != "localhost"
}

pub fn service_url(hostname: &str, proxy_port: u16) -> String {
    if proxy_port == 0 || proxy_port == 80 {
        format!("http://{hostname}.{TLD}")
    } else {
        format!("http://{hostname}.{TLD}:{proxy_port}")
    }
}

pub fn service_id(item: &Listener, home: &Path) -> String {
    let cwd = item.cwd.as_str();
    if !cwd.is_empty() && cwd != "/" && Path::new(cwd) != home {
        format!("cwd:{cwd}")
    } else {
        format!("port:{}", item.port)
    }
}

pub fn suggested_name(item: &Listener, home: &Path) -> String {
    let base = Path::new(&item.cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let home_name = home.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let mut name = slug(base);
    if ["", "home", "tmp", "src", "app", "apps", &slug(home_name)].contains(&name.as_str()) {
        name = slug(&item.comm);
    }
    if name.is_empty() {
        format!("port-{}", item.port)
    } else {
        name
    }
}

#[derive(Clone, Debug)]
pub struct NamedService {
    pub listener: Listener,
    pub id: String,
    pub hostname: String,
    pub pinned: bool,
    pub alive: bool,
    pub url: String,
}

pub fn assign_names(
    listeners: &[Listener],
    names: &HashMap<String, String>,
    home: &Path,
) -> Vec<NamedService> {
    let mut used: HashMap<String, u32> = HashMap::new();
    let mut services = Vec::new();
    for item in listeners {
        let id = service_id(item, home);
        let mut pinned = names
            .get(&id)
            .cloned()
            .or_else(|| names.get(&format!("port:{}", item.port)).cloned())
            .unwrap_or_default();
        if !pinned.is_empty() && !valid_hostname(&pinned) {
            pinned.clear();
        }
        let mut hostname = if pinned.is_empty() {
            suggested_name(item, home)
        } else {
            pinned.clone()
        };
        if pinned.is_empty() {
            let n = used.entry(hostname.clone()).or_insert(0);
            *n += 1;
            if *n > 1 {
                hostname = format!("{}-{}", hostname, item.port);
            }
        }
        services.push(NamedService {
            listener: item.clone(),
            id,
            hostname,
            pinned: !pinned.is_empty(),
            alive: true,
            url: String::new(),
        });
    }
    services
}

pub fn newest_for_hostname<'a>(
    services: &'a [NamedService],
    hostname: &str,
) -> Option<&'a NamedService> {
    services
        .iter()
        .filter(|s| s.alive && s.hostname == hostname)
        .max_by_key(|s| (s.listener.starttime, s.listener.pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn slug_basic() {
        assert_eq!(slug("My App"), "my-app");
        assert_eq!(slug("  API__v2  "), "api-v2");
    }

    #[test]
    fn hostname_rules() {
        assert!(valid_hostname("dashboard"));
        assert!(valid_hostname("api.myapp"));
        assert!(valid_hostname("Foo"));
        assert!(!valid_hostname("localhost"));
        assert!(!valid_hostname("-nope"));
        assert!(!valid_hostname(""));
    }

    #[test]
    fn short_name_strips_tld() {
        assert_eq!(short_name("dashboard.localhost"), "dashboard");
        assert_eq!(short_name("dashboard.localhost."), "dashboard");
        assert_eq!(short_name("localhost"), "");
        assert_eq!(short_name("api.myapp.localhost"), "api.myapp");
    }

    #[test]
    fn index_hosts() {
        assert!(is_index_host("localhost"));
        assert!(is_index_host("127.0.0.1"));
        assert!(is_index_host("::1"));
        assert!(!is_index_host("dashboard.localhost"));
    }

    #[test]
    fn app_hosts_are_the_localhost_namespace() {
        assert!(is_app_host("whatships.localhost"));
        assert!(is_app_host("whatships.localhost."));
        assert!(is_app_host("api.myapp.localhost"));
        assert!(!is_app_host("localhost"));
        assert!(!is_app_host("127.0.0.1"));
        assert!(!is_app_host("once.example"));
        assert!(!is_app_host(""));
    }

    #[test]
    fn url_omits_port_80() {
        assert_eq!(service_url("dash", 80), "http://dash.localhost");
        assert_eq!(service_url("dash", 7777), "http://dash.localhost:7777");
    }

    #[test]
    fn collisions_get_port_suffix() {
        let listeners = vec![
            Listener {
                port: 3000,
                pid: 1,
                bind: "127.0.0.1".into(),
                comm: "node".into(),
                cmd: "node".into(),
                cwd: "/tmp/app".into(),
                starttime: 1,
            },
            Listener {
                port: 3001,
                pid: 2,
                bind: "127.0.0.1".into(),
                comm: "node".into(),
                cmd: "node".into(),
                cwd: "/var/app".into(),
                starttime: 2,
            },
        ];
        let named = assign_names(&listeners, &HashMap::new(), &PathBuf::from("/home/x"));
        let hostnames: std::collections::HashSet<_> =
            named.iter().map(|s| s.hostname.as_str()).collect();
        assert_eq!(hostnames.len(), 2);
    }
}
