//! [PC] Hoverboard (HOVERBOARD_PLAN.md), the worker half: the built-in mod `hoverboard` (plugins.rs).
//!
//! The board button (d-pad Down, or L3) on the ground steps Ratchet onto the board and off again. While he rides, the game's
//! Ratchet still runs (retail's run state moves him, so walls, slopes, ledges and enemies stay retail's); between two fields `pre_field` only sets his run speed and his
//! logic facing (the words strafe_feel.rs already sets) to the board's, and hands the game a left stick pointing along the board's
//! heading, so retail never turns him by itself. The heading carves toward the player's stick, the speed builds up, coasts when
//! the stick is let go and brakes when it is pulled back. The ride reaches the renderer as `HeroParts.hoverboard` (`current`),
//! where viewer/hoverboard.rs draws the riding pose and the board. With the mod off nothing runs.
//!
//! `RAC_HOVER_LOG=1` writes `hoverboard:` lines to the run log: mounts, dismounts (with the reason), and twice a second while
//! riding the state, the board's speed against the speed retail really moved him, and the stick-to-world offset.
use crate::retail::disc::va;
use ee_oracle::Machine;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

/// The mod id (plugins.rs).
pub const ID: &str = "hoverboard";

const HERO_POS: u32 = 0x13f3d0;
const HERO_STATE: u32 = 0x1413d4;
/// Logic facing and the stick's world heading of the last field (strafe_feel.rs).
const HERO_YAW: u32 = 0x13f3e8;
const HERO_WISH: u32 = 0x13f4d0;
/// Run speed (gait, units per field: u/s times retail's dt) and retail's dt.
const RUN_SPEED: u32 = 0x13f4e4;
const DT: u32 = 0x15ed6c;
const HERO_AIRBORNE: u32 = 0x13f65e;
const BODY_FORM: u32 = 0x1413f4;
const GAME_MODE: u32 = 0x15f5a4;

/// The board button per setting `button`: d-pad Down (keyboard Down arrow; retail gives the d-pad nothing in gameplay) or L3
/// (Left Shift; Ragdoll Ratchet's button in the air, so L3 only works on the ground). Pad bits of `Machine::set_pad`'s word.
const PAD_BUTTONS: [u16; 2] = [0x4000, 0x200];
/// Game modes that only pause the ride (a menu, the map): anything else but gameplay (0) ends it.
const MENU_MODES: [u32; 2] = [1, 3];
/// Retail's run state: the one the board drives the speed in.
const RUN: u32 = 2;
/// Ground states he can step on or off the board in, and drive it in: stand, run, skid, the second run state.
const GROUND_STATES: [u32; 4] = [0, 2, 3, 0x73];
/// Air and crouch states the board stays under him in (crouch, falls, jumps, the crouch high jump, the wall jump).
const AIR_STATES: [u32; 9] = [4, 6, 7, 9, 0xa, 0xb, 0xe, 0xf, 0x11];
/// Away from the riding states longer than this (an attack, a hit, a ledge grab, a pack) he steps off the board (seconds).
const SUSPEND_MAX: f32 = 3.0;

/// Top speed per setting (units/s; retail's run tops out at about 5.7).
const TOP: [f32; 3] = [10.0, 14.0, 18.0];
/// Gain with the stick pushed along the board, coasting loss with it let go, and the brake with it pulled back (units/s^2).
const ACCEL: f32 = 9.0;
const COAST: f32 = 2.5;
const BRAKE: f32 = 20.0;
/// Carve rate at standstill per Turning setting (rad/s); at top speed it is `TURN_AT_TOP` of that.
const TURN: [f32; 2] = [3.2, 2.2];
const TURN_AT_TOP: f32 = 0.55;
/// The stick more than this far from the board's heading (rad) brakes instead of turning.
const BRAKE_ANGLE: f32 = 2.4;
/// Under this speed the heading follows the stick at once (turning on the spot), and with the stick let go the board stops.
const PIVOT_SPEED: f32 = 2.0;
const STOP_SPEED: f32 = 0.8;
/// Stick length (0..1) that counts as pushed.
const PUSHED: f32 = 0.3;
/// A field's real move under this share of the board's speed is a wall: the speed drops to the real one.
const BLOCKED: f32 = 0.5;
/// Lean per (turn rate x speed), its cap (rad) and how fast it follows (share per field).
const LEAN_K: f32 = 0.025;
const LEAN_MAX: f32 = 0.4;
const LEAN_EASE: f32 = 0.2;
/// A move bigger than this in one field is a respawn or a teleport (ragdoll.rs `JUMP`): he steps off.
const JUMP: f32 = 8.0;

/// States that end the ride at once: water, mud, the Swingshot, the ship / turret and the grind (ragdoll.rs `cancel_state`).
fn cancel_state(s: u32) -> bool {
	matches!(s, 0x33..=0x37 | 0x6a | 0x75 | 0x76 | 0x68 | 0x7b | 0x24..=0x26 | 0x2c | 0x2d | 0x32 | 0x74)
}

/// What the renderer needs of the ride this field.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Cue {
	/// On the board and drawn on it (false while the board waits hidden: an attack, a hit, a ledge grab).
	pub riding: bool,
	/// Lean into the turn (rad, positive = toward his left, the inside of a left turn).
	pub lean: f32,
	/// The board's speed (units/s).
	pub speed: f32,
	/// In the air (a jump or a fall on the board).
	pub air: bool,
	/// Counts the fields the worker ran the mod for (the hover bob's clock).
	pub tick: u32,
}

#[derive(Default)]
struct Mod {
	/// This field's pad, as `set_pad` got it (`note_pad`), and the buttons of the field before.
	buttons: u16,
	sticks: [u8; 4],
	last_buttons: u16,
	/// On the board (also while it waits hidden).
	on: bool,
	/// The board's heading (world yaw) and speed (units/s).
	heading: f32,
	speed: f32,
	/// Retail's stick-to-world turn: world heading = `off` + the stick's angle (measured, see `pre_field`).
	off: Option<f32>,
	/// The stick angle handed to the game last field (None = centred).
	handed: Option<f32>,
	/// Hero state and position after the last field.
	last_state: u32,
	last_pos: Option<[f32; 3]>,
	/// Fields away from the riding states.
	away: u32,
	lean: f32,
	tick: u32,
	cue: Cue,
}

static MOD: Mutex<Option<Mod>> = Mutex::new(None);

fn setting(key: &str) -> u8 {
	crate::plugins::value_at_start(ID, key)
}

fn log_on() -> bool {
	static L: OnceLock<bool> = OnceLock::new();
	*L.get_or_init(|| std::env::var("RAC_HOVER_LOG").is_ok_and(|v| v != "0" && !v.is_empty()))
}

/// Worker, each gameplay field before the built-in mods run: the pad `set_pad` just got.
pub fn note_pad(buttons: u16, sticks: [u8; 4]) {
	if !crate::plugins::on_at_start(ID) {
		return;
	}
	let mut g = MOD.lock().unwrap();
	let s = g.get_or_insert_with(Mod::default);
	s.buttons = buttons;
	s.sticks = sticks;
}

/// The ride of the field that just ran (retail/snapshot.rs puts it in `HeroParts`); not riding with the mod off.
pub fn current() -> Cue {
	MOD.lock().ok().and_then(|g| g.as_ref().map(|s| s.cue)).unwrap_or_default()
}

fn wrap(a: f32) -> f32 {
	let t = std::f32::consts::TAU;
	let a = a.rem_euclid(t);
	if a > std::f32::consts::PI { a - t } else { a }
}

/// A stick's angle (0 = right, pi/2 = up, as retail turns it into a world heading) and length (0..1), from its pad bytes.
fn stick(x: u8, y: u8) -> (f32, f32) {
	let f = |b: u8| ((b as f32 - 128.0) / 127.0).clamp(-1.0, 1.0);
	let (sx, sy) = (f(x), -f(y));
	(sy.atan2(sx), (sx * sx + sy * sy).sqrt().min(1.0))
}

/// The pad bytes of a full-length stick at `angle`.
fn stick_bytes(angle: f32) -> (u8, u8) {
	let b = |v: f32| (128.0 + 127.0 * v).round().clamp(0.0, 255.0) as u8;
	(b(angle.cos()), b(-angle.sin()))
}

/// One field of the board's physics: the new (heading, speed) and the turn made (rad/s), from the player's stick world heading
/// (`wish`, None = let go), with `top` and the standstill carve rate `turn`.
fn step(heading: f32, speed: f32, wish: Option<(f32, f32)>, top: f32, turn: f32, dt: f32) -> (f32, f32, f32) {
	let Some((w, len)) = wish else {
		return (heading, (speed - COAST * dt).max(0.0), 0.0);
	};
	if speed < PIVOT_SPEED {
		// Turning on the spot: no carve, so no lean.
		return (w, (speed + ACCEL * len * dt).min(top), 0.0);
	}
	let d = wrap(w - heading);
	if d.abs() > BRAKE_ANGLE {
		return (heading, (speed - BRAKE * len * dt).max(0.0), 0.0);
	}
	let rate = turn * (1.0 - (1.0 - TURN_AT_TOP) * (speed / top).min(1.0));
	let t = d.clamp(-rate * dt, rate * dt);
	// Speed builds with the stick along the board; a hard carve only holds it.
	let gain = ACCEL * len * d.cos().max(0.0);
	(wrap(heading + t), (speed + gain * dt).min(top.max(speed - COAST * dt)), t / dt)
}

/// The mod's `pre_field` (plugins.rs): mount / dismount, the board's physics and the pad and words handed to the game.
pub fn pre_field(m: &mut Machine) {
	let mut g = MOD.lock().unwrap();
	let s = g.get_or_insert_with(Mod::default);
	field(m, s);
	RIDING.store(s.on, Ordering::Relaxed);
}

/// He is on the board (also while it waits hidden): the SetState hand (`refuse_state`) reads it during the field.
static RIDING: AtomicBool = AtomicBool::new(false);
/// SetState calls refused so far (the log says the first of every `SKIM_LOG_EVERY`).
static SKIMS: AtomicU32 = AtomicU32::new(0);
const SKIM_LOG_EVERY: u32 = 50;

/// Mud's sinking states: sinking (quicksand, Aridia's mud) and the mud death.
const MUD_STATES: [u32; 2] = [0x68, 0x7b];

/// The SetState hand (strafe/chain.rs `set_state_any`), with the state SetState is asked for: true = refuse it. While he rides,
/// the mud's sinking states are refused, so the board skims over mud and quicksand (he stays in the run or the fall he was in;
/// retail asks again every tick he is on the mud, and is refused each time).
pub fn refuse_state(to: u32) -> bool {
	if !RIDING.load(Ordering::Relaxed) || !MUD_STATES.contains(&to) {
		return false;
	}
	if log_on() && SKIMS.fetch_add(1, Ordering::Relaxed) % SKIM_LOG_EVERY == 0 {
		eprintln!("hoverboard: skims over mud (SetState {to:#x} refused)");
	}
	true
}

/// One field of the mod (`pre_field`, under the lock).
fn field(m: &mut Machine, s: &mut Mod) {
	let dt = super::disc::field_dt();
	let top = TOP[setting("speed").min(2) as usize];
	let turn = TURN[setting("turn").min(1) as usize];

	let pressed = s.buttons & !s.last_buttons;
	s.last_buttons = s.buttons;

	let mode = m.rd32(va(GAME_MODE));
	if MENU_MODES.contains(&mode) && s.on {
		return;
	}
	s.tick = s.tick.wrapping_add(1);
	let form = m.rd8(BODY_FORM);
	let state = m.rd32(HERO_STATE);
	let airborne = m.rd16(HERO_AIRBORNE) != 0;
	let pos = m.rdv3(HERO_POS);
	let moved = s.last_pos.map(|p0| [pos[0] - p0[0], pos[1] - p0[1], pos[2] - p0[2]]);
	let teleport = moved.is_some_and(|d| d.iter().any(|v| v.abs() > JUMP));
	let real = moved.filter(|_| !teleport).map_or(0.0, |d| d[0].hypot(d[1]) / dt);
	let (last_state, handed) = (s.last_state, s.handed.take());
	s.last_state = state;
	s.last_pos = Some(pos);

	// Retail's world heading of the stick handed last field is the camera's turn plus that stick's angle.
	if let Some(a) = handed.filter(|_| GROUND_STATES.contains(&last_state)) {
		s.off = Some(wrap(m.rdf(HERO_WISH) - a));
	}

	let grounded = GROUND_STATES.contains(&state) && !airborne;
	let board_bit = PAD_BUTTONS[setting("button").min(1) as usize];
	let toggle = pressed & board_bit != 0;
	let limp = crate::retail::ragdoll::current().phase != crate::retail::ragdoll::Phase::Off;
	if !s.on {
		s.cue = Cue { tick: s.tick, ..Cue::default() };
		if toggle && grounded && mode == 0 && form == 0 && !limp {
			s.on = true;
			s.heading = m.rdf(HERO_YAW);
			// Stepping on while running keeps the run's speed.
			let rdt = m.rdf(DT);
			s.speed = if state == RUN && rdt > 0.0 { m.rdf(RUN_SPEED) / rdt } else { 0.0 };
			s.away = 0;
			s.lean = 0.0;
			if log_on() {
				eprintln!("hoverboard: mount (state {state:#x}, heading {:.2}, z {:.2})", s.heading, pos[2]);
			}
			m.set_pad(s.buttons & !board_bit, s.sticks);
		}
		return;
	}

	// Stepping off.
	let riding_state = GROUND_STATES.contains(&state) || AIR_STATES.contains(&state);
	s.away = if riding_state { 0 } else { s.away + 1 };
	let reason = if mode != 0 {
		Some("game mode")
	} else if form != 0 {
		Some("body form")
	} else if limp {
		Some("ragdoll")
	} else if cancel_state(state) {
		Some("state")
	} else if teleport {
		Some("teleport")
	} else if s.away as f32 * dt > SUSPEND_MAX {
		Some("away")
	} else if toggle && grounded {
		Some("button")
	} else {
		None
	};
	if let Some(r) = reason {
		s.on = false;
		s.cue = Cue { tick: s.tick, ..Cue::default() };
		if log_on() {
			eprintln!("hoverboard: dismount ({r}, state {state:#x}, speed {:.1})", s.speed);
		}
		if r == "button" {
			m.set_pad(s.buttons & !board_bit, s.sticks);
		}
		return;
	}

	// The player's stick as a world heading (needs the measured offset; until the first measure his own stick goes through).
	let (a, len) = stick(s.sticks[2], s.sticks[3]);
	let pushed = len > PUSHED;
	let mut turned = 0.0;
	if riding_state && s.off.is_some() {
		let off = s.off.unwrap_or(0.0);
		let wish = pushed.then(|| (wrap(off + a), len));
		if grounded {
			// A wall: retail moved him much less than the board's speed (only between two run fields: a start or a landing is slower).
			if state == RUN && last_state == RUN && s.speed > PIVOT_SPEED && real < s.speed * BLOCKED {
				if log_on() {
					eprintln!("hoverboard: blocked {:.1} -> {real:.1} u/s", s.speed);
				}
				s.speed = real;
			}
			let (h, v, t) = step(s.heading, s.speed, wish, top, turn, dt);
			(s.heading, s.speed, turned) = (h, v, t);
			if !pushed && s.speed < STOP_SPEED {
				s.speed = 0.0;
			}
		} else if let Some((w, _)) = wish {
			// In the air the board keeps its speed; the stick only carves it a little.
			let rate = turn * TURN_AT_TOP * 0.5;
			let t = wrap(w - s.heading).clamp(-rate * dt, rate * dt);
			s.heading = wrap(s.heading + t);
			turned = t / dt;
		}
	}

	// The pad and the words handed to the game.
	let mut sticks = s.sticks;
	if s.off.is_none() {
		// Not measured yet: his own stick goes through (and is measured next field), the board waits.
		s.handed = pushed.then_some(a);
	} else if riding_state && (s.speed > 0.0 || !grounded) {
		let ha = wrap(s.heading - s.off.unwrap_or(0.0));
		(sticks[2], sticks[3]) = stick_bytes(ha);
		s.handed = Some(ha);
		m.cpu.mem.wr32(HERO_YAW, s.heading.to_bits());
		if grounded && state == RUN {
			let rdt = m.rdf(DT);
			m.cpu.mem.wr32(RUN_SPEED, (s.speed * rdt).to_bits());
		}
	} else if riding_state {
		// Standing on the board: no stick, so he stands still on it.
		(sticks[2], sticks[3]) = (0x80, 0x80);
	}
	let buttons = if toggle { s.buttons & !board_bit } else { s.buttons };
	m.set_pad(buttons, sticks);

	let lean_want = (turned * s.speed * LEAN_K).clamp(-LEAN_MAX, LEAN_MAX);
	s.lean += (lean_want - s.lean) * LEAN_EASE;
	s.cue = Cue { riding: riding_state, lean: s.lean, speed: s.speed, air: !grounded, tick: s.tick };
	if log_on() && s.tick % 30 == 0 {
		eprintln!(
			"hoverboard: ride state {state:#x} air {} speed {:.2} real {real:.2} heading {:.2} off {:?} stick {:.2}@{:.2} lean {:.2}",
			!grounded, s.speed, s.heading, s.off, len, a, s.lean
		);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn stick_round_trip() {
		for a in [0.0f32, 1.0, -2.0, 3.0, -0.5] {
			let (x, y) = stick_bytes(a);
			let (b, len) = stick(x, y);
			assert!(wrap(a - b).abs() < 0.02, "{a} {b}");
			assert!(len > 0.98);
		}
		assert_eq!(stick(0x80, 0x80).1, 0.0);
		// Up on the stick (y byte 0) is angle pi/2.
		assert!((stick(0x80, 0x00).0 - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
	}

	#[test]
	fn step_builds_coasts_brakes() {
		let dt = 0.02;
		// Pushed along the board: gains, never past the top.
		let (_, v, _) = step(0.0, 5.0, Some((0.0, 1.0)), 14.0, 3.2, dt);
		assert!((v - (5.0 + ACCEL * dt)).abs() < 1e-5);
		assert_eq!(step(0.0, 14.0, Some((0.0, 1.0)), 14.0, 3.2, dt).1, 14.0);
		// Let go: coasts down.
		assert!((step(0.0, 5.0, None, 14.0, 3.2, dt).1 - (5.0 - COAST * dt)).abs() < 1e-5);
		// Pulled back: brakes, heading kept.
		let (h, v, _) = step(0.0, 5.0, Some((3.1, 1.0)), 14.0, 3.2, dt);
		assert_eq!(h, 0.0);
		assert!((v - (5.0 - BRAKE * dt)).abs() < 1e-5);
		// A carve turns at most the rate, slower at speed.
		let (h_slow, ..) = step(0.0, 3.0, Some((1.5, 1.0)), 14.0, 3.2, dt);
		let (h_fast, ..) = step(0.0, 14.0, Some((1.5, 1.0)), 14.0, 3.2, dt);
		assert!(h_slow > h_fast && h_slow <= 3.2 * dt + 1e-6);
		// Slow: the heading snaps to the stick.
		assert_eq!(step(0.0, 1.0, Some((1.5, 1.0)), 14.0, 3.2, dt).0, 1.5);
	}
}
