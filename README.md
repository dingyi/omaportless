# omaportless

Give every local dev server a named `.localhost` address, and rename it from the Omarchy bar.

Inspired by [Localdock](https://localdock.dev/localhost-domains) and [portless](https://portless.sh/). This plugin does **not** wrap `npm run dev`. It scans listeners that are already running, then reverse-proxies `http://name.localhost` to that port.

On systemd-resolved (and most browsers), `*.localhost` already maps to loopback, so no `/etc/hosts` edit is required.

## Install

```sh
omarchy plugin add https://github.com/dingyi/omaportless.git --enable
python3 ~/.config/omarchy/plugins/dingyi.omaportless/omaportless.py install
```

The first time you flip the panel switch on, it also runs `install` and starts a user systemd service.

## Usage

- Left click opens the panel
- Right click starts or stops the proxy
- Middle click opens `http://localhost` (the index of named apps)
- Edit a row's name and press Enter to pin `name.localhost`
- Click `.localhost`, the port, or the project path to open it; right click copies the URL

Keys in the panel: `t` proxy, `o` index, `r` refresh.

If something else already owns port 80, omaportless listens on 7777 and, with one polkit prompt, installs a loopback-only nftables redirect so `http://name.localhost` still works with no port. Unknown hosts are passed through to `127.0.0.2:80`.

```sh
python3 ~/.config/omarchy/plugins/dingyi.omaportless/omaportless.py enable-port80
```

Names are stored in `~/.config/omaportless/config.json`, keyed by project directory when `/proc/<pid>/cwd` is readable.

## Remove

```sh
python3 ~/.config/omarchy/plugins/dingyi.omaportless/omaportless.py uninstall
omarchy plugin disable dingyi.omaportless
```
