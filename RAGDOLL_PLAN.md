# Ragdoll Ratchet: plan

Status: phase 0 tools built (2026-10-26): `RAC_RAGDOLL_LOG` and `RAC_RAGDOLL_DUMP` in `rac-rs/src/retail/ragdoll.rs`. They
only read and log. No ragdoll yet.

## Phase 0: collecting the numbers

Start the game with both switches (PowerShell, in the repo folder):

```powershell
$env:RAC_RAGDOLL_LOG = "1"; $env:RAC_RAGDOLL_DUMP = "1"; .\Play.cmd
```

Then, in a level, do each of these once, and note roughly when (the log lines carry field numbers, not times):

1. Walk off a high ledge and fall without doing anything (a few heights, small to big).
2. Jump, then double jump, from flat ground (the normal drop heights, so the Low setting stays above them).
3. The same fall again, opening the Heli-Pack partway down.
4. The same with the Thruster-Pack, if you have it.
5. A wrench slam (jump, then the wrench button in the air).
6. Press L3 (Left Shift) and R3 (R) once each, on the ground and in the air.
7. Fall into a pit or water once.

Everything goes to `player/log.txt`; the lines start with `ragdoll:`. `RAC_RAGDOLL_LOG=2` also logs every field in the air.

A live plugin. When Ratchet falls from a big height without the Heli-Pack, Thruster-Pack or a wrench slam, he goes limp and
flops when he lands. A button makes him go limp at once while he is in the air (like GTA V). After a few seconds he gets up
and the player has control again.

## Goals and limits

- **Off = retail.** With the plugin off, nothing is registered, read or written: the game stays bit-exact (same rule as
  `strafe.rs`).
- **The game logic is never moved.** The ragdoll changes only what is drawn, plus one logic change: while Ratchet is limp, the
  pad the game reads is neutral (no buttons, centred sticks). Retail keeps simulating Ratchet as usual: he falls with retail
  gravity, lands, and stands idle where he landed. Pits, deaths, checkpoints, enemies and the camera keep working because the
  game still has its own Ratchet.
- **Replays stay deterministic.** The trigger and the lock length come only from RAM, the recorded pad and the plugin
  settings, never from the renderer's physics, so a replay with the same settings locks the same fields.
- Ratchet only (body form 0). Clank, Giant Clank, Hologuise, ships and turrets never ragdoll.

## Design: two halves, the same split as the Modern controls twist

`strafe.rs` already works like this: the worker decides each field and publishes a small `Draw` struct in `HeroParts`; the
renderer applies it to the posed joint matrices (`twist_mats`, `viewer/handler.rs:919`). The ragdoll does the same.

| Part | Thread | Owns |
|---|---|---|
| `retail/ragdoll.rs` | worker (per field) | fall tracking, the trigger, the pad lock, the phase timeline, the `Cue` it publishes |
| `viewer/ragdoll.rs` | renderer (per game tick) | the physics bodies, collision with the level, building the posed matrices, the get-up blend |

### Worker side (`retail/ragdoll.rs`)

Runs from `Plugin::pre_field` (after modcam / strafe / aim, before the field runs). It reads:

| Address | What | Source in the code |
|---|---|---|
| `0x13f3d0` | hero position | `strafe.rs HERO_POS` |
| `0x1413d4` | hero state | `strafe.rs`, `modcam.rs HERO_STATE` |
| `0x1413dc` | state group: 2 = falling, 4 = jumping | `modcam.rs HERO_GROUP` |
| `0x13f65e` | airborne (0 = on the ground) | `modcam.rs HERO_AIRBORNE` |
| `0x1413f4` | body form (0 = Ratchet) | `strafe.rs BODY_FORM` |
| `va(0x15f5a4)` | game mode (0 = gameplay) | `strafe.rs GAME_MODE` |
| `0x1415f8` | health | `debugkeys.rs HEALTH` |

Phases:

```
Idle -> Air (tracking) -> Limp (pad locked) -> GetUp (pad locked, blending) -> Idle
                 \-> (button) Limp
```

1. **Idle -> Air.** Group 2 or 4 and airborne for 2 fields in a row (the same debounce modcam uses for jumps). Store the apex
   height `apex_z`, starting at the take-off height.
2. **Air.** Each field: `apex_z = max(apex_z, z)`. If an *excluded* state shows up, the fall is marked "saved" and cannot trigger.
   If the ragdoll button is pressed (setting `button`) and the state is not excluded, go to Limp straight away (mid-air ragdoll).
3. **Air -> Limp on landing.** On the first grounded field: `drop = apex_z - z`. If the fall was not saved and
   `drop >= fall height`, go to Limp. Otherwise back to Idle.
4. **Limp.** For `limp time` fields the pad is neutral: `m.set_pad(0, [0x80; 4])` in `pre_field` (`set_pad` already ran for this
   field in `retail.rs:3216`, so calling it again overrides it). A mid-air Limp keeps the lock until he has landed and then for
   the limp time.
5. **GetUp.** 0.6 s (30 fields), pad still neutral, the renderer blends back to the animated pose. Then Idle.

Cancels (go straight to Idle, no get-up): game mode != 0 (cutscene, menu, level change), body form != 0, health 0, a death or
swim state, or a position jump over 8 units in one field (respawn or teleport; `modcam.rs JUMP`). On a cancel the pad lock ends
the same field.

The published `Cue` (a new `ragdoll: ragdoll::Cue` field in `retail.rs HeroParts`, next to `strafe`):

```rust
pub struct Cue {
	/// Counts up once per ragdoll, so the renderer can tell a new one from the same one.
	pub id: u32,
	pub phase: Phase, // Off, Limp, GetUp
	/// Fields since the phase started.
	pub t: u32,
	/// Hero velocity at the start (difference of HERO_POS over the last field, per second): the bodies start with it.
	pub vel: [f32; 3],
	/// Floppiness setting, copied so the renderer needs no plugin read.
	pub flop: u8,
}
```

Reading the button: the plugin's `pre_field(&mut Machine)` does not see the pad. Add `Machine::pad_buttons() -> u16` in
`oracle/src/lib.rs` (keeps the last value passed to `set_pad`). In the standard PS2 layout L3 = `0x200` and R3 = `0x400`, which
fits the bits `set_pad`'s doc lists (Select 0x100, Start 0x800). `binds.rs` writes them as `("ls", 0x40, ...)` / `("rs", 0x80, ...)`,
which is probably a byte-level mask. Confirm in phase 0. On the field it triggers, the button is masked off what the game reads.

### Hero states

Known from the code:

| State | What | Ragdoll |
|---|---|---|
| 6 | fall (`State::Fall`) | tracked |
| 7, 9, 0xb, 0xe, 0x11, 0x1c | jumps: stand, run, high / flips, double, wall, pull-up | tracked |
| 0x2d | swing release fall (group 2) | tracked |
| 0x10 | Thruster-Pack flight (`retail.rs:2604`) | **saves** the fall |
| 8 | glide (`player.rs` PARKED note) | **saves** the fall |
| 0x22 | landing strike (wrench slam; `player/states.rs:102`) | **saves** the fall |
| 0x15 | Comet-Strike throw | tracked (check in game) |
| 0x16, 0x3c | hit reaction, lava hurt | no trigger while in them (retail already throws him) |
| 0x33-0x37, 0x6a, 0x75, 0x76 | water | cancel |
| 0x68, 0x7b | mud sinking / death | cancel |
| 0x24-0x26, 0x2c | Swingshot pull / swing | no trigger while in them |
| 0x32 | ship / turret | never |
| 0x74 | grind | never |

**Unknown, measure in phase 0:** the Heli-Pack hover / slow-fall state id (the Heli-Pack must save the fall). Plan: a
`RAC_RAGDOLL_LOG=1` line per state change (field, state, group, airborne, z) while trying each pack on Veldin and Novalis.
Also confirm 0x22 is the slam and not some other landing strike.

### Renderer side (`viewer/ragdoll.rs`)

Called in `viewer/handler.rs` right after `strafe::twist_mats` (`handler.rs:919`), on the freshly posed `mats`. It needs:

- `mats`: skin matrices, bind -> posed model space, from `pose_held`.
- `local_held(...)`: the animated local joint transforms (already computed nearby for the held glove).
- The hero's model frame (`smooth.rs hero_frame(pos, yaw, tilt)`), to go from model space to world space and back.
- `self.world: Option<World>` (the level collision, `viewer.rs:1500`, built by `collision::collision` in `app.rs:555`).

**Bodies.** About 15 rigid bodies, one per main bone: pelvis, belly, chest, head, upper arm x2, forearm x2, hand x2, thigh x2,
shin x2, foot x2. Each has a capsule (radius, half length) for collision, a mass, and the hero joint it drives. Bonus
"secondary" bodies for the two ears and the tail: light, stiff springs toward their animated angle, so they wobble.

**Solver.** XPBD rigid bodies (Müller et al. 2020, "Detailed Rigid Body Simulation with Extended Position Based Dynamics"):
stable at large steps, joint limits are simple position and angle constraints, no stiff-spring blow-ups. 8 substeps per 50 Hz
tick, 1 iteration each. Joints: ball joints with a swing cone + twist limit (shoulders, hips, neck, spine), hinge joints with
an angle range (elbows, knees). Gravity from retail (measure: the hero fall acceleration, units are metres per
`player.rs`). Light linear and angular damping.

**Collision.** Each capsule is tested as 2-3 spheres with `World::push_sphere_masked` (sphere push-out, already used by the
player code). The push gives the position correction; friction removes part of the tangential velocity. Self-collision: none
in v1 (joint limits keep limbs out of the body well enough).

**Start pose.** On a new `Cue.id`, each body starts at its bone's world transform from the current posed `mats`. Its velocity
= `Cue.vel` plus the bone's own motion between the last two ticks (keep the previous tick's bone positions), so a flip keeps
spinning. A landing ragdoll also gets the fall speed in `Cue.vel`, so he hits the ground and crumples.

**Tether.** The body must stay near the game's Ratchet, since the camera, shadows of the logic and the get-up are all there.
The pelvis has a soft spring toward the logic position (hero frame origin + the pelvis offset) and a hard limit of 1.5 units.
While falling both move with the same gravity, so the spring does almost nothing; on the ground it stops him sliding away.
This is what lets us **never write his position back into the game**.

**Building the pose.** For each joint, in skeleton order (`Skeleton::parents`):
- a body joint: world = body transform x (fixed bone-to-body offset taken at start);
- any other joint: world = parent world x the animated local transform (fingers, face, eyelid blinks, Clank, held items keep
  following their parent bone);
- skin matrix = world (to model space) x inverse bind.

`Skeleton::inverse_bind` is private today: add a `pub fn inverse_bind(&self, j) -> Mat` in `anim.rs`.

Things attached to the hero follow without extra code because they are posed from hero joints: Clank on the back (attach
point 5), the held weapon / glove (joint 54), the wrench (wrist 56).

**Get-up.** During `GetUp`, blend from the ragdoll pose to the animated pose over 0.6 s, per joint (slerp the world rotation,
lerp the position), pelvis first and limbs last so he does not levitate in one piece. Later (phase 5): look for a retail clip
that starts lying down (the tumble state 0x77, knock-down, death clips) to play during the get-up instead of a plain blend.

**Caches and smoothing.**
- `viewer/herocache.rs`: add the ragdoll's `Cue.id`, phase and tick to `HeroKey` (today `[u64; 14]`), so a cached pose is never
  reused across ragdoll ticks.
- The ragdoll steps once per game tick, not per display frame. Pausing stops it, and `smooth.hero` blends between tick poses
  as it already does.
- Shadows (cascades, ray-traced movers in `rtmovers.rs`) use the skinned hero mesh, so they follow.

## Bone mapping

The hero skeleton has at least 111 joints (`twist_mats` checks `mats.len() < 111`). Known from the code:

| Joint | Part | Source |
|---|---|---|
| 0 | model root | `Skeleton::parents` doc |
| 1 | pelvis | `strafe.rs twist_mats` pivot |
| 2, 3, 4 | spine, bottom to top | `twist_share` 0.75 / 0.5 / 0.25 |
| 15-21 | eyelids (so the head is just above them) | `headpose.rs EYELID_JOINTS` |
| 54 | hand holding the glove / guns | `viewer.rs GLOVE_JOINT` |
| 56 | wrist holding the wrench | `viewer.rs WRIST` |
| 92-97, 98-103 | the two legs | `strafe.rs LEG_JOINTS` |
| 104-110 | lower body extras (twist share 1.0; maybe the tail) | `twist_share` |

Phase 0 dumps the rest: for each joint, its parent, bind position and how many vertices it skins, from the disc at run time
(`RAC_RAGDOLL_DUMP=1` writes to the run log; never committed, it comes from disc bytes). From that we fill the table
`BODIES: [(name, joint, child_joint, radius, mass, joint_kind, limits)]`. Expected chains: pelvis 1 -> 2 -> 3 -> 4 -> neck ->
head; 4 -> shoulder -> upper arm -> forearm -> 54 / 56; 1 -> hip -> thigh -> shin -> foot (92.., 98..); ear chains off the
head; tail off the pelvis. The joint indices are the same on both discs (same class data); check on USA too.

Joints between two bodies that are not themselves bodies (a collarbone, a twist bone) keep their animated local transform.

## Plugin and settings

In `plugins.rs`: `struct RagdollRatchet` and one entry in `PLUGINS`.

- id `ragdoll`, name `Ragdoll Ratchet`, live plugin, default off, `env_force` = `RAC_RAGDOLL`.
- Settings (all within the menu rules: ASCII letters, digits, spaces, `-`; names <= 24 chars, values <= 16, 2-4 values):

| id | Name | Values | Default | Meaning |
|---|---|---|---|---|
| `height` | Fall height | Low / Medium / High | Medium | drop that triggers it: 6 / 10 / 16 units (first guesses; a double jump is about 3, tune in game) |
| `button` | Ragdoll button | Off / L3 / R3 | L3 | mid-air ragdoll button. Keyboard defaults: L3 = Left Shift, R3 = R (`binds.rs`), so no new bind row is needed |
| `time` | Limp time | Short / Medium / Long | Medium | 1.5 / 3 / 5 s on the ground before getting up |
| `flop` | Floppiness | Stiff / Loose / Noodle | Loose | joint stiffness and damping, and how wide the limits are |

Switching it off (or any setting change) while Ratchet is limp ends the ragdoll with the get-up blend.

The name and values are checked by `plugin_ids_and_names_fit_the_file_and_the_menu`.

## Files

| File | Change |
|---|---|
| `rac-rs/src/plugins.rs` | `RagdollRatchet` plugin, settings, `PLUGINS` entry, `pre_field` calls `retail::ragdoll::pre_field` |
| `rac-rs/src/retail/ragdoll.rs` | new: worker phases, trigger, pad lock, `Cue`, log / dump env vars |
| `rac-rs/src/retail.rs` | `mod ragdoll;`, `ragdoll: ragdoll::Cue` in `HeroParts`, filled where `strafe` is |
| `oracle/src/lib.rs` | `Machine::pad_buttons()` |
| `rac-rs/src/viewer/ragdoll.rs` | new: bodies, XPBD solver, collision, pose building, get-up blend, tuning file |
| `rac-rs/src/viewer.rs` | `mod ragdoll;`, a `Ragdoll` state on the renderer (reset on level load) |
| `rac-rs/src/viewer/handler.rs` | call it after `twist_mats`; pass the world, hero frame and local transforms |
| `rac-rs/src/viewer/herocache.rs` | ragdoll id / phase / tick in `HeroKey` |
| `rac-rs/src/anim.rs` | `Skeleton::inverse_bind` accessor |
| `CHANGELOG.md`, `README.md` | player-facing note in simple English |

## Dev aids

- `RAC_RAGDOLL=1`: forces the plugin on.
- `RAC_RAGDOLL_LOG=1`: one log line per hero state change while tracking, and every trigger decision (state, drop, saved or not).
- `RAC_RAGDOLL_DUMP=1`: the hero skeleton dump (phase 0).
- `RAC_RAGDOLL_TUNE=<file>`: gravity, substeps, stiffness, damping, friction, tether strength, per-body radius and limits;
  re-read while running, like `RAC_RT_TUNE`.

## Phases

0. **Measure.** State log and skeleton dump. Find the Heli-Pack state, confirm 0x22, check L3 / R3 do nothing in the air,
   check the `set_pad` bit layout, measure the hero's fall acceleration and the usual drop heights of normal jumps.
1. **Plugin + worker.** Plugin entry, settings, phases, pad lock, `Cue`. Test with no physics: in Limp, draw him frozen in
   the pose he had when it triggered. That proves the trigger, the lock, the cancels and the exclusions.
2. **Limp body.** Bodies, XPBD joints, gravity, ground collision, tether, pose building. No get-up yet (snap back).
3. **Get-up blend** and the cache / smoothing work.
4. **Tuning.** Floppiness presets, the ears and tail, friction on slopes, no jitter at rest (sleep a body below a speed for a
   few ticks).
5. **Polish (optional).** A retail get-up clip, an enemy hit while limp adds a push, a sound on impact from retail's own sounds.

## Tests

Unit tests (they need a completed setup; the 2 GB debug test exe problem still applies, see CLAUDE.md):
- worker phases on a scripted sequence of (state, group, airborne, z): a high fall triggers, a short one does not, Thruster /
  glide / slam saves it, a cancel ends it, the button triggers only in the air;
- the solver: a capsule chain dropped on a flat `World` comes to rest above the floor, within the joint limits, and the
  pelvis stays inside the tether;
- the plugin names / values test (already exists, it covers the new entry).

In game (needs you to play):
- Veldin cliffs, Novalis ledges, Kerwan rooftops: falls just under and over each height setting.
- Heli-Pack, Thruster-Pack and wrench slam falls never ragdoll.
- Mid-air button: from a jump, a double jump, a flip; pressed on the ground does nothing.
- Falling into a pit or water while limp: retail's death / respawn or swim takes over cleanly.
- A cutscene or pause during Limp; a level change; quitting to the title.
- Moving platforms (the tether follows the logic position; check he does not lag behind).
- With frame smoothing on and off, ray-traced shadows on, PAL and USA discs.
- Plugin off: a RAM-diff run against the original exe stays identical.

## Risks

- **It looks bad before it looks good.** Ragdolls live or die by tuning: joint limits, mass ratios, damping. Expect several
  rounds of play-and-adjust in phase 4.
- **Moving platforms and mobies** are not in `World` (it is the static level collision), so the body can sink into a moving
  platform or a crate. The tether keeps it near the logic position, which stands on the platform; v1 accepts the overlap.
- **Get-up** from lying down to standing with a plain blend can look like he floats up. The ordered blend helps; a retail clip
  would be better (phase 5).
- **Slopes:** at rest on a slope the body may creep; the tether and body sleep stop it.
- **Gameplay change with it on:** a neutral pad for a few seconds is new behaviour. An enemy can hit him while he is limp.
  That is the point of the plugin, but it is worth saying in the changelog.
