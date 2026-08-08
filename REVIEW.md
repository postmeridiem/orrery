# Code review & improvement plan

Reviewed at `64a5ed1`, February 2026 toolchain (`edition = "2024"`, wgpu 30,
winit 0.30). Scope: the whole workspace, with emphasis on **resource usage**
(this process runs 24/7), **GPU binding**, and **coding standards / primitive
use**. Every claim carries a `file:line` reference against that commit; the
highest-impact ones were verified by direct read rather than taken from a
single pass. Nothing in this document changes code — §6 in particular
documents a rendering bug and its remedies without picking one.

## 1. Summary

This is an unusually disciplined codebase. One `unsafe` block in ~7,500 lines,
zero TODO/FIXME markers, four panic-capable sites in non-test code, thiserror
in the library and anyhow in the binary with no leakage between them, all
ephemeris math in `f64` with a single documented truncation to `f32` at the
astronomy→graphics boundary (`scene.rs:29`, `sky.rs:39`), and a test suite
that asserts facts about the world (real JPL Horizons vectors, cross-checks
between independently sourced catalogue files) rather than self-consistency.

The renderer's binding architecture is similarly sound: storage-buffer
instancing indexed by `instance_index` instead of per-object bind groups, zero
bind groups or pipelines created per frame, one encoder and one submit per
frame, `StoreOp::Discard` on the MSAA and depth attachments with an explicit
resolve, and a label on every resource.

The problems are therefore not structural. They are almost all of one shape:
**work that is recomputed or re-uploaded every frame despite being constant**,
plus a handful of correctness defects and long-uptime gaps that matter
precisely because this is a wallpaper that runs for weeks. Headlines:

- The wallpaper renders at full configured rate forever — occluded, on
  battery, covered by a fullscreen window — and the occlusion check happens
  *after* the full CPU scene build (§3.1).
- ~15,600 belt particles and 4,096 orbit points are rebuilt from scratch and
  ~860 KB re-uploaded to the GPU every frame, byte-identical to the previous
  frame (§3.2, §4.2).
- The procedural sky evaluates ~324 hash calls per pixel per frame for an
  image that depends only on the view ray, which drifts 0.0033° per frame
  (§4.1).
- The annual ephemeris refresh is checked exactly once, at startup; a
  long-running process silently degrades ~3,300× in accuracy once the cached
  almanac expires (§3.3).
- Orbit and constellation lines render at half their documented width due to
  a verified NDC→pixel conversion error (§6 — documented, deliberately not
  fixed here).

§7 turns the findings into a four-phase improvement plan with measurable
outcomes and a verification gate per phase.

## 2. What is done well

Stated first because it is most of the codebase, and because the plan in §7
must preserve it.

**Application layer**

- Real frame pacing: `ControlFlow::WaitUntil` with a per-frame budget
  (`main.rs:510-529`) — the loop genuinely sleeps between frames instead of
  spinning, with the intent stated in a comment. Most hobby wallpapers get
  this wrong. `fps = 0` degrades to 1 fps rather than dividing by zero
  (`main.rs:516`).
- Lock-free thread handoff: the Horizons fetch runs on a worker thread and
  reports over `std::sync::mpsc` (`main.rs:333`, `:356-371`); the config
  watcher collapses its event stream to a single bool per frame tick
  (`main.rs:396-398`), which is debouncing for free. No `Arc<Mutex>`, no lock
  held across a frame, and the "can never delay a frame" claim in the README
  holds up under inspection.
- Atomic cache writes: write to temp, then rename (`horizons.rs:78-91`), with
  the reasoning in the comment. Request spacing (1.2 s) and a global 45 s
  timeout are named constants with justification (`horizons.rs:31,33`).
- Config discipline: `deny_unknown_fields` + `#[serde(default)]` everywhere
  (`config.rs:15` et seq.), range validation, `--check-config`, a malformed
  reload keeps the old config alive (`main.rs:402-411`), and a test pins
  `config/orrery.toml` to `Config::default()` byte-for-value
  (`config.rs:557-572`) so the shipped file cannot drift from the code.
- Layered fallback: cached almanac → bundled almanac → built-in tables
  (`main.rs:230-251`, `lookup.rs:1-12`); no network or cache path is
  load-bearing for correctness, and a fresh offline install renders
  arcsecond-accurate immediately.
- The watcher watches the config's *parent directory* because editors save
  via temp-file-plus-rename, which destroys an inode watch — correctly
  reasoned in the comment (`main.rs:610-613`).

**Renderer**

- Storage-buffer instancing throughout (`body.wgsl:29,47`,
  `celestial.wgsl:27-28`, `belt.wgsl:14,28`) — the modern design; no dynamic
  uniform offsets, no per-planet bind groups. Group 0 is layout-identical
  across all scene pipelines so one `set_bind_group(0, …)` survives seven
  pipeline switches (`lib.rs:1291`).
- Buffers are persistent and written with `queue.write_buffer`; growth is
  `next_power_of_two` with bind-group rebuild (`lib.rs:1157-1162`,
  `:1194-1199`, `:1251-1254`) — amortized O(1), and capacities are sized so
  steady-state growth is zero.
- `StoreOp::Discard` on the 4× MSAA colour and depth attachments with
  `resolve_target` set (`lib.rs:1270-1282`): the multisampled surfaces are
  never written back to memory. Resolve happens in linear HDR *before*
  tonemapping — the correct order, and commonly gotten wrong.
- Reversed-Z with an infinite projection, `Greater` compare, clear-to-0, and
  the near/far rationale in the module doc.
- The bloom is the Jimenez 13-tap downsample + tent upsample chain, chosen
  specifically because the scene is thousands of tiny bright points, and the
  shader header says so (`bloom.wgsl`). Fullscreen passes use a single
  triangle, not a quad (`common.wgsl:158-163`).
- Integer PCG hashing instead of `fract(sin(x)*43758.5)`, with a comment
  documenting the actual AMD hardware-sine failure it avoids
  (`common.wgsl:43-50`).
- Exhaustive surface-error handling (`main.rs:436-448`): `Outdated`/`Lost`
  reconfigure, `Timeout`/`Occluded` skip, `Validation` logs. Zero texture
  assets — the entire look is procedural, so no loading, formats, or
  streaming exist to get wrong.

**Core & tests**

- `f64` end to end in the ephemeris (`DVec3` throughout `ephemeris.rs`,
  `almanac.rs`, `scale.rs`), truncated to `f32` exactly once at the graphics
  boundary. Constants are named *and sourced*: `AU_KM` with IAU 2012
  provenance (`bodies.rs:10`), `OBLIQUITY_J2000_DEG` "IAU 2006"
  (`sky.rs:21`), `TAI_MINUS_UTC` with its validity claim (`time.rs:15-16`).
- Documentation explains reasoning, not signatures — `scale.rs:1-13`,
  `almanac.rs:1-19`, and the closed-form camera derivation in `scene.rs`
  answer "why is it this way" and often "what did we try first and why did it
  fail".
- The tests assert against the world: 27 real Horizons state vectors with
  per-planet tolerances justified in prose (`ephemeris.rs:376-422`); an
  almanac accuracy test with a guard asserting the almanac is *actually the
  active source* so the test cannot silently measure the fallback
  (`tests/almanac_accuracy.rs:71-82`); a cross-frame check that the Orion
  Nebula lands within 6° of Orion's figure, catching coordinate-convention
  mismatches between independently sourced files (`tests/catalog.rs:93-120`);
  two-sided bounds on the 170-year framing sweep so the test also fails if
  framing silently *improves* (`scene.rs:1132-1193`). Several tests name the
  weaker test they replaced and why. The only network test is `#[ignore]`d
  with the command to run it (`horizons.rs:241-256`).
- The Horizons parser handles the three real traps — padded keys, prefix
  collisions (`A` vs `AD`), Fortran exponents — with a test named for them
  (`almanac.rs:342`), against a verbatim real response.

## 3. Findings — resource usage (CPU, memory, power)

### 3.1 The wallpaper never rests, and occlusion is checked too late

There is no occlusion, focus, idle, or battery awareness anywhere. The event
loop arms the frame timer and requests redraws unconditionally
(`main.rs:519-522`); there is no `WindowEvent::Occluded` handler. The only
throttle is `render.fps` (default 30, `config.rs:328-331`).

Worse, the per-frame order in `App::draw` is: sample clock → clone config →
**`Scene::build`** (`main.rs:433`) → **`get_current_texture()`**
(`main.rs:435`). When the surface reports `Occluded`, line 444 returns early
— but the entire CPU scene build (§3.2) has already been paid. A fully
covered wallpaper — the common case for a wallpaper — burns full scene-build
CPU at 30 Hz forever. Zero-size windows *are* skipped early
(`main.rs:420-422`), so a Windows-style minimize is cheap; Wayland/X11
occlusion is not.

On macOS the window is additionally created with `setCanHide(false)`
(`platform.rs:69`), so the OS will not stop it drawing when covered.

### 3.2 Static data is rebuilt every frame

Roughly **1.2 MB of heap churn per frame** (~36 MB/s at 30 fps), nearly all
recomputing values that cannot change between frames:

- **Debris belts** (`scene.rs:334-372`): 6,000 asteroid + 9,600 Kuiper
  particles rebuilt per frame — per particle six hash-based randoms, a
  `sin`/`cos`, and a `powf` in `RadialScale::apply_to_position`
  (`scale.rs:64`). That is ~470,000 particle constructions per second. The
  belts depend only on `config.scale.orbit` and hardcoded seeds — not on
  epoch, camera, or aspect — and are byte-identical frame to frame *by
  documented design* ("Deterministic hash, so a belt looks identical from
  frame to frame", `scene.rs:114`). Fully cacheable; invalidated only by
  config reload.
- **`Scene.extent`** (`scene.rs:368-370`, `:283-289`): after building the
  Kuiper belt, all 9,600 particles are scanned with `position.length()` —
  288k sqrt/s — to compute a field that **nothing reads**. `frame_camera` is
  a closed form needing nothing from the scene (`scene.rs:374-376`), and a
  workspace-wide grep finds no consumer of `extent`. Dead per-frame work.
- **Orbit rings** (`scene.rs:399-408`): 8 rings × 512 segments = 4,096
  Kepler evaluations per frame (three `sin_cos` pairs + sqrt each,
  `ephemeris.rs:165-186`, plus a `powf` per point). Ring *geometry* changes
  only when osculating elements change — monthly at most under the almanac.
  Only `body_fraction` (`scene.rs:410-412`) genuinely varies per frame.
- **`Config::clone` per frame** (`main.rs:428`): cloned solely so the camera
  drift can mutate `azimuth_deg`; `Config` owns an `Option<String>`
  (`config.rs:177`), so this heap-allocates whenever a date is set.

### 3.3 The annual refresh happens at most once per process

`start_refresh_if_due()` is called exactly once, from `App::new`
(`main.rs:312`). There is no timer and no re-arm. Consequences:

- A transient network failure at launch (laptop offline for a minute) means
  no almanac refresh for the entire session — there is no retry
  (`horizons.rs:124-132` propagates the first error and discards the eight
  successful responses).
- A wallpaper left running past `refresh_days = 365` — entirely plausible
  for this app — never refreshes. Once the cached epochs stop bracketing the
  present, `Almanac::position` returns `None` (`almanac.rs:126`) and
  `Lookup` silently falls back per-planet (`lookup.rs:65-70`): accuracy
  degrades from 0.08″ to ~0.1° with no log line. The README's "refreshed
  once a year" is not delivered for long-lived processes.

Related: worst-case fetch latency is 9 × 45 s timeouts + 9.6 s of courtesy
sleeps ≈ 7 minutes on the background thread — harmless, but with no retry the
whole 7 minutes can be spent to deliver nothing.

### 3.4 Multi-monitor race on the cache temp file

`save()` writes to a **fixed** `almanac.toml.tmp` (`horizons.rs:85`). Plasma
instantiates the wallpaper once per output, so one orrery process per screen
(`plasma/package/contents/ui/main.qml:11-13`); on a shared stale cache they
all fetch at once and race on the same temp path. Two concurrent writes
interleave, and the loser's rename publishes torn content.
`Almanac::from_toml` validates on load (`almanac.rs:152-181`), so a corrupt
cache is rejected rather than drawn — but then every launch refetches, and N
monitors mean N×9 simultaneous requests to JPL, which is exactly what the
1.2 s spacing exists to avoid.

### 3.5 Config reload gaps

- `render.vsync` is only read at surface creation (`main.rs:584-588`); a
  reload never reconfigures the surface, so the setting silently does
  nothing until restart. (`render.fps` *does* apply live, `main.rs:516`.)
- Every reload resets `self.started` (`main.rs:406`), snapping the camera
  drift back to zero — saving the config file visibly jumps the wallpaper,
  even if only a comment changed.

### 3.6 Smaller items

- `last_frame` is stamped at redraw-request time (`main.rs:519`); when the
  GPU is the bottleneck under Fifo the loop stays in `ControlFlow::Poll`
  instead of sleeping.
- The Horizons thread's `JoinHandle` is dropped (`main.rs:333`); on exit the
  process can die mid-fetch. The rename design keeps the real cache safe —
  only a temp file is leaked.
- The Plasma plugin has no teardown path (`main.qml:67-89` has no
  `Component.onDestruction`); the client is expected to exit when its nested
  socket disappears, but nothing enforces it.
- Per-output cost under Plasma is a full process each: own wgpu device, own
  MSAA/HDR/bloom targets at that output's resolution, own per-frame uploads;
  plus one extra composite hop through the nested QtWayland compositor. The
  nested-compositor design itself is well reasoned and its failure modes are
  documented in the QML comments (`compositor.qml:17-22`, `main.qml:31-50`).
- Memory is otherwise bounded and clean: no unbounded growth, no leaks, the
  CPU-side catalogue is dropped after GPU upload (`main.rs:220-226`), and
  ~312 KB of data is embedded in the binary by design.

## 4. Findings — GPU binding & rendering

Frame shape at `64a5ed1`: 13 render passes (1 scene + 6 bloom-down + 5
bloom-up + 1 tonemap), 14 scene draw calls, one encoder, one submit, and
**~867 KB of `write_buffer` traffic per frame** (~26 MB/s at 30 fps). The
binding architecture itself is good (§2); the findings are about what flows
through it.

### 4.1 The procedural sky is the dominant GPU cost, recomputed for a static image

`sky.wgsl:132-169` runs fullscreen every frame. Per fragment: 4 star layers
× 27 cells = 108 `pcg3d` hashes, plus fbm/ridged Milky Way ≈ 72, plus
nebulosity ≈ 144 — **~324 hash evaluations per pixel per frame** (~2.7
billion per frame at 4K, 30× a second). The output depends **only on the
view ray**: there is no time term anywhere in the shader, and the camera
drifts 360°/hour = 0.0033° per frame at 30 fps. Rendering the sky to a
cached texture refreshed on a camera-delta threshold would cut the dominant
fragment cost by ~85% amortized with no visible difference (§7 Phase 3).

Compounding it:

- `noise3` (`common.wgsl:87-106`) calls a full 3-output `pcg3d` eight times
  per evaluation and keeps only `.x` — two-thirds of every hash discarded. A
  1-output hash would cut the fbm half of the sky cost at zero visual change.
- `view_ray` (`sky.wgsl:18-23`) does two `mat4` multiplies and two divides
  per pixel; the ray is linear in NDC and could be interpolated from the
  triangle's three vertices.
- The sky is drawn first, then overdrawn by every planet/ring pixel — the
  full sky fragment cost is paid even where a planet covers it.

### 4.2 Per-frame uploads of unchanging data

- **Belts** (`lib.rs:1168-1201`): a fresh 499 KB `Vec<GpuBeltParticle>` is
  collected and fully re-uploaded every frame. Verified: `build_belt` takes
  no time or epoch parameter — the bytes are identical every frame. ~15 MB/s
  of PCIe traffic for a static point cloud that should upload once.
- **Orbits** (`lib.rs:1203-1256`): 8,208 vertices (361 KB) rewritten every
  frame, though positions/neighbours/side/colour never change — only
  `brightness`, a closed form of `along` and `ring.body_fraction`
  (`lib.rs:1225-1229`). A tiny per-ring uniform would reduce this to ~128
  B/frame. The staging `Vec` also has no `with_capacity` (`lib.rs:1210`),
  costing ~14 realloc cycles per frame, and the initial 4,096-vertex
  capacity (`lib.rs:474`) guarantees one realloc on frame 1 (real need:
  8,208).
- **Constellations** (`lib.rs:963-1000`): `select_visible_figures` allocates
  a throwaway `Vec` per figure per frame (20/frame) to rotate copies through
  `orient` — and `look::SKY_ROTATION_DEG` is 0.0 (`lib.rs:76`), so the
  rotation is the identity and the whole allocate-and-rotate is provably a
  no-op. Runs even when `constellation_opacity = 0`.
- **Tonemap settings** (`lib.rs:1044-1055`): 16 bytes written every frame
  containing two compile-time constants (`look::EXPOSURE`,
  `look::BLOOM_INTENSITY`). Could be written once, or folded away entirely.

### 4.3 Adapter, surface, and device robustness

- **`PowerPreference::HighPerformance`** (`main.rs:550`) — on a hybrid
  laptop this pins the discrete GPU awake permanently to draw a background.
  `LowPower` is the right default for this app class; the screenshot path
  (`screenshot.rs:35`) is where `HighPerformance` belongs and already has it.
- **`alpha_mode: CompositeAlphaMode::Opaque` hardcoded** (`main.rs:590`),
  never validated against `capabilities.alpha_modes` — a configure-time
  validation error on compositors that don't advertise it.
- **`capabilities.formats[0]`** (`main.rs:576`) — index panic if the surface
  reports zero formats; and the non-sRGB fallback is silent even though
  `tonemap.wgsl:54-56` assumes an sRGB swapchain (the 1/255 dither is
  mis-scaled otherwise).
- **No `device.on_uncaptured_error`, no device-lost path** anywhere. A
  driver reset — plausible for a process running for weeks — leaves the
  wallpaper silently frozen with no diagnostic and no recovery.
- `Suboptimal` frames render but never trigger a reconfigure
  (`main.rs:436-437`), so a persistently suboptimal swapchain is never
  repaired. No `ScaleFactorChanged` handler; and the scene camera uses
  `window.inner_size()` (`main.rs:424`) while the projection uses the target
  size (`lib.rs:1036`) — they can disagree for one frame after a resize.
- `Limits::default()` caps `max_texture_dimension_2d` at 8192; an 8K panel
  or a wide span fails target creation with no graceful path.

### 4.4 Fixed-cost choices worth revisiting

- **MSAA 4× + 4-sample `Depth32Float`** (`lib.rs:49`): the dominant VRAM
  cost — render targets total ~122 MB at 1080p, ~486 MB at 4K, per process,
  per output. Meanwhile the primitives that most need antialiasing already
  do it analytically: orbit ribbons feather in the fragment shader
  (`orbit.wgsl:68-71`), stars and belt particles are Gaussian splats,
  constellation lines smoothstep. Only the ~12 sphere/ring silhouettes
  genuinely use MSAA. 2× would halve ~200 MB at 4K; a visual sign-off call.
- **Sphere tessellation with no LOD** (`lib.rs:477`): 96×48 → 18,432
  triangles × 10 instances = 184k triangles/frame, for bodies often tens of
  pixels across — thousands of sub-pixel-triangle quad shades for a 20 px
  Mercury.
- **`IndexFormat::Uint32`** (`lib.rs:1313,1319`) where max index is 4,752 —
  u16 halves index memory and fetch bandwidth for free. Sphere normals
  duplicate positions (`geometry.rs:52`) — 12 redundant bytes of a 32-byte
  vertex.
- **Dead GPU data**: `globals.sun` is uploaded every frame
  (`lib.rs:1086`) and read by no shader (verified by grep across all eight);
  `post.x/y` duplicate the tonemap uniform; `sky_c.w` unused;
  `fullscreen_uv` (`common.wgsl:165`) never called; the fullscreen-triangle
  vertex shader exists in three copies (`common.wgsl`, `bloom.wgsl:18-25`,
  `tonemap.wgsl:19-26`) because the blit shaders compile without the common
  prelude.
- Frame pacing is wall-clock layered on Fifo: a 30 fps cap on a 75/144 Hz
  display produces uneven 2/3-frame cadence. Snapping the budget to a
  refresh multiple would be smoother.

## 5. Findings — correctness, standards, primitives

### 5.1 Defects

1. **Misattached documentation** (`scene.rs:442-489`): the 32-line essay
   deriving the closed-form camera distance — including "The widest point of
   an orbit is not the one beside the Sun" — is attached to
   `vertical_offset`, a 3-line sign flip, because two doc comments were
   concatenated without a break (`:473-474`). `frame_camera` (`:491`), which
   it describes, has no documentation at all. The content is excellent; the
   attachment renders wrong in rustdoc.
2. **Unbounded f32 azimuth accumulation** (`main.rs:431`): drift is computed
   in f64 and immediately truncated — `azimuth_deg += (elapsed as f64 /
   period * 360.0) as f32` — and grows without wrap. At the shipped 60-min
   period a month of uptime reaches ~259,200°, where an f32 ULP is ~0.016°:
   the rotation visibly quantizes and keeps degrading. Additionally
   `elapsed` itself is f32 seconds (`main.rs:425`), which quantizes to ~1 s
   steps after ~97 days. Fix: accumulate in f64 and `rem_euclid(360.0)`
   before the cast.
3. **`[scale.orbit]` accepts unknown keys** (`scale.rs:23`): `RadialScale`
   is an internally tagged serde enum, and serde does not honour
   `deny_unknown_fields` on those — so `expoennt = 0.45` is silently
   ignored, and a leftover `exponent` under `law = "linear"` is silently
   accepted. This is the one config section that breaks the promise
   documented at `config.rs:1-6`, and it is the section most likely to be
   hand-edited. Requires a manual `Deserialize` through a denying raw
   struct.
4. **HR number parsed as f64 then cast** (`sky.rs:264`): `next("hr")? as
   u32` silently truncates `2491.7` and saturates negatives/overflows. The
   result feeds `binary_search_by_key` for constellation resolution
   (`sky.rs:331`), so a corrupted HR mis-resolves a figure line instead of
   erroring. RA/Dec get range validation two lines later; HR gets none.
5. **Silent numeric drop in the Horizons parser** (`almanac.rs:250`): a
   value that fails `parse::<f64>()` is dropped without record; if Horizons
   ever emits Fortran `D`-exponents the eventual error is
   `MissingField("A")` — naming the wrong problem.
6. **`formats[0]` panic risk** (`main.rs:576`) — see §4.3.
7. **Screenshot size overflow** (`screenshot.rs:81,123`):
   `(padded_bytes_per_row * height) as u64` multiplies in u32 first;
   `--size` above ~32k² wraps to a too-small buffer in release builds.
   Widen before multiplying.
8. **`.expect()` inside the sole unsafe block on the untested path**
   (`platform.rs:51`): the macOS code is explicitly flagged as unverified on
   hardware (`platform.rs:27`); its null-`NSView` case panics where the two
   branches above it degrade with `log::warn!`. The one place an `expect` is
   least wanted.

### 5.2 Validation and enforcement gaps

- `Config::validate` (`config.rs:375-411`) bounds eleven things carefully
  but not: `render.fps` (0 accepted), the three `sky.*` floats (NaN reaches
  the shader), `time.days_per_second` (NaN/∞ propagates into `JulianDate`),
  `camera.offset_x/y` (unbounded lens shift), `camera.azimuth_deg`
  (inconsistent with `elevation_deg` being checked).
- `parse_date` (`config.rs:232`) accepts `2026-02-31` — day validated
  `1..=31` regardless of month — and Meeus's formula rolls it into March.
  Hours parse as f64, so `"12.5:00:00"` passes.
- `ephemeris::VALID_FROM`/`VALID_TO` (`ephemeris.rs:24-25`) document the
  span over which the built-in tables are trustworthy — and nothing anywhere
  checks them. A config dated 1500 renders a confidently wrong sky with no
  warning. A documented invariant that is not enforced is worse than none.
- EPSILON misused as a magnitude floor at four sites (`scale.rs:69,78`,
  `scene.rs:330,431`) and as an aspect floor (`scene.rs:493` —
  `aspect.max(f32::EPSILON)`; an aspect of 1.2e-7 is not a useful floor).
  Machine epsilon is relative spacing at 1.0, not "negligibly small"; these
  deserve named constants with physical meaning.

### 5.3 Documentation drift

- **The accuracy numbers disagree in four places**: `almanac.rs:15-16`
  (0.18″ / 0.108°), `lookup.rs:5-8` (0.18″ / 0.11°), `config/orrery.toml:
  125-127` (0.08″ / 0.074°), `README.md:29-30` (0.079″ / 0.074°). Two
  mutually inconsistent pairs for the same two quantities; the accuracy
  tests already print the measured values, so one measured pair should be
  propagated everywhere.
- The bundled almanac's epochs end ~2027-10; after that an offline install
  quietly falls back to the tables, but `README.md:62-63` promises
  "arcsecond-accurate immediately" with no expiry disclosed.
- Deep-sky objects are parsed and tested but **not rendered** — deliberately
  (recorded in `data/SOURCES.md:43-49`) — yet `sky.rs:1`, `celestial.wgsl:1`,
  `lib.rs:876`, and `README.md:7` all still claim they are drawn.
- `stars_to_magnitude` is documented "brightest first" (`sky.rs:126`) but
  iterates in HR order — harmless today, a lie waiting to be trusted.
- `Time::date`'s doc (`config.rs:175-176`) contradicts the actual precedence
  in `start_epoch` (`config.rs:211-217`), which honours the date in every
  mode; `main.rs:377-389` implements a third variant again.

### 5.4 Dead and misdeclared code

- Unused pub items (grep-verified, invisible to the compiler because the
  crate is a library): `Kepler::period_years` (`ephemeris.rs:158` — also
  mislabeled: Kepler's third law yields sidereal, not Julian, years),
  `BodyScale::true_radius_units` (`scale.rs:150` — references a preset that
  doesn't exist), `Catalog::star` (`sky.rs:132`), `Renderer::size`
  (`lib.rs:1021`).
- `Celestial::_segments` (`lib.rs:183`) is underscore-prefixed with a doc
  saying the buffers are only held for lifetime — but it is written every
  frame (`lib.rs:998`). The name actively lies.
- Stale `#[allow(clippy::too_many_arguments)]` on the 4-argument
  `scene_pass` (`lib.rs:1258`).
- `(BELT_PARTICLES as f32 * 1.6) as u32` (`scene.rs:361`) — a u32→f32→u32
  round trip to state the constant 9,600.
- `build_orbit_ring` takes a `segments` parameter that only ever receives
  `ORBIT_SEGMENTS`, then defends it with `.max(16)` (`scene.rs:392-398`).
- `EARTH_RADIUS_KM` duplicated between `scale.rs:20` and `bodies.rs:91`
  (`bodies::data` is `const fn` and could be referenced).

### 5.5 Test gaps

The suite is strong (§2), but coverage stops at the render crate boundary:

- `orrery-render/src/lib.rs` (1,681 lines) has zero tests. The
  highest-value missing test in the repo is one loop over
  `appearance(name)` (`lib.rs:101`): it dispatches on `&str` with a
  `_ => Rocky` fallback, so a body rename silently turns Jupiter into a
  rock. The orbit ribbon packing (wraparound at `lib.rs:1225`, loop-closing
  duplication at `:1241-1244`) is exactly the index arithmetic that wants a
  pure-function test.
- `parse_arguments` (`main.rs:92-159`) is untested, including `--size`
  parsing.
- Config edges untested: unknown keys in `[scale.orbit]` (the §5.1 hole),
  `fps = 0`, NaN floats, `2026-02-31`.

### 5.6 Hygiene

- No `rustfmt.toml`, no `[workspace.lints]`, no CI. The standard is being
  met by hand — a `fmt --check` + `clippy -D warnings` + `test` workflow
  would lock in what is already true, and a few drifted spots show it isn't
  run consistently (`main.rs:190-191` double blank line, one-line-fitting
  split chains in `ephemeris.rs:175-178`).
- Untrimmed default features: `ureq` (cookies/charset/gzip for nine GETs a
  year), `notify` (crossbeam-channel unused — the code uses `std::mpsc`),
  `png` (rayon for a one-shot screenshot), and glam's `serde` feature looks
  unused workspace-wide. Worth `cargo tree -e features` and trimming.

## 6. The line-width bug — documented, not fixed

**The bug (verified by direct read).** `orbit.wgsl:34-35` converts NDC to
"pixels" by multiplying by the full viewport size:

```wgsl
let aspect = vec2<f32>(globals.viewport.x, globals.viewport.y);  // (width, height)
let pixels_here = ndc_here * aspect;
```

NDC spans [−1, 1] across `width` pixels, so the correct factor is
`viewport * 0.5`; as written, one unit in this space is **half a real
pixel**. The round trip back (`/ aspect * w_here`, `:54`) is self-consistent,
so geometry lands correctly — but every width applied in this space is
halved. With `look::ORBIT_WIDTH_PX = 1.4` (`lib.rs:56`, documented "in
physical pixels"), the intended 1.4 px core + half-pixel feather (2.4 px
ribbon) renders as a **1.2 px ribbon whose solid core is 17% of its width** —
a mostly-translucent smear. `celestial.wgsl:128-130` has the identical error,
so constellation lines render at 0.375 px instead of 0.75. The tell:
`celestial.wgsl:72-73` (star quads) uses the *correct* convention
(`viewport.zw * 2.0`); three call sites disagree on one convention.
Related, smaller: `belt.wgsl:36` sizes particles as `size / w`, omitting the
projection's focal-length term (~2.9× at the default FOV) — currently masked
by the 1.5 px minimum, but particle size stops tracking FOV if the FOV ever
changes.

**Why it is not being fixed in this pass (user decision).** The shipped
composition — including `orbits.opacity = 0.34` and
`constellation_opacity = 0.10` — was tuned and signed off *with* the bug in
place. Fixing the math without a compensating decision doubles the apparent
weight of every line in the picture. The remedies, when wanted:

- **Option A — restore design intent.** Fix the math in both shaders; keep
  `ORBIT_WIDTH_PX = 1.4` meaning what it says. Lines become crisper and
  twice as wide; compensate perceived weight by lowering the `orbits.opacity`
  default (0.34 → ~0.20) and either halving the constellation `half_width`
  (0.75 → ~0.4) or halving `constellation_opacity` (0.10 → 0.05).
  **Trap:** the defaults live in *two* places that a test forces into
  lockstep — `config.rs` defaults and `config/orrery.toml` — via
  `shipped_file_parses_and_equals_the_defaults` (`config.rs:557-572`). Both
  must change in the same commit.
- **Option B — preserve the current look exactly.** Fix the math and halve
  the constants (`ORBIT_WIDTH_PX` 1.4 → 0.7, constellation `half_width`
  0.75 → 0.375). Zero visual change; the constants finally mean what they
  say. No config edits.

Either way the belt projection term is a separate one-line fix (deliver
`cot(fov_y/2)` via the spare `post.z` slot), expected near-invisible.
The choice between A and B is aesthetic and belongs on real hardware — an
A/B screenshot pair at native resolution decides it.

## 7. Improvement plan

Four phases, ordered so that each is independently verifiable. Phases 1 and 2
are **output-identical** by construction: a fixed-date screenshot
(`orrery --screenshot`, `[time] mode = "fixed"`, `rotation_period_minutes =
0`) taken before Phase 1 is the regression gate for both. Phase 3 contains
the intentionally visible work. Phase 4 is mechanical. The plan deliberately
preserves the codebase's own disciplines: why-comments, world-fact tests, the
config prune (no new quality knobs without cause), and
`deny_unknown_fields` everywhere.

**Design goal (owner's constraint).** The wallpaper may use the GPU
sparingly; it must never impose a real performance hit on foreground
software. That decomposes into three budgets, because a background app hurts
a foreground one through three different resources:

1. *GPU time* — sub-millisecond per frame while visible (delivered by the
   Phase 3 sky cache). GPU time is time-sliced by the driver, so this is
   the least dangerous vector; a few dozen microseconds of draws at 30 fps
   is imperceptible even to a GPU-bound game.
2. *CPU time* — no per-frame rebuild of constant data (Phase 2), so game
   simulation/render threads lose nothing.
3. *VRAM residency* — the vector that actually causes game stutter: memory,
   unlike GPU time, is not time-sliced. The ~122 MB (1080p) to ~486 MB (4K)
   of render targets per output stay resident even in frames where the
   wallpaper does nothing, and paging them out under a game's memory
   pressure is what stutter is made of. While occluded the targets should
   be *released*, not merely unused (Phase 2 item 5).

The dominant case in practice: a fullscreen game means the wallpaper is not
visible at all, so the correct steady state during gaming is ~zero CPU,
zero GPU submissions, and ~zero VRAM. The minimal subset that meets the
constraint is Phase 1 item 1 (occlusion reorder) + Phase 2 item 5
(LowPower, occlusion backoff, target release) + Phase 3 item 1 (sky cache,
for the visible-cost budget).

### Phase 1 — correctness & long-uptime robustness (output-identical)

1. **Reorder frame acquisition before scene build** (`main.rs:414-461`) so
   `Occluded`/`Timeout` skip the entire CPU build. Borrow note: keep
   sampling `epoch` and the drift azimuth before borrowing `graphics`
   mutably (the existing comment at `:415` explains the constraint); after
   the borrow, `Scene::build(&self.config, &self.lookup, …)` uses disjoint
   fields and borrow-checks. Derive `aspect` from the acquired frame's
   texture size, closing the one-frame aspect mismatch (§4.3).
2. **Azimuth in f64** with `rem_euclid(360.0)` and `as_secs_f64()`
   (`main.rs:425-431`).
3. **Surface hardening** (`main.rs:569-592`): `formats.first()` with
   context error instead of `[0]`; `log::warn!` on non-sRGB fallback;
   choose `alpha_mode` from `capabilities.alpha_modes`.
4. **Device-loss path**: install `on_uncaptured_error` logging (app +
   screenshot); device-lost callback sets an `Arc<AtomicBool>`; the event
   loop drops and recreates graphics on the next tick (callbacks may fire
   off-thread; winit work must stay on the loop thread).
5. **Refresh re-arm**: check `start_refresh_if_due()` daily from the
   existing `about_to_wait` tick (one `Instant` compare; `needs_refresh` in
   `horizons.rs:94` stays the staleness authority) and retry failures with
   1 h → 24 h exponential backoff, reset on success.
6. **Unique temp file**: pid-suffixed temp name in `save()`
   (`horizons.rs:85`); rename stays atomic.
7. **Reload fixes**: re-apply `vsync` on reload; only reset the drift clock
   when `[time]` actually changed (`Time: PartialEq`).
8. **Small fixes**: screenshot u64 widening (`screenshot.rs:81,123`); HR
   parsed as `u32` (`sky.rs:264`); Horizons parser errors on unparseable
   required fields (`almanac.rs:250`); `platform.rs:51` degrades with a
   warning instead of panicking.

*Verification:* existing suite green; new tests for backoff schedule (pure
function), temp-path uniqueness, parser rejection, HR rejection; fixed-date
screenshot byte-identical to the pre-phase baseline (llvmpipe here if
available, user hardware otherwise). *Outcome:* occluded frames cost ~0; the
process survives driver resets, format-less surfaces, and multi-monitor
races; accuracy no longer silently expires.

### Phase 2 — stop rebuilding static data (CPU, bandwidth; pixel-identical)

The architectural step. The static/dynamic split lives in **orrery-core**
(renderer-side hashing would stop the upload but not the ~470k particle
rebuilds/s):

1. **`SceneCache`** in `scene.rs`, with
   `Scene::build_cached(config, lookup, epoch, aspect, azimuth_override,
   &mut cache)` alongside the unchanged `Scene::build`. Belts become
   `Arc<[Belt]>` keyed on `(asteroid_belt, kuiper_belt, RadialScale)` —
   cache hit is an `Arc::clone`. Orbit points become `Arc<[Vec3]>`, rebuilt
   when the epoch moves > ~0.25 day (sub-pixel element drift), the key
   changes, or `invalidate()` is called — sited in `collect_refresh` where
   the almanac swaps in (`main.rs:364`). `body_fraction` (8 Kepler solves)
   stays per-frame. Generation counters (`belts_generation`,
   `orbits_generation`) ride on `Scene`; generation 0 — what plain
   `Scene::build` emits — means "always upload", so tests and the
   screenshot path need no special-casing.
2. **Delete `Scene.extent`** and its three computation sites — verified
   unread.
3. **`azimuth_override` replaces the per-frame `Config::clone`**
   (`main.rs:428`), which also makes the belt cache key stable.
4. **Renderer skips unchanged uploads**: `upload_belts` and the static part
   of `upload_orbits` compare generations. Orbit brightness moves to the
   vertex shader fed by a tiny per-ring uniform (`[vec4; 8]`: body_fraction,
   opacity, trail params — 128 B/frame; ranges drawn as
   `draw(range, i..i+1)` so `instance_index` selects the ring). Scratch
   `Vec`s get `with_capacity` or become reused members;
   `select_visible_figures` skips the identity rotation and repacks only
   when the visibility mask changes; the tonemap constants write moves to
   `Renderer::new`.
5. **Power**: `PowerPreference::LowPower` (`main.rs:550`; screenshot keeps
   HighPerformance). While frames come back `Occluded`, stretch the frame
   budget to 1 s — sub-second recovery is invisible for a wallpaper — and
   after ~5 s of continuous occlusion **drop the `Targets` struct
   entirely**, releasing the MSAA/HDR/depth/bloom allocations (~122 MB at
   1080p, ~486 MB at 4K, per output) back to the system; recreate on the
   first visible frame (`Targets::new` is milliseconds — `resize` already
   proves the rebuild path). VRAM is the one resource contention does not
   time-slice, so this is the item that protects fullscreen games (see the
   design-goal note above). Caveat to verify on hardware: under the Plasma
   nested compositor the surface may never report `Occluded` even when the
   desktop is fully covered — if so, the fallback is plumbing a visibility
   hint from the QML side (the wallpaper item's window state) through an
   environment the client already reads, and this item becomes
   load-bearing rather than opportunistic. Deliberately **not** adding
   quality knobs (MSAA/particles/segments), which would reverse the
   documented config prune.

*Verification:* new core tests — same-epoch `build_cached` twice →
`Arc::ptr_eq` + unchanged generations; cached vs uncached scenes identical;
+30 days bumps orbit generation; belt toggle bumps belt generation;
`invalidate()` bumps both. Renderer orbit-packing extracted as a pure
function and tested (vertex count `2n+2` per ring, alternating sides, closed
loop, disjoint ranges). Screenshot pixel-identical (≤1 ULP; brightness moved
to the VS reorders float ops). *Outcome:* steady-state uploads ~867 KB →
~3 KB/frame; heap churn ~1.2 MB → ~50 KB/frame; scene-build CPU down >90%;
dGPU no longer pinned on hybrid laptops.

### Phase 3 — GPU work (the visible phase)

1. **Cached sky** — screen-space cache, not a cubemap (matching screen
   angular resolution would need ~800 MB of cube faces; the screen-space
   cache costs one extra HDR target: +16.6 MB @1080p, +66 MB @4K, and is
   exact at every refresh). New single-sample `sky_refresh` pipeline renders
   the existing `sky.wgsl` fragment into the cache when due; the scene
   pass's first draw becomes a `sky_present` entry point that reprojects the
   view ray through the cached view-projection (`w = 0` drops translation;
   sky at infinity) and samples bilinearly. Refresh when: no cache, resize,
   sky-config/fov fingerprint change, or angular drift ≥ half a pixel — at
   default drift that is ~1 refresh per 7 frames; with a static camera the
   sky renders **once ever**. MSAA interaction is clean: the fullscreen
   triangle has no geometric edges, so all 4 samples already shade
   identically. Bloom is untouched (reads resolved HDR downstream).
   Extract the refresh predicate as a pure function for GPU-free testing.
   Expected: dominant fragment cost −85% amortized; between refreshes the
   procedural haze is a reprojected bilinear resample up to 0.5 px stale —
   needs a 2-minute eyeball on hardware; catalogue stars/figures stay
   per-frame analytic.
2. **Line-width options** — whichever of §6 A/B is chosen on hardware;
   plus the belt `cot(fov_y/2)` term via `post.z`.
3. **Binding cleanups** (invisible): drop `globals.sun`; repurpose `post`
   (`.x/.y` dead, `.z` gains the projection scale, `.w` stays orbit width)
   and delete the separate tonemap-settings buffer + binding; u16 indices
   for sphere/ring; remove `fullscreen_uv`; reconfigure on `Suboptimal`;
   handle `ScaleFactorChanged`.
4. **Declined, with rationale recorded**: sphere LOD (raster cost is
   trivial; never fragment-bound), compute-shader bloom (portability),
   half-float vertices, push constants, pipeline caching. **MSAA stays 4×**
   pending an explicit 2× A/B on a 4K panel (~100 MB saving) — a sign-off
   call, not a unilateral change.

### Phase 4 — standards, validation, tests, docs

Doc-comment reattachment (`scene.rs:442-489` split between `frame_camera`
and `vertical_offset`); `RadialScale` manual `Deserialize` through a
`deny_unknown_fields` raw struct with named errors ("`exponent` only applies
to `law = \"power\"`"), locked by a failing-typo test; `Config::validate`
additions (fps 1..=240, finite sky floats, finite bounded `days_per_second`,
bounded offsets, real days-in-month with leap years); EPSILON floors replaced
with named constants; dead pub items removed, `_segments` renamed, stale
allow dropped, `KUIPER_PARTICLES` named; accuracy numbers reconciled from a
one-shot `#[ignore]`d measurement test and propagated to all four sites;
bundled-almanac expiry disclosed; deep-sky claims corrected to match the
recorded decision in `data/SOURCES.md`; `VALID_FROM/TO` enforced as a
once-per-session warning (warn, never clamp — a wallpaper in 2051 should
keep drawing); `appearance()` name-coverage test; `parse_arguments`
refactored to take an iterator and tested; `rustfmt.toml`,
`[workspace.lints]`, CI (`fmt --check`, `clippy -D warnings`,
`test --workspace`); feature trims verified by build.

*Verification:* full workspace green under the new lints; screenshot still
byte-identical to Phase 3 output (nothing here touches rendering).

### What must be eyeballed on real hardware (consolidated)

1. §6 orbit/constellation A/B pair at native resolution — the only intended
   visual change in the whole plan.
2. Two minutes of default camera drift after the sky cache — no stepping or
   shimmer.
3. Optional: 2× vs 4× MSAA at 4K.
4. The adapter log line (`main.rs:556`) confirms an integrated GPU on hybrid
   machines; power before/after with `powertop`/`intel_gpu_top`.
5. One overnight run: daily refresh-check log lines appear, azimuth stays
   smooth past 24 h.
6. With a fullscreen game (or any fullscreen window) running: confirm the
   process actually goes quiet — CPU near zero, no GPU submissions, VRAM
   released (watch with `intel_gpu_top`/`nvidia-smi` and the occlusion log
   line). If the nested-compositor path never reports occlusion, the
   QML-side visibility fallback in Phase 2 item 5 must be implemented
   before the design goal is considered met.

## 8. Verification appendix — how this review was produced

Three independent exploration passes (application/CPU, renderer/GPU,
core/standards) read every Rust file and shader in the workspace and
cross-referenced the Plasma QML, installer, and data files. Claims that
drive the plan's architecture were then re-verified by direct read at
`64a5ed1` rather than trusted from a single pass:

- the NDC→pixel conversion in `orbit.wgsl:24-62` (including confirming the
  round trip is self-consistent, so only widths are affected);
- the `Scene::build` → `get_current_texture()` ordering in
  `main.rs:414-461`;
- `PowerPreference::HighPerformance` at `main.rs:550`;
- the doc-comment attachment at `scene.rs:442-491`;
- `build_belt`'s signature (no epoch input → belts provably static) and the
  full re-upload in `upload_belts` (`lib.rs:1168-1201`);
- `Scene.extent` having no readers and `globals.sun` appearing in no shader
  (workspace-wide grep).

Numbers quoted per frame (particle counts, buffer sizes, hash counts, VRAM)
are computed from the constants in the source (`scene.rs:50,60`,
`lib.rs:36,49,79`, shader loop structure), not measured on hardware; the
plan's verification steps include measuring the before/after on a real GPU.
