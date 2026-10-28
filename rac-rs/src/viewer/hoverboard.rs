//! [PC] Hoverboard (HOVERBOARD_PLAN.md), the renderer half: the riding pose and the board. Picture only.
//!
//! - The board is a small mesh of our own appended to the hero's model (app.rs `combined_hero!`, only with the mod on) with a
//!   generated texture under `BOARD_KEY`; `frame_hero.rs` appends its vertices after the hero afterimages every pose, hidden at the
//!   model origin when he is not riding (as every hidden part).
//! - The pose: while he rides, Ratchet's own hoverboard race clips replace the drawn pose (the ride loop, a carve per lean side,
//!   the jump; read from the race level when the level that runs lacks them, `race_clips`), the body and board tilting a little
//!   more into the turn. Without the clips, a plain stance frame (`stance`) turned sideways. The board sits under his drawn feet.
//!   Retail's pose is handed back so Clank, the packs and the held weapon follow the drawn one (`strafe::pose_fix_between`,
//!   `Smooth::set_limp`).
//!
//! `RAC_HOVER_TUNE=seq=S,frame=F,yaw=Y` picks another stance frame / stance turn live (no rebuild). `RAC_HOVER_DUMP=1` writes a
//! `hoverboard: seq` line per hero sequence at level load (feet apart along / across the model, pelvis height) to choose one.
use crate::level::Texture;
use crate::mesh::Mesh;
use crate::retail::hoverboard::Cue;
use std::sync::OnceLock;

type M34 = [f32; 12];

/// The board texture's key: a moby-group slot past the moby group's own, next to `FLAT_SLOT` and the metal keys (level.rs).
pub const BOARD_KEY: u32 = 1 << 16 | 0xfffc;
const HERO_JOINTS: usize = 111;
/// Joints read by the dump: pelvis, right / left ankle (RAGDOLL_PLAN.md "Hero skeleton").
const PELVIS: usize = 1;
const ANKLES: [usize; 2] = [94, 100];
/// Right / left shoulder.
const SHOULDERS: [usize; 2] = [53, 73];

/// The board: a rounded deck `HALF_LEN` x `HALF_WIDE` (superellipse, power `ROUND`), its top `DECK_TOP` over the ground and
/// `THICK` deep, outlined with `RIM` points.
const HALF_LEN: f32 = 0.66;
const HALF_WIDE: f32 = 0.22;
const ROUND: f32 = 4.0;
const DECK_TOP: f32 = 0.24;
const THICK: f32 = 0.06;
const RIM: usize = 24;
/// The hover bob on the ground (units, radians per field).
const BOB: f32 = 0.02;
const BOB_RATE: f32 = 0.11;

/// The default stance: sequence, frame, and the stance turn (rad about up). Chosen from the `RAC_HOVER_DUMP` list (USA disc,
/// 2026-10-27): seq 51 is a held crouch (5 still frames, pelvis 0.29 against 0.49 standing), left foot forward, the feet 0.44
/// apart on a line 30 degrees left of his model x, facing 70 degrees right of that line (likely the grind pose). Turned -30
/// degrees its feet lie along the board and he faces the right of the path.
const STANCE: (usize, usize, f32) = (51, 0, -0.52);

fn on() -> bool {
	crate::plugins::on_at_start(crate::retail::hoverboard::ID)
}

/// The board's mesh for the hero model (app.rs `combined_hero!`); None with the mod off (the hero model stays as it was).
pub fn board_mesh() -> Option<Mesh> {
	on().then(|| {
		let (ps, _) = board_model();
		let mut m = Mesh::new("hoverboard".into());
		m.uvs = board_uvs();
		m.faces = board_faces().into_iter().map(|f| (BOARD_KEY, f)).collect();
		m.positions = ps;
		m
	})
}

/// The board's texture (app.rs, next to the flat grey one); None with the mod off.
pub fn board_texture() -> Option<Texture> {
	on().then(|| {
		// Left half: the deck's top (dark blue, an orange stripe down the middle, a pale rim). Right half: the underside's glow.
		let (w, h) = (64usize, 128usize);
		let mut rgba = vec![[0u8; 4]; w * h];
		for y in 0..h {
			for x in 0..w {
				let c = if x < w / 2 {
					let u = (x as f32 + 0.5) / (w / 2) as f32 * 2.0 - 1.0;
					let v = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
					if u.abs() > 0.85 {
						[210, 214, 222]
					} else if u.abs() < 0.18 && v.abs() < 0.8 {
						[240, 130, 30]
					} else if (x + y) % 6 == 0 {
						[52, 60, 92]
					} else {
						[34, 42, 74]
					}
				} else {
					let u = ((x - w / 2) as f32 + 0.5) / (w / 2) as f32 * 2.0 - 1.0;
					let g = (1.0 - u.abs()).powf(0.6);
					[(40.0 + 60.0 * g) as u8, (120.0 + 120.0 * g) as u8, (160.0 + 95.0 * g) as u8]
				};
				rgba[y * w + x] = [c[0], c[1], c[2], 255];
			}
		}
		Texture { w, h, rgba }
	})
}

/// The deck's outline (x along the board, y across).
fn rim() -> Vec<[f32; 2]> {
	(0..RIM)
		.map(|k| {
			let t = k as f32 / RIM as f32 * std::f32::consts::TAU;
			let (c, s) = (t.cos(), t.sin());
			let e = |v: f32| v.signum() * v.abs().powf(2.0 / ROUND);
			[HALF_LEN * e(c), HALF_WIDE * e(s)]
		})
		.collect()
}

/// Board vertices in the board's own space (z up, x along): top centre + rim, bottom centre + rim, then the sides (two per rim
/// point, own normals). Positions and normals.
fn board_model() -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
	let r = rim();
	let (top, bot) = (DECK_TOP, DECK_TOP - THICK);
	let mut ps = vec![[0.0, 0.0, top]];
	let mut ns = vec![[0.0, 0.0, 1.0]];
	ps.extend(r.iter().map(|p| [p[0], p[1], top]));
	ns.extend(r.iter().map(|_| [0.0, 0.0, 1.0]));
	ps.push([0.0, 0.0, bot]);
	ns.push([0.0, 0.0, -1.0]);
	ps.extend(r.iter().map(|p| [p[0], p[1], bot]));
	ns.extend(r.iter().map(|_| [0.0, 0.0, -1.0]));
	for p in &r {
		let n = [p[0] / (HALF_LEN * HALF_LEN), p[1] / (HALF_WIDE * HALF_WIDE), 0.0];
		let l = (n[0] * n[0] + n[1] * n[1]).sqrt().max(1e-6);
		let n = [n[0] / l, n[1] / l, 0.0];
		ps.push([p[0], p[1], top]);
		ps.push([p[0], p[1], bot]);
		ns.push(n);
		ns.push(n);
	}
	(ps, ns)
}

fn board_uvs() -> Vec<[f32; 2]> {
	let r = rim();
	// The top on the left half of the texture, the underside on the right, the sides on the rim colour.
	let top = |p: [f32; 2]| [(p[1] / HALF_WIDE + 1.0) * 0.25, (p[0] / HALF_LEN + 1.0) * 0.5];
	let bot = |p: [f32; 2]| [0.5 + (p[1] / HALF_WIDE + 1.0) * 0.25, (p[0] / HALF_LEN + 1.0) * 0.5];
	let mut uv = vec![top([0.0, 0.0])];
	uv.extend(r.iter().map(|&p| top(p)));
	uv.push(bot([0.0, 0.0]));
	uv.extend(r.iter().map(|&p| bot(p)));
	for p in &r {
		uv.push([0.01, (p[0] / HALF_LEN + 1.0) * 0.5]);
		uv.push([0.01, (p[0] / HALF_LEN + 1.0) * 0.5]);
	}
	uv
}

fn board_faces() -> Vec<[u32; 3]> {
	let n = RIM as u32;
	let (top_c, bot_c, side) = (0u32, n + 1, 2 * n + 2);
	let mut f = Vec::new();
	for k in 0..n {
		let k1 = (k + 1) % n;
		f.push([top_c, 1 + k, 1 + k1]);
		f.push([bot_c, bot_c + 1 + k1, bot_c + 1 + k]);
		let (a, b, c, d) = (side + 2 * k, side + 2 * k + 1, side + 2 * k1, side + 2 * k1 + 1);
		f.push([a, b, d]);
		f.push([a, d, c]);
	}
	f
}

/// The board's vertex count (what `combined_hero!` appended).
pub fn board_len() -> usize {
	4 * RIM + 2
}

/// Live tuning: `RAC_HOVER_TUNE=seq=S,frame=F,yaw=Y` (read once).
fn stance() -> (usize, usize, f32) {
	static S: OnceLock<(usize, usize, f32)> = OnceLock::new();
	*S.get_or_init(|| {
		let mut s = STANCE;
		if let Ok(t) = std::env::var("RAC_HOVER_TUNE") {
			for kv in t.split(',') {
				let mut it = kv.splitn(2, '=');
				match (it.next().map(str::trim), it.next().map(str::trim)) {
					(Some("seq"), Some(v)) => s.0 = v.parse().unwrap_or(s.0),
					(Some("frame"), Some(v)) => s.1 = v.parse().unwrap_or(s.1),
					(Some("yaw"), Some(v)) => s.2 = v.parse().unwrap_or(s.2),
					_ => {}
				}
			}
			eprintln!("hoverboard: stance seq {} frame {} yaw {:.2} (RAC_HOVER_TUNE)", s.0, s.1, s.2);
		}
		s
	})
}

/// Row-major 3x4 product a * b.
fn mul34(a: &M34, b: &M34) -> M34 {
	let mut o = [0.0; 12];
	for r in 0..3 {
		for c in 0..4 {
			let mut v = a[r * 4] * b[c] + a[r * 4 + 1] * b[4 + c] + a[r * 4 + 2] * b[8 + c];
			if c == 3 {
				v += a[r * 4 + 3];
			}
			o[r * 4 + c] = v;
		}
	}
	o
}

fn apply(m: &M34, v: [f32; 3], w: f32) -> [f32; 3] {
	[0, 1, 2].map(|r| m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2] + m[r * 4 + 3] * w)
}

/// Ratchet's own hoverboard race clips (Rilgar and Kalebo III; the level-0 set has them too, every other level leaves them
/// empty). Found with `RAC_HOVER_DUMP` (2026-10-27, USA disc): in all of them the ankles stand 0.27 over his feet (on the race
/// board), 0.44-0.47 apart along his model x, and he faces 47-63 degrees left of x (sideways, his front to the left of the
/// path). 82: the ride loop (15 frames). 86: a ride loop standing taller (15), 104: one crouched lower (12), taken as the
/// heel-side and toe-side carves (he faces left, so a left turn leans toward his toes). 85: the jump (23: he crouches, the
/// feet rise 0.34 with the board, he lands). 97 / 98 end lying down (falling off; not used yet).
const RIDE: usize = 82;
const CARVE_HEEL: usize = 86;
const CARVE_TOE: usize = 104;
const JUMP_CLIP: usize = 85;
/// The race level the clips are read from when the level that runs lacks them (Rilgar).
const RACE_LEVEL: usize = 5;
/// Ankle joints over the soles (the board's top goes there).
const SOLE: f32 = 0.07;
/// Share of the lean the whole body and board tilt by on top of the carve clip's own lean.
const TILT: f32 = 0.5;

/// The race clips' source when the level that runs lacks them: Ratchet's animation set of `RACE_LEVEL` (read once).
static RACE: OnceLock<Option<crate::anim::Animated>> = OnceLock::new();

/// Level load (app.rs, with the disc open): with the mod on and the level's Ratchet lacking the race clips, reads them from
/// the race level once per run.
pub fn race_clips(disc: &mut crate::iso::Disc, own: Option<&crate::anim::Animated>) {
	if !on() || own.is_some_and(has_clips) || RACE.get().is_some() {
		return;
	}
	let read = |disc: &mut crate::iso::Disc| -> Result<Option<crate::anim::Animated>, String> {
		let levels = crate::toc::read_levels(disc).map_err(|e| e.to_string())?;
		let info = levels.get(RACE_LEVEL).ok_or("no race level")?;
		let l = crate::level::Level::load(disc, info)?;
		Ok(crate::anim::animation(&l, 0)?.filter(|a| has_clips(a) && a.skeleton.parents.len() == HERO_JOINTS))
	};
	let a = read(disc).unwrap_or_else(|e| {
		eprintln!("hoverboard: race clips: {e}");
		None
	});
	eprintln!("hoverboard: race clips {} (from level {RACE_LEVEL})", if a.is_some() { "read" } else { "not found: the plain stance is used" });
	let _ = RACE.set(a);
}

fn has_clips(a: &crate::anim::Animated) -> bool {
	[RIDE, CARVE_HEEL, CARVE_TOE, JUMP_CLIP].iter().all(|&s| a.sequences.get(s).is_some_and(|s| !s.frames.is_empty()))
}

/// Frames a sequence advances per game field from frame `f` (its own rate, else the frame's; 60 Hz ticks per field).
fn rate(s: &crate::anim::Sequence, f: usize, dt: f32) -> f32 {
	let per_tick = if s.speed > 0.0 { s.speed } else { s.frames[f.min(s.frames.len() - 1)].speed };
	per_tick * dt * 60.0
}

/// Rotation about x by -`lean` through the ground point: up (0, 0, 1) tips toward +y (his left) for a positive lean.
fn tilt(lean: f32, lift: f32) -> M34 {
	let (c, s) = ((-lean).cos(), (-lean).sin());
	[1.0, 0.0, 0.0, 0.0, 0.0, c, -s, 0.0, 0.0, s, c, lift]
}

#[derive(Default)]
pub struct Hoverboard {
	said: bool,
	/// The cue tick the clocks last stepped at.
	tick: Option<u32>,
	/// The ride loop's frame clock, the jump clip's (Some while it plays), and the air flag of the last step.
	clock: f32,
	jump: Option<f32>,
	was_air: bool,
	/// The board's placement for this pose (from the drawn feet), None = hidden.
	board: Option<M34>,
}

impl Hoverboard {
	/// Each fresh hero pose (after the strafe twist, before Ragdoll Ratchet): while `cue` says he rides, the race clips (or the
	/// plain stance) replace the drawn pose. Returns retail's pose it replaced (for `strafe::pose_fix_between`); None when it
	/// left `mats` alone.
	pub(super) fn apply(&mut self, mats: &mut [M34], a: &crate::anim::Animated, cue: Cue) -> Option<Vec<M34>> {
		self.board = None;
		if !cue.riding || !on() || mats.len() != HERO_JOINTS {
			self.tick = None;
			self.jump = None;
			self.was_air = false;
			return None;
		}
		let dt = crate::retail::disc::field_dt();
		let race = if has_clips(a) { Some(a) } else { RACE.get().and_then(|r| r.as_ref()) };
		let drawn = match race {
			Some(r) => self.race_pose(r, cue, dt),
			None => self.stance_pose(a, cue)?,
		};
		if drawn.len() != mats.len() {
			return None;
		}
		let logic = mats.to_vec();
		let bob = if cue.air { 0.0 } else { BOB * (cue.tick as f32 * BOB_RATE).sin() };
		let xf = tilt(cue.lean * TILT, bob);
		for (m, d) in mats.iter_mut().zip(&drawn) {
			*m = mul34(&xf, d);
		}
		// The board under his feet, along the line through them, its top at their soles.
		let at = |j: usize| {
			let w = a.joint_world(mats, j);
			[w[3], w[7], w[11]]
		};
		let (r, l) = (at(ANKLES[0]), at(ANKLES[1]));
		let yaw = (r[1] - l[1]).atan2(r[0] - l[0]);
		let (c, s) = (yaw.cos(), yaw.sin());
		let z = (r[2] + l[2]) * 0.5 - SOLE - DECK_TOP;
		let place: M34 = [c, -s, 0.0, (r[0] + l[0]) * 0.5, s, c, 0.0, (r[1] + l[1]) * 0.5, 0.0, 0.0, 1.0, z];
		self.board = Some(mul34(&place, &tilt(cue.lean * TILT, 0.0)));
		Some(logic)
	}

	/// The race clips: the ride loop blended toward the carve of the lean's side, the jump clip once from a take-off. Steps
	/// once per new cue tick (a cached or paused pose stays the same).
	fn race_pose(&mut self, r: &crate::anim::Animated, cue: Cue, dt: f32) -> Vec<M34> {
		let ride = &r.sequences[RIDE];
		let jump = &r.sequences[JUMP_CLIP];
		if self.tick != Some(cue.tick) {
			let first = self.tick.is_none();
			self.tick = Some(cue.tick);
			if cue.air && !self.was_air && !first {
				self.jump = Some(0.0);
			}
			self.was_air = cue.air;
			self.clock = (self.clock + rate(ride, self.clock as usize, dt)).rem_euclid(ride.frames.len() as f32);
			if let Some(j) = self.jump {
				let n = j + rate(jump, j as usize, dt);
				self.jump = (n < (jump.frames.len() - 1) as f32).then_some(n);
			}
		}
		if let Some(j) = self.jump {
			let k = j as usize;
			return r.pose_frames(&jump.frames[k], &jump.frames[(k + 1).min(jump.frames.len() - 1)], j.fract());
		}
		let k = self.clock as usize % ride.frames.len();
		let w = (cue.lean.abs() / 0.4).min(1.0);
		if w < 0.02 {
			return r.pose_frames(&ride.frames[k], &ride.frames[(k + 1) % ride.frames.len()], self.clock.fract());
		}
		let carve = &r.sequences[if cue.lean > 0.0 { CARVE_TOE } else { CARVE_HEEL }];
		let kc = (self.clock / ride.frames.len() as f32 * carve.frames.len() as f32) as usize % carve.frames.len();
		r.pose_frames(&ride.frames[k], &carve.frames[kc], w)
	}

	/// No race clips: the plain stance frame (`stance`), turned so the feet lie along the board and raised onto it.
	fn stance_pose(&mut self, a: &crate::anim::Animated, _cue: Cue) -> Option<Vec<M34>> {
		let (seq, frame, yaw) = stance();
		let Some(s) = a.sequences.get(seq).filter(|s| !s.frames.is_empty()) else {
			if !self.said {
				self.said = true;
				eprintln!("hoverboard: stance sequence {seq} has no frames: the game's pose stays");
			}
			return None;
		};
		let stance = a.pose_frame(seq, frame.min(s.frames.len() - 1));
		let (c, sn) = (yaw.cos(), yaw.sin());
		// The feet's midpoint goes to the board's centre.
		let mid = {
			let at = |j: usize| {
				let w = a.joint_world(&stance, j);
				[w[3], w[7]]
			};
			let (r, l) = (at(ANKLES[0]), at(ANKLES[1]));
			[(r[0] + l[0]) * 0.5, (r[1] + l[1]) * 0.5]
		};
		let (mx, my) = (mid[0] * c - mid[1] * sn, mid[0] * sn + mid[1] * c);
		let turn: M34 = [c, -sn, 0.0, -mx, sn, c, 0.0, -my, 0.0, 0.0, 1.0, DECK_TOP];
		Some(stance.iter().map(|st| mul34(&turn, st)).collect())
	}

	/// The board's vertices and normals in his model space for this pose: placed while he rides, hidden (at the origin) otherwise.
	pub(super) fn board(&self, cue: Cue) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
		let n = board_len();
		let Some(xf) = self.board.filter(|_| cue.riding) else {
			return (vec![[0.0; 3]; n], vec![[0.0, 0.0, 1.0]; n]);
		};
		let (ps, ns) = board_model();
		(ps.iter().map(|&v| apply(&xf, v, 1.0)).collect(), ns.iter().map(|&v| apply(&xf, v, 0.0)).collect())
	}
}

/// `RAC_HOVER_DUMP=1`, at level load: per hero sequence and a few frames, how far apart the feet are along (x) and across (y)
/// the model, and the pelvis height, to pick a stance (a board stance has the feet apart along one axis, the pelvis low).
pub fn dump_stances(level: usize, a: &crate::anim::Animated) {
	if !std::env::var("RAC_HOVER_DUMP").is_ok_and(|v| v == "1") {
		return;
	}
	eprintln!("hoverboard: dump level {level}: {} sequences", a.sequences.len());
	for (i, s) in a.sequences.iter().enumerate() {
		if s.frames.is_empty() {
			continue;
		}
		let n = s.frames.len();
		let parts: Vec<String> = [0, n / 4, n / 2, 3 * n / 4, n - 1]
			.iter()
			.map(|&f| {
				let mats = a.pose_frame(i, f);
				let at = |j: usize| {
					let w = a.joint_world(&mats, j);
					[w[3], w[7], w[11]]
				};
				let (r, l, p) = (at(ANKLES[0]), at(ANKLES[1]), at(PELVIS));
				// Facing: square to the line from the left shoulder to the right one (z x right), in degrees from +x.
				let (rs, ls) = (at(SHOULDERS[0]), at(SHOULDERS[1]));
				let face = (rs[0] - ls[0]).atan2(-(rs[1] - ls[1])).to_degrees();
				format!("f{f} along {:+.2} across {:+.2} pelvis {:.2} feet {:.2}/{:.2} faces {face:+.0}", r[0] - l[0], r[1] - l[1], p[2], r[2], l[2])
			})
			.collect();
		eprintln!("hoverboard: seq {i} frames {n} | {}", parts.join(" | "));
	}
}

/// `RAC_HOVER_DUMP=1`, once: Ratchet's sequence count on every level (the race levels may carry the race's riding clips), each
/// sequence that differs from the level-0 set, and on the hoverboard race levels (Rilgar 5, Kalebo 16) every moby class with
/// its size and joints (to find the race board).
pub fn dump_race_levels() {
	static ONCE: std::sync::Once = std::sync::Once::new();
	if !std::env::var("RAC_HOVER_DUMP").is_ok_and(|v| v == "1") {
		return;
	}
	ONCE.call_once(|| {
		let run = || -> Result<(), String> {
			let iso = std::env::var("ORACLE_ISO").map_err(|_| "ORACLE_ISO not set".to_string())?;
			let mut disc = crate::iso::Disc::open(std::path::Path::new(&iso)).map_err(|e| e.to_string())?;
			let levels = crate::toc::read_levels(&mut disc).map_err(|e| e.to_string())?;
			let mut base: Option<Vec<usize>> = None;
			for (i, info) in levels.iter().enumerate() {
				let l = crate::level::Level::load(&mut disc, info)?;
				let a = crate::anim::animation(&l, 0)?;
				let counts: Vec<usize> = a.as_ref().map_or(Vec::new(), |a| a.sequences.iter().map(|s| s.frames.len()).collect());
				let diff: Vec<String> = match &base {
					Some(b) => counts.iter().enumerate().filter(|&(k, &n)| b.get(k) != Some(&n)).map(|(k, n)| format!("{k}:{n}")).collect(),
					None => Vec::new(),
				};
				eprintln!("hoverboard: race level {i}: hero {} sequences, differ from level 0: [{}]", counts.len(), diff.join(" "));
				if base.is_none() {
					base = Some(counts);
				}
				if i == 5 || i == 16 {
					let classes = crate::mobies::moby_classes(&l)?;
					let mut ids: Vec<_> = classes.keys().copied().collect();
					ids.sort();
					for id in ids {
						let c = &classes[&id];
						let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
						for p in &c.mesh.positions {
							for k in 0..3 {
								lo[k] = lo[k].min(p[k]);
								hi[k] = hi[k].max(p[k]);
							}
						}
						let seqs = crate::anim::animation(&l, id).ok().flatten().map_or(0, |a| a.sequences.iter().filter(|s| !s.frames.is_empty()).count());
						if c.mesh.positions.is_empty() {
							continue;
						}
						eprintln!(
							"hoverboard: race level {i} class {id}: {} vertices, size {:.2} x {:.2} x {:.2}, {seqs} sequences",
							c.mesh.positions.len(),
							hi[0] - lo[0],
							hi[1] - lo[1],
							hi[2] - lo[2]
						);
					}
				}
			}
			Ok(())
		};
		if let Err(e) = run() {
			eprintln!("hoverboard: race dump failed: {e}");
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn board_mesh_counts_match() {
		let (ps, ns) = board_model();
		assert_eq!(ps.len(), board_len());
		assert_eq!(ns.len(), board_len());
		assert_eq!(board_uvs().len(), board_len());
		assert!(board_faces().iter().flatten().all(|&i| (i as usize) < board_len()));
	}

	#[test]
	fn lean_tips_toward_the_left() {
		let up = apply(&tilt(0.3, 0.0), [0.0, 0.0, 1.0], 0.0);
		assert!(up[1] > 0.2 && up[2] > 0.9);
	}
}
