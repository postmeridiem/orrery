# Target composition

Locked 2026-08-04 by Jeroen. The reference render is
[`target-composition.png`](target-composition.png), 3440 × 1440, produced by
`orrery --config config/orrery.toml --screenshot docs/target-composition.png
--size 3440x1440` — that is, from the shipped defaults with nothing overridden.

Any future change to the camera or framing code must still produce these
numbers. They are the oracle: the previous attempt at this failed by optimising
invented metrics with nothing to check them against.

Arrived at in two decisions:

1. **Angle and size** — 16° at what was then `zoom = 0.578`, picked from a
   rung-for-rung comparison against 22° at matched apparent size. 16° leaves
   roughly 25 % more room below the outer orbit at every tightness, widening to
   55 % at the tightest. The same framing is now `frame_radius_au = 35.33`.
2. **Vertical placement** — Sun 23 % from the top, picked from bottom-edge crops
   at native resolution. It was recorded as the value at which the outermost
   planet stops being clipped; that was wrong, and why is below. Neptune grazes
   the edge at 23 % and the graze was accepted on sight.

## The configuration that produced it

```toml
[camera]
elevation_deg   = 16.0    # honoured exactly, never adjusted
azimuth_deg     = 0.0
roll_deg        = 0.0
fov_deg         = 55.0
frame_radius_au = 35.33   # this radius lands on the left and right edges
offset_x        = 0.0
offset_y        = 0.27    # lens shift: Sun 23% from the nearer edge
```

`frame_radius_au` replaced `zoom`, `fill` and `fit_width` on 2026-08-04. The
picture is unchanged — the render before and after is 97.3 % bit-identical, the
remainder being stars landing a fraction of a pixel differently after a
0.0003-unit shift in camera distance.

Epoch for reproducible measurement: JD 2461255.5. Aspect 3440 : 1440.
`rotation_period_minutes` is set to 0.0 when measuring, so the azimuth stays put.

## Acceptance criteria

Measured in normalised device coordinates, where `1.0` is the frame edge on the
axis in question. **Orbits and bodies only — belt particles are never
measured.** Counting belts is what let `framing_actually_fills_the_frame` stay
green while the picture was broken.

| Quantity | Value | Tolerance |
|---|---|---|
| Sun, from the top of the frame | 23.00 % | ± 0.3 % |
| Visible half-width of the outermost orbit | 0.851 | ± 0.02 |
| Far edge of the outermost orbit, above centre | 0.790 | ± 0.02 |
| Near arc of the outermost orbit, below centre | 0.902 | ± 0.02 |
| Lowest planet against the bottom edge | −16 px at 1440p | ± 8 px |
| Highest drawn thing (Kuiper belt far edge) | 0.833 | ± 0.02 |
| Heliocentric radius landing on the left/right edge | 35.33 AU | ± 0.5 AU |
| Camera distance from target | 6.2334 scene units | ± 0.01 |
| Camera eye | (5.9919, 1.7181, 0.0) | ± 0.01 per axis |

"Visible half-width" counts only orbit points with `|ndc.y| <= 1`.

## The whole rotation, not just azimuth 0

The camera circles the Sun once an hour, so the numbers above have to hold at
every azimuth, not only the one they were measured at. Swept in 1° steps:

| Quantity | Across all 360° |
|---|---|
| Camera distance | 6.2334 exactly, unchanging |
| Near arc of the outermost orbit | −0.9239 … −0.7255, never off frame |
| Visible half-width of the outermost orbit | 0.839 … 0.869 |
| Lowest planet against the bottom edge | −19.9 px at worst, at azimuth 357 |

Turning the system is a rigid rotation: a near-circular orbit projects to the
same ellipse whatever the azimuth, and only the planets travel along it.

It did not behave that way at first. The distance was re-solved every frame,
against a quantity that diverges as the camera closes in, so it settled
somewhere different at every azimuth — **the camera crept 18 % in and out over
one rotation** (6.23 down to 5.12), the ecliptic was seen from a changing
height, and whole stretches of the outer orbit swung off the bottom of the frame
and back. `one_rotation_does_not_re_frame_the_scene` exists to stop that
returning.

Neptune's eccentricity is 0.0086. Nothing about the geometry justified that
movement, and no amount of tuning `offset_y` would have fixed it.

## Why the planet clearance is a criterion and not a detail

At the locked framing the outermost orbit is **fully contained** — near arc
0.902, comfortably inside the frame. An earlier version of this document said
containment was unreachable. That was measured with the picture centred, and
the lens shift removed the premise; the claim is withdrawn.

What is *not* contained is the planet sitting on that arc. Neptune is at the
lowest point of its own orbit at this epoch, so the orbit line can have
clearance while the planet riding on it is sliced by the frame edge.

**It is sliced, by about 16 px, and this document previously said the
opposite.** Corrected 2026-08-04.

The error was in how a planet's drawn size was measured. Every clearance figure
here came from `radius / (distance · tan(fov/2))` — the projected size of a
sphere *on the optical axis*. Neptune is not on the axis. It sits 37° off it,
near the bottom of the frame, and perspective stretches an off-axis sphere into
an ellipse elongated away from the centre. Its disc reaches **88 px** below its
own centre at 1440p, not the 56 px the approximation gives. Every row of the
table below was therefore 32 px optimistic, and so was the azimuth sweep.

The measurements now sample each sphere's surface and project the real
silhouette. Jeroen looked at the resulting graze in a native-resolution crop on
2026-08-04 and accepted it: the orbit line is what carries the composition, and
it stays comfortably on frame.

Measured at 1440p. The lens shift is a rigid translation in normalised device
coordinates — it moves the image and changes nothing else — so this is exactly
linear, and the orbit's visible half-width is 0.851 at every row:

| Sun from top | `offset_y` | lowest planet vs bottom edge |
|---|---|---|
| 25.0 % | 0.250 | −44.8 px |
| 24.0 % | 0.260 | −30.4 px |
| **23.0 %** | **0.270** | **−16.0 px — shipped, accepted** |
| 22.0 % | 0.280 | −1.6 px |
| 21.9 % | 0.2811 | exactly tangent |
| 21.0 % | 0.290 | +12.8 px |

### How far over it ever gets

Swept 2026-08-04 over 170 years — one full Neptune orbit and a little — against
a full turn of the camera, by `no_planet_falls_further_off_frame_over_a_whole_neptune_orbit`.

| | Lowest planet vs the bottom edge, at 1440p |
|---|---|
| Now, azimuth 0 | −16.0 px |
| Now, worst azimuth (357) | −19.9 px |
| **Worst over 170 years and every azimuth** | **−35.5 px**, Neptune, +17.4 years, azimuth 319 |

Neptune's disc is 176 px tall at 1440p, so the worst case cuts 20 % of it
against 9 % today. It is not tangency and it is not catastrophe — it is roughly
twice the graze that was signed off.

An earlier version of this document said a planet reaching the deepest point of
the outer orbit would sit "tangent to the edge — margin roughly zero". It does
reach that point, and it is not tangent: it hangs a fifth of its diameter over.
That estimate used the same on-axis approximation as everything else here.

The propagation is trusted this far out for this purpose specifically. The
built-in tables are stated valid to 2050 and the sweep runs to 2196, but over
those 170 years Neptune's elements move from a = 30.0700, e = 0.00860,
i = 1.7701 to a = 30.0704, e = 0.00869, i = 1.7707. The *shape and size* of the
orbit — which is all the framing depends on — is unchanged to five figures. What
degrades is where along the orbit the planet is on a given date, and the sweep
visits every point along it regardless.

## The golden screen shapes

Locked 2026-08-04 from renders. The shipped configuration was rendered at seven
common screen shapes and each was looked at; `the_golden_screen_shapes` asserts
the result. It replaced a test that demanded the whole system fit at every
aspect ratio and elevation — a promise the closed-form camera does not make,
because one parameter fixes the left and right edges and the vertical follows.

| Screen | | Lowest planet vs bottom edge |
|---|---|---|
| 32:9 | 5120 × 1440 | −838 px — Neptune off frame |
| 21:9 | 3440 × 1440 | −16 px — the reference |
| 16:9 | 1920 × 1080 | +282 px |
| 16:10 | 1920 × 1200 | +400 px |
| 3:2 | 2256 × 1504 | +560 px |
| 4:3 | 1600 × 1200 | +521 px |
| 9:16 | 1080 × 1920 | +237 px |

Nothing reaches the left or right edge on any shape.

**`offset_y` is measured from the nearer edge** — the top on a landscape screen,
the bottom on a portrait one. On a tall frame the system is small and pinning
the Sun near the top leaves the lower half empty, so the orrery hangs like a
chandelier instead of sitting like a foundation.

The flip at aspect 1 is not a chosen threshold; it is where the geometry changes
sign. Bottom-aligned, the lowest planet lands 127 px over the edge at 4:3 and
78 px over at 5:4, clears by 6 px at exactly 1:1, and only improves below that.
Square is the last shape with no room to spare.

## Do not do this

- Do not adjust `elevation_deg` to make anything fit. It is used verbatim. An
  earlier version silently lowered it, which is why "restore 27°" could not
  restore 27°.
- Do not measure framing against belt particles.
- Do not make `offset_y` move the camera again. It is a lens shift, applied
  after the framing solve. Moving the camera instead changes the angle the
  ecliptic is seen at, and at 16° that made the outer orbit diverge violently:
  at the offset that would put the Sun at 2/5, the orbit width went from 0.851
  to 2.52 and the near arc to 698.
- Do not apply the shift *during* the solve. The solver would pull back to
  compensate, so asking to move the picture would silently also resize it.
- Do not measure a planet's drawn size as `radius / distance`. That is the
  on-axis approximation, and the planets that matter for framing are precisely
  the ones far off axis. Sample the sphere's surface and project it.
- Do not reintroduce a second quantity that interacts with `frame_radius_au`.
  It replaced `zoom` / `fill` / `fit_width` — three knobs for one thing, where
  `fill` was measured before `zoom` was applied, so cropping in changed what the
  solver thought it was fitting.
