//! [PC] Ragdoll Ratchet (RAGDOLL_PLAN.md), phase 0: measuring only. Nothing here writes game memory or changes the pad.
//!
//! `RAC_RAGDOLL_LOG=1` writes `ragdoll:` lines to the run log while a level runs:
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
