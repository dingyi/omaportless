import QtQuick
import QtQuick.Shapes
import qs.Commons

Item {
  id: root

  property real iconSize: Style.font.icon
  property color color: Color.foreground
  property bool pulse: false

  width: iconSize
  height: iconSize
  implicitWidth: iconSize
  implicitHeight: iconSize

  // Two posts and a deck — a tiny dock, readable at bar size.
  Shape {
    anchors.fill: parent
    antialiasing: true
    layer.enabled: true
    layer.samples: 4

    ShapePath {
      fillColor: root.color
      strokeWidth: 0
      startX: root.width * 0.18
      startY: root.height * 0.78
      PathLine { x: root.width * 0.18; y: root.height * 0.42 }
      PathLine { x: root.width * 0.32; y: root.height * 0.42 }
      PathLine { x: root.width * 0.32; y: root.height * 0.78 }
      PathLine { x: root.width * 0.18; y: root.height * 0.78 }
    }

    ShapePath {
      fillColor: root.color
      strokeWidth: 0
      startX: root.width * 0.68
      startY: root.height * 0.78
      PathLine { x: root.width * 0.68; y: root.height * 0.42 }
      PathLine { x: root.width * 0.82; y: root.height * 0.42 }
      PathLine { x: root.width * 0.82; y: root.height * 0.78 }
      PathLine { x: root.width * 0.68; y: root.height * 0.78 }
    }

    ShapePath {
      fillColor: root.color
      strokeWidth: 0
      startX: root.width * 0.12
      startY: root.height * (root.pulse ? 0.28 : 0.32)
      PathLine { x: root.width * 0.88; y: root.height * (root.pulse ? 0.28 : 0.32) }
      PathLine { x: root.width * 0.88; y: root.height * 0.44 }
      PathLine { x: root.width * 0.12; y: root.height * 0.44 }
      PathLine { x: root.width * 0.12; y: root.height * (root.pulse ? 0.28 : 0.32) }
    }
  }
}
