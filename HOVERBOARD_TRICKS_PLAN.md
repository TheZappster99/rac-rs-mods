# Hoverboard tricks: plan

Status (2026-10-30): plan only, nothing built. Agreed with the player: Skate-style trick controls on top of the Hoverboard
mod, done natively (the route SK8V takes in GTA V, embedding a Skate 3 engine, was looked at and set aside: it would make Skate
move Ratchet instead of retail and needs a Skate 3 disc). Start after basic riding is settled (HOVERBOARD_PLAN.md: the race clip
lean sides and the jump clip timing are still untested).

## The idea

While riding, the right stick does tricks the way Skate's "Flick-It" does: pull it back to crouch, flick it up to pop an ollie,
flick it at an angle for a flip. Spin in the air with the left stick, grab with a held button, hold the stick half back for a
manual. Land with the trick finished, or bail, and with Ragdoll Ratchet on, bailing means going limp.

It feels like Skate. It is not Skate's physics: retail still moves Ratchet, so no quarter pipes, no vert, no grinding any ledge.

## Goals and limits (same rules as the board)

- **Settings of the Hoverboard mod, not a new mod** (plugins.rs rules: names <= 24, values <= 16, 2..=4 values). With Tricks off
  the board rides exactly as today, and with the Hoverboard mod off the game stays bit-exact.
- **Retail still moves Ratchet.** The pop is retail's own jump (the jump chooser hand strafe.rs already has on all 19 levels,
  `jump_any`): a small pop is the plain jump (state 7), a big one the crouch high jump (state 0xb). The worker never writes his
  position or velocity for a trick.
- **Picture only, the rest.** The board's flip and spin, the body spin, grabs and manuals are drawn by viewer/hoverboard.rs from
  the cue. The one gameplay effect is the bail, which ends the ride like any dismount.
- **Deterministic.** Gestures are read from the recorded pad, field by field, so replays stay exact.
- **Controller first.** Flick-It needs an analog right stick. Keyboard and mouse get a button scheme (see "Controls").

## Settings (added to the Hoverboard mod)

| id | Name | Values | Default | Meaning |
|---|---|---|---|---|
| `tricks` | Tricks | Off / On | On | the right stick does tricks while riding |
| `bails` | Bails | Off / On | On | an unfinished trick on landing knocks him off (ragdoll if Ragdoll Ratchet is on) |
| `spin` | Spin speed | Slow / Fast | Slow | how fast the left stick spins him in the air |

## Controls

Right stick, while riding with Tricks on (the "flick" is a stick move within `FLICK_WINDOW`, about 0.2 s):

| Gesture | Trick |
|---|---|
| back, then up | ollie (pop height from the flick's speed: plain jump or high jump) |
| up, then back | nollie (same pop, drawn off the nose) |
| back, then up-left / up-right | kickflip / heelflip (which is which follows his stance: he faces the right of the path) |
| back, then left / right | pop shove-it (board turns 180 under him) |
| back, then a quarter roll to up | 360 flip / tre flip (phase 3) |
| held half back on the ground | manual (nose held half forward: nose manual) |

In the air: the left stick spins body and board (180 / 360 / 540); a held grab button grabs (button chosen in phase 2, see open
questions); a flick does a flip trick off a jump that was not popped (falling off a ledge).

Keyboard and mouse: the right-stick keys (X / Z / E / Q, binds.rs) can be read as a digital stick, so "Z then X" is an ollie and
"Z then X+Q" a kickflip. Needs a try in game: if it feels bad, Cross pops and Cross + a stick direction picks the flip.

## Camera

The right stick is the camera with the Modern camera on (modcam.rs reads `sticks[0..1]`; modcam/sticks.rs hands the game its
right stick). While riding with Tricks on the trick stick must not turn the camera: the camera follows behind the board (modcam
already eases behind his heading) and the mouse still turns it. With the Modern camera off, retail's camera gets a centred right
stick while riding. First thing to check in phase 1: what modcam needs to know (a `hoverboard::tricks_hold_stick()` it reads).

## Worker (`retail/hoverboard.rs`, or a new `retail/hoverboard_tricks.rs` it calls)

- **Gesture reader**: a pure function over the last `FLICK_WINDOW` of right-stick samples (angle, length per field) returning
  `None | Ollie { pop } | Nollie | Flip(kind) | Manual`. Kept free of `Machine` so it gets unit tests like `step` (note: the
  debug test exe is over 2 GB and Windows will not run it, CLAUDE.md "Tests"; test it in a scratch crate if that still holds).
- **Pop**: on an ollie / flip gesture, grounded, the worker asks the jump chooser hand for state 7 or 0xb (a pending-pop flag the
  hand reads, as strafe.rs does for its flips) and hands the game a Cross press that field if that is what the hand needs.
- **Trick state**: kind, fields since the pop, the drawn spin angle (from the left stick, in the air only). The board heading and
  speed stay as they are: the air carve in `field` already handles the left stick, and with Tricks on the stick spins instead.
- **Landing**: on air -> grounded, the trick is clean when the flip's progress is done (or within `LAND_SLACK`) and the spin is
  within `SPIN_SLACK` of 0 or 180 degrees (180 = he rides on fakie: drawn only, the stance turns round). Otherwise a bail.
- **Bail**: with `bails` on, the ride ends (`dismount (bail)`). With Ragdoll Ratchet on, it goes limp with the landing speed (a
  new `ragdoll::bail(impact)` entry, since today a limp starts only from a fall height, L3 or a hit); without it, the race
  fall-off clips 97 / 98 play while he gets up.
- **Cue** gains: `trick` (kind, progress 0..1), `spin` (rad), `grab`, `manual`, `fakie`.

## Renderer (`viewer/hoverboard.rs`)

- **Flips**: the board turns about its long axis (kickflip / heelflip one turn each way), about its up axis (shove-it 180), or
  both (360 flip), over `progress`. The deck mesh is ours, so this is only a matrix on its vertices.
- **Body**: the jump clip (85) crouches him; during a flip his feet lift off the deck (a small raise of the pose over the board)
  and come back down for the catch. Spins turn pose and board together about the up axis.
- **Grabs and manuals**: a held pose built from his own sequences (pick with `RAC_HOVER_DUMP`, like the stance), the board
  tilted nose-up for a manual.
- **Fakie**: the stance mirrored (turned 180 about up).

## Phases

1. **Ollie and flip tricks with bails.** The gesture reader, the pop through the jump hand, kickflip / heelflip / shove-it drawn,
   landing check, bail (dismount, ragdoll if on). The camera hold. Log lines `hoverboard: trick ...`.
   Test: pop on flat ground, pop off a ledge, flip and land clean, flip too late and bail, all on a controller.
2. **Spins, grabs, manuals, fakie.** Left-stick spins with the landing rule, a grab button, manuals. Keyboard scheme tried.
3. **Rolled flips (360 flip, tre flip, varial)** and trick names in the log (a score and combo HUD only if wanted later: it needs
   text ids and RAM outside upstream's reserved ranges, CLAUDE.md "Built-in mods").
4. **Grinding retail's rails on the board** (research): today the grind state ends the ride (`cancel_state`). Let it through, draw
   him on the board on the rail, and let him pop off it. Needs the grind states and whether retail's grind pose can be swapped.

## Open questions

- Which button grabs? In the air Square is the wrench and Circle fires; L2 / R2 strafe with the Modern controls. Candidate: R1
  (retail's crouch, unused in the air on the board).
- Does handing retail a Cross press on the pop field interfere with the double jump, or should the pop go through the jump
  chooser hand alone?
- Should a clean landing give a little speed (Skate rewards it) or leave speed alone?
