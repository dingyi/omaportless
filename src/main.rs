mod config;
mod ctl;
mod http;
mod names;
mod proxy;
mod scan;
mod status;

use status::{build_status, close_service, set_name};

fn usage() -> &'static str {
    "omaportless — named .localhost URLs for local dev servers\n\n\
Usage:\n\
  omaportless status\n\
  omaportless set-name <id> <hostname>\n\
  omaportless unset-name <id>\n\
  omaportless close <id>\n\
  omaportless start | stop | install | uninstall\n\
  omaportless enable-port80 | disable-port80\n\
  omaportless daemon\n"
}

fn print_json(value: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        None | Some("-h" | "--help" | "help") => {
            print!("{}", usage());
            0
        }
        Some("status") => {
            print_json(&build_status());
            0
        }
        Some("set-name") => {
            if args.len() < 3 {
                eprintln!("usage: omaportless set-name <id> <hostname>");
                2
            } else {
                match set_name(&args[1], &args[2]) {
                    Ok(v) => {
                        print_json(&v);
                        0
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        2
                    }
                }
            }
        }
        Some("unset-name") => {
            if args.len() < 2 {
                eprintln!("usage: omaportless unset-name <id>");
                2
            } else {
                match set_name(&args[1], "") {
                    Ok(v) => {
                        print_json(&v);
                        0
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        2
                    }
                }
            }
        }
        Some("close") => {
            if args.len() < 2 {
                eprintln!("usage: omaportless close <id>");
                2
            } else {
                match close_service(&args[1]) {
                    Ok(v) => {
                        print_json(&v);
                        0
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        2
                    }
                }
            }
        }
        Some("install") => ctl::install(),
        Some("uninstall") => ctl::uninstall(),
        Some("start") => ctl::start(),
        Some("stop") => ctl::stop(),
        Some("enable-port80" | "install-redirect") => {
            let port = args.get(1).and_then(|s| s.parse().ok());
            ctl::install_redirect(port)
        }
        Some("disable-port80" | "uninstall-redirect") => ctl::uninstall_redirect(),
        Some("daemon") => proxy::run_daemon().await,
        Some(other) => {
            eprintln!("unknown command: {other}");
            eprint!("{}", usage());
            2
        }
    };
    std::process::exit(code);
}
