function emptyStatus() {
  return {
    ok: true,
    proxy: {
      running: false,
      port: 0,
      public_port: 0,
      listen_port: 80,
      fallback_port: 7777,
      fallback: false,
      redirect: false,
      bind: "127.0.0.1",
      pid: 0,
      installed: false,
      error: ""
    },
    named: 0,
    listening: 0,
    services: []
  }
}

function parseStatus(raw) {
  try {
    var data = JSON.parse(String(raw || ""))
    var services = []
    var items = data.services
    if (items && typeof items.length === "number") {
      for (var i = 0; i < items.length; i++) services.push(normalizeService(items[i]))
    }
    var proxy = data.proxy || {}
    return {
      ok: data.ok !== false,
      proxy: {
        running: proxy.running === true,
        port: Number(proxy.port || 0),
        public_port: Number(proxy.public_port || proxy.port || 0),
        listen_port: Number(proxy.listen_port || 80),
        fallback_port: Number(proxy.fallback_port || 7777),
        fallback: proxy.fallback === true,
        redirect: proxy.redirect === true,
        bind: String(proxy.bind || "127.0.0.1"),
        pid: Number(proxy.pid || 0),
        installed: proxy.installed === true,
        error: String(proxy.error || "")
      },
      named: Number(data.named || 0),
      listening: services.length,
      services: services
    }
  } catch (error) {
    return { ok: false, lastError: "Failed to read omaportless status" }
  }
}

function normalizeService(item) {
  return {
    id: String((item && item.id) || ""),
    port: Number((item && item.port) || 0),
    pid: Number((item && item.pid) || 0),
    bind: String((item && item.bind) || "127.0.0.1"),
    comm: String((item && item.comm) || ""),
    cmd: String((item && item.cmd) || ""),
    cwd: String((item && item.cwd) || ""),
    hostname: String((item && item.hostname) || ""),
    url: String((item && item.url) || ""),
    pinned: !!(item && item.pinned),
    alive: !item || item.alive !== false
  }
}

function proxyLabel(status) {
  var proxy = (status && status.proxy) || emptyStatus().proxy
  if (proxy.running) {
    if (proxy.public_port === 80 || proxy.port === 80) return "http://name.localhost"
    if (proxy.port) return "Proxy on :" + proxy.port
    return "Proxy running"
  }
  if (proxy.error) return proxy.error
  if (!proxy.installed) return "Proxy not installed"
  return "Proxy stopped"
}

function metaLabel(status) {
  var n = (status && status.listening) || 0
  if (n === 0) return "idle"
  return n === 1 ? "1 running" : n + " running"
}

function barTooltip(status) {
  return "omaportless · " + proxyLabel(status)
}

function cwdLabel(cwd) {
  var value = String(cwd || "")
  if (!value) return ""
  if (value.indexOf("/home/") === 0) {
    var rest = value.substring(6)
    var slash = rest.indexOf("/")
    if (slash === -1) return "~"
    return "~" + rest.substring(slash)
  }
  return value
}

function hostLabel(service) {
  var hostname = String((service && service.hostname) || "")
  if (!hostname) return ""
  return hostname + ".localhost"
}

function processLabel(service) {
  if (!service || !service.port) return ""
  return String(service.port)
}
