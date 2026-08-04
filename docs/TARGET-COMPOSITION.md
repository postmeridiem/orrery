# Target composition

Locked 2026-08-04 by Jeroen, from a rung-for-rung comparison of 16° against 22°
at matched apparent size. The reference render is
[`target-composition.png`](target-composition.png), 3440 × 1440.

Any future change to the camera or framing code must still produce these
numbers. They are the oracle: yesterday's failure was optimising invented
metrics with nothing to check them against.

## The configuration that produced it

```toml
[camera]
elevation_deg = 16.0     # honoured exactly, never adjusted
azimuth_deg   = 0.0
roll_deg      = 0.0
fov_deg       = 55.0
zoom          = 0.578
offset_x      = 0.0
offset_y      = 0.0
fill          = 0.94
fit_width     = true
rotation_period_minutes = 0.0   # held still for measurement only
```

Epoch for reproducible measurement: JD 2461255.5. Aspect 3440 : 1440.

## Acceptance criteria

Measured in normalised device coordinates, where `1.0` is the frame edge on the
axis in question. **Orbits only — belt particles are never measured.** Counting
belts is what let `framing_actually_fills_the_frame` stay green while the
picture was broken.

| Quantity | Value | Tolerance |
|---|---|---|
| Visible half-width of the outermost orbit | 0.851 | ± 0.02 |
| Far edge of the outermost orbit, above centre | 0.250 | ± 0.02 |
| Near arc of the outermost orbit, below the bottom edge | 1.442 | ± 0.05 |
| Heliocentric radius landing on the left/right edge | 35.33 AU | ± 0.5 AU |
| Camera distance from target | 6.2334 scene units | ± 0.01 |
| Camera eye | (5.9919, 1.7181, 0.0) | ± 0.01 per axis |

"Visible half-width" counts only orbit points with `|ndc.y| <= 1` — points
already off the top or bottom of the frame are excluded. This is deliberate.

## Why the near arc overflows, and why that is correct

The outermost orbit **cannot be fully contained** at any framing that leaves the
system reading large on a 21:9 screen. As the camera closes in, the near arc of
the outer ellipse diverges non-linearly:

| visible half-width | zoom | near arc reaches |
|---|---|---|
| 0.55 | 0.752 | 0.71 — inside the frame |
| **0.85** | **0.578** | **1.44 — the locked target** |
| 1.22 | 0.497 | 2.76 |
| 1.52 | 0.450 | 5.82 |

Only the loosest rung contains everything, and it leaves the system small. So
the criterion is the **visible silhouette width**, not vertical containment.
The near arc leaving the bottom of the frame is the intended look — it is what
the reference crop Jeroen supplied on 2026-08-03 was doing.

At 22° the same overflow is roughly 25 % worse at every rung, and 55 % worse at
the tightest, which is why 16° won.

## Do not do this

- Do not adjust `elevation_deg` to make anything fit. It is used verbatim. An
  earlier version silently lowered it, which is why "restore 27°" could not
  restore 27°.
- Do not measure framing against belt particles.
- Do not add a second knob that interacts with `zoom`. The build brief replaces
  `zoom` / `fill` / `fit_width` with one explicit parameter — the heliocentric
  radius in AU that lands at the frame edge, currently **35.33 AU** — solved in
  closed form rather than searched.
