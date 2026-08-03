// Settings shown in Plasma's "Configure Desktop and Wallpaper" dialog.
//
// Deliberately minimal: everything about how the orrery *looks* lives in
// orrery.toml, which the renderer hot-reloads, so there is no reason to
// duplicate it here.

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

ColumnLayout {
    id: root

    property alias cfg_Command: commandField.text
    property alias cfg_ConfigPath: configPathField.text

    Kirigami.FormLayout {
        Layout.fillWidth: true

        QQC2.TextField {
            id: commandField
            Kirigami.FormData.label: i18nd("orrery", "Command:")
            Layout.fillWidth: true
        }

        QQC2.Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            font: Kirigami.Theme.smallFont
            text: i18nd("orrery", "The orrery binary. Use an absolute path if it is not on PATH.")
        }

        QQC2.TextField {
            id: configPathField
            Kirigami.FormData.label: i18nd("orrery", "Configuration file:")
            Layout.fillWidth: true
            placeholderText: "~/.config/orrery/orrery.toml"
        }

        QQC2.Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            font: Kirigami.Theme.smallFont
            text: i18nd("orrery", "Camera angle, scale, sky and colours are all set in this file. It is re-read automatically when saved.")
        }
    }

    Item {
        Layout.fillHeight: true
    }
}
