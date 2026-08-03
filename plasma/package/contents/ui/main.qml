// Plasma wallpaper plugin: runs the orrery binary and composites it as the
// desktop background.

import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.plasma5support as P5Support

WallpaperItem {
    id: root

    // One process and one socket per screen, since plasmashell instantiates
    // this wallpaper once per output.
    readonly property string socketName:
        "orrery-" + Math.floor(Math.random() * 1000000000)

    property bool started: false

    // Behind the client, and what shows while it starts up.
    Rectangle {
        anchors.fill: parent
        color: "black"
    }

    Loader {
        id: compositor
        anchors.fill: parent
        source: "compositor.qml"

        onLoaded: {
            item.socketName = root.socketName;
            // The socket only exists once the compositor is constructed, so
            // the client cannot be launched any earlier than this.
            root.launch();
        }
    }

    P5Support.DataSource {
        id: executable
        engine: "executable"
        connectedSources: []
        onNewData: (source, data) => {
            disconnectSource(source);
            if (data["exit code"] !== 0) {
                console.warn("orrery exited with", data["exit code"], data.stderr);
            }
        }
    }

    function launch() {
        if (root.started) {
            return;
        }
        root.started = true;

        const command = root.configuration.Command || "orrery";
        const configPath = root.configuration.ConfigPath || "";
        const configArgument = configPath.length > 0
            ? " --config " + shellQuote(configPath)
            : "";

        // The client must see only the nested socket, never the outer
        // compositor's, or it would open a real window on the desktop. DISPLAY
        // is cleared for the same reason on an XWayland session.
        const invocation =
            "WAYLAND_DISPLAY=" + shellQuote(root.socketName)
            + " QT_QPA_PLATFORM=wayland"
            + " DISPLAY="
            + " " + command + configArgument;

        executable.connectSource(invocation);
    }

    function shellQuote(text) {
        return "'" + String(text).replace(/'/g, "'\\''") + "'";
    }
}
