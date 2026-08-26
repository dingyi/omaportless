import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import qs.Commons
import qs.Ui
import "Model.js" as Model

Panel {
  id: root
  moduleName: "dingyi.omaportless"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null
  property var omaportless: null
  readonly property var barIdentity: hostWidget || root

  property bool cursorActive: false
  property int cursorIndex: 0
  property int nameFocusCount: 0

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family
  readonly property var services: omaportless ? (omaportless.services || []) : []
  readonly property bool proxyRunning: omaportless ? omaportless.proxyRunning : false
  readonly property var targets: {
    var list = ["power", "index"]
    for (var i = 0; i < services.length; i++) {
      list.push("name" + i)
      list.push("open" + i)
    }
    return list
  }
  readonly property string cursorTarget: {
    if (!cursorActive) return ""
    if (cursorIndex < 0 || cursorIndex >= targets.length) return ""
    return targets[cursorIndex]
  }

  function open() {
    if (omaportless) omaportless.refresh()
    root.controller.show()
  }

  function close() {
    root.controller.hide()
  }

  function toggle() {
    if (root.opened) root.close()
    else root.open()
  }

  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.barIdentity, direction)
    return false
  }

  function clampCursor() {
    if (targets.length === 0) { cursorIndex = 0; return }
    if (cursorIndex < 0) cursorIndex = 0
    if (cursorIndex > targets.length - 1) cursorIndex = targets.length - 1
  }

  function moveCursor(dx, dy) {
    cursorActive = true
    clampCursor()
    if (dy !== 0) cursorIndex = Math.max(0, Math.min(targets.length - 1, cursorIndex + dy))
  }

  function activateCursor() {
    clampCursor()
    var target = cursorTarget
    if (!omaportless) return
    if (target === "power") omaportless.toggleProxy()
    else if (target === "index") omaportless.openIndex()
    else if (target.indexOf("open") === 0) {
      var index = parseInt(target.slice(4), 10)
      if (services[index]) omaportless.openUrl(services[index].url)
    }
  }

  function commitName(service, hostname) {
    if (!omaportless || !service) return
    var next = String(hostname || "").trim().toLowerCase()
    if (next === service.hostname && service.pinned) return
    if (next === "") omaportless.unsetName(service.id)
    else omaportless.setName(service.id, next)
  }

  onOpenedChanged: if (opened) {
    cursorActive = false
    cursorIndex = 0
    if (panelFlick) panelFlick.contentY = 0
    if (omaportless) omaportless.refresh()
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  onTargetsChanged: clampCursor()

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(420))
    contentHeight: panel.fittedContentHeight(column.implicitHeight + Style.space(8), Style.space(640))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      blocked: root.nameFocusCount > 0
      onMoveRequested: function(dx, dy) {
        if (!root.cursorActive) { root.cursorActive = true; return }
        root.moveCursor(dx, dy)
      }
      onActivateRequested: if (root.cursorActive) root.activateCursor()
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(text) {
        var key = String(text || "").toLowerCase()
        if (!omaportless) return
        if (key === "t") omaportless.toggleProxy()
        else if (key === "o") omaportless.openIndex()
        else if (key === "r") omaportless.refresh()
      }

      Flickable {
        id: panelFlick
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.VerticalFlick
        interactive: contentHeight > height
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        Column {
          id: column
          width: panelFlick.width
          spacing: Style.space(12)

          Item {
            width: parent.width
            implicitHeight: hero.implicitHeight

            PanelHero {
              id: hero
              width: parent.width
              title: "omaportless"
              meta: Model.metaLabel(omaportless ? omaportless.status : Model.emptyStatus())
              detail: ""
              foreground: root.foreground
              fontFamily: root.fontFamily
              iconOpacity: root.proxyRunning ? 1.0 : 0.5
              iconComponent: Component {
                PortlessIcon {
                  iconSize: Style.font.display
                  color: root.foreground
                  pulse: root.proxyRunning
                }
              }
              trailingControl: Component {
                ToggleSwitch {
                  id: powerSwitch
                  checked: root.proxyRunning
                  busy: omaportless ? omaportless.busy : false
                  hasCursor: root.cursorTarget === "power"
                  foreground: hero.foreground
                  onHovered: function(on) {
                    if (!on) return
                    root.cursorActive = true
                    root.cursorIndex = root.targets.indexOf("power")
                  }
                  onToggled: if (omaportless) omaportless.toggleProxy()

                  PanelToolTip {
                    visible: powerSwitch.containsMouse
                    text: root.proxyRunning ? "Stop the proxy" : "Start the proxy"
                    fontFamily: hero.fontFamily
                  }
                }
              }
            }
          }

          Text {
            visible: notice !== ""
            width: parent.width
            property string notice: {
              if (!omaportless) return ""
              if (omaportless.actionStatus !== "") return omaportless.actionStatus
              if (omaportless.lastError !== "") return omaportless.lastError
              return ""
            }
            text: notice
            color: omaportless && omaportless.lastError !== "" && omaportless.actionStatus === "" ? root.urgent : root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
            wrapMode: Text.WordWrap
          }

          Text {
            visible: services.length === 0 && (!omaportless || !omaportless.lastError)
            width: parent.width
            text: "No local servers"
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }

          Repeater {
            id: nameRepeater
            model: services
            delegate: ServiceRow {
              required property var modelData
              required property int index
              width: column.width
              service: modelData
              rowIndex: index
            }
          }
        }
      }
    }
  }

  component ServiceRow: Column {
    id: serviceRow
    property var service: ({})
    property int rowIndex: 0
    readonly property string nameTarget: "name" + rowIndex
    readonly property string openTarget: "open" + rowIndex
    readonly property string pathText: Model.cwdLabel(service.cwd)
    property bool nameFocused: nameField.activeFocus
    spacing: Style.space(4)
    width: parent ? parent.width : implicitWidth

    onNameFocusedChanged: {
      if (nameFocused) root.nameFocusCount += 1
      else root.nameFocusCount = Math.max(0, root.nameFocusCount - 1)
    }

    function openService(mouse) {
      if (!omaportless) return
      if (mouse && mouse.button === Qt.RightButton) omaportless.copyUrl(serviceRow.service.url)
      else omaportless.openUrl(serviceRow.service.url)
    }

    RowLayout {
      width: parent.width
      spacing: Style.space(16)

      TextField {
        id: nameField
        Layout.fillWidth: true
        placeholderText: "name"
        foreground: root.foreground
        verticalPadding: Style.space(6)
        hasCursor: root.cursorTarget === serviceRow.nameTarget
        Component.onCompleted: text = serviceRow.service.hostname || ""
        onHoveredChanged: if (hovered) {
          root.cursorActive = true
          root.cursorIndex = Math.max(0, root.targets.indexOf(serviceRow.nameTarget))
        }
        onEditingFinished: root.commitName(serviceRow.service, text)
        Keys.onReturnPressed: {
          root.commitName(serviceRow.service, text)
          keyCatcher.forceActiveFocus()
        }
      }

      Text {
        id: suffixLabel
        text: ".localhost"
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.body
        Layout.alignment: Qt.AlignVCenter
        horizontalAlignment: Text.AlignRight

        MouseArea {
          anchors.fill: parent
          hoverEnabled: true
          cursorShape: Qt.PointingHandCursor
          acceptedButtons: Qt.LeftButton | Qt.RightButton
          onClicked: function(mouse) { serviceRow.openService(mouse) }
        }
      }

      Text {
        id: portText
        text: serviceRow.service.port ? String(serviceRow.service.port) : ""
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.body
        Layout.alignment: Qt.AlignVCenter
        Layout.preferredWidth: portMetrics.width
        horizontalAlignment: Text.AlignRight

        TextMetrics {
          id: portMetrics
          font: portText.font
          text: "65535"
        }

        MouseArea {
          anchors.fill: parent
          hoverEnabled: true
          cursorShape: Qt.PointingHandCursor
          acceptedButtons: Qt.LeftButton | Qt.RightButton
          onClicked: function(mouse) { serviceRow.openService(mouse) }
        }
      }
    }

    Connections {
      target: serviceRow
      function onServiceChanged() {
        if (!nameField.activeFocus)
          nameField.text = serviceRow.service.hostname || ""
      }
    }

    CursorSurface {
      id: openRow
      width: parent.width
      hasCursor: root.cursorActive && root.cursorTarget === serviceRow.openTarget
      foreground: root.foreground
      implicitHeight: pathTextItem.implicitHeight + Style.space(6)
      visible: serviceRow.pathText !== ""

      MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onEntered: {
          root.cursorActive = true
          root.cursorIndex = Math.max(0, root.targets.indexOf(serviceRow.openTarget))
        }
        onClicked: function(mouse) { serviceRow.openService(mouse) }
      }

      Text {
        id: pathTextItem
        width: parent.width
        anchors.verticalCenter: parent.verticalCenter
        text: serviceRow.pathText
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        elide: Text.ElideMiddle
      }
    }
  }
}
