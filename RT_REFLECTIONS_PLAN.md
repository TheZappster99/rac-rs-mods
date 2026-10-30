# Ray-traced reflections: plan

Status (2026-10-30): phases 1-6 done, phase 7 half done (mirror off and 2 rays at Ultra; textured movers and glass next). Phase 4 checked on Pokitaru (RAC_RT_REFL_FORCE=1): the shopkeeper and the island show in
the water with the picture's own lighting; `water shade` 0.50 ms with every water pixel traced and a lot of water on screen.
Without the force switch Performance traces little (the planar mirror covers flat water on most levels). Dev switches:
RAC_RT_REFL_SHOW=1 (tint: yellow from the picture, red shaded, blue nothing hit), RAC_RT_REFL_FORCE=1 (trace everywhere),
RAC_RT_REFLECTIONS=2 / 3 (start at Balanced / Ultra without the launcher). In game, Insert steps Reflections Off -> Performance ->
Balanced -> Ultra for the session (needs Reflections on at start-up; the traced pipelines are rebuilt on the next frame, a short
hitch; the run log says which, `rtrefl::cycle`).
Phase 5 (Balanced) checked in game on Pokitaru, Gaspar lava and Orxon sludge: `water shade` 0.31 ms median, 0.67 ms at worst
(budget 1.5 ms); with RAC_RT_REFL_FORCE=1 (every water pixel traced, mirror off) 0.65 ms at worst on Pokitaru. A straight
cut-off at the end of a Gaspar lava pool by a pillar shows at Performance too (no traced lava there): not the reflections.
Phase 6 (chrome and glass) built (rtrefl.rs `Metal`): the game starts at Balanced with the traced metal pipelines (log: "chrome
and glass traced"); looked at in game at Ultra and switched live with Insert (looks good), not yet timed. It has no pass of its own to time (the metal draws inside the level
pass), so its cost is the level pass's time with and without it. The wrench head (a request, not retail: a plain textured moby,
class 71) is built too: the hero batch's draws with the wrench's own textures (`Gpu::set_wrench_gloss`) use the metal module with
RT_GLOSS = 1 (`rt_wrench`): bright grey texels take 75% of the chrome's share of their mirror hit. Looked at in game (looks good; Clank shines too, likely a texture he shares with the wrench). Later: a sun glint on the metal where the mirror ray misses and points at the sun.
Phase 6 timed (2026-10-30, Novalis, RAC_GPU_PASSES=120, the same view switched with Insert): the level pass 1.84 ms at
Performance (metal not traced) against 1.93 ms at Balanced, so the traced metal costs about 0.1 ms there.
Phase 7 (Ultra) part 1 built and timed (2026-10-30): at Ultra the mirrored planes trace too (`rtrefl::mirrors_traced`, read in
gpu/draw/passes.rs: no plane is picked, so the planar mirror's pass over the level is not drawn, once the traced water can run),
and rippled water averages a second ray halfway to the full ripple normal's mirror (`rt_water`, RT_ULTRA). Same Novalis view:
Performance sum 5.12 ms (water shade 0.19, water mirror 0.46), Balanced 5.14 (shade 0.23, mirror 0.43), Ultra 4.82 (shade
0.33, no mirror): **Ultra is about 0.3 ms cheaper than Balanced**, as the mirror pass costs more than tracing the planes.
Ultra's water shade at worst 0.59 ms on Novalis. Still to do for phase 7: movers textured off screen (needs the movers' vertex
and index buffers in the water's group 2, which is near the 16 storage buffers the device asks for: pack them first), and the
second ray on glass (the metal module cannot tell glass from chrome yet).

The **Reflections** setting of the **Ray tracing** built-in mod (`ray-tracing`), next to its Shadows and Sun rays settings
(2026-10-28: the three ray-traced mods became one, each a setting). It uses the same level structure (`rtshadow.rs`), so it needs Vulkan and a graphics
card with ray tracing, and it brings that structure up on its own when Shadows and Sun rays are off.

**Main rule: it must not tank the frame rate.** The default quality is cheap, and the costly modes are opt-in. Every phase
is measured against a budget (below) before it counts as done.

## What reflects today (no ray tracing)

- **Water** (`waterfx.rs`, `fs_water` in `waterfx/shader.rs`): for flat water planes, a planar mirror (`mirror.rs`, the level
  drawn a second time, mirrored, at half size). Everywhere else, screen-space reflections (`reflect_ray`: a 32-step march against
  the depth). Where the march finds nothing (the thing is off screen, behind something, or past the edge), `env()` guesses a mix of
  the water's own colour and the fog colour. Lava, sludge and mud use the same `reflect_ray` for their gloss.
- **Shiny surfaces**: the retail chrome / glass packets of metal mobies (`gpu/metal.rs`, `mobies::metal_env`: bolts, ships,
  glass) sample a fixed chrome or glass environment texture with the PS2's env matrix. It is a fake reflection that never
  shows the real level.

The visible flaws that ray tracing fixes: reflections that vanish or smear at screen edges, water that reflects fog when you
look
down at it, reflections of things behind the camera or hidden behind a pillar, and chrome that never shows the place around it.

## What we already have to build on

- The static world's structure (terrain, LOD-0 ties, near tfrags, shrubs; up to `MAX_GEOS` = 8 geometries, about 1.6M triangles
  on Kerwan), with see-through leaves tested by texel alpha (`rt_texel_casts`), and the moving things in slot 1 (`rtmovers.rs`).
- Ray queries in compute and fragment shaders (`rt_ray_masked`), the shadow mask pass that rebuilds world points from depth
  (`cs_mask`), and the bindless texture array (`bindless.rs`, `TEX_CAP` 4096 slots).
- `waterfx/skymap.rs` (where the sun reaches the water), the fog colour and the sky.

## The hard part: what colour does a ray see?

A shadow ray only asks "blocked or not". A reflection ray needs the **colour** of whatever it hits. There are two sources,
cheapest first:

1. **Last frame's picture (re-projected).** If the hit point was on screen last frame, at the same depth, take its colour from
   last frame's finished image. This is exact (it has all the lighting, fog, effects) and costs almost nothing. It covers most
   hits, because most reflected things are on screen. It is the same idea as `rt_mask_at`, which re-projects the shadow mask.
2. **Hit shading** for everything else (off screen, behind something): look up the triangle that was hit, put together its UV and
   vertex colour from the barycentrics, and sample its texture from the bindless array at a coarse mip level. Then light it like
   the level shader lights the world: texel × vertex colour (the PS2's baked light), times the sun (a shadow ray at the hit, on
   higher qualities only), plus the fog. This needs new per-triangle data:
   - the batches' vertex buffers bound as storage (add `STORAGE` through `rtshadow::usage`, like `BLAS_INPUT` today),
   - the compact index buffer the structure is built from, kept and bound as storage,
   - a per-triangle texture slot (one u32 each, about 6.4 MB on Kerwan; or a draw-range table if memory turns out tight),
   - for movers, the CPU-posed positions already in `rtmovers.rs` and the props batch's UVs; at Performance they get a flat
     colour per slot (their texture's average) instead of a texture sample.

   A **miss** (a ray that hits nothing within its reach) returns the sky: the fog colour graded towards the horizon, as `env()`
   does now, until there is something better.

Hit shading will never match the real picture exactly (no particles, no moby lighting rig, no post effects). Mixing it in only
where source 1 fails keeps those differences small. It's also why reflections get blurred by distance and roughness, the same
as the screen-space ones.

## Quality (the Reflections setting: Off / Performance / Balanced / Ultra)

**Performance** is the default, then **Balanced** and **Ultra** (Off switches Reflections off). Read at start-up, like every mod setting.

| | Performance | Balanced | Ultra |
|---|---|---|---|
| Where it traces | Only where the screen-space reflection or mirror found nothing (replaces the `env()` guess) | Every water pixel that is not on a mirrored plane; metal | Every reflective pixel, including the mirrored planes (planar mirror redraw switched off, which wins some time back) |
| Resolution | Full, but only on the pixels that need it (traced inside `fs_water`, phase 3) | Full on water off the mirrored planes (or half × half if over budget) | Full |
| Rays per pixel | 1 | 1 | 2 (rough ripples, glass) |
| Hit colour | Last frame's picture, else texel × vertex colour, no shadow at the hit | + a sun shadow ray at the hit | + a sun shadow ray at the hit, movers textured |
| Movers (Ratchet, enemies) in reflections | Only through last frame's picture | Yes, flat colour off screen | Yes, textured |
| Chrome / glass | Unchanged (retail env texture) | Traced, mixed with the env texture | Traced |
| Reach | 60 units | 120 | 200 |
| Budget (RTX 3080 Ti, 1440p, Kerwan spawn and the Novalis / Pokitaru water) | **≤ 0.5 ms** | ≤ 1.5 ms | ≤ 3.5 ms |

Performance is cheap because water is often a small part of the screen, the screen-space march already covers most pixels, and
tracing at a quarter of the pixels with re-projected colour costs little. If a phase misses its budget, we cut features from
that quality level until it fits, and we never raise the budget instead.

A second setting may come later if testing calls for it: `Surfaces`: *Water and metal* / *Water only*.

## Measuring cost honestly

So far the RT work has been measured with the FPS overlay and nvidia-smi. That is too rough for a 0.5 ms budget. The game
already has a GPU pass timer: `RAC_GPU_PASSES=120` (`viewer/passtime.rs`, wgpu timestamp queries; the value is how many frames each line averages, so `=1` prints every frame) prints a `gpupass:` line to the
run log with each pass's average GPU ms. Phase 1 put the existing RT passes on it (`rt shadow mask`,
`rt rays`). Every new reflection pass takes its stamps from `passtime::cw` / `rw` too, so every phase can report
"reflections: X ms" at fixed test spots. Not timed: the per-frame build of the moving things' structure
(`build_acceleration_structures` is not a pass; it would need `TIMESTAMP_QUERY_INSIDE_ENCODERS`). Its CPU side is in the
`rt movers` log summary.

Test spots (the same each phase, with `--shot` A/B screenshots of the screen-space vs traced reflections):
- Kerwan spawn and the terrace (lots of ties and shrubs, few reflections: checks that the cost stays near zero when there is
  little water),
- Novalis water quads and Pokitaru's ponds (flat water, mirror on),
- Aridia / Batalia ocean (big non-flat sea, worst case for coverage),
- Gaspar lava (`reflect_ray` gloss), Orxon sludge,
- a metal moby close up (the Novalis ship, bolts).

## Phases

| # | Step | Result |
|---|------|--------|
| 0 | Commit or stash the Ragdoll and Hoverboard work in progress, so this work gets clean diffs | Clean baseline |
| 1 | The built-in mod (`plugins.rs`, Quality setting, `RAC_RT_REFLECTIONS` env force, `viewer/rtrefl.rs`); `rtshadow::wanted()` includes it; the RT passes timed by `RAC_GPU_PASSES` | Mod switches on, structure built, costs measurable; nothing visible yet |
| 2 | Hit data: vertex buffers + compact index buffer + per-triangle texture slot as storage; WGSL `rt_hit(p, r, reach)` returning colour and distance (texel × vertex colour, misses give the sky) | A debug view (`RAC_POST_VIEW=rtrefl`) showing the hit colour of every pixel's mirror ray |
| 3 | Water, Performance: `fs_water` (a second pipeline, rtrefl.rs `Water`) traces one ray inline where `reflect_ray` is unsure and no mirrored plane lies under the pixel, in place of the `env()` guess; static world only, reach 60. (Planned as a quarter-size compute pass; tracing inline only where needed is simpler, needs no upsampling, and the phase 2 numbers said it fits.) | **First visible step:** no more fog reflections at screen edges / looking down. Budget ≤ 0.5 ms |
| 4 | Re-projection into the current picture: a hit that is on screen at the picture's depth takes the picture's colour (`soft_scene`), off-screen / hidden hits keep the hit shading; the moving things are traced too and show where the picture has them. (Planned with last frame's picture; `fs_water` already has this frame's, so there is no lag or ghosting.) | Reflections match the real picture, Ratchet included when he is on screen |
| 5 | Balanced: trace all non-mirror water pixels, sun shadow ray at the hit, movers with flat colour; lava, sludge and mud gloss too. (Planned at half size; traced inline at full size as in phase 3, since phase 4 traced every water pixel in 0.5 ms. The trace runs after the screen-space reflection and wins where it hits; picture hits are moved by the ripples as the screen-space ones are. Movers off screen: their texture's mean from a per-triangle texture table rtmovers.rs fills each frame.) | Rivers, waves and the sea reflect correctly. Budget ≤ 1.5 ms |
| 6 | Chrome and glass: the metal packets' fragment shader traces one ray (inline, like `sun_occ`) and mixes the hit with the env texture (Balanced), or replaces it (Ultra) | Bolts and ships reflect the level |
| 7 | Ultra: full size, 2 rays on rough water and glass, mirrored planes traced too and `mirror.rs`'s redraw switched off, movers textured | Best looks. Budget ≤ 3.5 ms |
| 8 | Tuning pass at the test spots, README / CHANGELOG wording for players, mod description | Ready to ship |

## Risks and open questions

- **wgpu's experimental ray queries and big storage bindings.** The level structure's bind group already sits at bindings 4..8
  of the level shader's group 0. The reflection passes are compute passes with their own bind groups, so they don't add to
  that. Only phase 6 (metal) touches a draw pipeline.
- **Hit shading fidelity.** Tfrag / tie vertex colours carry the baked light, but the level shader's real formula (Lighting
  Enhanced) is more than texel × colour. If off-screen hits look too flat next to re-projected ones, a shared WGSL function taken
  from `SHADER` is the fix, not a second copy that drifts.
- **Re-projection ghosts** when something moves fast (a one-frame lag, like the moving shadows). Depth tests and a confidence
  fade keep this small. It is the same trade-off already accepted for the shadow mask.
- **LOD mismatch.** The traced mesh is the near LOD, while far surfaces are drawn with coarser ones. For reflections this only
  shifts where a ray starts slightly. Start rays a little above the water surface (as `reflect_ray` does, `gsm * 0.02`).
- **Lower-end RTX cards** (2060, 3060): the budget is set on a 3080 Ti, so roughly 2–3× it on those. Performance should still
  stay around 1–1.5 ms there. Check on a weaker card if one is available, or with a lower GPU clock.
- **Split view** (`DrawFx.water_on` per view): the reflection pass needs one target per view, like the water targets.
- **Dynamic shadows dependency.** The level structure and the moving things are built only while the sun is shadowed
  (`gpu/draw/prepare.rs`: Lighting Enhanced with Dynamic shadows on), as the other RT mods need. Reflections should not need
  Dynamic shadows: by phase 3, build the structure whenever this mod is on and the water or metal is drawn.

## Files

- New `rac-rs/src/viewer/rtrefl.rs` (the passes, the hit-shading WGSL, the Quality levels), maybe `rtrefl/` children if it grows.
- `rtshadow.rs`: `wanted()`, storage usage on batches, keep the compact index buffer and the per-triangle texture table.
- `rtmovers.rs`: posed positions and a per-slot flat colour, readable by the reflection pass.
- `waterfx/shader.rs`, `waterfx/passes.rs`, `waterfx/setup.rs`: read the traced reflection in `fs_water` / lava / sludge.
- `gpu/metal.rs` (+ its shader): the traced chrome (phase 6).
- `mirror.rs`: skipped on Ultra (phase 7).
- `plugins.rs`: the mod and its setting.
