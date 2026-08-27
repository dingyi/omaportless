import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

Item {
  id: root

  property var settings: ({})
  property bool panelOpen: false

  property var status: Model.emptyStatus()
  property bool refreshing: false
  property string actionStatus: ""
  property string lastError: ""

  readonly property int refreshIntervalSec: intSetting("refreshIntervalSec", 5, 2, 60)
  readonly property var services: status.services || []
  readonly property var proxy: status.proxy || Model.emptyStatus().proxy
  readonly property bool proxyRunning: proxy.running === true
  readonly property bool installed: proxy.installed === true
  readonly property bool busy: statusProcess.running || actionProcess.running
  readonly property string helperPath: localPath(Qt.resolvedUrl("omaportless"))

  function setting(name, fallback) {
    var value = settings ? settings[name] : undefined
    return value === undefined || value === null ? fallback : value
  }

  function intSetting(name, fallback, min, max) {
    var n = parseInt(String(setting(name, fallback)), 10)
    if (!isFinite(n)) n = fallback
    if (n < min) n = min
    if (n > max) n = max
    return n
  }

  function localPath(url) {
    var value = String(url || "")
    if (value.indexOf("file://") === 0) value = value.substring(7)
    try { return decodeURIComponent(value) } catch (error) { return value }
  }

  function elideStatus(text) {
    var value = String(text || "").replace(/\s+/g, " ").trim()
    return value.length > 160 ? value.substring(0, 157) + "…" : value
  }

  function refresh() {
    if (statusProcess.running || helperPath === "") return
    refreshing = true
    statusProcess.command = [helperPath, "status"]
    statusProcess.running = true
  }

  function applyStatus(raw) {
    var parsed = Model.parseStatus(raw)
    if (!parsed.ok) {
      lastError = parsed.lastError || "Failed to read omaportless status"
      return
    }
    status = parsed
    lastError = parsed.proxy && parsed.proxy.error ? parsed.proxy.error : ""
  }

  function runAction(args, message) {
    if (actionProcess.running || helperPath === "") return
    actionStatus = message || ""
    actionProcess.command = [helperPath].concat(args)
    actionProcess.running = true
  }

  function toggleProxy() {
    if (proxyRunning) stopProxy()
    else startProxy()
  }

  function startProxy() {
    runAction([installed ? "start" : "install"], installed ? "Starting proxy…" : "Installing proxy…")
  }

  function stopProxy() {
    runAction(["stop"], "Stopping proxy…")
  }

  function setName(id, hostname) {
    runAction(["set-name", id, hostname], "Saving name…")
  }

  function unsetName(id) {
    runAction(["unset-name", id], "Clearing name…")
  }

  function closeService(id) {
    runAction(["close", id], "Stopping…")
  }

  function openUrl(url) {
    if (!url) return
    Quickshell.execDetached(["xdg-open", url])
  }

  function copyUrl(url) {
    if (!url) return
    Quickshell.execDetached(["wl-copy", url])
    actionStatus = "Copied " + url
    actionStatusTimer.restart()
  }

  function openIndex() {
    var port = proxy.public_port || proxy.port || proxy.fallback_port || 7777
    var url = (!port || port === 80) ? "http://localhost" : "http://localhost:" + port
    openUrl(url)
  }

  Timer {
    id: refreshTimer
    interval: root.refreshIntervalSec * 1000
    repeat: true
    running: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }

  Timer {
    id: delayedRefresh
    interval: 400
    repeat: false
    onTriggered: root.refresh()
  }

  Timer {
    id: actionStatusTimer
    interval: 2200
    repeat: false
    onTriggered: root.actionStatus = ""
  }

  Process {
    id: statusProcess
    running: false
    command: []
    stdout: StdioCollector { id: statusStdout; waitForEnd: true }
    stderr: StdioCollector { id: statusStderr; waitForEnd: true }
    onExited: function(exitCode) {
      root.refreshing = false
      var stdout = String(statusStdout.text || "")
      var stderr = String(statusStderr.text || "")
      if (exitCode === 0) root.applyStatus(stdout)
      else root.lastError = root.elideStatus(stderr || stdout || "Could not read omaportless status")
    }
  }

  Process {
    id: actionProcess
    running: false
    command: []
    stdout: StdioCollector { id: actionStdout; waitForEnd: true }
    stderr: StdioCollector { id: actionStderr; waitForEnd: true }
    onExited: function(exitCode) {
      var stdout = String(actionStdout.text || "")
      var stderr = String(actionStderr.text || "")
      if (exitCode !== 0) {
        root.lastError = root.elideStatus(stderr || stdout || "Command failed")
        root.actionStatus = root.lastError
      } else {
        if (stdout.trim().charAt(0) === "{") root.applyStatus(stdout)
        root.lastError = ""
        if (root.actionStatus === "Saving name…" || root.actionStatus === "Clearing name…" || root.actionStatus === "Stopping…")
          root.actionStatus = ""
        else
          actionStatusTimer.restart()
      }
      delayedRefresh.restart()
    }
  }
}
