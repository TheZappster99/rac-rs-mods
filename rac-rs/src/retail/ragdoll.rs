//! [PC] Ragdoll Ratchet (RAGDOLL_PLAN.md), the worker half: the built-in mod `ragdoll-ratchet` (plugins.rs) and its measuring tools.
//!
//! The mod: a big fall that no Heli-Pack, Thruster-Pack or wrench slam broke, L3 in the air, or an enemy hit (setting `hits`)
//! makes Ratchet go limp, except in the hoverboard races (`RACE_CLIPS`). Each field
//! `pre_field` tracks the fall from RAM, runs the phases and, while he is limp, hands the game a pad with only Start and the
//! camera stick (the game's Ratchet then lands and stands where he is). The phase reaches the renderer as `HeroParts.ragdoll`
//! (`current`), where viewer/ragdoll.rs draws the limp body. Nothing else is written, and with the mod off nothing runs.
//!
//! The measuring tools only read and log. `RAC_RAGDOLL_LOG=1` writes `ragdoll:` lines to the run log while a level runs:
//! - `state`: every change of the hero state, its group or the airborne flag, with the height and vertical speed. This finds the
//!   Heli-Pack hover state and confirms the wrench slam's state (try each pack and the slam, then read the states of that fall).
//! - `fall`: one line per landing after at least `MIN_AIR` airborne fields: take-off, apex and landing heights, the drop
//!   (apex - landing, the number the trigger compares), the fields in the air, the fastest fall speed, the hero's fall
//!   acceleration (median of the second differences of z over the airborne fields) and every state seen in the air.
//! - `pad`: the bits of buttons newly pressed this field, as `Machine::set_pad` gets them (press L3 and R3 to read their bits).
//! - `mode`: changes of the game mode, body form and health (the ragdoll's cancel signals).
//!
//! `RAC_RAGDOLL_LOG=2` also writes one `air` line per airborne field (z and vertical speed) for a closer look at a fall.
use crate::retail::disc::va;
use ee_oracle::Machine;
use std::sync::{Mutex, OnceLock};

const HERO_POS: u32 = 0x13f3d0;
const HERO_STATE: u32 = 0x1413d4;
/// Hero state group word (hero_movement.md section 2): 2 = fall states, 4 = jump states.
const HERO_GROUP: u32 = 0x1413dc;
/// Airborne flag (0 = grounded).
const HERO_AIRBORNE: u32 = 0x13f65e;
/// Hero body form byte: 0 Ratchet, 1 Clank, 2 Giant Clank, 3 Hologuise.
const BODY_FORM: u32 = 0x1413f4;
const HEALTH: u32 = 0x1415f8;
const GAME_MODE: u32 = 0x15f5a4;
/// Landings after fewer airborne fields than this are steps and bumps, not falls: no `fall` line.
const MIN_AIR: u32 = 10;
/// A height change bigger than this in one field is a respawn or a teleport (modcam.rs `JUMP`): the fall being tracked is dropped.
const JUMP: f32 = 8.0;

// ---- the mod --------------------------------------------------------------------------------------------------------------

/// The mod id (plugins.rs).
pub const ID: &str = "ragdoll-ratchet";

/// Pad bits of `Machine::set_pad`'s word (measured, RAGDOLL_PLAN.md "Phase 0 results").
const PAD_L3: u16 = 0x200;
const PAD_START: u16 = 0x800;
/// States that break a fall: Heli-Pack glide, Thruster-Pack, wrench slam.
const SAVE_STATES: [u32; 3] = [0x8, 0x10, 0x14];
/// Hurt states (an enemy hit, lava): retail already throws him, so that fall does not count.
const HURT_STATES: [u32; 2] = [0x16, 0x3c];
/// Ledge grab / hang: the fall starts again from the ledge.
const LEDGE_GROUP: u32 = 3;
/// Game modes that only pause the ragdoll (a menu, the map): everything else but gameplay (0) ends it.
const MENU_MODES: [u32; 2] = [1, 3];
/// An in-engine cutscene (snapshot.rs `arrived`): the scene's own Ratchet is drawn, and L3 (setting `scenes`) makes him go limp.
const SCENE_MODE: u32 = 2;
/// The get-up: long enough for the renderer's get-up clip (viewer/ragdoll.rs `GetUp`: settle into the clip's first pose, play it,
/// settle into the game's pose). The free get-up after `EDGE_FALL_STATES` is the quick plain blend: the pad is his then.
const GETUP_S: f32 = 1.4;
const GETUP_FREE_S: f32 = 0.6;
/// The hit reaction (an enemy hit; RAGDOLL_PLAN.md "Hero states").
const HIT_STATE: u32 = 0x16;
const LAVA_STATE: u32 = 0x3c;
/// Fields after a hit over which retail's knock-back is measured (his move over them, per second) before it is handed to the body.
const HIT_FIELDS: u32 = 4;
/// After a hit, no new one for this many fields: the state change and the health drop of one hit can come a field apart.
const HIT_COOL: u32 = 20;
/// The game's Ratchet falling (plain fall, tumble): after a hit this is a knock-back off a ledge, and with the pad locked he
/// fell into the pit every time (logs 2026-10-26). The ragdoll then gets up at once and the pad is his again, so he can steer.
const EDGE_FALL_STATES: [u32; 2] = [0x6, 0x77];
/// Knockdown guard (setting `guard`): small enemies in a group hit him again and again while he lay there, each hit started the
/// limp time again, and he never got up before he died (player, 2026-10-27). With the guard a hit while limp still throws the
/// body but does not start the limp time again, and no hit knocks him down during the get-up or for this long after it (retail's
/// hit still lands: he loses health and is knocked back as usual, with the pad his). 3 s: the hits in that log came 2.5 to 3.3 s
/// apart, so a 2 s guard ended before the next one.
const GUARD_S: f32 = 3.0;
/// With the guard, a knockdown by a hit lies only this long (the Limp time setting is for falls), and the pad is his again as
/// soon as the get-up starts. Second test (2026-10-27): hits came every 1.7 s against his 4 health, so the 3 s locked down took
/// two of them and he got up with 1 health; the player chose control during the get-up over a locked get-up.
const HIT_LIMP_S: f32 = 0.5;

/// The hero moby (aim.rs `HERO_MOBY`); its +0x53 is the sequence slot it plays.
const HERO_MOBY: u32 = 0x1413d0;
/// Ratchet's own hoverboard race clips (viewer/hoverboard.rs: ride, carves, jump, falling off). Every level but the race
/// levels (and level 0) leaves these slots empty, so the hero playing one is riding a race. The ragdoll stays off then: the
/// ramps are big drops and the race's crashes are retail's own (player, 2026-10-30: "it makes it really difficult").
const RACE_CLIPS: [u8; 6] = [82, 85, 86, 97, 98, 104];
/// The race counts on for this long after the last race clip, so a field or two in another clip (a hit) does not let a fall start.
const RACE_HOLD_S: f32 = 1.0;

/// States that end a ragdoll at once: water, mud, the Swingshot, the ship / turret and the grind.
fn cancel_state(s: u32) -> bool {
	matches!(s, 0x33..=0x37 | 0x6a | 0x75 | 0x76 | 0x68 | 0x7b | 0x24..=0x26 | 0x2c | 0x32 | 0x74)
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Phase {
	#[default]
	Off,
	/// Limp in the air (L3); becomes `Ground` on the landing.
	Fall,
	/// Limp on the ground for the limp time.
	Ground,
	/// Blending back to the game's pose (`Cue::blend`).
	GetUp,
}

/// What the renderer needs of the ragdoll this field.
#[derive(Clone, Copy, Default, Debug)]
pub struct Cue {
	/// One per ragdoll: a new id starts a new body.
	pub id: u32,
	pub phase: Phase,
	/// Get-up progress, 0 = all ragdoll, 1 = all the game's pose.
	pub blend: f32,
	/// A menu is up: the body holds still.
	pub frozen: bool,
	/// Floppiness setting (0 Stiff, 1 Loose, 2 Noodle).
	pub flop: u8,
	/// Bounciness setting (0 Off, 1 Some, 2 Lots).
	pub bounce: u8,
	/// A ragdoll started by a landing: his speed in the last field in the air (units/s, world). By the field the landing is seen,
	/// retail has already stopped him, so the drawn pose alone starts the body at rest: no impact, no bounce.
	pub impact: Option<[f32; 3]>,
	/// The latest enemy hit's push: (one id per hit, retail's knock-back speed in units/s, world). The renderer adds it to the body
	/// once per id. It comes `HIT_FIELDS` after the hit, since retail only starts throwing him then.
	pub push: Option<(u32, [f32; 3])>,
	/// The ragdoll started from a hit: the body starts at rest. The drawn pose jumps into retail's hit reaction that field, and
	/// the body read that jump as a launch of 10-12 units/s up on top of the push (logs 2026-10-26: pelvis 4.5 above him).
	pub from_hit: bool,
	/// The quick get-up with the pad free (`EDGE_FALL_STATES`): the renderer blends straight to the game's pose, no clip.
	pub free: bool,
	/// A ragdoll in a cutscene: the scene owns the camera, so the camera does not follow the body.
	pub scene: bool,
	/// Counts the fields the worker ran the mod for (not the menus): the renderer steps the body once per new value. Its own field
	/// count runs on in the pause menu, where this cue is not renewed, and the body went on falling and settling behind the menu.
	pub tick: u32,
}

/// The fall being tracked.
#[derive(Clone, Copy)]
struct Track {
	apex: f32,
	/// False once a save or hurt state was seen.
	counts: bool,
	fields: u32,
	/// His speed in the latest airborne field.
	vel: [f32; 3],
}

#[derive(Default)]
struct Mod {
	/// This field's pad, as `set_pad` got it (`note_pad`).
	buttons: u16,
	sticks: [u8; 4],
	last_buttons: u16,
	last_pos: Option<[f32; 3]>,
	air: Option<Track>,
	/// The impact speed of the ragdoll running now (`Cue::impact`).
	impact: Option<[f32; 3]>,
	tick: u32,
	/// The previous field's hero state and health (the hit signals).
	last_state: u32,
	last_health: i32,
	/// A hit being measured: fields since it, his position at it.
	hit: Option<(u32, [f32; 3])>,
	hit_cool: u32,
	/// Knockdown guard: fields left in which a hit cannot knock him down (`GUARD_S` after a get-up).
	guard: u32,
	push_id: u32,
	push: Option<(u32, [f32; 3])>,
	from_hit: bool,
	/// The get-up after `EDGE_FALL_STATES`: the pad is not locked.
	free: bool,
	/// This ragdoll started in a cutscene (`SCENE_MODE`); it ends when the scene does.
	scene: bool,
	phase: Phase,
	/// Fields in the phase.
	t: u32,
	/// Fields this ragdoll has lain on the ground (the limp time). Not `t`: a hit's knock-back lifts the game's Ratchet off the
	/// ground for a moment, Ground -> Fall -> Ground, and restarting the count there kept him down under every hit, guard or not.
	down: u32,
	/// Fields left in which he counts as in a hoverboard race (`RACE_CLIPS`): no ragdoll at all.
	race: u32,
	id: u32,
	cue: Cue,
}

static MOD: Mutex<Option<Mod>> = Mutex::new(None);

fn setting(key: &str) -> u8 {
	crate::plugins::value_at_start(ID, key)
}

/// Worker, each gameplay field before the built-in mods run: the pad `set_pad` just got (the mod's `pre_field` has no `FieldIn`).
pub fn note_pad(buttons: u16, sticks: [u8; 4]) {
	if !crate::plugins::on_at_start(ID) {
		return;
	}
	let mut g = MOD.lock().unwrap();
	let s = g.get_or_insert_with(Mod::default);
	s.buttons = buttons;
	s.sticks = sticks;
}

/// The ragdoll of the field that just ran (retail/snapshot.rs puts it in `HeroParts`); `Off` with the mod off.
pub fn current() -> Cue {
	MOD.lock().ok().and_then(|g| g.as_ref().map(|s| s.cue)).unwrap_or_default()
}

/// The mod's `pre_field` (plugins.rs): the phases, and the pad lock while limp.
pub fn pre_field(m: &mut Machine) {
	let dt = super::disc::field_dt();
	let height = [6.0, 8.0, 12.0][setting("height").min(2) as usize];
	let button = setting("button") == 1;
	// Limp time: halved on 2026-10-27 (was 1.5 / 3 / 5 s) once the get-up clips came in: the player wanted him up sooner.
	let limp = ([0.75, 1.5, 2.5][setting("time").min(2) as usize] / dt).round() as u32;
	let getup = |free: bool| ((if free { GETUP_FREE_S } else { GETUP_S }) / dt).round() as u32;
	let flop = setting("flop").min(2);
	let bounce = setting("bounce").min(2);
	let hits = setting("hits") == 1;
	let guard = setting("guard") == 1;
	let scenes = setting("scenes") == 1;

	let mut g = MOD.lock().unwrap();
	let s = g.get_or_insert_with(Mod::default);
	let pressed = s.buttons & !s.last_buttons;
	s.last_buttons = s.buttons;

	let mode = m.rd32(va(GAME_MODE));
	let scene = mode == SCENE_MODE;
	let form = m.rd8(BODY_FORM);
	let health = m.rd32(HEALTH) as i32;
	let state = m.rd32(HERO_STATE);
	let group = m.rd32(HERO_GROUP);
	let airborne = m.rd16(HERO_AIRBORNE) != 0;
	let pos = m.rdv3(HERO_POS);
	let z = pos[2];
	let moved = s.last_pos.map(|p0| [pos[0] - p0[0], pos[1] - p0[1], pos[2] - p0[2]]);
	let teleport = moved.is_some_and(|d| d.iter().any(|v| v.abs() > JUMP));
	let vel = moved.filter(|_| !teleport).map_or([0.0; 3], |d| d.map(|v| v / dt));
	s.last_pos = Some(pos);
	let hero = m.rd32(HERO_MOBY) & 0x01ff_ffff;
	let race_clip = hero != 0 && RACE_CLIPS.contains(&m.rd8(hero + 0x53));
	let was_race = s.race > 0;
	s.race = if race_clip { (RACE_HOLD_S / dt).round() as u32 } else { s.race.saturating_sub(1) };
	if was_race != (s.race > 0) && log_on() {
		eprintln!("ragdoll: hoverboard race {}", if was_race { "over" } else { "seen: ragdoll off" });
	}
	let old = s.phase;
	let mut restart = false;

	if MENU_MODES.contains(&mode) && s.phase != Phase::Off {
		// A menu: hold everything as it is (the menu needs its buttons, so no lock).
		s.cue.frozen = true;
		return;
	}
	s.tick = s.tick.wrapping_add(1);
	// An enemy hit: the hit reaction starting, or health going down anywhere but in lava (the hit reaction can show a field late).
	let hit = hits
		&& !scene
		&& s.hit_cool == 0
		&& ((state == HIT_STATE && s.last_state != HIT_STATE) || (health < s.last_health && state != LAVA_STATE));
	let shielded = guard && (s.guard > 0 || s.phase == Phase::GetUp);
	if hit && shielded && log_on() {
		eprintln!("ragdoll: hit guarded (state {state:#x}, health {health}) in {:?}", s.phase);
	}
	let hit = hit && !shielded;
	s.hit_cool = s.hit_cool.saturating_sub(1);
	s.guard = s.guard.saturating_sub(1);
	s.last_state = state;
	s.last_health = health;
	if scene {
		// An in-engine cutscene: the scene spawns its own Ratchet and plays him (snapshot.rs), and the renderer's ragdoll replaces
		// that drawn pose like the hero's. L3 makes him go limp for the limp time; then he blends straight back into the scene's
		// pose (`free`: he may be sitting or mid-gesture, so no get-up clip). The placed hero's state and position mean nothing here.
		if s.phase != Phase::Off && !s.scene {
			// A ragdoll from gameplay when the scene starts: the scene's Ratchet takes over the picture.
			s.phase = Phase::Off;
			s.air = None;
			s.hit = None;
		}
		match s.phase {
			Phase::Off => {
				if scenes && button && pressed & PAD_L3 != 0 {
					s.phase = Phase::Ground;
					s.id = s.id.wrapping_add(1);
					s.air = None;
					s.impact = None;
					s.from_hit = false;
					s.free = true;
					s.scene = true;
				}
			}
			Phase::Fall | Phase::Ground => {
				if s.down >= limp {
					s.phase = Phase::GetUp;
				}
			}
			Phase::GetUp => {
				if s.t >= getup(true) {
					s.phase = Phase::Off;
				}
			}
		}
	} else if mode != 0 || s.scene || form != 0 || health <= 0 || teleport || cancel_state(state) || s.race > 0 {
		s.phase = Phase::Off;
		s.air = None;
		s.hit = None;
	} else if hit {
		// Knocked down: limp from this field (he lies down where retail throws him); a hit while limp starts the limp time again.
		// The push follows once retail's knock-back can be measured.
		s.hit = Some((0, pos));
		s.hit_cool = HIT_COOL;
		match s.phase {
			Phase::Off => {
				s.phase = if airborne { Phase::Fall } else { Phase::Ground };
				s.id = s.id.wrapping_add(1);
				s.air = None;
				s.impact = None;
				s.from_hit = true;
			}
			Phase::Ground => restart = !guard,
			Phase::GetUp => {
				s.phase = Phase::Ground;
				restart = true;
			}
			Phase::Fall => restart = !guard,
		}
		if log_on() {
			eprintln!("ragdoll: hit (state {state:#x}, health {health}, airborne {}) in {:?}", airborne as u8, old);
		}
	} else {
		match s.phase {
			Phase::Off => {
				if airborne {
					let tr = s.air.get_or_insert(Track { apex: z, counts: true, fields: 0, vel });
					tr.fields += 1;
					tr.vel = vel;
					tr.apex = if group == LEDGE_GROUP { z } else { tr.apex.max(z) };
					if SAVE_STATES.contains(&state) || HURT_STATES.contains(&state) {
						tr.counts = false;
					}
					if button && pressed & PAD_L3 != 0 && tr.fields >= 2 && !SAVE_STATES.contains(&state) {
						s.phase = Phase::Fall;
					}
				} else if let Some(tr) = s.air.take() {
					if tr.counts && tr.apex - z >= height {
						s.phase = Phase::Ground;
						s.impact = Some(tr.vel);
					}
					if log_on() {
						eprintln!("ragdoll: landed drop {:.2} counts {} -> {}", tr.apex - z, tr.counts, if s.phase == Phase::Ground { "limp" } else { "no ragdoll" });
					}
				}
				if s.phase == Phase::Fall {
					// Limp in the air: the drawn pose is still moving, the body takes its speed from it.
					s.impact = None;
				}
				if s.phase != Phase::Off {
					s.id = s.id.wrapping_add(1);
					s.air = None;
					s.from_hit = false;
				}
			}
			Phase::Fall | Phase::Ground if s.from_hit && EDGE_FALL_STATES.contains(&state) => {
				s.phase = Phase::GetUp;
				s.free = true;
			}
			Phase::Fall => {
				if !airborne {
					s.phase = Phase::Ground;
				}
			}
			Phase::Ground => {
				if airborne {
					// Slid off a ledge while limp: limp in the air again.
					s.phase = Phase::Fall;
				} else if s.down >= if guard && s.from_hit { (HIT_LIMP_S / dt).round() as u32 } else { limp } {
					s.phase = Phase::GetUp;
				}
			}
			Phase::GetUp => {
				if s.t >= getup(s.free) {
					s.phase = Phase::Off;
				}
			}
		}
	}
	if let Some((n, p0)) = s.hit.as_mut() {
		*n += 1;
		if *n >= HIT_FIELDS {
			let v = [0, 1, 2].map(|k| (pos[k] - p0[k]) / (*n as f32 * dt));
			s.push_id = s.push_id.wrapping_add(1);
			s.push = Some((s.push_id, v));
			s.hit = None;
			if log_on() {
				eprintln!("ragdoll: hit push {:.2} {:.2} {:.2}", v[0], v[1], v[2]);
			}
		}
	}
	if s.phase == Phase::Off {
		s.push = None;
		s.free = false;
		s.scene = false;
	}
	if restart {
		s.t = 0;
		s.down = 0;
	}
	if s.phase != old {
		s.t = 0;
		if old == Phase::Off {
			s.down = 0;
		}
		if old == Phase::GetUp && s.phase == Phase::Off {
			s.guard = (GUARD_S / dt).round() as u32;
		}
		if log_on() {
			eprintln!("ragdoll: phase {old:?} -> {:?} (id {}, state {state:#x}, mode {mode}, z {z:.2})", s.phase, s.id);
		}
	}
	s.t += 1;
	if s.phase == Phase::Ground {
		s.down += 1;
	}
	let hit_getup = guard && s.from_hit && s.phase == Phase::GetUp;
	// In a cutscene the scene owns the pad (`free`), so nothing is locked.
	if s.phase != Phase::Off && !s.free && !hit_getup {
		// Only Start (pause) and the camera stick reach the game while he is limp.
		m.set_pad(s.buttons & PAD_START, [s.sticks[0], s.sticks[1], 0x80, 0x80]);
	}
	let blend = if s.phase == Phase::GetUp { (s.t as f32 / getup(s.free).max(1) as f32).min(1.0) } else { 0.0 };
	s.cue = Cue { id: s.id, phase: s.phase, blend, frozen: false, flop, bounce, impact: s.impact, push: s.push, from_hit: s.from_hit, free: s.free, scene: s.scene, tick: s.tick };
}

// ---- the measuring tools ----------------------------------------------------------------------------------------------------

/// 0 = off, 1 = state / fall / pad / mode lines, 2 = also a line per airborne field.
fn level() -> u8 {
	static L: OnceLock<u8> = OnceLock::new();
	*L.get_or_init(|| std::env::var("RAC_RAGDOLL_LOG").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0))
}

pub fn log_on() -> bool {
	level() > 0
}

/// One fall being tracked, from the first airborne field to the landing.
struct Air {
	start_field: u32,
	takeoff_z: f32,
	apex_z: f32,
	/// Fastest downward speed (units per second, positive = down).
	max_down: f32,
	/// z of every airborne field, for the acceleration.
	zs: Vec<f32>,
	/// The hero states seen in the air, in order of first appearance.
	states: Vec<u32>,
}

#[derive(Default)]
struct Log {
	/// The previous field's (state, group, airborne), None before the first gameplay field.
	last: Option<(u32, u32, bool)>,
	last_z: Option<f32>,
	last_buttons: u16,
	/// (game mode, body form, health) of the previous field.
	last_mode: Option<(u32, u8, i32)>,
	air: Option<Air>,
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);

/// Worker, each gameplay field before it runs (after the pad is set). `buttons` = what `set_pad` got this field.
pub fn log_field(m: &Machine, field: u32, buttons: u16) {
	if !log_on() {
		return;
	}
	let dt = super::disc::field_dt();
	let mut g = LOG.lock().unwrap();
	let s = g.get_or_insert_with(Log::default);

	let new = buttons & !s.last_buttons;
	if new != 0 {
		eprintln!("ragdoll: pad field {field} pressed {new:#06x} (held {buttons:#06x})");
	}
	s.last_buttons = buttons;

	let mode = (m.rd32(va(GAME_MODE)), m.rd8(BODY_FORM), m.rd32(HEALTH) as i32);
	if s.last_mode != Some(mode) {
		eprintln!("ragdoll: mode field {field} game mode {} body form {} health {}", mode.0, mode.1, mode.2);
		s.last_mode = Some(mode);
	}

	let z = m.rdf(HERO_POS + 8);
	let state = m.rd32(HERO_STATE);
	let group = m.rd32(HERO_GROUP);
	let airborne = m.rd16(HERO_AIRBORNE) != 0;
	let dz = s.last_z.map(|z0| z - z0);
	let teleport = dz.is_some_and(|d| d.abs() > JUMP);
	let vz = dz.filter(|_| !teleport).map_or(0.0, |d| d / dt);
	s.last_z = Some(z);
	if teleport {
		eprintln!("ragdoll: jump field {field} z moved {:.2} in one field (respawn / teleport)", dz.unwrap_or(0.0));
		s.air = None;
	}

	let now = (state, group, airborne);
	if s.last != Some(now) {
		eprintln!("ragdoll: state field {field} state {state:#x} group {group} airborne {} z {z:.2} vz {vz:.2}", airborne as u8);
		s.last = Some(now);
	}

	if airborne {
		let a = s.air.get_or_insert_with(|| Air { start_field: field, takeoff_z: z, apex_z: z, max_down: 0.0, zs: Vec::new(), states: Vec::new() });
		a.apex_z = a.apex_z.max(z);
		a.max_down = a.max_down.max(-vz);
		a.zs.push(z);
		if !a.states.contains(&state) {
			a.states.push(state);
		}
		if level() >= 2 {
			eprintln!("ragdoll: air field {field} state {state:#x} z {z:.3} vz {vz:.2}");
		}
	} else if let Some(a) = s.air.take() {
		let fields = field - a.start_field;
		if fields >= MIN_AIR {
			let states: Vec<String> = a.states.iter().map(|s| format!("{s:#x}")).collect();
			let accel = median_accel(&a.zs, dt).map_or("-".to_string(), |g| format!("{g:.2}"));
			eprintln!(
				"ragdoll: fall field {field} takeoff {:.2} apex {:.2} land {z:.2} drop {:.2} air {fields} fields ({:.2} s) max fall speed {:.2} accel {accel} states [{}]",
				a.takeoff_z,
				a.apex_z,
				a.apex_z - z,
				fields as f32 * dt,
				a.max_down,
				states.join(" ")
			);
		}
	}
}

/// Joints the code already names (RAGDOLL_PLAN.md "Bone mapping"), shown next to their dump line.
fn known(j: usize) -> &'static str {
	match j {
		0 => "root",
		1 => "pelvis (strafe pivot)",
		2..=4 => "spine (strafe twist)",
		15..=21 => "eyelid",
		54 => "glove / gun hand",
		56 => "wrench wrist",
		92..=97 => "leg A",
		98..=103 => "leg B",
		104..=110 => "lower body extra",
		_ => "",
	}
}

/// `RAC_RAGDOLL_DUMP=1`: the hero skeleton, once per run, when the first level with Ratchet loads. Read from the disc at run
/// time, written only to the run log (never commit a copy, it is derived from disc bytes). Lines:
/// - `joint`: index, parent, depth, children, bind position (game units, z up), bone length to the parent, the vertices whose
///   largest weight is on this joint, their farthest distance from the joint (a capsule radius guess) and their box around the
///   joint, the attach points and attach slots on this joint, and the name the code already gives it;
/// - `chain`: the root-to-leaf path of every leaf joint, to read the arms, neck, ears and tail off;
/// - `model`: the bind mesh's box (the hero's size in game units).
pub fn dump_skeleton(level: usize, class: &crate::mobies::MobyClass, a: &crate::anim::Animated) {
	static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
	if !std::env::var("RAC_RAGDOLL_DUMP").is_ok_and(|v| v == "1") || DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
		return;
	}
	let sk = &a.skeleton;
	let n = sk.parents.len();
	let pos: Vec<[f32; 3]> = (0..n).map(|j| sk.bind_world(j)).map(|m| [m[0][3], m[1][3], m[2][3]]).collect();
	let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
	for (j, p) in sk.parents.iter().enumerate() {
		if let Some(p) = *p {
			children[p].push(j);
		}
	}
	let depth = |mut j: usize| {
		let mut d = 0;
		while let Some(p) = sk.parents[j] {
			j = p;
			d += 1;
		}
		d
	};
	// Each vertex goes to the joint of its largest weight.
	let mut verts: Vec<Vec<[f32; 3]>> = vec![Vec::new(); n];
	let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
	for (p, s) in class.mesh.positions.iter().zip(&class.skin) {
		for k in 0..3 {
			lo[k] = lo[k].min(p[k]);
			hi[k] = hi[k].max(p[k]);
		}
		let k = (0..3).max_by(|&x, &y| s.weights[x].total_cmp(&s.weights[y])).unwrap();
		if let Some(v) = verts.get_mut(s.joints[k] as usize) {
			v.push(*p);
		}
	}
	eprintln!("ragdoll: dump level {level}: {n} joints, {} vertices, {} sequences", class.mesh.positions.len(), a.sequences.len());
	eprintln!("ragdoll: model box x {:.3}..{:.3} y {:.3}..{:.3} z {:.3}..{:.3}", lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]);
	for j in 0..n {
		let p = pos[j];
		let len = sk.parents[j].map_or(0.0, |q| crate::physics::length(crate::physics::sub(p, pos[q])));
		let vs = &verts[j];
		let reach = vs.iter().map(|v| crate::physics::length(crate::physics::sub(*v, p))).fold(0.0f32, f32::max);
		let (mut vlo, mut vhi) = ([0.0f32; 3], [0.0f32; 3]);
		for v in vs {
			for k in 0..3 {
				vlo[k] = vlo[k].min(v[k] - p[k]);
				vhi[k] = vhi[k].max(v[k] - p[k]);
			}
		}
		let points: Vec<String> = a.attach_points.iter().enumerate().filter(|(_, x)| **x == Some(j)).map(|(i, _)| i.to_string()).collect();
		let slots: Vec<String> = a.slot_map.iter().enumerate().filter(|(_, x)| **x == Some(j)).map(|(i, _)| i.to_string()).collect();
		let kids: Vec<String> = children[j].iter().map(|c| c.to_string()).collect();
		eprintln!(
			"ragdoll: joint {j} parent {} depth {} children [{}] at {:.3} {:.3} {:.3} len {len:.3} verts {} reach {reach:.3} box {:.3}..{:.3} {:.3}..{:.3} {:.3}..{:.3} attach [{}] slots [{}] {}",
			sk.parents[j].map_or("-".to_string(), |q| q.to_string()),
			depth(j),
			kids.join(" "),
			p[0],
			p[1],
			p[2],
			vs.len(),
			vlo[0],
			vhi[0],
			vlo[1],
			vhi[1],
			vlo[2],
			vhi[2],
			points.join(" "),
			slots.join(" "),
			known(j)
		);
	}
	for leaf in (0..n).filter(|&j| children[j].is_empty()) {
		let mut path = vec![leaf];
		while let Some(p) = sk.parents[*path.last().unwrap()] {
			path.push(p);
		}
		let path: Vec<String> = path.iter().rev().map(|j| j.to_string()).collect();
		eprintln!("ragdoll: chain {}", path.join(" > "));
	}
	dump_clips(a);
}

/// `seq` lines of the dump (phase 5, the get-up clip): every hero sequence sampled at 0, 1/4, 1/2, 3/4 and the end of one loop,
/// with the pelvis position and how far the head stands above the pelvis (about 0.6 standing, near 0 lying). A get-up clip
/// starts low and ends standing; `lying` marks the sequences that start with the head under 0.25 above the pelvis.
fn dump_clips(a: &crate::anim::Animated) {
	let n = a.skeleton.parents.len();
	if n <= 8 {
		return;
	}
	for (i, s) in a.sequences.iter().enumerate().filter(|(_, s)| !s.frames.is_empty()) {
		let total = s.tick_len().max(1.0);
		let mut parts = Vec::new();
		let mut ups = Vec::new();
		for k in 0..5 {
			let t = (total * k as f32 / 4.0).min(total - 0.01);
			let mats = a.pose_after(i, t);
			let w = |j: usize| {
				let m = a.joint_world(&mats, j);
				[m[3], m[7], m[11]]
			};
			let (pelvis, head) = (w(1), w(8));
			let up = head[2] - pelvis[2];
			ups.push(up);
			parts.push(format!("{:.2} {:.2} {:.2} up {up:.2}", pelvis[0], pelvis[1], pelvis[2]));
		}
		let tag = if ups[0] < 0.25 && ups[4] > 0.45 { " lying -> standing" } else if ups[0] < 0.25 { " lying" } else { "" };
		eprintln!("ragdoll: seq {i} frames {} ticks {total:.0} | {}{tag}", s.frames.len(), parts.join(" | "));
		// The clips that go down to the ground somewhere: every frame, to find a stretch from lying to standing. `fwd` is the
		// height of the pelvis's forward axis (model x): -1 face down, +1 face up.
		if ups.iter().any(|u| *u < 0.15) {
			let frames: Vec<String> = s.frames.iter().map(|f| {
				let mats = a.pose_single(f);
				let pm = a.joint_world(&mats, 1);
				let hm = a.joint_world(&mats, 8);
				format!("{:.2}/{:.2}/{:+.1}", hm[11] - pm[11], pm[11], pm[8])
			}).collect();
			eprintln!("ragdoll: seq {i} frames up/pelvis/fwd {}", frames.join(" "));
		}
	}
}

/// The median of the second differences of `zs` (units per second squared, negative = down): the fall acceleration with the
/// take-off and landing fields and any hover in between outvoted by the plain falling fields.
fn median_accel(zs: &[f32], dt: f32) -> Option<f32> {
	let mut d2: Vec<f32> = zs.windows(3).map(|w| (w[2] - 2.0 * w[1] + w[0]) / (dt * dt)).collect();
	if d2.is_empty() {
		return None;
	}
	d2.sort_by(f32::total_cmp);
	Some(d2[d2.len() / 2])
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn median_accel_of_a_parabola() {
		// z = -g t^2 / 2 with g = 20, sampled every 0.02 s.
		let zs: Vec<f32> = (0..30).map(|i| -10.0 * (i as f32 * 0.02).powi(2)).collect();
		assert!((median_accel(&zs, 0.02).unwrap() + 20.0).abs() < 0.01);
		assert_eq!(median_accel(&zs[..2], 0.02), None);
	}
}
