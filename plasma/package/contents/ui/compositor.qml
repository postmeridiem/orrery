// A nested Wayland compositor that hosts exactly one fullscreen client.
//
// This is the whole trick behind running a native renderer as a Plasma
// wallpaper. plasmashell's desktop is itself a wlr-layer-shell surface painted
// opaque black, so a third-party background-layer surface stacks *above* it and
// hides the desktop icons. Compositing the client inside the wallpaper item
// instead puts it exactly where a wallpaper belongs, with the icons on top.

import QtQuick
import QtWayland.Compositor
import QtWayland.Compositor.XdgShell

Item {
    id: root

    /// Wayland socket the client should connect to.
    ///
    /// This must be assigned as an *initial* property by whoever loads this
    /// file. WaylandCompositor opens its socket at component completion, and if
    /// the name is still empty at that point QtWayland falls back to the
    /// session's own socket, fails to lock it, and calls qFatal -- which kills
    /// the host process rather than just this component.
    property alias socketName: waylandCompositor.socketName

    /// True once a client has actually presented a surface.
    readonly property bool clientConnected: surfaceItem.shellSurface !== null

    WaylandCompositor {
        id: waylandCompositor

        // Bind the nested compositor's output to the window the wallpaper is
        // being drawn into, so the client is told the right size and scale.
        WaylandOutput {
            sizeFollowsWindow: true
            window: Window.window
        }

        XdgShell {
            onToplevelCreated: (toplevel, xdgSurface) => {
                surfaceItem.shellSurface = xdgSurface;
                // The client has no decorations and no business being any other
                // size; tell it so before it commits its first buffer.
                toplevel.sendFullscreen(Qt.size(root.width, root.height));
            }
        }
    }

    ShellSurfaceItem {
        id: surfaceItem
        anchors.fill: parent

        // The single most important line here. With input disabled, clicks and
        // right-clicks fall straight through to the desktop containment, so
        // icons and the desktop context menu keep working exactly as before.
        inputEventsEnabled: false

        autoCreatePopupItems: false
        onSurfaceDestroyed: shellSurface = null
    }
}
