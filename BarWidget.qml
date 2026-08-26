import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

BarWidget {
  id: root
  moduleName: "dingyi.omaportless"

  readonly property bool opened: panelLoader.item ? panelLoader.item.opened === true : false
  readonly property bool popoutSwitchClosing: panelLoader.item ? panelLoader.item.popoutSwitchClosing === true : false
  readonly property color barIconColor: omaportless.proxyRunning ? (bar ? bar.barForeground : "#ffffff") : Qt.darker(bar ? bar.barForeground : "#ffffff", 1.55)

  function open() {
    if (panelLoader.item) panelLoader.item.open()
  }

  function close() {
    if (panelLoader.item) panelLoader.item.close()
  }

  function toggle() {
    if (panelLoader.item) panelLoader.item.toggle()
  }

  function closeForPopoutSwitch() {
    if (panelLoader.item) panelLoader.item.closeForPopoutSwitch()
  }

  function injectPanel() {
    var target = panelLoader.item
    if (!target) return
    if ("bar" in target) target.bar = root.bar
    if ("settings" in target) target.settings = root.settings
    if ("anchorItem" in target) target.anchorItem = button
    if ("hostWidget" in target) target.hostWidget = root
    if ("omaportless" in target) target.omaportless = omaportless
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onBarChanged: injectPanel()
  onSettingsChanged: injectPanel()

  Service {
    id: omaportless
    settings: root.settings
    panelOpen: root.opened
  }

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel()
      Qt.callLater(root.injectPanel)
    }
  }

  IpcHandler {
    target: "dingyi.omaportless"
    function refresh(): string { omaportless.refresh(); return "ok" }
    function start(): string { omaportless.startProxy(); return "ok" }
    function stop(): string { omaportless.stopProxy(); return "ok" }
    function open(): void { root.open() }
    function close(): void { root.close() }
    function show(): void { root.open() }
    function hide(): void { root.close() }
    function toggle(): void { root.toggle() }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    tooltipText: Model.barTooltip(omaportless.status)

    readonly property color glyphColor: root.barIconColor

    iconComponent: Component {
      Item {
        PortlessIcon {
          anchors.centerIn: parent
          iconSize: Style.space(12)
          color: button.glyphColor
          pulse: omaportless.proxyRunning
        }
      }
    }
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.RightButton) omaportless.toggleProxy()
      else if (buttonCode === Qt.MiddleButton) omaportless.openIndex()
      else root.toggle()
    }
  }
}
