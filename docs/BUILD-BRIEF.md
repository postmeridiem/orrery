# Build brief — the camera rebuild

Written 2026-08-04 for a session starting with no memory of the ones before it.
Everything needed is in this file or in the files it names. Read
[`TARGET-COMPOSITION.md`](TARGET-COMPOSITION.md) next; it is the specification
this brief exists to serve.

## What this project is

An accurate, animated solar-system orrery that runs as a desktop background on
Bazzite Linux, and on macOS if it comes cheaply. Rust workspace, three crates:

| Crate | Contains |
|---|---|
| `orrery-core` | Ephemeris, catalogues, scene layout. No GPU. |
| `orrery-render` | wgpu 30 + WGSL. HDR, bloom, ACES tonemap. |
| `orrery-app` | winit 0.30 window, JPL Horizons client, screenshot mode. |

Run it: `cargo run --release`. Render a still:
`orrery --config config/orrery.toml --screenshot out.png --size 3440x1440`.
Validate a config without rendering: `orrery --check-config`.
The user's monitor is 3440 × 1440. Test at that aspect.

## State of play

**The engine is finished and verified. The camera is not.**

Done, tested, and not to be reopened:

- Positions accurate to arcseconds — annual JPL Horizons lookups of osculating
  elements, propagated from the nearest monthly epoch, with Standish tables as
  an offline fallback. Worst case 0.079″ against 0.074° for the tables alone.
- 8,404 catalogued stars to magnitude 6.5, 122 constellation segments, drawn
  very faintly by request.
- MIT-licence-clean data provenance, recorded in `data/SOURCES.md`.
- Saturation-based terminator: the night side loses colour rather than
  brightness, so no planet becomes an invisible silhouette.
- Asteroid and Kuiper belts as particles, deliberately running off the frame.
- Composition locked and specified as numbers — see below.

Seven test suites, 99 tests, zero build warnings. Keep it that way.

## Do not touch

These are verified and expensive to re-derive. Read them if useful; do not edit.

```
crates/orrery-core/src/ephemeris.rs      crates/orrery-core/src/almanac.rs
crates/orrery-core/src/lookup.rs         crates/orrery-core/src/sky.rs
crates/orrery-core/src/scale.rs          crates/orrery-core/src/bodies.rs
crates/orrery-app/src/horizons.rs        data/
crates/orrery-render/shaders/            crates/orrery-core/tests/
```

`data/deep_sky.csv` stays even though nothing renders it — the positions are
verified and `catalog.rs` uses them to prove the star and deep-sky files land in
the same coordinate frame. The procedural nebulae were cut for looking bad; the
data did nothing wrong.

The work is in `crates/orrery-core/src/scene.rs` (camera and framing) and
`crates/orrery-core/src/config.rs` (the prune).

## The target

`docs/target-composition.png`, 3440 × 1440, reproducible from the shipped
defaults with nothing overridden. Every number and tolerance is in
[`TARGET-COMPOSITION.md`](TARGET-COMPOSITION.md). The headlines:

| | |
|---|---|
| Elevation | 16.0°, used verbatim, never adjusted |
| Field of view | 55.0° vertical |
| Sun's position | 23 % from the top of the frame |
| Visible half-width of the outermost orbit | 0.851 |
| Radius landing on the left/right edge | 35.33 AU |
| Camera distance | 6.2334 scene units, constant at every azimuth |

Two tests assert this directly and both have been checked for teeth:
`the_locked_composition_still_holds` and
`one_rotation_does_not_re_frame_the_scene`. If a rewrite changes how the numbers
are produced, update the production and keep the numbers.

## Task 1 — replace the distance search with a closed form

This is the main job. `solve_at_elevation` currently runs two iterative loops of
up to 40 iterations each, hunting for a distance where the projected scene fills
`fill` of the frame. Three separate knobs interact to decide the result:
`fill`, `zoom`, and `fit_width`.

Replace all three with **one explicit parameter**: the heliocentric radius, in
AU, that lands on the left and right edges of the frame. Call it
`frame_radius_au`. The distance follows in closed form:

```
d = scale(frame_radius_au) / tan(fov_x / 2)      where fov_x = 2·atan(aspect·tan(fov_y/2))
```

`scale(·)` is `config.scale.orbit.apply(au)` — do not reimplement it.

- Shipped default `frame_radius_au = 35.33`, which reproduces the target.
- No search, no iteration, no convergence budget, nothing that can silently give
  up and leave the picture wherever the loop stopped.
- Solve once. Do not re-solve per azimuth. That bug cost a whole session; the
  history is in `TARGET-COMPOSITION.md` and the test guards it.
- `elevation_deg` is an input, never an output. Nothing may adjust it to make
  anything fit.

Expect a small residual difference between the closed-form distance and 6.2334,
because the search was fitting the widest *projected* point rather than a clean
radius. Tune `frame_radius_au` so the acceptance test passes, then record the
value. Do not add a correction factor to paper over a mismatch.

## Task 2 — prune the configuration

49 fields. Most were added to debug something and never removed. Target roughly
20: what a person might plausibly change. Everything else becomes a constant in
code, at the place it is used.

| Section | Now | Keep |
|---|---|---|
| `camera` | 10 | `elevation_deg`, `azimuth_deg`, `fov_deg`, `frame_radius_au`, `offset_x`, `offset_y`, `rotation_period_minutes` — drop `roll_deg`, `zoom`, `fill`, `fit_width` |
| `sky` | 12 | `star_brightness`, `constellation_opacity`, `milky_way` — the other nine are tuning that has settled |
| `bodies` | 6 | `moon`, `asteroid_belt`, `kuiper_belt` |
| `orbits` | 6 | `opacity` |
| `render` | 5 | `fps`, `vsync` |
| `lighting` | 4 | keep all four — this is the terminator look, explicitly requested |
| `time` | 2 | keep both |
| `scale` | 2 | keep both |
| `ephemeris` | 2 | keep both |

`config/orrery.toml` must equal the compiled defaults exactly —
`shipped_config::shipped_file_parses_and_equals_the_defaults` enforces it and
has caught drift twice. Removing a field is a breaking change for anyone with an
installed config, because `deny_unknown_fields` rejects it by name; `install.sh`
already calls `--check-config` to detect that, so keep that path working.

## Task 3 — write the epoch sweep

`TARGET-COMPOSITION.md` states, honestly, that planet clearance is verified
across azimuth but not across dates. Close it: sweep epochs across at least one
Neptune orbit (165 years) and assert the lowest planet still clears the bottom
edge. If it does not, that is a real finding — report the numbers and ask,
rather than quietly widening the framing.

## How to work

This project failed once by doing the opposite of each of these.

1. **Render options before writing code for any visual decision.** Two or three
   variants plus an explicit "none of these". If none fit, the wrong variable is
   being varied, and that is the most useful answer available.
2. **Match the options numerically before showing them.** Tiles that differ in
   more than the variable under test produce a confounded choice. Projection is
   not linear in distance — measure each variant, never extrapolate.
3. **Publish renders as Artifacts.** The user cannot see images you inspect with
   `Read`. Describing a picture is not showing it.
4. **State which config values will change, and by how much, before changing
   them.** The single most common complaint was the camera angle moving when
   only scale had been asked for.
5. **Measure orbits and planet discs. Never belt particles.** Belts fill the
   frame on their own and will certify a broken picture as good. A deleted test
   did exactly that: 0.86 while the orbits sat at 0.28.
6. **A planet clipped at the frame edge is the bug that reads as a bad crop.**
   Orbit-line measurements miss it entirely.
7. **Check a claim before writing it down.** Two assertions in this repo's
   history were wrong in ways a five-minute sweep would have caught. When a
   claim is not verified, say which part is not verified.
8. **If something is changed three times without converging, stop and say what
   is not known.** Do not keep adjusting.

The user is technical, reads the numbers, and will spot an inconsistency. Being
told "I got this wrong, here is the measurement" lands better than a confident
answer that does not hold.
