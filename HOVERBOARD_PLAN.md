# Hoverboard: plan

Status (2026-10-27): first version built (hotkey only). The game boots to the title with it on (the board joins the hero
model), but riding is **not tested in game yet**. Not committed. Stance chosen from the dump: seq 51 frame 0, turned -0.52 rad
(`STANCE` in viewer/hoverboard.rs). First test to watch: the stick-to-world offset (`off` in the `ride` log lines, the board
should go where the stick points), whether retail's run accepts the board speed (`speed` vs `real`), jumps at speed, and
the stance (feet on the deck, facing the right of the path).

## The mod

A built-in mod (`plugins.rs`): id `hoverboard`, name `Hoverboard`, `env_force` = `RAC_HOVERBOARD`.

Press the board button (default d-pad Down, keyboard Down arrow) on the ground to step onto the board, and press it again to
step off. While riding, Ratchet stands sideways on a board, builds up speed, carves turns, coasts when the stick is let go, and
brakes when the stick is pulled back. Cross jumps (retail's own run jump, at board speed). The default was L3 at first, but L3
(Left Shift) is Ragdoll Ratchet's button in the air, so stepping off after a jump would ragdoll: the player pointed out the
clash, and the d-pad (unused by retail in gameplay) became the default, L3 a choice.

| id | Name | Values | Default | Meaning |
|---|---|---|---|---|
| `button` | Board button | Down / L3 | Down | the hop on / off button (L3 works on the ground only) |
| `speed` | Top speed | Slow / Medium / Fast | Medium | 10 / 14 / 18 units/s (retail's run is about 5.7) |
| `turn` | Turning | Tight / Loose | Tight | carve rate: 3.2 / 2.2 rad/s at low speed, about half that at top speed |

Next steps (agreed with the player): get the riding right with the hotkey, then a gadget-swap option (picking a chosen gadget
on Quick Select hands you the board), then a real Quick Select slot of its own (needs research: item ids, icon, name text).

## Goals and limits

- **Off = as before.** With the mod off nothing is read or written and the hero mesh is unchanged: the game stays bit-exact.
- **Retail still moves Ratchet.** The board does not move him itself: while riding, the game's Ratchet runs (state 2) and the mod
  only changes, between two fields, the words strafe_feel.rs already changes (run speed `0x13f4e4`, logic facing `0x13f3e8`) and
  the left stick the game reads (`set_pad`). So walls, slopes, ledges, pits, enemies, checkpoints and the camera all stay
  retail's. The stick handed to the game always points along the board's heading, so retail never turns him by itself.
- **Deterministic.** Mount, heading and speed come only from RAM, the recorded pad and the settings (replays stay exact).
- **Picture only, the rest:** the riding pose, the lean, the hover bob and the board itself are drawn by the renderer
  (`viewer/hoverboard.rs`); held items, Clank and the packs follow the drawn pose (`strafe::pose_fix_between`, `Smooth::set_limp`).
- Ratchet only (body form 0), gameplay only (game mode 0; the menus 1 / 3 only pause it).
- Ragdoll Ratchet wins: when it makes him limp (`ragdoll::current().phase != Off`) he falls off the board.

## Worker (`retail/hoverboard.rs`)

`field_run.rs` hands it the field's pad (`note_pad`), its `pre_field` runs with the built-in mods (after Ragdoll Ratchet).

- **Mount**: the board button pressed this field, grounded, in stand / run / skid. **Dismount**: the button again on the ground, any `CANCEL` state
  (water, mud, Swingshot, ship / turret, grind), body form not 0, game mode not gameplay or menu, Ragdoll limp, or more than
  `SUSPEND_MAX` fields outside the riding states (an attack, a hit, a ledge grab: the board waits that long, hidden).
- **Heading and speed** (ours, units/s): with the stick pushed the heading turns toward the stick's world direction at the
  carve rate (less at speed) and the speed rises toward the top speed; pulled back past `BRAKE_ANGLE` brakes; let go coasts down
  (`COAST`). Under `PIVOT_SPEED` the heading snaps to the stick (turning on the spot). A wall shows as the field's real move
  falling short of the speed: the speed drops to the real one.
- **Stick to world**: retail turns the stick into a world heading (`0x13f4d0`) with the camera. The mod measures that turn
  every field the handed stick was pushed (`off` = retail's heading minus the stick's angle), and while it cannot measure (the
  board coasting with the stick let go) it follows the Modern camera's yaw change (`modcam::cam_yaw`).
- **The pad handed to the game**: buttons as pressed (the board button removed on the mount / dismount field); the left stick full length
  along the board's heading while it moves, centred once it stops (he stands on the board).
- Publishes `Cue` (riding, heading relative to his drawn yaw, lean, speed, a tick counter) through `HeroParts.hoverboard`.

## Renderer (`viewer/hoverboard.rs`)

- **Pose**: a stance frame of one of Ratchet's own sequences (`STANCE`, tune with `RAC_HOVER_TUNE=seq=..,frame=..`), turned
  sideways (he faces the right of the board's path, left foot forward), raised by the board's height, leaning into turns.
  `RAC_HOVER_DUMP=1` lists every hero sequence's stance numbers (feet apart along / across, pelvis height) to pick it.
- **Board**: a procedural mesh appended to the hero's model (`combined_hero!` in app.rs, only with the mod on), drawn like the
  wrench: its vertices come after the hero afterimages in `frame_hero.rs`, hidden at the origin when not riding. Its texture is a
  generated one under `BOARD_KEY` (next to `FLAT_SLOT`). A later step can swap in the race board model from the disc.

## Testing

`$env:RAC_HOVERBOARD = "1"; $env:RAC_HOVER_LOG = "1"; .\Play.cmd` (PowerShell, in the F: folder). Log lines `hoverboard:` in
`player/log.txt` (dev builds without `windows_gui` need `RAC_RUN_LOG=1` too). Close the game before building.

## Round 2 (2026-10-27, after the first test: "really solid")

- **Mud and quicksand**: the player sank (log: `dismount (state, state 0x68)`, Aridia-style mud). Retail asks SetState for 0x68
  (sinking) whenever he stands on mud. While he rides, the SetState hand that strafe/chain.rs already has on all 19 levels (both
  discs) now refuses 0x68 / 0x7b (`hoverboard::refuse_state`), so the board skims over mud. With the Modern controls off,
  machine.rs registers those hands for the board alone. Log: `hoverboard: skims over mud`.
- **Round 3 (2026-10-28): fell through the map on mud.** The sinking state is what stands him on the mud face (a soft face
  over a lower floor), so with it refused he dropped, and past about a unit under the surface retail stops asking (its mud
  check in Aridia FUN_0021b698 asks only while z + 0.25 is within 1.0 of the surface `0x13f644`), so he fell on through. Now
  each refusal notes the surface and `field` lifts him back onto it before the next `SKIM_HOLD` fields (only when he is under it
  by less than `MUD_WINDOW`). Safety net: deeper than `MUD_GIVE_UP` under the surface the sinking state is let through, so he
  sinks and steps off as before instead of falling. Log: `skims over mud (..., z, surface)` / `too deep in the mud`. Tested in
  game (2026-10-28, after the v0.4.1 merge): the player says it works very well.
- **The race clips**: `RAC_HOVER_DUMP` found Ratchet's own hoverboard race sequences, present only on level 0, Rilgar (5) and
  Kalebo III (16): 82 ride loop, 86 / 104 carves (taken as heel / toe side), 85 jump, 97 / 98 falling off. The pose now plays
  them (`race_pose`), read from Rilgar at level load when the level lacks them (`race_clips`), with the plain stance as a fall-
  back. The board now sits under his drawn feet (so it rises in the jump clip). Not tested yet: whether 86 / 104 are the right
  way round (swap the two constants if he leans out of the turn), and the jump clip's timing.
