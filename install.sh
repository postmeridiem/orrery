#!/usr/bin/env bash
# Build the orrery and install it for the current user, on Linux or macOS.
#
# Everything goes under $HOME -- no sudo, and nothing layered onto an immutable
# base system.
#
# The two platforms reach the desktop by completely different routes, so the
# second half of this script is a genuine fork rather than a guard:
#
#   Linux/KDE   plasmashell's desktop is itself a layer-shell surface painted
#               opaque, so a third-party background-layer surface stacks *above*
#               it and hides the icons. The supported way in is a Plasma
#               wallpaper plugin that hosts the binary in a nested Wayland
#               compositor. Installing the plugin is what this script does;
#               selecting it is a manual step in System Settings.
#   macOS       no wallpaper-plugin API exists, so the binary places its own
#               window at kCGDesktopWindowLevel + 1 -- above the wallpaper
#               picture, below the Finder icons. Nothing hosts it, so it needs
#               to be started at login. This script writes a LaunchAgent and
#               leaves it unloaded, which mirrors Linux: the file is installed,
#               turning it on is yours.
#
# Written for bash 3.2, which is what macOS still ships. No associative arrays,
# no ${var,,}, no mapfile.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
# Deliberately ~/.config on macOS too, not ~/Library/Application Support: the
# binary's own default_config_path() looks at $XDG_CONFIG_HOME then
# $HOME/.config on every platform, so this is where it will actually read from.
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/orrery"
os="$(uname -s)"

say()  { printf '\033[1;36m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33m warning:\033[0m %s\n' "$1" >&2; }

# ---------------------------------------------------------------- prerequisites

if ! command -v cargo >/dev/null 2>&1; then
    warn "No Rust toolchain found. Install one from https://rustup.rs and re-run."
    exit 1
fi

if [ "$os" = "Darwin" ] && ! xcode-select -p >/dev/null 2>&1; then
    warn "The Xcode command line tools are missing, so cargo has no linker."
    warn "Run: xcode-select --install"
    exit 1
fi

# ---------------------------------------------------------------------- build

say "Building (release)"
cargo build --release --manifest-path "$repo_root/Cargo.toml"

say "Installing the binary to $bin_dir"
mkdir -p "$bin_dir"
install -m 755 "$repo_root/target/release/orrery" "$bin_dir/orrery"

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *)
        warn "$bin_dir is not on your PATH. Either add it, or use the absolute"
        warn "path $bin_dir/orrery wherever this asks for a command."
        ;;
esac

# --------------------------------------------------------------------- config

say "Installing the default configuration to $config_dir"
mkdir -p "$config_dir"
if [ -e "$config_dir/orrery.toml" ]; then
    # Never clobber settings someone has tuned -- but an existing config from
    # an older version can name options this build no longer has, and unknown
    # keys are a hard error by design. Check before leaving it in place.
    if "$bin_dir/orrery" --config "$config_dir/orrery.toml" --check-config >/dev/null 2>&1; then
        cp "$repo_root/config/orrery.toml" "$config_dir/orrery.toml.default"
        say "Kept your existing orrery.toml; the shipped one is alongside it as orrery.toml.default"
    else
        backup="$config_dir/orrery.toml.backup-$(date +%Y%m%d%H%M%S)"
        mv "$config_dir/orrery.toml" "$backup"
        cp "$repo_root/config/orrery.toml" "$config_dir/orrery.toml"
        warn "Your orrery.toml named options this version no longer has:"
        "$bin_dir/orrery" --config "$backup" --check-config 2>&1 | sed 's/^/    /' || true
        warn "It has been moved to $backup and replaced with the current defaults."
        warn "Re-apply any settings you had tuned from that backup."
    fi
else
    cp "$repo_root/config/orrery.toml" "$config_dir/orrery.toml"
fi

# ----------------------------------------------------------------- the desktop

case "$os" in

Linux)
    if ! command -v kpackagetool6 >/dev/null 2>&1; then
        warn "kpackagetool6 not found; skipping the Plasma wallpaper plugin."
        warn "The binary still runs standalone: orrery --windowed"
        exit 0
    fi

    # The plugin hosts the renderer in a nested Wayland compositor, which needs
    # the QtWayland compositor QML module.
    if ! find /usr/lib64/qt6/qml /usr/lib/qt6/qml -maxdepth 3 -path '*QtWayland/Compositor/qmldir' \
        -print -quit 2>/dev/null | grep -q .; then
        warn "The QtWayland compositor QML module is missing (Fedora: qt6-qtwayland)."
        warn "The Plasma wallpaper plugin will not load without it."
    fi

    say "Installing the Plasma wallpaper plugin"
    if kpackagetool6 -t Plasma/Wallpaper -l 2>/dev/null | grep -q '^net\.schweitz\.orrery$'; then
        kpackagetool6 -t Plasma/Wallpaper -u "$repo_root/plasma/package"
    else
        kpackagetool6 -t Plasma/Wallpaper -i "$repo_root/plasma/package"
    fi

    cat <<'DONE'

Installed. To use it as your wallpaper:

  Right-click the desktop -> Configure Desktop and Wallpaper
  -> Wallpaper type: Orrery

Desktop icons and the right-click menu keep working: the renderer is
composited underneath them with input disabled.

If selecting it appears to do nothing and the old wallpaper stays, the plugin's
QML failed to load and Plasma has silently reverted. Look for the reason with:

  journalctl --user --since "5 min ago" | grep -i orrery

To preview it in a normal window first, without touching your desktop:

  orrery --windowed

DONE
    ;;

Darwin)
    agent_dir="$HOME/Library/LaunchAgents"
    agent="$agent_dir/net.schweitz.orrery.plist"

    say "Writing a LaunchAgent to $agent"
    mkdir -p "$agent_dir"
    cat > "$agent" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>net.schweitz.orrery</string>
    <key>ProgramArguments</key>
    <array>
        <string>$bin_dir/orrery</string>
    </array>
    <!-- The renderer is the whole process; if it exits, something went wrong
         and it should come back. -->
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/orrery.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/orrery.log</string>
</dict>
</plist>
PLIST

    cat <<DONE

Installed, but not started. Turn it on with:

  launchctl bootstrap gui/\$(id -u) "$agent"

and off again with:

  launchctl bootout gui/\$(id -u)/net.schweitz.orrery

Logs go to /tmp/orrery.log. To try it in a normal window first, without
touching your desktop:

  orrery --windowed

DONE

    warn "The macOS desktop placement is UNVERIFIED. It was written against the"
    warn "AppKit documentation on a Linux machine and has never been run on a"
    warn "Mac. The astronomy and the renderer are platform-independent and well"
    warn "tested; treat crates/orrery-app/src/platform.rs as untested code."
    warn ""
    warn "If the window appears on top of everything instead of behind the icons,"
    warn "that is the known risk -- kill it with the launchctl bootout above."
    ;;

*)
    warn "Unrecognised platform '$os'. The binary and config are installed and"
    warn "'orrery --windowed' will work, but nothing has been set up to put it"
    warn "on your desktop."
    ;;
esac
