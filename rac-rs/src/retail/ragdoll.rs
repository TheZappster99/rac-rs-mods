//! [PC] Ragdoll Ratchet (RAGDOLL_PLAN.md), the worker half: the built-in mod `ragdoll-ratchet` (plugins.rs) and its measuring tools.
//!
//! The mod: a big fall that no Heli-Pack, Thruster-Pack or wrench slam broke, or L3 in the air, makes Ratchet go limp. Each field
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
const GETUP_S: f32 = 0.6;

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
	phase: Phase,
	/// Fields in the phase.
	t: u32,
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
	let limp = ([1.5, 3.0, 5.0][setting("time").min(2) as usize] / dt).round() as u32;
	let getup = (GETUP_S / dt).round() as u32;
	let flop = setting("flop").min(2);
	let bounce = setting("bounce").min(2);

	let mut g = MOD.lock().unwrap();
	let s = g.get_or_insert_with(Mod::default);
	let pressed = s.buttons & !s.last_buttons;
	s.last_buttons = s.buttons;

	let mode = m.rd32(va(GAME_MODE));
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
	let old = s.phase;

	if MENU_MODES.contains(&mode) && s.phase != Phase::Off {
		// A menu: hold everything as it is (the menu needs its buttons, so no lock).
		s.cue.frozen = true;
		return;
	}
	s.tick = s.tick.wrapping_add(1);
	if mode != 0 || form != 0 || health <= 0 || teleport || cancel_state(state) {
		s.phase = Phase::Off;
		s.air = None;
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
				}
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
				} else if s.t >= limp {
					s.phase = Phase::GetUp;
				}
			}
			Phase::GetUp => {
				if s.t >= getup {
					s.phase = Phase::Off;
				}
			}
		}
	}
	if s.phase != old {
		s.t = 0;
		if log_on() {
			eprintln!("ragdoll: phase {old:?} -> {:?} (id {}, state {state:#x}, mode {mode}, z {z:.2})", s.phase, s.id);
		}
	}
	s.t += 1;
	if s.phase != Phase::Off {
		// Only Start (pause) and the camera stick reach the game while he is limp.
		m.set_pad(s.buttons & PAD_START, [s.sticks[0], s.sticks[1], 0x80, 0x80]);
	}
	let blend = if s.phase == Phase::GetUp { (s.t as f32 / getup.max(1) as f32).min(1.0) } else { 0.0 };
	s.cue = Cue { id: s.id, phase: s.phase, blend, frozen: false, flop, bounce, impact: s.impact, tick: s.tick };
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
