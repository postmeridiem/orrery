# Target composition

Locked 2026-08-04 by Jeroen. The reference render is
[`target-composition.png`](target-composition.png), 3440 × 1440, produced by
`orrery --config config/orrery.toml --screenshot docs/target-composition.png
--size 3440x1440` — that is, from the shipped defaults with nothing overridden.

Any future change to the camera or framing code must still produce these
numbers. They are the oracle: the previous attempt at this failed by optimising
invented metrics with nothing to check them against.

Arrived at in two decisions:

1. **Angle and size** — 16° at `zoom = 0.578`, picked from a rung-for-rung
   comparison against 22° at matched apparent size. 16° leaves roughly 25 %
   more room below the outer orbit at every tightness, widening to 55 % at the
   tightest.
2. **Vertical placement** — Sun 23 % from the top, picked from bottom-edge crops
   at native resolution. This is the value at which the outermost planet stops
   being clipped.

## The configuration that produced it

```toml
[camera]
elevation_deg = 16.0     # honoured exactly, never adjusted
azimuth_deg   = 0.0
roll_deg      = 0.0
fov_deg       = 55.0
zoom          = 0.578
offset_x      = 0.0
offset_y      = 0.27     # lens shift: Sun 23% from the top
fill          = 0.94
fit_width     = true
```

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
| Clearance under the lowest planet | 16.0 px at 1440p | ± 4 px |
| Highest drawn thing (Kuiper belt far edge) | 0.833 | ± 0.02 |
| Heliocentric radius landing on the left/right edge | 35.33 AU | ± 0.5 AU |
| Camera distance from target | 6.2334 scene units | ± 0.01 |
| Camera eye | (5.9919, 1.7181, 0.0) | ± 0.01 per axis |

"Visible half-width" counts only orbit points with `|ndc.y| <= 1`.

## Why the planet clearance is a criterion and not a detail

At the locked framing the outermost orbit is **fully contained** — near arc
0.902, comfortably inside the frame. An earlier version of this document said
containment was unreachable. That was measured with the picture centred, and
the lens shift removed the premise; the claim is withdrawn.

What is *not* automatically contained is the planet sitting on that arc.
Neptune is drawn 56 px across at 1440p, and at the moment it sits almost exactly
at the lowest point of its own orbit — so the orbit line can have clearance
while the planet on it is sliced flat by the frame edge. That slice is what
reads as a deliberate crop.

Because Neptune is currently at that low point, this is the **worst case**.
Clearing it here means no planet clips at the bottom at any date, and the
framing stops depending on where anything happens to be.

For reference, measured at 1440p:

| Sun from top | `offset_y` | lowest planet vs bottom edge |
|---|---|---|
| 25.0 % | 0.250 | −12.8 px, clipped |
| 24.11 % | 0.2589 | exactly tangent |
| 24.0 % | 0.260 | +1.6 px |
| **23.0 %** | **0.270** | **+16.0 px** |
| 22.0 % | 0.280 | +30.4 px |

The orbit's visible half-width is 0.851 at every row: `offset_y` moves the
picture and nothing else.

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
- Do not add a knob that interacts with `zoom`. The build brief replaces
  `zoom` / `fill` / `fit_width` with one explicit parameter — the heliocentric
  radius in AU that lands at the frame edge, currently **35.33 AU** — solved in
  closed form rather than searched.
