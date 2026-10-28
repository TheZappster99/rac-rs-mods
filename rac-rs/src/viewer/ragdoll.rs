//! [PC] Ragdoll Ratchet, the renderer half (RAGDOLL_PLAN.md; the worker half and the mod are retail/ragdoll.rs): while the game's
//! Ratchet is limp, his drawn pose comes from a small rigid-body ragdoll instead of the animation. Picture only: nothing here reads
//! or writes game RAM, and the game's own Ratchet (position, collision, camera) carries on as retail moves him.
//!
//! - Bodies: 11 rigid bodies on the main bones (`DEFS`), started from the drawn pose of the tick and moving with the motion of the
//!   last tick, so a fall keeps its speed and a flip keeps spinning. Collision spheres along each bone (plus the hands and toes).
//! - Solver: XPBD rigid bodies (Mueller et al. 2020, "Detailed Rigid Body Simulation with Extended Position Based Dynamics"): ball
//!   joints with a swing cone and a twist limit centred on the pose he went limp in, a soft drive back toward that pose
//!   (Floppiness), sphere contacts against the level collision with friction.
//! - The body stays near the game's Ratchet: the pelvis is pulled sideways toward him (hard limit `TETHER_MAX`), and where he stands
//!   on something the level collision does not have (a moving platform, a crate) his feet height is a floor.
//! - It steps once per game field, never per display frame: a pause or a menu holds it, and a cached hero pose equals a fresh one.
//! - Pose: a driven joint takes its body's frame; every other joint follows its parent with its animated local transform (fingers,
//!   face, ears, tail, and so Clank and the held items). Skin = new world x old world^-1 x old skin.
//!
//! `RAC_RAGDOLL_TUNE=<file>`: `key=value` lines (see `Tune`), re-read while the game runs.
use crate::physics::{add, cross, dot, length, normalize, scale, sub, World, V3};
use crate::retail::ragdoll::{Cue, Phase};

type Q = [f32; 4];
type M34 = [f32; 12];

/// One body: the joints it drives, the bone it lies along (joint a -> joint b), its radius and mass, and an extra collision
/// sphere (a hand, a toe).
struct Def {
	drive: &'static [usize],
	seg: (usize, usize),
	r: f32,
	mass: f32,
	extra: Option<(usize, f32)>,
}

/// The hero's bodies (joint numbers: RAGDOLL_PLAN.md "Hero skeleton"; the right side is negative y). Masses: the limbs are
/// heavier than their size (first tests with 0.4 to 1.0 against a 3.0 torso: the light limbs flailed at every touch).
const DEFS: [Def; 11] = [
	Def { drive: &[1], seg: (1, 3), r: 0.13, mass: 3.0, extra: None },
	Def { drive: &[5], seg: (4, 6), r: 0.15, mass: 3.5, extra: None },
	Def { drive: &[8], seg: (8, 18), r: 0.17, mass: 2.0, extra: None },
	Def { drive: &[53], seg: (53, 54), r: 0.06, mass: 1.0, extra: None },
	Def { drive: &[54, 56], seg: (54, 56), r: 0.055, mass: 0.9, extra: Some((56, 0.08)) },
	Def { drive: &[73], seg: (73, 74), r: 0.06, mass: 1.0, extra: None },
	Def { drive: &[74, 76], seg: (74, 76), r: 0.055, mass: 0.9, extra: Some((76, 0.08)) },
	Def { drive: &[92], seg: (92, 93), r: 0.09, mass: 1.8, extra: None },
	Def { drive: &[93, 94], seg: (93, 94), r: 0.07, mass: 1.4, extra: Some((96, 0.08)) },
	Def { drive: &[98], seg: (98, 99), r: 0.09, mass: 1.8, extra: None },
	Def { drive: &[99, 100], seg: (99, 100), r: 0.07, mass: 1.4, extra: Some((102, 0.08)) },
];

/// The joints between the bodies: (parent body, child body, joint at the pivot, swing cone degrees, twist degrees).
const JOINTS: [(usize, usize, usize, f32, f32); 10] = [
	(0, 1, 4, 30.0, 20.0),
	(1, 2, 7, 40.0, 45.0),
	(1, 3, 53, 80.0, 45.0),
	(3, 4, 54, 70.0, 15.0),
	(1, 5, 73, 80.0, 45.0),
	(5, 6, 74, 70.0, 15.0),
	(0, 7, 92, 60.0, 25.0),
	(7, 8, 93, 65.0, 10.0),
	(0, 9, 98, 60.0, 25.0),
	(9, 10, 99, 65.0, 10.0),
];

/// `RAC_RAGDOLL_LOG`: set with each `where` line, so the next `Smooth::attach` says what it carried onto the body.
pub(super) static LIMP_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The hero model's joint count (the only skeleton this drives).
const HERO_JOINTS: usize = 111;
/// The pelvis never gets further than this (sideways) from the game's Ratchet.
const TETHER_MAX: f32 = 1.0;
/// Speeds the start motion is capped at (a respawn between two ticks is not a launch).
const MAX_V: f32 = 40.0;
const MAX_W: f32 = 30.0;

/// Tuning (`RAC_RAGDOLL_TUNE`).
#[derive(Clone, Copy)]
struct Tune {
	gravity: f32,
	substeps: u32,
	/// Linear / angular damping per second.
	damp: f32,
	adamp: f32,
	/// Share of the sliding speed a contact takes away per substep.
	friction: f32,
	/// Drive compliance per Floppiness (Stiff, Loose; Noodle has none): bigger = weaker.
	drive: [f32; 2],
	/// Joint limit scale per Floppiness (Stiff, Loose, Noodle).
	limits: [f32; 3],
	/// Sideways pull toward the game's Ratchet, per second.
	tether: f32,
	/// Share of the speed into the ground a contact gives back per Bounciness (Off, Some, Lots).
	bounce: [f32; 3],
	/// An enemy hit's push: retail's knock-back speed x `hitpush`, plus `hitlift` units/s up so he tips over instead of sliding.
	hitpush: f32,
	hitlift: f32,
	/// The pelvis's leash after a hit (units, instead of `TETHER_MAX`): the push throws the body further than retail throws him.
	hitleash: f32,
	/// The get-up clips' frame ranges (first, last; last < first plays it backwards): face down `crouch` (seq `CROUCH_SEQ`),
	/// face up `kipup` (seq `KIPUP_SEQ`).
	crouch: [f32; 2],
	kipup: [f32; 2],
	/// Where the clip starts and ends in the get-up (shares of `Cue::blend`): before it the limp pose settles into the clip's
	/// first frame, after it the clip's last frame settles into the game's pose.
	getup: [f32; 2],
}

/// Defaults. Second round (2026-10-26, "a bit spastic when hitting the ground"): more damping, a firmer drive and narrower
/// Loose limits than the first (damp 0.4, adamp 1.5, drive 0.05 / 0.5, limits 0.6 / 1.0 / 1.6, 8 substeps).
const TUNE: Tune = Tune { gravity: 24.0, substeps: 10, damp: 0.8, adamp: 4.0, friction: 0.3, drive: [0.02, 0.2], limits: [0.6, 0.85, 1.4], tether: 2.0, bounce: [0.0, 0.35, 0.6], hitpush: 2.5, hitlift: 5.0, hitleash: 6.0, crouch: [5.0, 14.0], kipup: [14.0, 0.0], getup: [0.25, 0.85] };

/// See `capsule_ii`.
const INERTIA: f32 = 2.5;
/// A single part's own bounce, as a share of the body's `bounce`: the whole-body bounce carries the hop, the parts touching
/// down one by one at full bounce made the limbs chatter.
const PART_BOUNCE: f32 = 0.3;
/// Speed caps during the simulation (units/s, rad/s): one bad push cannot fling a part.
const STEP_MAX_V: f32 = 30.0;
const STEP_MAX_W: f32 = 18.0;
/// The body rests (stops simulating) after this many fields with every part slower than `REST_V` / `REST_W`.
const REST_FIELDS: u32 = 15;
const REST_V: f32 = 0.25;
const REST_W: f32 = 0.6;
/// A body at rest wakes when the game's Ratchet gets this far from it (units).
const REST_WAKE: f32 = 0.3;

/// Below this speed into the ground (units/s) a contact does not bounce, so a body at rest settles instead of buzzing.
const BOUNCE_MIN: f32 = 1.0;
/// How far under a floor a sphere's centre may have slipped and still be lifted back on top of it.
const SINK: f32 = 0.3;
/// The whole body bounces when its centre comes down faster than this (units/s).
const BODY_BOUNCE_MIN: f32 = 3.0;
/// Spin of a whole-body bounce per unit of (contact offset from the centre x bounce speed), and its cap (rad/s).
const SPIN: f32 = 2.0;
const MAX_SPIN: f32 = 8.0;
/// A hit push slower than this sideways (units/s: retail barely moved him) goes backwards from where he faces at this speed.
const HIT_MIN: f32 = 1.0;
const HIT_BACK: f32 = 8.0;
/// A hit push moves the whole body as one: the same speed for every part plus a spin about its centre (rad/s per unit/s of the
/// sideways push, capped at `MAX_SPIN`) that tips the head along the push, so he topples over his feet. The first version gave
/// the top more speed than the legs instead: on a big launch the joints could not hold that and a limb stretched out, then snapped
/// back. The push itself is capped at `HIT_MAX` (units/s).
const HIT_SPIN: f32 = 0.4;
/// `RAC_RAGDOLL_LOG`: a `stretch` line when a joint's two bodies are further apart than this at its pivot (units).
const STRETCH_LOG: f32 = 0.1;
const HIT_MAX: f32 = 22.0;
/// Camera follow (`cam_offset`): the share of the way to the body's offset the camera moves each field, and how much of the
/// height difference it follows (lying down, the pelvis is a third of his standing height; a full drop dipped the view too far).
const CAM_EASE: f32 = 0.15;
const CAM_Z: f32 = 0.5;
/// The followed camera stays at least this far from the body's pelvis (units). Hits throw him back toward the camera, which eases
/// after him, and the body flew into the lens: a tail or an ear across the screen (screenshot 2026-10-26).
const CAM_MIN: f32 = 2.0;
/// The followed camera moves at most this far up or down (units): a body that fell down a drop took the camera with it.
const CAM_DZ: f32 = 1.5;
/// Ledges, while the game's Ratchet stands: no floor under the pelvis within `EDGE_DEPTH` of his feet is a drop, and the body
/// may not move out over it (its outward speed is taken off, and it is drawn back toward him at `EDGE_BACK` units/s). Below
/// `DROP_MAX` under his feet it is lifted back, whatever is under it. With the 6 unit leash a thrown body went over a ledge
/// and fell out of the world (logs 2026-10-26: pelvis at z -102 with him at z 48).
const EDGE_DEPTH: f32 = 2.0;
const EDGE_BACK: f32 = 1.0;
const DROP_MAX: f32 = 3.0;
/// The get-up clips (RAGDOLL_PLAN.md "Phase 5 clip search": no retail clip goes from lying to standing). Face down: seq 12's rise
/// out of a deep forward hunch. Face up: seq 97, a backflip that ends on his back, played backwards: a flip from his back onto his
/// feet. A clip whose first frame is not low (head more than `CLIP_LOW` above the pelvis) or whose last is not standing (less
/// than `CLIP_STAND`) is not used, in case another disc numbers the hero's sequences differently.
const CROUCH_SEQ: usize = 12;
const KIPUP_SEQ: usize = 97;
const CLIP_LOW: f32 = 0.3;
const CLIP_STAND: f32 = 0.45;
/// The limp body gets up with a clip when its head is less than this above its pelvis (units; about 0.55 standing).
const LYING_UP: f32 = 0.3;
/// Without a clip (he is not lying down) the plain blend takes this share of the get-up, about the 0.6 s it had before the clips.
const PLAIN_SHARE: f32 = 0.45;

/// A get-up clip, planned when the get-up starts. The clip plays where the body lies, turned to lie the way the body lies, and
/// slides and turns onto the game's Ratchet while it plays (a thrown body lies up to `hitleash` away from him).
struct GetUp {
	/// The clip's frames in play order: every joint's frame in the clip's model space.
	frames: Vec<Vec<M34>>,
	/// Where the clip starts: its model origin in the world and its turn about z from the game's Ratchet's heading.
	at: V3,
	turn: f32,
}

impl GetUp {
	/// The clip's place at slide `k` (0 = where the body lies, 1 = the game's Ratchet).
	fn place_at(&self, place: Place, k: f32) -> Place {
		let turn = self.turn * (1.0 - k);
		Place { pos: add(self.at, scale(sub(place.pos, self.at), k)), rows: place.rows.map(|r| rot_z(turn, r)) }
	}

	/// The clip's joints at progress `u` (0..1), between its two nearest frames.
	fn clip(&self, u: f32) -> Vec<M34> {
		let x = u.clamp(0.0, 1.0) * (self.frames.len() - 1) as f32;
		let i = (x as usize).min(self.frames.len() - 2);
		self.frames[i].iter().zip(&self.frames[i + 1]).map(|(a, b)| blend34(a, b, x - i as f32)).collect()
	}

	/// How far the get-up has moved onto the game's Ratchet (the camera follow fades out by it).
	fn slide(&self, blend: f32, t: &Tune) -> f32 {
		let [g0, g1] = t.getup;
		smoothstep((blend - g0) / (g1 - g0).max(1e-3))
	}

	/// The drawn joints (model space of the game's Ratchet): the limp pose settling into the clip's first frame, the clip, then its
	/// last frame settling into the game's pose.
	fn pose(&self, limp: &[M34], anim: &[M34], place: Place, blend: f32, t: &Tune) -> Vec<M34> {
		let [g0, g1] = t.getup;
		let placed = |fr: &[M34], at: Place| fr.iter().map(|m| place.to_model(&at.to_world(m))).collect::<Vec<M34>>();
		if blend < g0 {
			let s = smoothstep(blend / g0);
			let first = placed(&self.frames[0], self.place_at(place, 0.0));
			limp.iter().zip(&first).map(|(a, b)| blend34(a, b, s)).collect()
		} else if blend < g1 {
			let u = (blend - g0) / (g1 - g0).max(1e-3);
			placed(&self.clip(u), self.place_at(place, smoothstep(u)))
		} else {
			let s = smoothstep((blend - g1) / (1.0 - g1).max(1e-3));
			let last = placed(self.frames.last().unwrap(), place);
			last.iter().zip(anim).map(|(a, b)| blend34(a, b, s)).collect()
		}
	}
}

fn tune() -> Tune {
	static CACHE: std::sync::Mutex<Option<(std::time::Instant, Tune)>> = std::sync::Mutex::new(None);
	let Ok(path) = std::env::var("RAC_RAGDOLL_TUNE") else { return TUNE };
	let mut g = CACHE.lock().unwrap();
	if let Some((at, t)) = *g {
		if at.elapsed().as_secs_f32() < 1.0 {
			return t;
		}
	}
	let mut t = TUNE;
	for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
		let Some((k, v)) = line.split_once('=') else { continue };
		let v: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
		let Some(&x) = v.first() else { continue };
		match k.trim() {
			"gravity" => t.gravity = x,
			"substeps" => t.substeps = (x as u32).clamp(1, 32),
			"damp" => t.damp = x,
			"adamp" => t.adamp = x,
			"friction" => t.friction = x.clamp(0.0, 1.0),
			"drive" if v.len() >= 2 => t.drive = [v[0], v[1]],
			"limits" if v.len() >= 3 => t.limits = [v[0], v[1], v[2]],
			"tether" => t.tether = x,
			"bounce" if v.len() >= 3 => t.bounce = [v[0], v[1], v[2]],
			"hitpush" => t.hitpush = x,
			"hitlift" => t.hitlift = x,
			"hitleash" => t.hitleash = x,
			"crouch" if v.len() >= 2 => t.crouch = [v[0], v[1]],
			"kipup" if v.len() >= 2 => t.kipup = [v[0], v[1]],
			"getup" if v.len() >= 2 => t.getup = [v[0].clamp(0.0, 1.0), v[1].clamp(v[0], 1.0)],
			_ => {}
		}
	}
	*g = Some((std::time::Instant::now(), t));
	t
}

/// Where the game's Ratchet is this tick: his feet position and model rows (viewer/handler/hero_rows.rs `hero_rows`: a model
/// vector v goes to the world as v0 R0 + v1 R1 + v2 R2).
#[derive(Clone, Copy)]
pub(super) struct Place {
	pub pos: V3,
	pub rows: [[f32; 3]; 3],
}

impl Place {
	fn to_world_v(&self, v: V3) -> V3 {
		let r = &self.rows;
		[0, 1, 2].map(|i| v[0] * r[0][i] + v[1] * r[1][i] + v[2] * r[2][i])
	}
	fn to_model_v(&self, d: V3) -> V3 {
		let r = &self.rows;
		[0, 1, 2].map(|k| dot(d, r[k]))
	}
	fn to_world(&self, m: &M34) -> M34 {
		let mut o = [0.0; 12];
		for c in 0..4 {
			let w = self.to_world_v([m[c], m[4 + c], m[8 + c]]);
			for r in 0..3 {
				o[r * 4 + c] = w[r];
			}
		}
		for r in 0..3 {
			o[r * 4 + 3] += self.pos[r];
		}
		o
	}
	fn to_model(&self, m: &M34) -> M34 {
		let mut o = [0.0; 12];
		for c in 0..4 {
			let mut v = [m[c], m[4 + c], m[8 + c]];
			if c == 3 {
				v = sub(v, self.pos);
			}
			let l = self.to_model_v(v);
			for r in 0..3 {
				o[r * 4 + c] = l[r];
			}
		}
		o
	}
}

struct Body {
	x: V3,
	q: Q,
	v: V3,
	w: V3,
	xp: V3,
	qp: Q,
	im: f32,
	/// Inverse inertia in the body's frame.
	ii: [[f32; 3]; 3],
	/// Collision spheres: centre in the body's frame, radius.
	spheres: Vec<(V3, f32)>,
}

struct Joint {
	p: usize,
	c: usize,
	/// The pivot in each body's frame.
	rp: V3,
	rc: V3,
	/// The child's bone axis and a perpendicular twist reference, in each body's frame.
	ap: V3,
	ac: V3,
	bp: V3,
	bc: V3,
	/// The child's rotation relative to the parent's at the start (the drive's target).
	rel: Q,
	swing: f32,
	twist: f32,
}

/// The ragdoll state on the app (viewer.rs).
#[derive(Default)]
pub(super) struct Ragdoll {
	id: u32,
	active: bool,
	last_field: Option<u32>,
	bodies: Vec<Body>,
	joints: Vec<Joint>,
	/// (joint, body, the joint's frame in the body's frame) of every driven joint.
	driven: Vec<(usize, usize, M34)>,
	/// The bodies' frames from the animation last field (world), for the start motion.
	prev: Option<(u32, Vec<(V3, Q)>)>,
	flop: u8,
	/// Fields in a row the body was nearly still (`REST_FIELDS`: at rest), and where the game's Ratchet stood then.
	still: u32,
	rest_at: V3,
	/// The id of the last hit push added (`Cue::push`).
	push_seen: Option<u32>,
	/// This ragdoll was hit: the pelvis's leash is `hitleash` and nothing pulls him back inside it (first test: at the old 1 unit
	/// hard limit the thrown body stopped dead in the air "like an invisible barrier"). The get-up blend brings him back.
	thrown: bool,
	/// The camera's offset toward the limp body (`cam_place`): last field's and this field's.
	cam: [V3; 2],
	/// The body's pelvis, last field's and this field's, while the camera follows it (None: not limp or the setting is off).
	cam_body: Option<[V3; 2]>,
	/// The get-up clip (None: the plain blend) and whether this get-up was planned yet. While a clip plays the body is not stepped.
	getup: Option<GetUp>,
	getup_planned: bool,
	/// The joints drawn last (model space): a hit during a get-up clip starts the body again from them.
	last_out: Vec<M34>,
}

impl Ragdoll {
	/// Each fresh hero pose, after the strafe twist: replaces the drawn pose while `cue` says he is limp. `mats` = the skin matrices
	/// (bind -> posed model space), `place` = the game's Ratchet this tick. It steps once per new `cue.tick`. Returns the pose it
	/// replaced (the game's own), for the parts retail hangs on that pose (`strafe::pose_fix_between`); None when it left `mats` alone.
	pub(super) fn apply(&mut self, mats: &mut [M34], a: &crate::anim::Animated, cue: Cue, place: Place, world: &World) -> Option<Vec<M34>> {
		let field = cue.tick;
		if !crate::plugins::on_at_start(crate::retail::ragdoll::ID) {
			return None;
		}
		if mats.len() != HERO_JOINTS || a.skeleton.parents.len() != HERO_JOINTS {
			self.active = false;
			self.prev = None;
			self.cam = [[0.0; 3]; 2];
			self.cam_body = None;
			return None;
		}
		let ticked = self.last_field != Some(field);
		let steps = self.last_field.map_or(1, |f| field.wrapping_sub(f).clamp(1, 4));
		self.last_field = Some(field);
		if cue.phase == Phase::Off {
			self.active = false;
			self.getup = None;
			self.getup_planned = false;
			if ticked {
				// Only the bodies' joints: the start motion of a ragdoll that may begin next field.
				let mut anim = vec![[0.0f32; 12]; HERO_JOINTS];
				for d in &DEFS {
					for j in [d.drive[0], d.seg.0, d.seg.1] {
						anim[j] = a.joint_world(mats, j);
					}
				}
				self.prev = Some((field, self.frames(&anim, place)));
				self.follow_cam([0.0; 3], None);
			}
			return None;
		}
		let anim: Vec<M34> = (0..HERO_JOINTS).map(|j| a.joint_world(mats, j)).collect();
		if !self.active || cue.id != self.id {
			self.start(&anim, place, field, cue);
		} else if ticked && !cue.frozen && self.getup.is_none() {
			let dt = crate::retail::disc::field_dt();
			let t0 = std::time::Instant::now();
			for _ in 0..steps {
				self.step(dt, &anim, place, cue, world);
			}
			if crate::retail::ragdoll::log_on() {
				// A joint pulled apart (the pivot as each of its two bodies sees it): a stretched limb on screen.
				let gap = |jt: &Joint| length(sub(add(self.bodies[jt.p].x, qrot(self.bodies[jt.p].q, jt.rp)), add(self.bodies[jt.c].x, qrot(self.bodies[jt.c].q, jt.rc))));
				if let Some((k, g)) = self.joints.iter().map(gap).enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).filter(|(_, g)| *g > STRETCH_LOG) {
					eprintln!("ragdoll: stretch field {field} joint {} gap {g:.2} ({:?}, thrown {})", JOINTS[k].2, cue.phase, self.thrown);
				}
			}
			if crate::retail::ragdoll::log_on() && field % 30 == 0 {
				let p = self.bodies[0].x;
				eprintln!("ragdoll: step field {field} {steps} step(s) {:.2} ms, {} faces near, pelvis z {:.2} (game z {:.2})", t0.elapsed().as_secs_f32() * 1000.0, self.faces_near(world, dt).len(), p[2], place.pos[2]);
				let c = self.cam[1];
				eprintln!("ragdoll: where field {field} pelvis {:.2} {:.2} {:.2} game {:.2} {:.2} {:.2} apart {:.2} camera offset {:.2} {:.2} {:.2} thrown {}", p[0], p[1], p[2], place.pos[0], place.pos[1], place.pos[2], (p[0] - place.pos[0]).hypot(p[1] - place.pos[1]), c[0], c[1], c[2], self.thrown);
				LIMP_LOG.store(true, std::sync::atomic::Ordering::Relaxed);
			}
		}
		if cue.phase == Phase::GetUp {
			if !self.getup_planned {
				self.getup_planned = true;
				self.getup = self.plan_getup(a, place, cue);
			}
		} else if self.getup_planned {
			// A hit during the get-up (back to Ground): the body starts again, at rest, from the pose drawn last; the push follows.
			if self.getup.take().is_some() && self.last_out.len() == HERO_JOINTS {
				let (thrown, last) = (self.thrown, std::mem::take(&mut self.last_out));
				self.start(&last, place, field, Cue { from_hit: true, impact: None, ..cue });
				self.thrown = thrown;
			}
			self.getup_planned = false;
		}
		if let Some((pid, v)) = cue.push {
			if self.push_seen != Some(pid) {
				self.push_seen = Some(pid);
				if !cue.frozen {
					self.hit(v, place);
				}
			}
		}
		if ticked {
			// The camera eases toward the body (less of it while he gets up), measured between the body's pelvis and the game's.
			let on = crate::plugins::value_at_start(crate::retail::ragdoll::ID, "camera") == 1;
			let s = match &self.getup {
				Some(g) if cue.phase == Phase::GetUp => g.slide(cue.blend, &tune()),
				_ => plain_blend(cue),
			};
			let d = sub(self.bodies[0].x, self.frames(&anim, place)[0].0);
			let d = [d[0], d[1], d[2].clamp(-CAM_DZ / CAM_Z, CAM_DZ / CAM_Z)];
			let pelvis = self.bodies[0].x;
			self.follow_cam(if on { scale([d[0], d[1], d[2] * CAM_Z], 1.0 - s) } else { [0.0; 3] }, on.then_some(pelvis));
		}
		let logic = mats.to_vec();
		self.pose(mats, &anim, a, place, cue);
		Some(logic)
	}

	fn follow_cam(&mut self, target: V3, body: Option<V3>) {
		let now = self.cam[1];
		self.cam = [now, add(now, scale(sub(target, now), CAM_EASE))];
		self.cam_body = body.map(|b| [self.cam_body.map_or(b, |c| c[1]), b]);
	}

	/// Where to draw from: the game's camera `eye` moved to follow the limp body (Camera follow setting) and kept `CAM_MIN` off it,
	/// between the last two fields like the drawn hero (`alpha` = the frame-smoothing blend, 1 = the newest field). `eye` as it is
	/// when he is not limp.
	pub(super) fn cam_place(&self, eye: V3, alpha: f32) -> V3 {
		let lerp = |a: V3, b: V3| add(a, scale(sub(b, a), alpha));
		let at = add(eye, lerp(self.cam[0], self.cam[1]));
		let Some(body) = self.cam_body.map(|b| lerp(b[0], b[1])) else { return at };
		let d = sub(at, body);
		let l = length(d);
		if l < CAM_MIN && l > 1e-4 { add(body, scale(d, CAM_MIN / l)) } else { at }
	}

	/// The body frames (centre, rotation) of the animated pose, in the world.
	fn frames(&self, anim: &[M34], place: Place) -> Vec<(V3, Q)> {
		DEFS.iter()
			.map(|d| {
				let w = place.to_world(&anim[d.drive[0]]);
				let (pa, pb) = (place.to_world(&anim[d.seg.0]), place.to_world(&anim[d.seg.1]));
				(scale(add(tr(&pa), tr(&pb)), 0.5), q_of(&w))
			})
			.collect()
	}

	fn start(&mut self, anim: &[M34], place: Place, field: u32, cue: Cue) {
		let t = tune();
		let frames = self.frames(anim, place);
		// The last pose can be a few fields old when the display ran slower than the game.
		let gap = self.prev.as_ref().map_or(0, |(f, _)| field.wrapping_sub(*f));
		let dt = crate::retail::disc::field_dt() * gap.max(1) as f32;
		let prev = self.prev.as_ref().filter(|_| (1..=4).contains(&gap)).map(|(_, p)| p);
		let world_of = |j: usize| place.to_world(&anim[j]);
		self.bodies = DEFS
			.iter()
			.zip(&frames)
			.enumerate()
			.map(|(i, (d, &(x, q)))| {
				let (pa, pb) = (tr(&world_of(d.seg.0)), tr(&world_of(d.seg.1)));
				let len = length(sub(pb, pa));
				let axis = qrot(qconj(q), if len > 1e-4 { normalize(sub(pb, pa)) } else { [0.0, 0.0, 1.0] });
				let (v, w) = match prev.map(|p| p[i]) {
					Some((xp, qp)) => {
						let v = scale(sub(x, xp), 1.0 / dt);
						let dq = qmul(q, qconj(qp));
						let s = if dq[3] < 0.0 { -2.0 } else { 2.0 };
						(clamp_len(v, MAX_V), clamp_len([dq[0] * s / dt, dq[1] * s / dt, dq[2] * s / dt], MAX_W))
					}
					None => ([0.0; 3], [0.0; 3]),
				};
				let local = |p: V3| qrot(qconj(q), sub(p, x));
				let mut spheres = vec![(local(pa), d.r), (local(pb), d.r)];
				if len > 2.0 * d.r {
					spheres.push((local(scale(add(pa, pb), 0.5)), d.r));
				}
				if let Some((j, r)) = d.extra {
					spheres.push((local(tr(&world_of(j))), r));
				}
				Body { x, q, v, w, xp: x, qp: q, im: 1.0 / d.mass, ii: capsule_ii(d.mass, d.r, len, axis), spheres }
			})
			.collect();
		if cue.from_hit {
			for b in self.bodies.iter_mut() {
				(b.v, b.w) = ([0.0; 3], [0.0; 3]);
			}
		}
		// A landing: retail has already stopped him, so the body takes the speed he hit the ground with (the pelvis moves at it, the
		// limbs keep their own motion on top).
		if let Some(iv) = cue.impact {
			let dv = sub(iv, self.bodies[0].v);
			for b in self.bodies.iter_mut() {
				b.v = clamp_len(add(b.v, dv), MAX_V);
			}
		}
		let lim = t.limits[cue.flop.min(2) as usize];
		self.joints = JOINTS
			.iter()
			.map(|&(p, c, j, swing, twist)| {
				let (bp, bc) = (&self.bodies[p], &self.bodies[c]);
				let pivot = tr(&world_of(j));
				let d = &DEFS[c];
				let dir = sub(tr(&world_of(d.seg.1)), tr(&world_of(d.seg.0)));
				let dir = if length(dir) > 1e-4 { normalize(dir) } else { [0.0, 0.0, 1.0] };
				let perp = normalize(cross(dir, if dir[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] }));
				Joint {
					p,
					c,
					rp: qrot(qconj(bp.q), sub(pivot, bp.x)),
					rc: qrot(qconj(bc.q), sub(pivot, bc.x)),
					ap: qrot(qconj(bp.q), dir),
					ac: qrot(qconj(bc.q), dir),
					bp: qrot(qconj(bp.q), perp),
					bc: qrot(qconj(bc.q), perp),
					rel: qmul(qconj(bp.q), bc.q),
					swing: (swing * lim).min(170.0).to_radians(),
					twist: (twist * lim).min(170.0).to_radians(),
				}
			})
			.collect();
		self.driven = DEFS
			.iter()
			.enumerate()
			.flat_map(|(i, d)| d.drive.iter().map(move |&j| (j, i)))
			.map(|(j, i)| {
				let b = &self.bodies[i];
				(j, i, mul34(&inv34(&frame(b.x, b.q)), &world_of(j)))
			})
			.collect();
		if cue.id != self.id {
			self.getup = None;
			self.getup_planned = false;
		}
		self.id = cue.id;
		self.flop = cue.flop;
		self.active = true;
		self.still = 0;
		self.thrown = false;
		if crate::retail::ragdoll::log_on() {
			let v = self.bodies[0].v;
			eprintln!("ragdoll: body start id {} pelvis at {:.2} {:.2} {:.2} moving {:.2} {:.2} {:.2}", cue.id, self.bodies[0].x[0], self.bodies[0].x[1], self.bodies[0].x[2], v[0], v[1], v[2]);
		}
	}

	/// An enemy hit (`Cue::push`): throws the body with retail's knock-back, or backwards when retail hardly moved him, and wakes it.
	fn hit(&mut self, knock: V3, place: Place) {
		let t = tune();
		let side = if knock[0].hypot(knock[1]) < HIT_MIN { place.to_world_v([-HIT_BACK, 0.0, 0.0]) } else { scale(knock, t.hitpush) };
		let v = clamp_len([side[0], side[1], side[2].max(0.0) + t.hitlift], HIT_MAX);
		let flat = v[0].hypot(v[1]);
		let spin = if flat > 1e-3 { scale(cross([0.0, 0.0, 1.0], [v[0] / flat, v[1] / flat, 0.0]), (flat * HIT_SPIN).min(MAX_SPIN)) } else { [0.0; 3] };
		let mass: f32 = self.bodies.iter().map(|b| 1.0 / b.im).sum();
		let com = scale(self.bodies.iter().fold([0.0; 3], |c, b| add(c, scale(b.x, 1.0 / b.im))), 1.0 / mass);
		for b in self.bodies.iter_mut() {
			b.v = add(b.v, add(v, cross(spin, sub(b.x, com))));
			b.w = add(b.w, spin);
		}
		self.still = 0;
		self.thrown = true;
		if crate::retail::ragdoll::log_on() {
			eprintln!("ragdoll: body hit push {:.2} {:.2} {:.2}", v[0], v[1], v[2]);
		}
	}

	fn step(&mut self, dt: f32, anim: &[M34], place: Place, cue: Cue, world: &World) {
		let t = tune();
		let h = dt / t.substeps as f32;
		let drive = (self.flop < 2).then(|| t.drive[self.flop as usize]);
		// The game's Ratchet: his pelvis (the tether's target) and, where the level collision has nothing under his feet (a moving
		// platform, a crate), a floor at his feet height while he stands.
		let target = tr(&place.to_world(&anim[1]));
		if self.still >= REST_FIELDS {
			// At rest; woken when the game's Ratchet moves away from it (a moving platform, a knock-back).
			// Measured against where he was, not against the body: a thrown body rests away from him (`thrown`).
			if length(sub(place.pos, self.rest_at)) < REST_WAKE {
				return;
			}
			self.still = 0;
		}
		self.rest_at = place.pos;
		let up = add(place.pos, [0.0, 0.0, 0.3]);
		let floor = (cue.phase != Phase::Fall && world.ground(up, 0.6).is_none_or(|(z, _)| (z - place.pos[2]).abs() > 0.15)).then_some(place.pos[2]);
		// Water faces (surface 0) do not hold the body (the hero's own rule).
		let mask = crate::physics::qmask::SKIP_SURFACE;
		let bounce = t.bounce[cue.bounce.min(2) as usize].clamp(0.0, 0.95);
		// The faces near the body with their boxes: each sphere runs the (exact, slow) face test only on the faces its own box
		// touches, a few instead of all of them (that cost 3 to 8 ms a field).
		let faces: Vec<(u32, V3, V3)> = self
			.faces_near(world, dt)
			.into_iter()
			.map(|id| {
				let v = &world.tris[id as usize].v;
				let lo = [0, 1, 2].map(|k| v[0][k].min(v[1][k]).min(v[2][k]));
				let hi = [0, 1, 2].map(|k| v[0][k].max(v[1][k]).max(v[2][k]));
				(id, lo, hi)
			})
			.collect();
		let mut near: Vec<u32> = Vec::new();
		let mass: f32 = self.bodies.iter().map(|b| 1.0 / b.im).sum();
		// (body, offset of the contact point, normal, speed along the normal before this substep).
		let mut contacts: Vec<(usize, V3, V3, f32)> = Vec::new();
		for _ in 0..t.substeps {
			let vc0 = com(&self.bodies, mass, |b| b.v);
			for b in self.bodies.iter_mut() {
				b.v[2] -= t.gravity * h;
				b.xp = b.x;
				b.qp = b.q;
				b.x = add(b.x, scale(b.v, h));
				let dq = qmul([b.w[0], b.w[1], b.w[2], 0.0], b.q);
				b.q = qnorm([b.q[0] + 0.5 * h * dq[0], b.q[1] + 0.5 * h * dq[1], b.q[2] + 0.5 * h * dq[2], b.q[3] + 0.5 * h * dq[3]]);
			}
			for j in &self.joints {
				solve_joint(&mut self.bodies, j, h, drive);
			}
			contacts.clear();
			for (i, b) in self.bodies.iter_mut().enumerate() {
				for k in 0..b.spheres.len() {
					let (lc, r) = b.spheres[k];
					let c = add(b.x, qrot(b.q, lc));
					let reach = r + SINK;
					near.clear();
					near.extend(faces.iter().filter(|f| (0..3).all(|k| f.1[k] <= c[k] + reach && f.2[k] >= c[k] - reach)).map(|f| f.0));
					if near.is_empty() && floor.is_none() {
						continue;
					}
					let mut corr = [0.0; 3];
					let mut normal = None;
					if let Some(o) = world.nearest_among(&near, c, 0.0, r, r * r * 0.9995, mask) {
						corr = sub(o.to, c);
						normal = Some(o.normal);
					}
					// The faces only push from their front: a sphere whose centre slipped under the floor stayed stuck in it. A
					// floor up to `SINK` above the centre lifts it back on top.
					let at = add(c, corr);
					let ground = world.ground_among(&near, add(at, [0.0, 0.0, SINK]), SINK + r);
					let floor_z = match (ground, floor) {
						(Some(g), Some(f)) => Some(g.max(f)),
						(g, f) => g.or(f),
					};
					if let Some(fz) = floor_z {
						let lift = fz + r - at[2];
						if lift > 0.0 {
							corr[2] += lift;
							normal = Some(normal.filter(|n: &V3| n[2] > 0.5).unwrap_or([0.0, 0.0, 1.0]));
						}
					}
					if let Some(n) = normal {
						let r = sub(c, b.x);
						let vn = dot(add(b.v, cross(b.w, r)), n);
						if length(corr) > 1e-6 {
							push_body(b, corr, r, 0.0, h);
						}
						contacts.push((i, sub(add(c, corr), b.x), n, vn));
					}
				}
			}
			// The tether: sideways only, so he can lie on the ground below his standing pelvis. The soft pull moves the pelvis (the
			// joints bring the rest); the hard limit moves the whole body as one, which also takes the outward speed off every part.
			// Moving only the pelvis there left a limb hanging apart from the body for a moment before the joints pulled it back.
			let mut off = sub(target, self.bodies[0].x);
			off[2] = 0.0;
			let d = length(off);
			if d > 1e-5 {
				let (max, pull) = if self.thrown { (t.hitleash, 0.0) } else { (TETHER_MAX, t.tether) };
				let hard = (d - max).max(0.0);
				let soft = (d - hard) * (pull * h).min(1.0);
				if hard > 0.0 {
					let shift = scale(off, hard / d);
					for b in self.bodies.iter_mut() {
						b.x = add(b.x, shift);
					}
				}
				push_body(&mut self.bodies[0], scale(off, soft / d), [0.0; 3], 0.0, h);
			}
			if cue.phase != Phase::Fall {
				// Ledges (`EDGE_DEPTH`): moved as one body, like the hard leash.
				let pel = self.bodies[0].x;
				let mut back = sub(target, pel);
				back[2] = 0.0;
				let bl = length(back);
				let dir = if bl > 1e-4 { scale(back, 1.0 / bl) } else { [0.0; 3] };
				let feet = place.pos[2];
				let drop = world.ground(add(pel, [0.0, 0.0, 0.3]), EDGE_DEPTH + 0.3 + (pel[2] - feet).max(0.0)).is_none_or(|(z, _)| z < feet - EDGE_DEPTH);
				let mut shift = [0.0; 3];
				if drop {
					let out = (-dot(self.bodies[0].v, dir)).max(0.0);
					shift = scale(dir, (out * h + EDGE_BACK * h).min(bl));
				}
				let low = feet - DROP_MAX - pel[2];
				if low > 0.0 {
					shift = add(shift, [dir[0] * low.min(bl), dir[1] * low.min(bl), low]);
				}
				if shift != [0.0; 3] {
					for b in self.bodies.iter_mut() {
						b.x = add(b.x, shift);
					}
				}
			}
			for b in self.bodies.iter_mut() {
				b.v = scale(sub(b.x, b.xp), 1.0 / h);
				let dq = qmul(b.q, qconj(b.qp));
				let s = if dq[3] < 0.0 { -2.0 / h } else { 2.0 / h };
				b.w = [dq[0] * s, dq[1] * s, dq[2] * s];
			}
			for &(i, r, n, vn0) in &contacts {
				let b = &mut self.bodies[i];
				let vp = add(b.v, cross(b.w, r));
				let vt = sub(vp, scale(n, dot(vp, n)));
				if length(vt) > 1e-5 {
					kick_body(b, scale(vt, -t.friction), r);
				}
				// Bounciness: a contact that hit leaves at `bounce` x the speed it hit with, one that only rests leaves at none. This
				// SETS the speed: the position push alone also adds speed (penetration / h), a pop that ate the bounce and kept a body
				// at rest buzzing.
				if vn0 < 0.0 {
					let vn = dot(add(b.v, cross(b.w, r)), n);
					let want = if vn0 < -BOUNCE_MIN { -bounce * PART_BOUNCE * vn0 } else { 0.0 };
					if (want - vn).abs() > 1e-4 {
						kick_body(b, scale(n, want - vn), r);
					}
				}
			}
			// The whole body bounces too. Per contact only the part that touched bounced (feet first: the feet), and the heavy upper
			// body falling on through the joints soaked it up, so a big landing just crumpled. A hard hit (the body's centre coming
			// down faster than `BODY_BOUNCE_MIN`) sends the centre back up at `bounce` x that speed, with the spin a push at the
			// contact gives (a tilted landing tips him over).
			let hits: Vec<&(usize, V3, V3, f32)> = contacts.iter().filter(|c| c.3 < -BOUNCE_MIN).collect();
			if bounce > 0.0 && !hits.is_empty() {
				let n = normalize(hits.iter().fold([0.0; 3], |a, c| add(a, c.2)));
				let vn0 = dot(vc0, n);
				if vn0 < -BODY_BOUNCE_MIN {
					let gain = -bounce * vn0 - dot(com(&self.bodies, mass, |b| b.v), n);
					if gain > 0.0 {
						let dv = scale(n, gain);
						let cx = com(&self.bodies, mass, |b| b.x);
						let cp = scale(hits.iter().fold([0.0; 3], |a, c| add(a, add(self.bodies[c.0].x, c.1))), 1.0 / hits.len() as f32);
						let arm = sub(cp, cx);
						let arm = sub(arm, scale(n, dot(arm, n)));
						let spin = clamp_len(scale(cross(arm, dv), SPIN), MAX_SPIN);
						for b in self.bodies.iter_mut() {
							b.v = add(b.v, add(dv, cross(spin, sub(b.x, cx))));
							b.w = add(b.w, spin);
						}
					}
				}
			}
			for b in self.bodies.iter_mut() {
				b.v = clamp_len(scale(b.v, (1.0 - t.damp * h).max(0.0)), STEP_MAX_V);
				b.w = clamp_len(scale(b.w, (1.0 - t.adamp * h).max(0.0)), STEP_MAX_W);
			}
		}
		// Settled: every part nearly still for a while ends the simulation until the next ragdoll (no buzzing at rest).
		let still = self.bodies.iter().all(|b| length(b.v) < REST_V && length(b.w) < REST_W);
		self.still = if still { self.still + 1 } else { 0 };
		if self.still >= REST_FIELDS {
			for b in self.bodies.iter_mut() {
				b.v = [0.0; 3];
				b.w = [0.0; 3];
			}
		}
	}

	/// The collision faces the body can reach this field: one lookup for the box around all its spheres (grown by how far they can
	/// move in a field), cut to the faces whose own box touches it. Every sphere test then runs over this short list (one
	/// `World::nearest` per sphere and substep looked the faces up, sorted and filtered them again each time: hundreds a field,
	/// which slowed the game down).
	fn faces_near(&self, world: &World, dt: f32) -> Vec<u32> {
		let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
		let mut speed = 0.0f32;
		for b in &self.bodies {
			speed = speed.max(length(b.v) + length(b.w) * 0.5);
			for &(lc, r) in &b.spheres {
				let c = add(b.x, qrot(b.q, lc));
				for k in 0..3 {
					lo[k] = lo[k].min(c[k] - r);
					hi[k] = hi[k].max(c[k] + r);
				}
			}
		}
		let grow = speed * dt + SINK + 0.1;
		let (lo, hi) = (lo.map(|v| v - grow), hi.map(|v| v + grow));
		let mut ids = world.ids_in_box(lo, hi);
		ids.retain(|&id| {
			let v = &world.tris[id as usize].v;
			(0..3).all(|k| v.iter().map(|p| p[k]).fold(f32::MAX, f32::min) <= hi[k] && v.iter().map(|p| p[k]).fold(f32::MIN, f32::max) >= lo[k])
		});
		ids
	}

	/// Writes the ragdoll's pose into `mats`: the limp body, and while he gets up the get-up clip or the plain blend toward the
	/// animation.
	fn pose(&mut self, mats: &mut [M34], anim: &[M34], a: &crate::anim::Animated, place: Place, cue: Cue) {
		let out = match &self.getup {
			Some(g) if cue.phase == Phase::GetUp => g.pose(&self.limp_pose(anim, a, place, 0.0), anim, place, cue.blend, &tune()),
			_ => self.limp_pose(anim, a, place, plain_blend(cue)),
		};
		for j in 0..HERO_JOINTS {
			mats[j] = mul34(&mul34(&out[j], &inv34(&anim[j])), &mats[j]);
		}
		self.last_out = out;
	}

	/// The limp body's joints (model space), moved toward the animation by `s`.
	fn limp_pose(&self, anim: &[M34], a: &crate::anim::Animated, place: Place, s: f32) -> Vec<M34> {
		let parents = &a.skeleton.parents;
		let mut out: Vec<M34> = anim.to_vec();
		let mut by_joint: Vec<Option<M34>> = vec![None; HERO_JOINTS];
		for &(j, i, off) in &self.driven {
			let b = &self.bodies[i];
			let m = place.to_model(&mul34(&frame(b.x, b.q), &off));
			by_joint[j] = Some(if s > 0.0 { blend34(&m, &anim[j], s) } else { m });
		}
		// The root (joint 0) and any other parentless joint sit at the game's Ratchet's feet. Left there, the vertices weighted to
		// them stayed behind when the body was thrown and stretched into a line from his feet to the limp body; they ride the
		// pelvis instead, as if rigidly attached to it.
		let pelvis = by_joint[1].map(|m| mul34(&m, &inv34(&anim[1])));
		let rigid = |j: usize| pelvis.map_or(anim[j], |d| mul34(&d, &anim[j]));
		out[0] = rigid(0);
		for j in 1..HERO_JOINTS {
			out[j] = match (by_joint[j], parents[j]) {
				(Some(m), _) => m,
				(None, Some(p)) => mul34(&out[p], &mul34(&inv34(&anim[p]), &anim[j])),
				(None, None) => rigid(j),
			};
		}
		out
	}

	/// The get-up clip for the body as it lies (None: the plain blend). Face down or up by the pelvis's forward axis; none when
	/// he is not lying (a sitting or upright body), on a free get-up, or when the disc's clip does not look like the measured one.
	fn plan_getup(&self, a: &crate::anim::Animated, place: Place, cue: Cue) -> Option<GetUp> {
		let log = crate::retail::ragdoll::log_on();
		if cue.free || self.bodies.len() != DEFS.len() {
			return None;
		}
		let world = |j: usize| self.driven.iter().find(|d| d.0 == j).map(|&(_, i, off)| mul34(&frame(self.bodies[i].x, self.bodies[i].q), &off));
		let (pelvis, chest, head) = (world(1)?, world(5)?, world(8)?);
		let up = head[11] - pelvis[11];
		if up > LYING_UP {
			if log {
				eprintln!("ragdoll: get-up plain (not lying: head {up:.2} above the pelvis)");
			}
			return None;
		}
		let face_up = pelvis[8] > 0.0;
		let t = tune();
		let (seq, range) = if face_up { (KIPUP_SEQ, t.kipup) } else { (CROUCH_SEQ, t.crouch) };
		let s = a.sequences.get(seq)?;
		let last = s.frames.len().checked_sub(1)? as f32;
		let (f0, f1) = (range[0].clamp(0.0, last) as usize, range[1].clamp(0.0, last) as usize);
		let order: Vec<usize> = if f0 <= f1 { (f0..=f1).collect() } else { (f1..=f0).rev().collect() };
		if order.len() < 2 {
			return None;
		}
		let frames: Vec<Vec<M34>> = order
			.iter()
			.map(|&i| {
				let skin = a.pose_single(&s.frames[i]);
				(0..HERO_JOINTS).map(|j| a.joint_world(&skin, j)).collect()
			})
			.collect();
		let clip_up = |f: &[M34]| f[8][11] - f[1][11];
		let (u0, u1) = (clip_up(&frames[0]), clip_up(frames.last()?));
		if u0 > CLIP_LOW || u1 < CLIP_STAND {
			if log {
				eprintln!("ragdoll: get-up plain (seq {seq} frames {f0}..{f1} do not go from low to standing: up {u0:.2} -> {u1:.2})");
			}
			return None;
		}
		// Turned so the clip's pelvis-to-chest line lies along the body's, its pelvis on the body's pelvis, and no lower than the floor
		// the body lies on (the lowest point of its spheres): the hunch's pelvis is higher than a body lying flat.
		let first = &frames[0];
		let along_clip = place.to_world_v(sub(tr(&first[5]), tr(&first[1])));
		let along_body = sub(tr(&chest), tr(&pelvis));
		let turn = if along_clip[0].hypot(along_clip[1]) > 0.05 && along_body[0].hypot(along_body[1]) > 0.05 {
			wrap_angle(along_body[1].atan2(along_body[0]) - along_clip[1].atan2(along_clip[0]))
		} else {
			0.0
		};
		let cp = rot_z(turn, place.to_world_v(tr(&first[1])));
		let bp = tr(&pelvis);
		let floor = self.bodies.iter().flat_map(|b| b.spheres.iter().map(move |&(c, r)| add(b.x, qrot(b.q, c))[2] - r)).fold(f32::MAX, f32::min);
		let at = [bp[0] - cp[0], bp[1] - cp[1], (bp[2] - cp[2]).max(floor)];
		if log {
			eprintln!(
				"ragdoll: get-up clip face {} seq {seq} frames {f0}..{f1} turn {:.0} deg, {:.2} from him, floor {:.2} (his feet {:.2})",
				if face_up { "up" } else { "down" },
				turn.to_degrees(),
				(at[0] - place.pos[0]).hypot(at[1] - place.pos[1]),
				floor,
				place.pos[2]
			);
		}
		Some(GetUp { frames, at, turn })
	}
}

/// The plain get-up blend (0 = limp, 1 = the game's pose): the whole of a free get-up, `PLAIN_SHARE` of the others.
fn plain_blend(cue: Cue) -> f32 {
	if cue.phase != Phase::GetUp {
		return 0.0;
	}
	smoothstep(if cue.free { cue.blend } else { cue.blend / PLAIN_SHARE })
}

/// `v` turned by `a` radians about the world's z.
fn rot_z(a: f32, v: V3) -> V3 {
	let (s, c) = a.sin_cos();
	[v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]]
}

fn wrap_angle(a: f32) -> f32 {
	let t = std::f32::consts::TAU;
	a - t * ((a + std::f32::consts::PI) / t).floor()
}

// ---- the solver -------------------------------------------------------------------------------------------------------------

/// The inverse inertia (body frame) of a solid capsule of mass `m`, radius `r` and length `len` along the unit `axis`, made
/// `INERTIA` times harder to turn: a thin limb's true inertia is tiny, so every contact push spun it wildly.
fn capsule_ii(m: f32, r: f32, len: f32, axis: V3) -> [[f32; 3]; 3] {
	let par = (INERTIA * 0.5 * m * r * r).max(1e-5);
	let perp = (INERTIA * m * (3.0 * r * r + len * len) / 12.0).max(1e-5);
	let (a, b) = (1.0 / perp, 1.0 / par - 1.0 / perp);
	[0, 1, 2].map(|i| [0, 1, 2].map(|k| if i == k { a } else { 0.0 } + b * axis[i] * axis[k]))
}

/// The body's inverse inertia applied to the world vector `v`.
fn ii_w(b: &Body, v: V3) -> V3 {
	let l = qrot(qconj(b.q), v);
	qrot(b.q, [0, 1, 2].map(|i| dot(b.ii[i], l)))
}

/// The generalised inverse mass of a body at offset `r` along unit `n`.
fn w_pos(b: &Body, r: V3, n: V3) -> f32 {
	let rn = cross(r, n);
	b.im + dot(rn, ii_w(b, rn))
}

fn rotate_by(b: &mut Body, rv: V3, sign: f32) {
	let dq = qmul([rv[0], rv[1], rv[2], 0.0], b.q);
	b.q = qnorm([0, 1, 2, 3].map(|k| b.q[k] + 0.5 * sign * dq[k]));
}

/// Moves one body (against the static world) by the correction `corr` at offset `r` from its centre (compliance `alpha`).
fn push_body(b: &mut Body, corr: V3, r: V3, alpha: f32, h: f32) {
	let c = length(corr);
	if c < 1e-7 {
		return;
	}
	let n = scale(corr, 1.0 / c);
	let lambda = c / (w_pos(b, r, n) + alpha / (h * h));
	let p = scale(n, lambda);
	b.x = add(b.x, scale(p, b.im));
	let rv = ii_w(b, cross(r, p));
	rotate_by(b, rv, 1.0);
}

/// A velocity change `dv` at offset `r` (friction).
fn kick_body(b: &mut Body, dv: V3, r: V3) {
	let l = length(dv);
	if l < 1e-7 {
		return;
	}
	let n = scale(dv, 1.0 / l);
	let p = scale(n, l / w_pos(b, r, n));
	b.v = add(b.v, scale(p, b.im));
	b.w = add(b.w, ii_w(b, cross(r, p)));
}

/// A positional correction between two bodies: moves body `i`'s point (offset `ri`) by +`corr`, body `k`'s (offset `rk`) by -`corr`.
fn push_pair(bodies: &mut [Body], i: usize, k: usize, corr: V3, ri: V3, rk: V3, h: f32, alpha: f32) {
	let c = length(corr);
	if c < 1e-7 {
		return;
	}
	let n = scale(corr, 1.0 / c);
	let lambda = c / (w_pos(&bodies[i], ri, n) + w_pos(&bodies[k], rk, n) + alpha / (h * h));
	let p = scale(n, lambda);
	let (bi, bk) = pair(bodies, i, k);
	bi.x = add(bi.x, scale(p, bi.im));
	let rv = ii_w(bi, cross(ri, p));
	rotate_by(bi, rv, 1.0);
	bk.x = sub(bk.x, scale(p, bk.im));
	let rv = ii_w(bk, cross(rk, p));
	rotate_by(bk, rv, -1.0);
}

/// An angular correction: turns body `i` by +`rv` and body `k` by -`rv`, shared by their inertia (compliance `alpha`).
fn turn_pair(bodies: &mut [Body], i: usize, k: usize, rv: V3, h: f32, alpha: f32) {
	let th = length(rv);
	if th < 1e-7 {
		return;
	}
	let n = scale(rv, 1.0 / th);
	let w = dot(n, ii_w(&bodies[i], n)) + dot(n, ii_w(&bodies[k], n));
	let lambda = th / (w + alpha / (h * h));
	let p = scale(n, lambda);
	let (bi, bk) = pair(bodies, i, k);
	let r = ii_w(bi, p);
	rotate_by(bi, r, 1.0);
	let r = ii_w(bk, p);
	rotate_by(bk, r, -1.0);
}

fn pair(bodies: &mut [Body], i: usize, k: usize) -> (&mut Body, &mut Body) {
	assert!(i != k);
	if i < k {
		let (a, b) = bodies.split_at_mut(k);
		(&mut a[i], &mut b[0])
	} else {
		let (a, b) = bodies.split_at_mut(i);
		(&mut b[0], &mut a[k])
	}
}

fn solve_joint(bodies: &mut [Body], j: &Joint, h: f32, drive: Option<f32>) {
	// The pivot.
	let (rp, rc) = (qrot(bodies[j.p].q, j.rp), qrot(bodies[j.c].q, j.rc));
	let corr = sub(add(bodies[j.c].x, rc), add(bodies[j.p].x, rp));
	push_pair(bodies, j.p, j.c, corr, rp, rc, h, 0.0);
	// The swing cone: the child's bone within `swing` of where it points in the parent's frame.
	let a1 = qrot(bodies[j.p].q, j.ap);
	let a2 = qrot(bodies[j.c].q, j.ac);
	let phi = dot(a1, a2).clamp(-1.0, 1.0).acos();
	if phi > j.swing {
		let n = cross(a1, a2);
		if length(n) > 1e-6 {
			turn_pair(bodies, j.p, j.c, scale(normalize(n), phi - j.swing), h, 0.0);
		}
	}
	// The twist about the bone.
	let a1 = qrot(bodies[j.p].q, j.ap);
	let a2 = qrot(bodies[j.c].q, j.ac);
	let n = add(a1, a2);
	if length(n) > 1e-6 {
		let n = normalize(n);
		let flat = |v: V3| {
			let v = sub(v, scale(n, dot(v, n)));
			if length(v) > 1e-6 { Some(normalize(v)) } else { None }
		};
		if let (Some(b1), Some(b2)) = (flat(qrot(bodies[j.p].q, j.bp)), flat(qrot(bodies[j.c].q, j.bc))) {
			let psi = dot(cross(b1, b2), n).atan2(dot(b1, b2));
			if psi.abs() > j.twist {
				turn_pair(bodies, j.p, j.c, scale(n, psi - psi.signum() * j.twist), h, 0.0);
			}
		}
	}
	// The drive back toward the starting pose (Floppiness).
	if let Some(alpha) = drive {
		let target = qmul(bodies[j.p].q, j.rel);
		let mut e = qmul(target, qconj(bodies[j.c].q));
		if e[3] < 0.0 {
			e = e.map(|x| -x);
		}
		let rv = [2.0 * e[0], 2.0 * e[1], 2.0 * e[2]];
		turn_pair(bodies, j.p, j.c, scale(rv, -1.0), h, alpha);
	}
}

/// The mass-weighted mean of `f` over the bodies (`mass` = their total): the centre's position or speed.
fn com(bodies: &[Body], mass: f32, f: impl Fn(&Body) -> V3) -> V3 {
	scale(bodies.iter().fold([0.0; 3], |a, b| add(a, scale(f(b), 1.0 / b.im))), 1.0 / mass.max(1e-6))
}

// ---- maths --------------------------------------------------------------------------------------------------------------

fn tr(m: &M34) -> V3 {
	[m[3], m[7], m[11]]
}

fn clamp_len(v: V3, max: f32) -> V3 {
	let l = length(v);
	if l > max { scale(v, max / l) } else { v }
}

fn smoothstep(x: f32) -> f32 {
	let x = x.clamp(0.0, 1.0);
	x * x * (3.0 - 2.0 * x)
}

fn qmul(a: Q, b: Q) -> Q {
	[
		a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
		a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
		a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
		a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
	]
}

fn qconj(q: Q) -> Q {
	[-q[0], -q[1], -q[2], q[3]]
}

fn qnorm(q: Q) -> Q {
	let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
	if l > 1e-9 { q.map(|x| x / l) } else { [0.0, 0.0, 0.0, 1.0] }
}

fn qrot(q: Q, v: V3) -> V3 {
	let u = [q[0], q[1], q[2]];
	let t = scale(cross(u, v), 2.0);
	add(add(v, scale(t, q[3])), cross(u, t))
}

/// Row-major 3x3 rotation of a quaternion.
fn mat_of(q: Q) -> [[f32; 3]; 3] {
	let [x, y, z, w] = q;
	[
		[1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
		[2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
		[2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
	]
}

/// The rotation of a 3x4 matrix (columns normalised first, so a scaled joint still gives its turn).
fn q_of(m: &M34) -> Q {
	let col = |c: usize| {
		let v = [m[c], m[4 + c], m[8 + c]];
		if length(v) > 1e-9 { normalize(v) } else { [0.0; 3] }
	};
	let (cx, cy, cz) = (col(0), col(1), col(2));
	let r = [[cx[0], cy[0], cz[0]], [cx[1], cy[1], cz[1]], [cx[2], cy[2], cz[2]]];
	let tr = r[0][0] + r[1][1] + r[2][2];
	let q = if tr > 0.0 {
		let s = (tr + 1.0).sqrt() * 2.0;
		[(r[2][1] - r[1][2]) / s, (r[0][2] - r[2][0]) / s, (r[1][0] - r[0][1]) / s, 0.25 * s]
	} else if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
		let s = (1.0 + r[0][0] - r[1][1] - r[2][2]).max(1e-9).sqrt() * 2.0;
		[0.25 * s, (r[0][1] + r[1][0]) / s, (r[0][2] + r[2][0]) / s, (r[2][1] - r[1][2]) / s]
	} else if r[1][1] > r[2][2] {
		let s = (1.0 + r[1][1] - r[0][0] - r[2][2]).max(1e-9).sqrt() * 2.0;
		[(r[0][1] + r[1][0]) / s, 0.25 * s, (r[1][2] + r[2][1]) / s, (r[0][2] - r[2][0]) / s]
	} else {
		let s = (1.0 + r[2][2] - r[0][0] - r[1][1]).max(1e-9).sqrt() * 2.0;
		[(r[0][2] + r[2][0]) / s, (r[1][2] + r[2][1]) / s, 0.25 * s, (r[1][0] - r[0][1]) / s]
	};
	qnorm(q)
}

/// The 3x4 matrix of a body frame.
fn frame(x: V3, q: Q) -> M34 {
	let r = mat_of(q);
	[r[0][0], r[0][1], r[0][2], x[0], r[1][0], r[1][1], r[1][2], x[1], r[2][0], r[2][1], r[2][2], x[2]]
}

fn mul34(a: &M34, b: &M34) -> M34 {
	let mut o = [0.0; 12];
	for r in 0..3 {
		for c in 0..4 {
			let mut s = a[r * 4] * b[c] + a[r * 4 + 1] * b[4 + c] + a[r * 4 + 2] * b[8 + c];
			if c == 3 {
				s += a[r * 4 + 3];
			}
			o[r * 4 + c] = s;
		}
	}
	o
}

/// The inverse of an affine 3x4 matrix (any invertible linear part).
fn inv34(m: &M34) -> M34 {
	let a = |r: usize, c: usize| m[r * 4 + c];
	let cof = |r: usize, c: usize| {
		let (r1, r2, c1, c2) = ((r + 1) % 3, (r + 2) % 3, (c + 1) % 3, (c + 2) % 3);
		a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)
	};
	let det = a(0, 0) * cof(0, 0) + a(0, 1) * cof(0, 1) + a(0, 2) * cof(0, 2);
	let id = if det.abs() > 1e-12 { 1.0 / det } else { 0.0 };
	let mut o = [0.0; 12];
	for r in 0..3 {
		for c in 0..3 {
			o[r * 4 + c] = cof(c, r) * id;
		}
	}
	for r in 0..3 {
		o[r * 4 + 3] = -(o[r * 4] * m[3] + o[r * 4 + 1] * m[7] + o[r * 4 + 2] * m[11]);
	}
	o
}

/// `a` moved toward `b` by `s` (0 = a, 1 = b): turn slerped, position and column lengths lerped.
fn blend34(a: &M34, b: &M34, s: f32) -> M34 {
	let len = |m: &M34, c: usize| length([m[c], m[4 + c], m[8 + c]]);
	let (qa, mut qb) = (q_of(a), q_of(b));
	if qa.iter().zip(&qb).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
		qb = qb.map(|x| -x);
	}
	let q = qnorm([0, 1, 2, 3].map(|k| qa[k] + (qb[k] - qa[k]) * s));
	let r = mat_of(q);
	let mut o = [0.0; 12];
	for c in 0..3 {
		let l = len(a, c) + (len(b, c) - len(a, c)) * s;
		for k in 0..3 {
			o[k * 4 + c] = r[k][c] * l;
		}
	}
	for k in 0..3 {
		o[k * 4 + 3] = a[k * 4 + 3] + (b[k * 4 + 3] - a[k * 4 + 3]) * s;
	}
	o
}

#[cfg(test)]
mod tests {
	use super::*;

	fn floor() -> World {
		World::new([[[-50.0, -50.0, 0.0], [50.0, -50.0, 0.0], [50.0, 50.0, 0.0]], [[-50.0, -50.0, 0.0], [50.0, 50.0, 0.0], [-50.0, 50.0, 0.0]]], 4.0)
	}

	#[test]
	fn affine_inverse_and_quaternions_agree() {
		let q = qnorm([0.3, -0.2, 0.5, 0.8]);
		let m = frame([1.0, 2.0, 3.0], q);
		let i = mul34(&m, &inv34(&m));
		for (k, &x) in i.iter().enumerate() {
			let want = if k % 5 == 0 { 1.0 } else { 0.0 };
			assert!((x - want).abs() < 1e-5, "{k}: {x}");
		}
		let p = q_of(&m);
		assert!(p.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>().abs() > 0.9999);
		let v = [0.4, -1.0, 2.0];
		let a = qrot(q, v);
		let b = [0, 1, 2].map(|r| m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2]);
		assert!(length(sub(a, b)) < 1e-5);
	}

	#[test]
	fn a_dropped_body_rests_on_the_floor() {
		let w = floor();
		let q = [0.0, 0.0, 0.0, 1.0];
		let mut b = Body { x: [0.0, 0.0, 1.0], q, v: [0.0; 3], w: [0.0; 3], xp: [0.0; 3], qp: q, im: 1.0, ii: capsule_ii(1.0, 0.1, 0.3, [1.0, 0.0, 0.0]), spheres: vec![([-0.15, 0.0, 0.0], 0.1), ([0.15, 0.0, 0.0], 0.1)] };
		let mut r = Ragdoll { bodies: Vec::new(), ..Default::default() };
		let h = 1.0 / 480.0;
		for _ in 0..2000 {
			b.v[2] -= 24.0 * h;
			b.xp = b.x;
			b.x = add(b.x, scale(b.v, h));
			for k in 0..b.spheres.len() {
				let (lc, rad) = b.spheres[k];
				let c = add(b.x, qrot(b.q, lc));
				if let Some(o) = w.nearest(c, 0.0, rad, rad * rad * 0.9995, 0) {
					push_body(&mut b, sub(o.to, c), sub(c, b.x), 0.0, h);
				}
			}
			b.v = scale(sub(b.x, b.xp), 1.0 / h);
		}
		r.bodies.push(b);
		let z = r.bodies[0].x[2];
		assert!((z - 0.1).abs() < 0.02, "rests at {z}");
	}
}
