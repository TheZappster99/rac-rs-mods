//! [PC] Slam Shockwave, the built-in mod `slam-shockwave` (plugins.rs): when a wrench slam (the Hyper-Strike, Square in the air)
//! lands, every crate within the chosen radius breaks, as the slam does in the later games. No retail counterpart.
//!
//! Each field `pre_field` watches the hero state: the slam is state 0x14 (RAGDOLL_PLAN.md "Hero states"), and the field it is on
//! the ground (or leaves 0x14 while on the ground) is the impact. Then it walks the moby table for live crates near Ratchet and
//! posts each one a wrench hit through the game's own hit path, the two functions every attack uses (rac1.json `hit_desc_build`
//! and `hit_ring_write`, the same path Arsenal Mastery's shockwave takes). The crate reads the hit on its next update and breaks
//! as if the wrench struck it: its debris, sound and drop are retail's. With the mod off nothing runs.
//!
//! `RAC_SLAM_LOG=1` writes a `slamwave:` line per impact (position, crates in range, hits posted) to the run log.
use crate::retail::disc::va;
use ee_oracle::Machine;
use std::sync::Mutex;

/// The mod id (plugins.rs).
pub const ID: &str = "slam-shockwave";

const HERO_POS: u32 = 0x13f3d0;
const HERO_STATE: u32 = 0x1413d4;
const HERO_AIRBORNE: u32 = 0x13f65e;
const HERO_MOBY: u32 = 0x1413d0;
const GAME_MODE: u32 = 0x15f5a4;
/// The wrench slam (ragdoll.rs `SAVE_STATES`).
const SLAM_STATE: u32 = 0x14;

/// Crate classes: bolt 500, nanotech 501, ammo 511 and the two other breakable boxes (sound.rs `crate_break`).
const CRATES: [u16; 5] = [500, 501, 502, 505, 511];
/// Moby slot layout (rac1.json `mobys`): position, state byte (0x80 and up = deleted), hit queue slot (0xff = no hit waiting),
/// class word.
const M_POS: u32 = 0x10;
const M_STATE: u32 = 0x20;
const M_HIT_SLOT: u32 = 0xa4;
const M_CLASS: u32 = 0xa6;
const STRIDE: u32 = 0x100;
const NO_HIT: u8 = 0xff;

/// Game functions by their NTSC numbers (omni/rac1_funcs.txt): build a hit descriptor, post it on a victim.
const HIT_DESC_BUILD: u32 = 0x259498;
const HIT_RING_WRITE: u32 = 0x259698;
/// The descriptor's kind bytes (rac1.json `hit_desc`): type +0x18 (wrench 0), strength class +0x19 (1), attacker class +0x1a
/// (the wrench, 71); mask 0x10000 is an ordinary attack (Arsenal Mastery's decoy and flame hits).
const KIND_TYPE: u32 = 0x18;
const KIND_STRENGTH: u32 = 0x19;
const KIND_CLASS: u32 = 0x1a;
const WRENCH_CLASS: u16 = 71;
const HIT_MASK: u32 = 0x10000;
/// More than any crate's health: one hit breaks it.
const DAMAGE: f32 = 10.0;
/// A crate this far above or below Ratchet's feet is on another floor and stays.
const REACH_Z: f32 = 3.0;
/// At most this many crates per slam (the hit queue holds 64 entries for everything that tick).
const MAX_HITS: usize = 24;

#[derive(Default)]
struct State {
	/// The slam of the hero state now has struck already.
	struck: bool,
	slams: u32,
}

static ST: Mutex<State> = Mutex::new(State { struck: false, slams: 0 });

fn log_on() -> bool {
	static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
	*ON.get_or_init(|| std::env::var("RAC_SLAM_LOG").is_ok_and(|v| v == "1"))
}

fn radius() -> f32 {
	[3.0, 5.0, 8.0][crate::plugins::value_at_start(ID, "radius").min(2) as usize]
}

/// The mod's `pre_field` (plugins.rs).
pub fn pre_field(m: &mut Machine) {
	let Ok(mut s) = ST.lock() else { return };
	if m.rd32(va(GAME_MODE)) != 0 {
		return;
	}
	let state = m.rd32(HERO_STATE);
	let grounded = m.rd16(HERO_AIRBORNE) == 0;
	if state != SLAM_STATE {
		// A slam that left its state on the ground before a field saw it grounded in it still lands here.
		let missed = !s.struck && s.slams > 0 && grounded;
		s.slams = 0;
		s.struck = false;
		if !missed {
			return;
		}
	} else {
		s.slams += 1;
		if s.struck || !grounded {
			return;
		}
	}
	s.struck = true;
	let pos = m.rdv3(HERO_POS);
	strike(m, pos);
}

/// Posts a wrench hit on every live crate within the radius of `at`.
fn strike(m: &mut Machine, at: [f32; 3]) {
	let r = radius();
	let level = m.rd8(super::ram_globals::LEVEL) as u32;
	let (s, e) = super::ram_globals::table(level);
	let (s, e) = (m.rd32(s) & 0x01ff_ffff, m.rd32(e) & 0x01ff_ffff);
	if s == 0 || e < s || e - s > STRIDE * 4096 {
		return;
	}
	let mut near: Vec<(f32, u32)> = Vec::new();
	for a in (s..e).step_by(STRIDE as usize) {
		let st = m.rd8(a + M_STATE);
		if st == 0xff {
			break;
		}
		if st >= 0x80 || !CRATES.contains(&m.rd16(a + M_CLASS)) {
			continue;
		}
		let p = m.rdv3(a + M_POS);
		let (dx, dy, dz) = (p[0] - at[0], p[1] - at[1], p[2] - at[2]);
		let d = (dx * dx + dy * dy).sqrt();
		if d <= r && dz.abs() <= REACH_Z {
			near.push((d, a));
		}
	}
	near.sort_by(|a, b| a.0.total_cmp(&b.0));
	near.truncate(MAX_HITS);
	let hero = m.rd32(HERO_MOBY) & 0x01ff_ffff;
	let mut posted = 0;
	let mut why = None;
	for &(_, victim) in &near {
		match hit(m, hero, victim, at) {
			Ok(true) => posted += 1,
			Ok(false) => {}
			Err(e) => {
				why = Some(e);
				break;
			}
		}
	}
	if log_on() || why.is_some() {
		eprintln!(
			"slamwave: slam at {:.1} {:.1} {:.1}: {} crates within {r} m, {posted} hits posted{}",
			at[0],
			at[1],
			at[2],
			near.len(),
			why.map_or(String::new(), |w| format!(", stopped: {w}"))
		);
	}
}

/// Builds a wrench hit on the game stack and posts it on `victim`; true when the victim then holds a hit.
fn hit(m: &mut Machine, attacker: u32, victim: u32, from: [f32; 3]) -> Result<bool, String> {
	let c = &mut **m.cpu;
	let sp = c.g(29) as u32;
	let desc = sp.wrapping_sub(0x200) & !0xf;
	let dir = desc + 0x40;
	let below = desc.wrapping_sub(0x400);
	let saved: Vec<u8> = (0..0x600).map(|i| c.mem.rd8(below + i)).collect();
	let (px, py) = (f32::from_bits(c.mem.rd32(victim + M_POS)), f32::from_bits(c.mem.rd32(victim + M_POS + 4)));
	let (dx, dy) = (px - from[0], py - from[1]);
	let l = (dx * dx + dy * dy).sqrt().max(0.01);
	for (i, v) in [dx / l, dy / l, 0.0, 0.0].into_iter().enumerate() {
		c.mem.wr32(dir + 4 * i as u32, f32::to_bits(v));
	}
	let call_sp = Some(below.wrapping_sub(0x40));
	let r = crate::omni::call_builtin(c, HIT_DESC_BUILD, &[desc, attacker, HIT_MASK, dir], &[DAMAGE], call_sp).and_then(|_| {
		c.mem.wr8(desc + KIND_TYPE, 0);
		c.mem.wr8(desc + KIND_STRENGTH, 1);
		c.mem.wr16(desc + KIND_CLASS, WRENCH_CLASS);
		crate::omni::call_builtin(c, HIT_RING_WRITE, &[victim, desc], &[], call_sp)
	});
	for (i, b) in saved.into_iter().enumerate() {
		c.mem.wr8(below + i as u32, b);
	}
	r.map(|_| c.mem.rd8(victim + M_HIT_SLOT) != NO_HIT)
}
