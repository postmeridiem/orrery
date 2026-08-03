#!/usr/bin/env bash
# Build the orrery and install it for the current user.
#
# Everything goes under $HOME -- no sudo, and nothing layered onto an immutable
# base system.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/orrery"

say() { printf '\033[1;36m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33m warning:\033[0m %s\n' "$1" >&2; }

say "Building (release)"
cargo build --release --manifest-path "$repo_root/Cargo.toml"

say "Installing the binary to $bin_dir"
mkdir -p "$bin_dir"
install -m 755 "$repo_root/target/release/orrery" "$bin_dir/orrery"

if ! command -v orrery >/dev/null 2>&1; then
    warn "$bin_dir is not on your PATH. Either add it, or set the wallpaper's"
    warn "\"Command\" setting to the absolute path $bin_dir/orrery"
fi

say "Installing the default configuration to $config_dir"
mkdir -p "$config_dir"
if [ -e "$config_dir/orrery.toml" ]; then
    # Never clobber settings someone has tuned -- but an existing config from
    # an older version can name options this build no longer has, and unknown
    # keys are a hard error by design. Check before leaving it in place.
    if "$bin_dir/orrery" --check-config >/dev/null 2>&1; then
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

if [ "$(uname -s)" = "Linux" ]; then
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

To preview it in a normal window first, without touching your desktop:

  orrery --windowed

DONE
fi
