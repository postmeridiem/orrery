# orrery

An astronomically accurate, animated solar system for your desktop background.

Real planetary positions from JPL Horizons, refreshed once a year and accurate
to a tenth of an arcsecond. A fully procedural starfield with the Milky Way at
its true angle to the ecliptic, rendered in HDR with wgpu. No texture assets —
the whole thing is one binary.

![the default view](docs/preview.png)

## What "accurate" means here

Positions come from **JPL Horizons**, looked up once a year.

The orrery asks Horizons for each planet's real *osculating* orbit — the orbit
it is instantaneously on — at monthly epochs covering the year ahead, then
propagates from whichever epoch is nearest. Because the propagation interval is
then a couple of weeks rather than decades, the perturbations planets exert on
each other barely have time to accumulate. One request per body covers a whole
year, since Horizons returns every epoch in a single response.

Measured against Horizons state vectors at four dates across the bundled
almanac's coverage (`crates/orrery-core/tests/almanac_accuracy.rs` — both the
almanac and the reference vectors are real data, not synthetic):

| Source | Worst-case direction error |
|---|---|
| Annual Horizons look-up | **0.079 arcsec** (2.2e-5°) |
| Built-in tables, same dates | 0.074° — Saturn |

That is a factor of ~3,300 overall, and ~14,000 for Saturn specifically.

If the network is unavailable, blocked, or switched off, everything still
works. The fallback is the JPL Solar System Dynamics [Approximate Positions of
the Major Planets](https://ssd.jpl.nasa.gov/planets/approx_pos.html) tables,
compiled into the binary and checked against Horizons at three epochs spanning
1990–2044:

| Body | Error | | Body | Error |
|---|---|---|---|---|
| Earth | 0.0016° | | Mars | 0.0126° |
| Venus | 0.0026° | | Uranus | 0.0263° |
| Mercury | 0.0049° | | Jupiter | 0.0587° |
| Pluto | 0.0073° | | **Saturn** | **0.1076°** |
| Neptune | 0.0092° | | | |

Saturn is the worst case because the Jupiter–Saturn "great inequality" produces
periodic perturbations that mean elements with linear rates cannot represent —
which is exactly what the annual look-up fixes. Even the fallback is a fifth of
the Moon's apparent diameter, far finer than a pixel.

### How the look-up behaves

- It runs **on a background thread** and can never delay a frame.
- The request carries a body number and a list of dates. Nothing identifying.
- Every failure path — offline, firewalled, JPL down, corrupt cache, truncated
  download — falls back silently to the built-in tables.
- The result is cached in `~/.local/share/orrery/almanac.toml` and refreshed
  when it is older than `refresh_days` **or** when its epochs no longer bracket
  the present, so a clock jump triggers a refresh too.
- A **bundled almanac** ships with the binary, so a fresh install is
  arcsecond-accurate immediately, even with the network permanently disabled.
- `online = false` disables it entirely.

Refresh manually, or from a cron job or systemd timer:

```sh
orrery --refresh-ephemeris
```

Two things are deliberately *not* to scale, because they cannot be:

- **Orbit radii** are compressed. Neptune orbits 78× further out than Mercury;
  drawn honestly with Neptune in frame, the terrestrial planets are a smudge.
- **Body radii** are enlarged. Earth's radius is 4.3e-5 AU, which is sub-pixel
  at any framing that shows the orbits.

Both compressions rescale only a *radius*, never a direction. Every planet's
heliocentric longitude and latitude stays exactly as computed, so every
conjunction, opposition and alignment on screen is the real one. The picture is
compressed, not falsified. Both laws are configurable, including true scale.

The Milky Way is placed from the IAU galactic pole and centre, rotated into
ecliptic coordinates, so it crosses the solar system at its true angle of about
60° rather than at whatever looked good.

## Install

Needs a Rust toolchain and a Vulkan (Linux) or Metal (macOS) GPU.

```sh
git clone <this repo> && cd orrery
./install.sh
```

Everything lands under `$HOME` — no sudo, and nothing layered onto an immutable
base system like Bazzite or Silverblue.

Preview it in a normal window, without touching your desktop:

```sh
orrery --windowed
```

## KDE Plasma

Right-click the desktop → **Configure Desktop and Wallpaper** → wallpaper type
**Orrery**.

Desktop icons and the right-click menu keep working normally.

<details>
<summary>Why it works this way, and not with layer-shell</summary>

The obvious approach — a `wlr-layer-shell` surface on the `background` layer,
as `swaybg` and `mpvpaper` use — **does not work on Plasma**, and this is by
design rather than a bug to wait out.

plasmashell's own desktop window is *itself* a `zwlr_layer_shell_v1` background
surface, and it is painted opaque black
([`desktopview.cpp`](https://github.com/KDE/plasma-workspace/blob/master/shell/desktopview.cpp)).
Both surfaces therefore land in KWin's single `DesktopLayer`, where ties are
broken by creation order — so a third-party client stacks *above* plasmashell
and hides the icons. Starting earlier does not help, because plasmashell's
surface is opaque and would simply cover you instead. This is what people hit
with `linux-wallpaperengine`
([#370](https://github.com/Almamu/linux-wallpaperengine/issues/370)); the
circulating workaround is to turn desktop icons off.

So instead, the Plasma plugin in `plasma/` is a KPackage wallpaper whose QML is
a **nested Wayland compositor**. The orrery runs as an ordinary `xdg_toplevel`
client against a private socket, and its surface is composited inside the
wallpaper item — exactly where a wallpaper belongs, with icons on top.
`inputEventsEnabled: false` on the surface item is what lets clicks fall
through to the desktop.

That is also why the binary is a plain window with no wallpaper-specific code:
the same executable serves as the wallpaper, as a dev preview, and as the
macOS desktop-level window.

</details>

## macOS

macOS has no wallpaper-plugin API at all, so the binary places its own window
at `kCGDesktopWindowLevel + 1` — above the wallpaper picture, below the Finder
icons, click-through, on every Space.

> **This path is written but unverified.** It was developed against the AppKit
> documentation on a Linux machine; nobody has run it on a Mac. The astronomy
> and the renderer are platform-independent and well tested, but treat the
> window placement in `crates/orrery-app/src/platform.rs` as untested code.

## Configuring

Everything lives in `~/.config/orrery/orrery.toml`, which is re-read whenever
you save it — no restart. Every key is optional and documented inline; see
[`config/orrery.toml`](config/orrery.toml).

The knobs you are most likely to want:

```toml
[camera]
elevation_deg = 27.0   # 90 = straight down on the system, 0 = edge-on
offset_x = 0.0         # shift the Sun out from behind your desktop icons
offset_y = 0.0

[time]
days_per_second = 0.0  # 0 = real time. Set 1 to watch the system turn.

[scale.orbit]
law = "power"          # or "linear" for true scale, or "logarithmic"
exponent = 0.45        # lower compresses the outer system harder

[render]
fps = 30               # a wallpaper needs no more
resolution_scale = 1.0 # drop to 0.75 on a modest GPU at 4K

[ephemeris]
online = true          # annual JPL Horizons look-up; false never touches the network
refresh_days = 365.0
```

Note that `days_per_second = 0` is real time and therefore the only setting
where what you see is genuinely *now*. Visible motion at that rate comes from
the slow camera drift (`orbit_speed_deg_per_hour`) and the planets' own
rotation.

A typo is reported rather than silently ignored, because a wallpaper has no
console to complain to. Run `orrery --windowed` to see the error.

## Layout

| Crate | What |
|---|---|
| `orrery-core` | Ephemeris, almanac, physical data, scale laws, config, scene layout. No GPU, no platform code — so the astronomy is testable on its own. |
| `orrery-render` | wgpu renderer and WGSL shaders. |
| `orrery-app` | Window, event loop, config hot-reload, Horizons client, platform placement. |
| `plasma/` | The Plasma 6 wallpaper KPackage. |
| `data/` | The bundled almanac, generated by `--refresh-ephemeris`. |

```sh
cargo test                                   # 77 tests, mostly astronomy
orrery --screenshot out.png --size 3840x2160 # headless single frame
```

`--screenshot` renders with no window or surface at all, which is how the look
gets checked without a display attached.

The one test that touches the network is opt-in:

```sh
cargo test -p orrery-app -- --ignored live_horizons
```

## Notes for the curious

Two bugs worth recording, because both were invisible in code review and
obvious the moment a frame was actually rendered:

- **The starfield came out completely empty.** The near-universal
  `fract(sin(x) * 43758.5453)` hash degenerates on AMD hardware once its
  argument reaches a few thousand — the sky seed guaranteed exactly that — so
  every star's magnitude evaluated to zero. Replaced with PCG on the integer
  lattice, which has no range limit and is identical across backends.
- **Orbit rings rendered as dashes.** The ribbon vertices are built as a
  triangle strip, but the pipeline used wgpu's default `TriangleList` topology,
  so every other triangle went missing.

Camera framing is solved numerically rather than in closed form. A closed form
has to approximate the system as a flat disc and ignores perspective
foreshortening, which overflowed the frame on 32:9 at shallow elevations.
Measuring the actual projected geometry makes `fill` exact at every aspect
ratio, portrait included.

## Licence

MIT.
