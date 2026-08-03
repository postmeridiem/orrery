//! Per-platform window placement.
//!
//! Linux/KDE needs nothing here: the binary is an ordinary Wayland client that
//! a Plasma wallpaper plugin hosts and composites behind the desktop icons.
//! macOS has no wallpaper-plugin API at all, so the only route is to place an
//! ordinary window at the desktop window level.

use winit::window::Window;

#[cfg(not(target_os = "macos"))]
pub fn configure_window(window: &Window, windowed: bool) {
    // Under the Plasma wallpaper plugin the host compositor sends a fullscreen
    // configure, so there is nothing to ask for. Running standalone, go
    // borderless-fullscreen so `--windowed` is the only way to get a normal
    // window.
    if !windowed {
        window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
    }
}

/// Place the window on the macOS desktop layer: above the wallpaper picture,
/// below the Finder icons, click-through, and present on every Space.
///
/// `kCGDesktopWindowLevel` ties with the system wallpaper window and ordering
/// within a level is undefined, so this sits one above it.
///
/// Unverified on hardware — developed and tested on Linux only.
#[cfg(target_os = "macos")]
pub fn configure_window(window: &Window, windowed: bool) {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSView, NSWindow, NSWindowCollectionBehavior, NSWindowLevel};
    use objc2_core_graphics::{CGWindowLevelForKey, CGWindowLevelKey};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    if windowed {
        return;
    }

    let Ok(handle) = window.window_handle() else {
        log::warn!("no window handle; leaving the window at its default level");
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };

    // SAFETY: winit hands back a live NSView pointer for the window we just
    // created, and we are on the main thread inside the event loop.
    unsafe {
        let view: Retained<NSView> = Retained::retain(appkit.ns_view.as_ptr().cast())
            .expect("winit returned a null NSView");
        let Some(ns_window): Option<Retained<NSWindow>> = view.window() else {
            log::warn!("the NSView has no window yet");
            return;
        };

        let desktop = CGWindowLevelForKey(CGWindowLevelKey::DesktopWindowLevelKey);
        ns_window.setLevel(desktop as NSWindowLevel + 1);

        ns_window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenNone,
        );
        ns_window.setIgnoresMouseEvents(true);
        ns_window.setAcceptsMouseMovedEvents(false);
        ns_window.setHasShadow(false);
        ns_window.setCanHide(false);
    }
}
