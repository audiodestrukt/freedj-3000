# Jog wheel: analytic truncated-cone render with lighting

Status: **planned** (not implemented). See GitHub issue for tracking.

## Goal

Make the on-screen jog wheel read as a real, shiny, *rotating* 3D platter — a
lit metal cone whose specular highlight sweeps as the deck plays — instead of
the current flat photo crop. Do it inside the renderer we already have, with no
measurable frame-rate cost.

## Why a cone, not the STL mesh

The deck "screen" is **not** a 3D scene. It is a single fullscreen-triangle
**fragment shader** (`crates/app/src/renderer.rs`, `fs_main`, issued as
`pass.draw(0..3, 0..1)`) that procedurally paints the waveform region, plus an
egui overlay on top. There is **no vertex pipeline, no depth buffer, no camera
transform**.

The jog today is not geometry either: `screen.rs` (`draw_faceplate`, jog
section, ~line 286) lifts a **circular crop of the faceplate photo** as a
triangle-fan disc, and ~line 339 spins a single **marker dot** over it. Only
the dot moves; the platter is a static image.

Loading `hardware/cad/build/jog_wheel.stl` would therefore mean bolting a whole
mesh rasterizer (vertex buffers + depth + MVP transforms + a new pipeline) onto
a renderer that has none of that — a large amount of new machinery for one
widget.

A **truncated cone viewed near-on** (which the platter essentially is) is
**analytic per pixel**, so it needs none of that:

- For a pixel at radius `r` and angle `θ` from the jog centre, the cone's
  surface normal tilts outward by a constant half-angle. No mesh, no
  ray-marching, no intersection test.
- Lighting is then **one Lambert term + one Blinn-Phong specular** — a couple of
  dot products. The specular highlight is what sells "shiny metal platter", and
  **sweeping it with the rotation phase is the animation**.
- The profile is the **same `(r, z)` we already revolve for the STL**
  (`OUTER_R → REC_R → HUB`, in `hardware/cad/jog_wheel.py`). One set of
  constants drives both the printed part and the on-screen render — they can't
  drift.

## Frame-rate feasibility

The jog covers roughly a 150 px disc on the deck screen (~18k pixels), a dozen
ALU ops each. That is noise next to the waveform shader, which already runs
**fullscreen** every frame. Per our own profiling the render path was never the
Pi bottleneck — R3 timestretch is (see memory `pi4-render-bound`). Expected
frame-budget impact: none measurable.

## Shading model

Analytic cone, evaluated in shader space for pixels inside the jog rect:

1. **Base surface.** From `r` (distance to centre) select the profile band —
   outer platter slant, dished centre recess, hub — matching the STL `(r,z)`
   profile. Each band yields a surface normal `N`.
2. **Tilt cheat.** Squash the disc vertically into a slight ellipse to fake the
   faceplate's real viewing angle. This reads as 3D without any camera matrix.
3. **Diffuse.** `max(dot(N, L), 0)` with a fixed key light `L` (upper-left).
4. **Specular.** Blinn-Phong `pow(max(dot(N, H), 0), s)` — the glint. `H` is the
   half-vector for the fixed light + view.
5. **Rotation.** Perturb the normal with the grip ridges,
   `cos(N_grip · θ + phase)`, so the ridged grip edge glints travel around as it
   spins. `phase` is the jog rotation we already track (drives today's marker
   dot).
6. **Environment.** A cheap 2-tone gradient along the normal stands in for a
   reflection (no cubemap).
7. **Centre indicator.** Leaves room to composite the #34 spinning centre
   graphic in the hub band.

## Where it lives — the one real decision

egui (where the jog is painted now) does 2D vector fills; it **cannot** do
per-pixel lighting. So the lit cone must be GPU. Two options:

- **(A) Extend the existing fullscreen shader (recommended).** Add
  `jog_center`, `jog_radius`, `jog_phase`, `tilt` to `WaveformParams`; add an
  `if inside_jog_rect { return shade_cone(...); }` branch at the top of
  `fs_main` before the waveform code. One pass, one shader, ~30 lines of WGSL.
  All the uniform plumbing already exists. Least new machinery; stays in the
  pass we already present.
- **(B) A separate tiny pipeline** drawing just a jog quad. Cleaner isolation,
  but a second pipeline + bind group + draw call of boilerplate.

**Recommendation: (A).**

## Plan of work

1. Add jog uniforms to `WaveformParams` (renderer.rs) and populate them from the
   snapshot's pitch/rotation phase.
2. Port the STL `(r, z)` profile constants into the shader (or a shared
   constants source) so screen and print agree.
3. Write `shade_cone()` in the WGSL: band select → normal → diffuse + specular +
   grip glint + env gradient.
4. Branch `fs_main` to call it for pixels in the jog rect; leave the photo crop
   in place behind a flag for A/B comparison.
5. Tune the light direction, specular exponent, and tilt against
   `reference/photos/XDJ1000Mk2-faceplate.jpg`.
6. Confirm no frame-budget regression on Pi (`make perf` / on-device check).

## Relationship to other work

- **#34** (spinning centre indicator) composites into the hub band of this cone;
  build the cone first, then drop the indicator into its centre.
- **#35** (touch/push feedback) is separate button-state UI, not the platter
  render — keep it off the screen surface per `screen-fidelity-separate-button-ui`.
- The photo crop (`screen.rs` jog section) stays as a fallback until the cone is
  tuned.

## Files

- `crates/app/src/renderer.rs` — `WaveformParams`, `fs_main`, pipeline.
- `crates/app/src/screen.rs` — current jog photo crop + marker (fallback / rect).
- `hardware/cad/jog_wheel.py` — source of the `(r, z)` profile constants.
- `reference/photos/XDJ1000Mk2-faceplate.jpg` — tuning reference.
