# Ray-traced sun shadows: prototype scope

Status: phases 0-6 (moving casters, soft shadows, foliage) built and tested in game (2026-10-25). Code in `rac-rs/src/viewer/rtshadow.rs`; off unless
`RAC_RT_SHADOWS=1` or the Ray-traced shadows plugin. Committed on the `plugins` branch.

## Prototype results (Kerwan, RTX 3080 Ti, Vulkan)

- Works: the device gets ray queries, the driver accepts the traced shader, and the level structure builds on level load
  (Kerwan: 1,389,112 triangles, about 5 ms, once per level).
- Proven active: with the traced result inverted through `RAC_WGSL_DIR`, lit and shadowed areas swapped on screen.
- Matches the shadow maps on the static world: same shadows under the gates, bridge and buildings, with no acne or striping.
- GPU load (nvidia-smi, same two spots): 39% / 39% traced vs 36% / 37% shadow maps. The cascades still render while
  tracing, so this is the cost of tracing on top of the maps, not instead of them.
- **Foliage (phase 6, done 2026-10-25):** see-through casters are traced by texel alpha, with the same cut-offs as the cascades
  (terrain below 0x60, ties below 0x40, shrubs below 0x30). Shrubs now cast too. Kerwan: 1,606,297 triangles, 634,740 of them
  see-through (492k ties, 141k shrubs, 1.4k terrain), 63 masks in 44 KB, 25 to 75 ms per level (the index readback).
  Vines cast strand shadows instead of solid blobs.
- **Cost of the alpha test** (FPS overlay, uncapped, spawn point; another game instance was using the GPU at the same time):
  no texel test 172 FPS (5.9 ms), terrain only 172, ties only 170, shrubs only 170, all three 166 FPS (6.1 ms).
- **Soft shadows (phase 5, done 2026-10-25):** a compute pass after the level pass rebuilds each pixel's world point from the
  depth and traces 4 rays over the sun's disc (tangent radius 0.012) into a shadow mask, with the camera it used. The next
  frame's receivers project their world point into that mask and average the 3x3 pixels whose depth matches. The casters
  are static, so this is exact under camera motion. Where fewer than 3 match (off screen, newly revealed, a see-through layer
  in front) they trace one ray of their own. Vine strands are now soft shadows with no speckle, and turning the camera showed
  no holes or stale patches.
- **Cost** (FPS overlay, uncapped): spawn 147 FPS (6.9 ms) with the mask, vs 166 (6.1 ms) one ray per fragment and 103 (9.9 ms)
  four rays per fragment; terrace 155 FPS (6.5 ms) with the mask, vs 85 (12.1 ms) four rays per fragment.
- Dev knob: `RAC_RT_TUNE=<file>` with `cut=tfrag,tie,shrub solid=1.0` picks which casters get the texel test; the game re-reads
  the file while running and rebuilds the structure.
- **Moving casters (phase 4, done 2026-10-25, `rtmovers.rs`):** every frame, the shown props and scene actors within 80 units
  of the eye are posed on the CPU (gpuskin.rs `emulate_pos`, the props shader's own maths) and go into a bottom-level structure
  of their own with the player, the clipped props and the debris, in the top level's slot 1. Kerwan spawn: 35 to 39 slots
  (about 18k triangles) plus 57k other triangles, 0.35 ms CPU per frame; 6.3 ms frame without, 6.9 ms with. Checked with
  `world=0` in RAC_RT_TUNE (only the movers cast): Ratchet casts his shape on the grass and shades his own back. The shadow mask
  is last frame's, so a moving shadow lags one frame. Props past 80 units cast nothing.
- **Fix (2026-10-25): moving receivers do not shadow themselves.** The player turned dull brown all over in the sun: the rays
  from his own surface hit his own shape in the movers' structure, which took away the moby lighting's wrap light on his far
  side. The top level now tags the static world 0x01 and the movers 0x02; `sun_lit` (lit mobies, the player) takes
  `sun_occ_moby`, one ray through 0x01 only. Ratchet still casts on the ground; mobies no longer receive other movers'
  shadows. Checked on Veldin (scripted New Game) with `world=0`.
- Dev keys in RAC_RT_TUNE: `movers=0` (no moving casters), `world=0` (only the moving casters).
- Testing note: `cargo test -p rac-rs --lib` builds a 2.18 GB debug test exe that Windows refuses to run (over 2 GB). That
  predates this work. The traced shader was checked with naga in a separate scratch crate instead.

## How to try it

Run `Play.cmd` with `RAC_RT_SHADOWS=1` set first (Lighting Enhanced and Dynamic shadows must be on). The run log says
`rt shadows: on` and, per level, `rt shadows: level structure, N triangles`. The original exe is kept as
`build\release\rac-rs.orig.exe`.

## Findings

- **Nobody upstream is doing this.** github.com/AsobiCodes/rac-rs: no issues, PRs or changelog entries mention ray tracing
  (the "RTX" hits are GPU names in bug reports). The 4 forks are platform ports (Linux, aarch64, Android).
- **wgpu 30.0.1 has it, Vulkan only.** `Features::EXPERIMENTAL_RAY_QUERY` (plus the unsafe `ExperimentalFeatures` opt-in token on
  the device descriptor), BLAS/TLAS via `CommandEncoder::build_acceleration_structures`, buffers need `BufferUsages::BLAS_INPUT`.
  WGSL: `enable wgpu_ray_query;`, `rayQueryInitialize/Proceed`, `RAY_FLAG_TERMINATE_ON_FIRST_HIT`, candidate intersections
  (for alpha-tested leaves). The game already defaults to Vulkan (`player/log.txt`: Vulkan, RTX 3080 Ti).
- **The integration point is small.** All shadow receivers go through one WGSL function, `sun_occ(p)` in `rac-rs/src/viewer.rs`
  (~line 427), which today samples the 3 shadow-map cascades. A ray-traced version replaces that one function; `sun_baked`,
  `sun_lit`, the key-light direction, strength and the Graphics option logic in `viewer/sunshadow.rs` stay as they are.
- **Geometry is RT-friendly.** `Vertex` is 64 bytes with `pos: [f32; 3]` at offset 0; index buffers are `Uint32`. The static
  batches (`world`, `tfrags`, `ties`, `shrubs`) are already in world space. The caster list in `SunShadow::encode` tells us
  exactly what should block light.

## Hard parts

1. **LODs share one buffer.** `tfrags`/`ties` hold every LOD mesh in one index buffer (`tfrag_ranges[i][lod]`). The BLAS must
   hold one LOD only (near), or surfaces shadow themselves. The drawn far LOD will then sit slightly off the traced mesh, so the
   ray needs an origin offset along the normal. The shadow-map code had the same problem (see `encode` comments).
2. **Mobies are posed in the vertex shader** (`vs_sh_props`: `poses` + `insts` storage buffers). A BLAS needs real posed
   positions, so a compute pass must port that posing into a buffer, then the dynamic BLAS is rebuilt every frame
   (`PREFER_FAST_BUILD`). Debris, clip and marker batches are CPU-written each frame and rebuild the same way.
3. **Alpha-cut casters.** Shrub leaves (and some terrain/tie overlays) cast by texel alpha. With ray queries that means non-opaque
   geometry + a candidate loop that fetches the UV (vertex/index buffers as storage) and samples the bindless texture array.
4. **Shader variant.** `SHADER` is shared by every device; `enable wgpu_ray_query;` and a TLAS binding would fail without RT.
   Build a second module (same source, RT `sun_occ` swapped in) and pick the pipeline at startup.
5. **Experimental API.** Possible wgpu bugs or validation gaps; DX12 users and non-RT GPUs keep the shadow maps.

## Phases

| # | Step | Result |
|---|------|--------|
| 0 | `git init` + commit the untouched source | A clean baseline to diff against |
| 1 | Request `EXPERIMENTAL_RAY_QUERY` when the adapter has it (Vulkan); switch on with env `RAC_RT_SHADOWS=1` | Nothing visible; the device logs RT support |
| 2 | Static BLAS per level: world + near-LOD tfrags + LOD-0 ties, opaque; TLAS with identity instances | Acceleration structure built on level load |
| 3 | RT shader variant: `sun_occ` traces one ray from `p + n*eps` toward `u.sun.xyz`, max distance `REACH` (200), first hit ends it | **First visible prototype:** sharp, pixel-exact world shadows |
| 4 | Dynamic BLAS: compute-posed props + hero, debris, clip, marker; rebuilt each frame | Ratchet and enemies cast shadows |
| 5 | Soft shadows: jitter rays inside the sun's disc (1-4 rays per pixel), lean on the existing frame smoothing | Contact-hardening soft edges |
| 6 | Alpha cut: shrubs (and tfrag/tie overlays) as non-opaque with a texel test | Leaf shadows with holes |
| 7 | Graphics menu entry (Dynamic shadows: Ray traced), `pcext.rs` / `pcopts.rs` | Normal players can turn it on |

Phases 1-3 are the prototype; 4-6 make it complete; 7 is polish for an upstream PR.

## Files touched

- `rac-rs/src/viewer/gpu.rs`: device features, BLAS input usage on the static batches, RT pipeline choice.
- `rac-rs/src/viewer/sunshadow.rs`: BLAS/TLAS ownership and per-frame build next to `encode` (or a new `viewer/rtshadow.rs`).
- `rac-rs/src/viewer.rs`: RT `sun_occ` variant, uniforms bind group gains the TLAS (binding 3).
- Later: `retail/pcext.rs`, `retail/pcopts.rs` for the menu.

## How to check it

- A/B screenshots at the same spots with shadow maps vs ray traced (the game has `--shot`).
- GPU time from the existing `profile::span` timings ("sun_shadow" vs the new pass).
- The crate's tests already validate WGSL with naga; add the RT variant to that test.
