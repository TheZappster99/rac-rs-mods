# Ragdoll Ratchet: plan

Status (2026-10-26, end of the first session): phases 0-4 done and tuned. The player tested it on Veldin and approved the feel
("perfect settings"). **Not committed yet** (branch `mods-migration`, F: folder). Next: phase 6, enemy hits.

## Progress so far (read this first when picking it up)

**Works (tested in game, Veldin, USA disc):**
- Big falls (drop >= 8 on Medium) go limp on landing; L3 in the air goes limp at once (the Heli-Pack + L3 flops best).
- Heli-Pack, wrench slam and ledge grabs never ragdoll; small drops (up to 6.2 measured) never do on Medium.
- Hard landings hit at the speed of the fall (`Cue::impact`) and bounce the whole body (`BODY_BOUNCE_MIN`, `SPIN`).
- Clank, the packs and the head items stay on the limp body (`strafe::pose_fix_between`, `Smooth::attach`).
- No sinking into the floor (`SINK` floor check), no slowdown (per-field face list + per-sphere box test), pause holds the body
  (`Cue::tick`), the body comes to rest and stops buzzing (`REST_*`).

**Tuning history (`viewer/ragdoll.rs`):**
1. First pass: light limbs (0.4-1.0 vs a 3.0 torso), damp 0.4, adamp 1.5, drive 0.05 / 0.5, limits 0.6 / 1.0 / 1.6, 8 substeps.
   Player: flops well in the air, but landings just collapsed, Clank stayed in the air, slowdowns on high falls.
2. Fixes: impact speed, whole-body bounce, Clank, the floor check, the face lists, stepping on `Cue::tick`.
   Player: "much better", but "a bit spastic when hitting the ground".
3. **Current defaults (approved):** limbs heavier (arms 0.9-1.0, thighs 1.8, shins 1.4, head 2.0, chest 3.5, pelvis 3.0), `INERTIA`
   2.5, damp 0.8, adamp 4.0, drive 0.02 / 0.2, limits 0.6 / 0.85 / 1.4, 10 substeps, `PART_BOUNCE` 0.3, speed caps 30 / 18,
   rest after 15 still fields. Bounciness Some (0.35); Lots (0.6) did little before the whole-body bounce existed.

**Open / next:**
- **Commit** the work (the files under "Files", plus `physics.rs` `ids_in_box` / `nearest_among` / `ground_among` and
  `strafe.rs` `pose_fix_between`). `Cargo.lock` shows a large diff from the builds: check it before committing it.
- **Phase 6, enemy hits knock him down**: built 2026-10-26, **not tested in game yet**. Setting `hits` (Enemy hits, Off / On,
  default On). The worker counts a hit when the state turns 0x16 or health drops outside lava (`HIT_COOL` 20 fields between
  hits). He goes limp that field (Ground, or Fall in the air), and a hit while limp restarts the limp time (GetUp goes back to
  Ground). `HIT_FIELDS` (4) later, `Cue::push` carries retail's knock-back speed (his move over those fields). The renderer adds
  it once per push id: x `hitpush` (2.5) plus `hitlift` (5.0 up), capped at `HIT_MAX` (22), the same speed on every part plus a
  whole-body spin (`HIT_SPIN`) so he topples. (A first version gave the top more speed than the legs: on big launches a limb
  stretched out and snapped back. `RAC_RAGDOLL_LOG` now writes a `stretch` line for any joint gap over 0.1.)
  When retail barely moved him, it pushes backwards from where he faces (`HIT_BACK` 4). Log lines: `ragdoll: hit`, `hit push`,
  `body hit push`. Tune live with `hitpush=` / `hitlift=` / `hitleash=` in `RAC_RAGDOLL_TUNE`.
  First test: the push felt right, but the body stopped dead in the air "like an invisible barrier" at the 1-unit tether. After
  a hit, the leash is now `hitleash` (6.0; 3.0 at first, the player asked for more) with no pull back inside it (`thrown`), and the get-up blend brings him back. A
  resting body now wakes only when the game's Ratchet moves (`rest_at`), so a thrown body can rest away from him.
- **Hit start at rest** (`Cue::from_hit`): the body used to take the drawn pose's jump into the hit reaction as speed
  (10-12 units/s up, `body start ... moving`), on top of the push: the pelvis flew 4.5 above him, back into the camera, and a
  tail or ear crossed the screen. A hit ragdoll now starts still and only the push throws it (raised from 1.5 / 2.5 to keep the throw).
- **Streak after hits** (fixed and tested 2026-10-27, see the end of this item): `hide` puts a hidden part (gun, wrench, ghosts) at the model origin, the game's
  feet; the tick blend drew a part hidden one tick and shown the next as a streak from there to the limp body. While limp,
  `frame_hero.rs` moves the hidden vertices to the body's pelvis. (Not a joint stretch: no `stretch` lines in the log.)
  Player, testing that build (2026-10-27): still "a line", "one specific part stuck", "tied to his feet when I get launched".
  Second cause: `pose` never moved the root (joint 0) or other parentless joints, which sit at the game's feet, so vertices
  weighted to them stayed there. They now ride the pelvis rigidly (blending back over the get-up). Built, untested.
  Player: the line got bigger, "attached to his torso and below", on hitting the ground. **Real cause** (found with the
  `stray` log line in `frame_hero.rs`): `anim::skin` keeps each vertex as retail's 16 bits (`vu0_quantise`), about +-4.53 units
  around the model origin (the game's feet). A body thrown 4.4-5 units away put legs / tail / hands past it and they wrapped to
  the far side (x -4.65 with the pelvis at +4.42). While limp the hero now uses `anim::skin_wide` (same truncation, no wrap).
  **Fixed, tested 2026-10-27**: the player confirmed the line is gone; 6 hit throws up to 5.98 units away, no `stray` or
  `stretch` lines. (The `stray` check stays as a log-only diagnosis.)
- **Ledges and drops** (untested): with the 6 unit leash a thrown body went over a ledge and fell out of the world (pelvis z
  -102, camera following it down). While the game's Ratchet stands, no floor under the pelvis within `EDGE_DEPTH` (2) of his feet
  stops the body's outward motion and draws it back (`EDGE_BACK`); `DROP_MAX` (3) below his feet lifts it back. The camera
  follows at most `CAM_DZ` (1.5) up or down.
- **Held weapon while limp** (untested): retail draws it as a moby of its own and `Smooth::attach` did not carry it (`parts 0`):
  it stayed in the game's Ratchet's hand. While limp (`Smooth::set_limp`) the held classes (`game::held::rig_classes`) are carried too.
- **Knocked into a pit** (untested): the falls the player saw were the game's Ratchet knocked off the ledge (state 0x6 then
  tumble 0x77, z 48 -> 0, pit death) with the pad locked; the body itself stayed on the ledge. A hit ragdoll now gets up at once
  with the pad free when the game's state turns 0x6 / 0x77 (`EDGE_FALL_STATES`, `free`).
- **Clank still left behind** (open): `attach` lists him as carried (601 607 1204) yet the player sees him at the game's
  Ratchet; the weapon is not in the list. Diagnosis: `around` (every shown prop near him, class@distance) and `carried`
  (how far `props` moved each carried prop).
- Diagnosis lines (`RAC_RAGDOLL_LOG`): `where` (body vs the game's Ratchet, camera offset) and `attach` (what was carried).
- **Camera follow** (setting `camera`, Off / On, default On; built 2026-10-26, not tested yet): `frame_post.rs frame_camera` moves
  retail's camera with `Ragdoll::cam_place`, kept `CAM_MIN` (2.0) off the body's pelvis: the body's pelvis minus the game's, eased 15% per field (`CAM_EASE`), half the height
  (`CAM_Z`), faded out over the get-up, blended between fields like the hero. Drawing only, nothing written. No camera collision
  on the offset: watch for the view clipping into walls after a throw.
- Untested: the Thruster-Pack (0x10), pits and water while limp, moving platforms, cutscenes, the PAL disc, other planets.
- The get-up is still a plain 0.6 s blend (phase 5: a retail clip would look better).
- Settings are read at start-up only (built-in mods rule): change them in the launcher or `mods/settings.json`, then restart.

**Testing it:** `$env:RAC_RAGDOLL = "1"; $env:RAC_RAGDOLL_LOG = "1"; .\Play.cmd` (PowerShell, in the F: folder). The log
(`player/log.txt`, lines `ragdoll:`) has every landing (`landed drop`), phase change, body start and, twice a second while limp,
`step` (ms per field, faces near, pelvis vs game height). Reset `mods/settings.json` to the defaults before asking the player
to test (the player asked for that). Close the game before building: it locks `build/release/rac-rs.exe`.

When Ratchet falls from a big height without the Heli-Pack, the Thruster-Pack or a wrench slam, he goes limp when he lands.
L3 makes him go limp at once while he is in the air (like GTA V). After a few seconds he gets up and the player has control again.

## Goals and limits

- **Off = as before.** With the mod off nothing is read or written: the game stays bit-exact. The `RAC_RAGDOLL_LOG` /
  `RAC_RAGDOLL_DUMP` measuring tools only read and log.
- **The game logic is never moved.** The ragdoll changes only the picture, plus one input change: while Ratchet is limp, the game
  reads a pad with no buttons (Start stays, so you can pause) and the left stick centred (the camera stick and the mouse still
  turn the camera). Retail keeps simulating Ratchet as usual: he falls with retail gravity, lands and stands idle where he
  landed. Pits, deaths, checkpoints, enemies and the camera keep working because the game still has its own Ratchet.
- **Replays stay deterministic.** The trigger and the lock length come only from RAM, the recorded pad and the mod settings,
  never from the renderer's physics.
- Ratchet only (body form 0), in gameplay (game mode 0).

## The mod

A built-in mod (`plugins.rs`): it shows in the launcher's Mods page with its settings, and it is read once at start-up.

- id `ragdoll-ratchet`, name `Ragdoll Ratchet`, `env_force` = `RAC_RAGDOLL`.
- Settings (the menu rules: ASCII letters, digits, spaces, `-`; names <= 24, values <= 16, 2-4 values):

| id | Name | Values | Default | Meaning |
|---|---|---|---|---|
| `height` | Fall height | Low / Medium / High | Medium | drop that triggers it: 6 / 8 / 12 units |
| `button` | Ragdoll button | Off / L3 | L3 | go limp in the air (keyboard: Left Shift) |
| `time` | Limp time | Short / Medium / Long | Medium | 1.5 / 3 / 5 s on the ground before getting up |
| `flop` | Floppiness | Stiff / Loose / Noodle | Loose | joint limits and how strongly the body keeps its pose |
| `bounce` | Bounciness | Off / Some / Lots | Some | share of the impact speed a contact gives back: 0 / 0.35 / 0.6 |

R3 is not offered: it opens the map (measured: game mode 3 the field after R3).

## Phase 0 results (Veldin, `player/log.txt` 2026-10-26)

- Pad bits as `set_pad` gets them: Cross `0x40`, Square `0x80`, Triangle `0x10`, Circle `0x20`, R1 `0x08`, **L3 `0x200`**,
  **R3 `0x400`**. L3 does nothing on the ground or in a jump.
- Fall acceleration: 24 units/s² in the plain fall (state 6), 29.6 in the jumps, 11 in the long jump (0xa). Ratchet is 1.42 units
  tall (model box). Fall speed reached 25 in an 11-unit drop.
- Drops (apex - landing): jump / double jump on flat ground at most 2.5; long jump off a small ledge 5.0; walking off ledges
  3.8, 4.0, 6.0, 11.0; a double jump off a high ledge 8.5; the Heli-Pack glide 15.1 (saved).

### Hero states

| State | Group | What | Ragdoll |
|---|---|---|---|
| 0, 2, 3 | 0 / 1 | idle, run, skid | ground |
| 4 | 12 | crouch | ground |
| 6 | 2 | fall (walked off a ledge, from run state 2) | tracked |
| 7, 9, 0xa, 0xe, 0xf | 4 | stand jump, run jump, long jump, double jump, high jump | tracked |
| 0x11 | 4 | wall jump | tracked |
| 0x1c | 4 | pull-up after a ledge grab | tracked |
| **0x8** | 5 | **Heli-Pack glide** | **saves the fall** |
| 0x10 | | Thruster-Pack (`retail.rs` afterimage note; not measured yet) | saves the fall |
| **0x14** | 6 | **wrench slam** (Square in the air) | **saves the fall** |
| 0x18, 0x19 | 3 | ledge grab / hang | restarts the fall from here |
| 0x16 | 7 | hit by an enemy | no trigger from this fall |
| 0x3c | 4 | lava hurt | no trigger |
| 0x64, 0x65, 0x67 | | arrival on the level (game mode 6) | never |
| 0x33-0x37, 0x6a, 0x75, 0x76 | | water | cancel |
| 0x68, 0x7b | | mud sinking / death | cancel |
| 0x24-0x26, 0x2c, 0x2d | | Swingshot | cancel (0x2d release fall: tracked) |
| 0x32, 0x74 | | ship / turret, grind | cancel |

Game mode: 0 gameplay, 1 and 3 menus (3 = the map / pause), 6 the level start. Anything but 0 cancels.

### Hero skeleton (dump + the loader's binding `skeletons.hero`, `omni/rac1.json`)

111 joints, parents always before children. Model space: z up, x forward, **right side = negative y**, arms straight out to the
sides in the bind pose. Bind heights: pelvis 0.49, chest 0.90, head 1.08, hips 0.46, knees 0.27, ankles 0.08.

| Joints | Part |
|---|---|
| 0 | root |
| 1 | pelvis (137 vertices, reach 0.13) |
| 2-5 | spine (5 = the upper body, 317 vertices, reach 0.20) |
| 6 / 7 / 8 | chest / neck / head (699 vertices, reach 0.27); 18 = top of the head |
| 41-43, 44-46 | left / right ear (big, reach 0.26) |
| 52 / 53 / 54 / 55 / 56 | right clavicle / shoulder / elbow / forearm twist / wrist (hand); fingers 57-70 |
| 72 / 73 / 74 / 75 / 76 | left clavicle / shoulder / elbow / forearm twist / wrist; fingers 77-90 |
| 92 / 93 / 94 / 95-97 | right hip / knee / ankle / foot and toes |
| 98 / 99 / 100 / 101-103 | left hip / knee / ankle / foot and toes |
| 104-110 | tail |

## Design

The same split as the Modern controls twist: the worker decides each field and publishes a small struct in `HeroParts`; the
renderer applies it to the posed joint matrices.

### Worker (`retail/ragdoll.rs`)

`field_run.rs` hands the field's pad to the module (`note_pad`) before the built-in mods run; the mod's `pre_field` then runs the
phases and, while limp, overrides the pad with `m.set_pad` (it runs after `set_pad`, before the field).

```
Idle -> Air (tracking) --landing, drop >= height--> Ground (limp, locked) -> GetUp (0.6 s, locked) -> Idle
              \--L3 in the air--> Fall (limp in the air, locked) --landing--> Ground
```

- **Air**: airborne in gameplay. Keeps the apex. A save state (0x8, 0x10, 0x14) or a hurt state (0x16, 0x3c) marks the fall as
  not counting; a ledge grab (group 3) restarts it from the current height.
- **Landing**: drop = apex - landing height; Ground when it counts and the drop reaches the setting.
- **L3**: newly pressed after 2 airborne fields, not in a save state: Fall.
- **Cancels** (straight to Idle): game mode != 0, body form != 0, health 0, a teleport (over 8 units in a field), a cancel state.
- Published as `HeroParts.ragdoll` (`Cue`): `id` (one per ragdoll), `phase`, `blend` (the get-up blend 0..1).

### Renderer (`viewer/ragdoll.rs`)

Called in `viewer/handler/frame_hero.rs` right after `twist_mats`. It steps once per game field (the field count), never per
display frame, so a pause freezes it and a cached hero pose stays equal to a fresh one.

- **Bodies** (11 rigid bodies, sized from the start pose): pelvis (1), upper body (5), head (8), upper arms (53, 73), forearms
  with the hands (54 + 56, 74 + 76), thighs (92, 98), shins with the feet (93 + 94, 99 + 100). Each has a few collision
  spheres along its bone (plus the hand and the toe), a mass and a capsule inertia.
- **Joints**: ball joints at the child's joint position with a swing cone and a twist limit centred on the pose he had when he
  went limp (so an elbow bent at the start can bend more or straighten, never fold back far), and a soft drive back toward that
  pose. Floppiness scales the limits and the drive.
- **Solver**: XPBD rigid bodies (Müller et al. 2020), 8 substeps per field, gravity 24 units/s², linear and angular damping.
- **Collision**: each sphere against the level collision (`World::nearest`, water faces skipped), friction on contact. Where the
  game's Ratchet stands on something that is not in the level collision (a moving platform, a crate), a floor at his feet
  height takes over.
- **Tether**: the pelvis is pulled sideways toward the game's Ratchet (soft, hard limit 1.0 unit; height is left to gravity and
  the floor), so the body never wanders off and nothing is ever written back to the game.
- **Start**: bodies start at the drawn pose of the tick, moving with the motion between the last two ticks (a fall keeps its
  speed, a flip keeps spinning).
- **Pose**: a driven joint takes its body's frame; every other joint follows its parent with its animated local transform
  (fingers, face, ears, tail, Clank and held items). The skin matrix is `new world x old world^-1 x old skin`, so no inverse bind is
  needed.
- **Get-up**: blends each driven joint from the ragdoll to the animated pose (smoothstep over `blend`).
- **Clank, the packs, the head items**: retail draws them as mobys of their own at its pose of the hero. While limp, `apply` hands
  back the game's pose and `strafe::pose_fix_between` (the Modern-strafe pose fix) carries them onto the limp body
  (`Smooth::attach`). First test: Clank stayed in the air without it.
- **Bounce**: after each substep, a contact that hit faster than 1 unit/s gets back `bounce` x its impact speed along the normal.

## Files

| File | Change |
|---|---|
| `rac-rs/src/plugins.rs` | `RagdollRatchet` mod, settings, `PLUGINS` entry |
| `rac-rs/src/retail/ragdoll.rs` | phases, trigger, pad lock, `Cue`; the measuring log and dump stay |
| `rac-rs/src/retail/worker/field_run.rs` | `note_pad` before the built-in mods |
| `rac-rs/src/retail/moby_state.rs` | `ragdoll: ragdoll::Cue` in `HeroParts` |
| `rac-rs/src/retail/snapshot.rs` | fill it next to the strafe draw |
| `rac-rs/src/viewer/ragdoll.rs` | new: bodies, solver, collision, pose, get-up |
| `rac-rs/src/viewer.rs`, `viewer/startup.rs` | `mod ragdoll;`, the `Ragdoll` state on the app |
| `rac-rs/src/viewer/handler/frame_hero.rs` | the call after `twist_mats` |
| `CHANGELOG.md`, `README.md` | player-facing note |

## Dev aids

- `RAC_RAGDOLL=1` forces the mod on. `RAC_RAGDOLL_LOG=1` also logs every phase change (`ragdoll: phase ...`).
- `RAC_RAGDOLL_TUNE=<file>`: `key=value` lines, re-read every second while the game runs: `gravity`, `substeps`, `damp`, `adamp`,
  `friction`, `tether`, and per setting value `drive=stiff,loose`, `limits=stiff,loose,noodle`, `bounce=off,some,lots`.
- `RAC_RAGDOLL_DUMP=1`: the hero skeleton dump (phase 0).

## Phases

0. **Measure.** Done (above).
1. **Mod + worker**: trigger, pad lock, cancels, `Cue`. Done.
2. **Limp body**: bodies, joints, collision, tether, pose. Done.
3. **Get-up blend.** Done (plain blend).
4. **Tuning** in game. Done (approved defaults above). Not done: wobbly ears and tail, sliding on slopes.
5. **Polish (optional)**: a retail get-up clip, an impact sound.
6. **Enemy hits knock him down** (requested 2026-10-26): the hit reaction (state 0x16, health going down) starts a ragdoll too,
   with the push of the hit (retail's knock-back velocity) on the body; a setting to switch it on. A hit while already limp
   pushes the body again. Notes for it: the worker sees the hit as state 0x16 (group 7) and health dropping (`mode` log lines);
   today 0x16 only marks a fall as not counting (`HURT_STATES`). A new `Phase` entry path in `pre_field` plus a `Cue::impact`
   from retail's knock-back motion (the hero's position change in the first 0x16 fields) is likely all the renderer needs.

## Tests

- Unit: the worker phases on scripted (state, group, airborne, z) fields; the solver drops a body onto a flat `World` and it comes
  to rest above the floor with the pelvis inside the tether; the mod names test covers the new entry. (The 2 GB test exe
  problem in CLAUDE.md still applies.)
- In game: falls just under and over each height; Heli-Pack, slam and ledge grab never ragdoll; L3 in a jump, a double jump, a
  flip; L3 on the ground does nothing; a pit or water while limp; the map, a pause or a cutscene while limp; moving platforms;
  frame smoothing on and off; ray-traced shadows on; mod off = unchanged.

## Risks

- **Looks:** ragdolls live or die by tuning. Expect a few rounds of play-and-adjust (phase 4, `RAC_RAGDOLL_TUNE` helps).
- **Get-up** from lying down with a plain blend can look like floating up; a retail clip would be better (phase 5).
- **Moving platforms and mobies** are not in the level collision; the feet-height floor covers the common case.
- **Gameplay change with it on:** no control for a few seconds, and an enemy can hit him meanwhile. That is the point of the
  mod; the launcher description says so.
