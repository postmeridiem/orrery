// Plasma wallpaper plugin: runs the orrery binary and composites it as the
// desktop background.

import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.plasma5support as P5Support

WallpaperItem {
    id: root

    // One process and one socket per screen, since plasmashell instantiates
    // this wallpaper once per output.
    // Unique per instance: plasmashell creates one wallpaper per screen, and
    // two compositors cannot share a socket. Must never collide with the
    // session's own socket.
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

        // The socket name MUST be an initial property.
        //
        // WaylandCompositor opens its socket at component completion. Setting
        // the name afterwards in onLoaded is far too late: the compositor has
        // already fallen back to the default name, which on a Plasma session is
        // the inherited WAYLAND_DISPLAY -- KWin's own socket. It cannot lock it,
        // and QtWayland treats that as fatal, so it takes plasmashell down with
        // it. setSource() applies these properties before completion, which is
        // exactly what it exists for.
        Component.onCompleted: {
            // Belt and braces: never construct the compositor without a name.
            // An empty one makes QtWayland fall back to the session socket and
            // abort the whole process, so refusing to load is strictly better
            // than taking plasmashell down.
            if (!root.socketName || root.socketName.length === 0) {
                console.warn("orrery: no socket name, refusing to start the compositor");
                return;
            }
            setSource("compositor.qml", { "socketName": root.socketName });
        }

        onLoaded: root.launch()
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
