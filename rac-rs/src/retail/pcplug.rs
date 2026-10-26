//! The Plugins page (Pause -> Options -> Plugins): one On / Off row per plugin of `crate::plugins`, and a page of its own for every
//! plugin that has settings.
//!
//! Built like the Controls page (pcctl.rs): the Options list gains a row (`pcext::patch_graphics`), and the row opens a copy of the
//! retail Camera Options page with its own header and multi-choice item, so it is drawn, steered and sounds like the retail value
//! pages. The row table (24-byte records `{label id, byte*, choice 0..3}`, ended by a zero record) is data the host rewrites.
//!
//! Sections (`sections`): the On / Off list first, then one section per plugin with settings (its `Plugin::settings`, one row each,
//! cycling through the setting's two to four values). A page shows at most 7 rows. With up to 7 plugins and no settings anywhere
//! there is one section and every row is a plugin; otherwise row 0 is the "Page" stepper of pcctl.rs (Cross, Left / Right) whose
//! value names the section ("< Plugins >", "< Ray-traced shadows >"), and a section holds 6 rows (a longer list or settings list
//! is split into parts, "< Plugins 1/2 >"). A row's byte is its value (plugin rows: 0 Off, 1 On); the retail item flips or cycles
//! it and `sync` hands it to `plugins::set` / `plugins::set_setting`. With no plugin at all the page shows one inert "No plugins
//! installed" row.
//!
//! Texts: ids 0x7fb0..0x7fdf, answered by `text_hook` (`pcext::text_hook` passes them on) from a pool of their own. Plugin and
//! setting labels and setting values are the plugin's own words in every language. A value of a restart plugin that is not the
//! running one reads "<value> after restart" (so the value ids are per row), live plugins' On / Off rows use retail's own texts.
//!
//! Reserved RAM (`PBASE`, inside pcext's BASE window, above the Debug pool of pcctl.rs): the row bytes, the page copy, its header
//! and value items, its row table and the text pool. All of it is written only after `build_page` set the magic word.
use super::pcctl::{self, Pager, T5};
use super::mouse_menu;
use super::pcext::BASE;
use crate::plugins;
use ee_oracle::cpu::Cpu;
use ee_oracle::mem::Mem;
use std::sync::Mutex;

/// The region: BASE + 0xC000 .. + 0xD000 (the canary of prompts.rs skips it).
pub const PBASE: u32 = BASE + 0xC000;
pub const RANGE: std::ops::Range<u32> = PBASE..PBASE + 0x1000;
/// Row bytes: the stepper's at PBASE, row r's at PBASE + r.
const SEL: u32 = PBASE;
const MAGIC_AT: u32 = PBASE + 0xfc;
const MAGIC: u32 = 0x4755_4c50;
const PPAGE: u32 = PBASE + 0x100;
const PHDR: u32 = PBASE + 0x200;
const PVAL: u32 = PBASE + 0x280;
const PTAB: u32 = PBASE + 0x300;
/// Text pool: one 0x40-byte slot per id from `ID0`.
const POOL: u32 = PBASE + 0x400;
const SLOT: u32 = 0x40;
/// Rows of a page and records of its table (the rows and the zero record).
const ROWS: usize = 7;
const TABLE_RECS: usize = ROWS + 1;

/// Text ids: the fixed ones, the rows' labels `ID_NAME + r` and their value texts `ID_V[c] + r` (choice c = 0..3), r = 0..7.
const ID0: u32 = 0x7fb0;
pub const ID_END: u32 = 0x7fe0;
pub const ID_PLUGINS: u32 = 0x7fb0;
const ID_HEADER: u32 = 0x7fb1;
pub const ID_DESC: u32 = 0x7fb2;
const ID_PAGE: u32 = 0x7fb3;
const ID_SEL: u32 = 0x7fb4;
const ID_NONE: u32 = 0x7fb5;
const ID_BLANK: u32 = 0x7fb6;
const ID_NAME: u32 = 0x7fb8;
const ID_V: [u32; 4] = [0x7fc0, 0x7fc8, 0x7fd0, 0x7fd8];
const ID_ON: u32 = 0x4f5a;
const ID_OFF: u32 = 0x4f5b;

static PLUGINS_T: T5 = [b"Plugins", b"Plugins", b"Plugins", b"Plugins", b"Plugin"];
static DESC: T5 = [b"Switch optional features on or off", b"Activer ou couper des extras", b"Zusatzfunktionen ein- oder aus", b"Activa o desactiva extras", b"Attiva o disattiva extra"];
static NONE: T5 = [b"No plugins installed", b"Aucun plugin", b"Keine Plugins", b"Ning\x9cn plugin", b"Nessun plugin"];
/// Appended to a restart plugin's value that waits for a restart.
static AFTER: T5 = [b" after restart", b" au red\x8cmarrage", b" nach Neustart", b" al reiniciar", b" al riavvio"];
/// Retail's own On / Off texts per language, for the rows of restart plugins (their value ids are ours).
static ON_T: T5 = [b"On", b"Oui", b"An", b"S\x94", b"S\x94"];
static OFF_T: T5 = [b"Off", b"Non", b"Aus", b"No", b"No"];

// ---- the model: plugins, sections, rows --------------------------------------------------------------------------------------

/// What the page needs to know of a plugin (`model`; tests build their own).
struct PInfo {
	name: &'static str,
	restart: bool,
	at_start: bool,
	settings: Vec<SInfo>,
}

struct SInfo {
	name: &'static str,
	values: &'static [&'static str],
	at_start: u8,
}

fn model() -> Vec<PInfo> {
	plugins::list()
		.iter()
		.enumerate()
		.map(|(i, p)| PInfo {
			name: p.name(),
			restart: p.needs_restart(),
			at_start: plugins::chosen_at_start(i),
			settings: p.settings().iter().enumerate().map(|(k, s)| SInfo { name: s.name, values: s.values, at_start: plugins::setting_at_start(i, k) }).collect(),
		})
		.collect()
}

/// A section of the page: part `part` of `parts` of the On / Off list, or of plugin `plugin`'s settings; `first` is the first plugin
/// or setting it shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sec {
	List { first: usize, part: usize, parts: usize },
	Settings { plugin: usize, first: usize, part: usize, parts: usize },
}

/// What a row shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
	Plugin(usize),
	Setting(usize, usize),
	Blank,
}

/// The sections for the plugins `m` and the rows a section holds (7 on a page without the stepper, else 6).
fn sections(m: &[PInfo]) -> (Vec<Sec>, usize) {
	let single = m.len() <= ROWS && m.iter().all(|p| p.settings.is_empty());
	let per = if single { ROWS } else { ROWS - 1 };
	let parts = m.len().div_ceil(per).max(1);
	let mut v: Vec<Sec> = (0..parts).map(|part| Sec::List { first: part * per, part, parts }).collect();
	for (plugin, p) in m.iter().enumerate().filter(|(_, p)| !p.settings.is_empty()) {
		let parts = p.settings.len().div_ceil(per);
		v.extend((0..parts).map(|part| Sec::Settings { plugin, first: part * per, part, parts }));
	}
	(v, per)
}

/// The row `r` of section `sec` (an index into `sections`).
fn row(m: &[PInfo], sec: usize, r: usize) -> Row {
	let (secs, per) = sections(m);
	let paged = secs.len() > 1;
	let Some(k) = r.checked_sub(paged as usize).filter(|&k| k < per) else { return Row::Blank };
	match secs.get(sec.min(secs.len() - 1)) {
		Some(&Sec::List { first, .. }) if first + k < m.len() => Row::Plugin(first + k),
		Some(&Sec::Settings { plugin, first, .. }) if first + k < m[plugin].settings.len() => Row::Setting(plugin, first + k),
		_ => Row::Blank,
	}
}

// ---- texts ------------------------------------------------------------------------------------------------------------------

/// "name" or "name 2/3" for part `part` of `parts`.
fn part_name(name: &[u8], part: usize, parts: usize) -> Vec<u8> {
	let mut v = name.to_vec();
	if parts > 1 {
		v.extend(format!(" {}/{}", part + 1, parts).bytes());
	}
	v
}

/// Every text of the page in `lang`, for section `sec`, from the plugins `m`.
fn text(id: u32, lang: usize, sec: usize, m: &[PInfo]) -> Vec<u8> {
	let l = lang.min(4);
	let fixed = |t: &T5| t[l].to_vec();
	let after = |mut v: Vec<u8>, waits: bool| {
		if waits {
			v.extend_from_slice(AFTER[l]);
		}
		v
	};
	match id {
		ID_PLUGINS | ID_HEADER => fixed(&PLUGINS_T),
		ID_DESC => fixed(&DESC),
		ID_PAGE => fixed(&pcctl::PAGE),
		ID_SEL => {
			let (secs, _) = sections(m);
			pcctl::stepper(&match secs[sec.min(secs.len() - 1)] {
				Sec::List { part, parts, .. } => part_name(PLUGINS_T[l], part, parts),
				Sec::Settings { plugin, part, parts, .. } => part_name(m[plugin].name.as_bytes(), part, parts),
			})
		}
		ID_NONE => fixed(&NONE),
		_ if (ID_NAME..ID_NAME + ROWS as u32).contains(&id) => match row(m, sec, (id - ID_NAME) as usize) {
			Row::Plugin(i) => m[i].name.as_bytes().to_vec(),
			Row::Setting(i, k) => m[i].settings[k].name.as_bytes().to_vec(),
			Row::Blank => Vec::new(),
		},
		_ if (ID_V[0]..ID_V[0] + 4 * 8).contains(&id) => {
			let (c, r) = (((id - ID_V[0]) / 8) as usize, ((id - ID_V[0]) % 8) as usize);
			match row(m, sec, r) {
				// A restart plugin: the value that differs from the running one waits for a restart.
				Row::Plugin(i) if c < 2 => after(fixed(if c == 1 { &ON_T } else { &OFF_T }), m[i].restart && (c == 1) != m[i].at_start),
				Row::Setting(i, k) => {
					let s = &m[i].settings[k];
					s.values.get(c).map_or(Vec::new(), |v| after(v.as_bytes().to_vec(), m[i].restart && c as u8 != s.at_start))
				}
				_ => Vec::new(),
			}
		}
		_ => Vec::new(),
	}
}

/// The text lookup hand for the page's ids (`pcext::text_hook`, `ID0..ID_END`): the string goes into the id's pool slot and its
/// address into v0, in the game's language `lang`.
pub fn text_hook(c: &mut Cpu, id: u32, lang: usize) -> bool {
	let sec = state(|st| st.pager.sec);
	let s = text(id, lang, sec, &model());
	let at = POOL + SLOT * (id - ID0);
	let n = s.len().min(SLOT as usize - 1);
	for (i, b) in s[..n].iter().enumerate() {
		c.mem.wr8(at + i as u32, *b);
	}
	c.mem.wr8(at + n as u32, 0);
	c.s(2, at as u64);
	true
}

// ---- the table and the page -------------------------------------------------------------------------------------------------

/// The row table of section `sec` (the zero record ends it; `write_table` pads). A restart plugin's row and every setting row use
/// the page's own value ids, a live plugin's row retail's Off / On.
fn table(m: &[PInfo], sec: usize) -> Vec<u32> {
	if m.is_empty() {
		return vec![ID_NONE, PBASE + 1, ID_BLANK, ID_BLANK, 0, 0];
	}
	let mut t = Vec::new();
	if sections(m).0.len() > 1 {
		t.extend([ID_PAGE, SEL, ID_SEL, ID_SEL, 0, 0]);
	}
	for r in 0..ROWS {
		let ru = r as u32;
		let v = |c: usize| ID_V[c] + ru;
		match row(m, sec, r) {
			Row::Plugin(i) if m[i].restart => t.extend([ID_NAME + ru, PBASE + ru, v(0), v(1), 0, 0]),
			Row::Plugin(_) => t.extend([ID_NAME + ru, PBASE + ru, ID_OFF, ID_ON, 0, 0]),
			Row::Setting(i, k) => {
				let n = m[i].settings[k].values.len();
				t.extend([ID_NAME + ru, PBASE + ru, v(0), v(1), if n > 2 { v(2) } else { 0 }, if n > 3 { v(3) } else { 0 }]);
			}
			Row::Blank => {}
		}
	}
	t
}

/// Writes the row table, padded with zero records to its capacity.
fn write_table(mem: &mut Mem, t: &[u32]) {
	let mut t = t.to_vec();
	t.resize(TABLE_RECS * 6, 0);
	for (i, w) in t.iter().enumerate() {
		let a = PTAB + 4 * i as u32;
		if mem.rd32(a) != *w {
			mem.wr32(a, *w);
		}
	}
}

/// The row bytes of section `sec` from the plugins' values (a plugin row's byte is its value, a setting row's its value index, the
/// others 0). Row 0 of a paged page is the stepper's byte: it keeps its value (the pager reads its changes).
fn write_bytes(mem: &mut Mem, m: &[PInfo], sec: usize) {
	let paged = sections(m).0.len() > 1;
	for r in (paged as usize)..ROWS {
		let v = match row(m, sec, r) {
			Row::Plugin(i) => plugins::chosen(i) as u8,
			Row::Setting(i, k) => plugins::setting(i, k),
			Row::Blank => 0,
		};
		let a = PBASE + r as u32;
		if mem.rd8(a) != v {
			mem.wr8(a, v);
		}
	}
}

/// Builds the page in reserved RAM: a copy of the Camera Options page `cp` (header item `hdr`, multi-choice item `val`), as
/// `pcctl::build_page` builds the Controls page; `sound_seq` is the Sound page's taller value panel. The caller repoints the Options
/// list afterwards. A rebuild after a level load keeps the section the player left.
pub fn build_page(mem: &mut Mem, cp: u32, hdr: u32, val: u32, sound_seq: Option<u32>) {
	pcctl::copy_words(mem, cp, PPAGE, 0x60);
	for o in (0x60..0x100).step_by(4) {
		mem.wr32(PPAGE + o, 0);
	}
	if let Some(seq) = sound_seq {
		mem.wr32(PPAGE + 0xc, seq);
	}
	for o in (0x40..0x60).step_by(4) {
		match mem.rd32(PPAGE + o) {
			a if a == hdr => mem.wr32(PPAGE + o, PHDR),
			a if a == val => mem.wr32(PPAGE + o, PVAL),
			_ => {}
		}
	}
	pcctl::copy_words(mem, hdr, PHDR, 0x58);
	mem.wr32(PHDR + 0x34, ID_HEADER);
	pcctl::copy_words(mem, val, PVAL, 0x40);
	mem.wr32(PVAL + 0x34, PTAB);
	mem.wr32(PVAL + 0x38, 0);
	mem.wr32(PVAL + 0x3c, 0);
	let fresh = mem.rd32(MAGIC_AT) != MAGIC;
	let m = model();
	state(|st| {
		if fresh {
			mem.wr8(SEL, 0);
			st.pager = Pager { sec: 0, seen: 0xff };
		}
		write_table(mem, &table(&m, st.pager.sec));
		write_bytes(mem, &m, st.pager.sec);
	});
	mem.wr32(MAGIC_AT, MAGIC);
}

/// The page's address (the Options record's argument).
pub const PAGE_AT: u32 = PPAGE;

// ---- worker -----------------------------------------------------------------------------------------------------------------

struct St {
	pager: Pager,
	/// Fields Left / Right have been held (auto-repeat).
	held: [u32; 2],
	/// `sync` has put the plugins' values into the row bytes once this session (RAM from a savestate may hold older ones).
	synced: bool,
}

fn state<R>(f: impl FnOnce(&mut St) -> R) -> R {
	static ST: Mutex<St> = Mutex::new(St { pager: Pager { sec: 0, seen: 0xff }, held: [0; 2], synced: false });
	f(&mut ST.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Worker, each field of a level (`pcext::pre_field`): the section stepper, when the page is in RAM and has sections. A step
/// rewrites the table and puts the new section's values into the row bytes.
pub fn field(mem: &mut Mem, level: u32, buttons: u16) {
	let m = model();
	let nsec = sections(&m).0.len();
	if mem.rd32(MAGIC_AT) != MAGIC || nsec < 2 {
		return;
	}
	state(|st| {
		let mut dir = [false; 2];
		for (i, bit) in [mouse_menu::LEFT, mouse_menu::RIGHT].into_iter().enumerate() {
			st.held[i] = if buttons & bit != 0 { st.held[i].saturating_add(1) } else { 0 };
			dir[i] = pcctl::repeat_due(st.held[i]);
		}
		let sel = mem.rd8(SEL);
		if let Some(s) = pcctl::pager_step(&mut st.pager, sel, dir, nsec, || pcctl::on_selector(mem, level, PPAGE, PVAL)) {
			st.pager.sec = s;
			write_table(mem, &table(&m, s));
			write_bytes(mem, &m, s);
		}
	});
}

/// Worker, every field after the page patch: the rows the retail item flipped or cycled go to `plugins::set` /
/// `plugins::set_setting`. True when something changed (the caller remembers it in the options file). Does nothing until
/// `build_page` ran on this RAM.
pub fn sync(mem: &mut Mem) -> bool {
	if mem.rd32(MAGIC_AT) != MAGIC {
		return false;
	}
	let m = model();
	let (sec, first) = state(|st| (st.pager.sec, !std::mem::replace(&mut st.synced, true)));
	if first {
		write_bytes(mem, &m, sec);
		return false;
	}
	let paged = sections(&m).0.len() > 1;
	let mut changed = false;
	for r in (paged as usize)..ROWS {
		let a = PBASE + r as u32;
		let b = mem.rd8(a);
		match row(&m, sec, r) {
			Row::Plugin(i) => changed |= plugins::set(i, b != 0),
			Row::Setting(i, k) => {
				changed |= plugins::set_setting(i, k, b);
				// A byte past the last value (it never should be) reads as the last one.
				let v = plugins::setting(i, k);
				if v != b {
					mem.wr8(a, v);
				}
			}
			// The "No plugins installed" row only shows text.
			Row::Blank if b != 0 => mem.wr8(a, 0),
			Row::Blank => {}
		}
	}
	changed
}

#[cfg(test)]
mod tests {
	use super::*;

	fn plugin(name: &'static str, restart: bool, at_start: bool, settings: Vec<SInfo>) -> PInfo {
		PInfo { name, restart, at_start, settings }
	}

	fn quality(at_start: u8) -> SInfo {
		SInfo { name: "Quality", values: &["Low", "Medium", "High"], at_start }
	}

	#[test]
	fn region_and_ids_fit() {
		assert!(RANGE.start >= pcctl::DBG_RANGE.end && RANGE.end <= BASE + 0x10000);
		assert!(POOL + SLOT * (ID_END - ID0) == RANGE.end);
		assert!(PTAB + 24 * TABLE_RECS as u32 <= POOL && PVAL + 0x40 <= PTAB && PHDR + 0x58 <= PVAL && PPAGE + 0x100 <= PHDR && MAGIC_AT + 4 <= PPAGE);
		assert!(ID0 >= pcctl::ID_DBG_END && ID_V[3] + 8 == ID_END && ID_NAME + ROWS as u32 <= ID_V[0] && ID_BLANK < ID_NAME);
	}

	/// Up to 7 plugins without settings: one section, no stepper. More plugins, or any settings: the stepper, 6 rows a section,
	/// the list first, then each plugin's settings.
	#[test]
	fn sections_list_first_then_settings() {
		let plain = |n: usize| (0..n).map(|_| plugin("P", false, false, Vec::new())).collect::<Vec<_>>();
		assert_eq!(sections(&plain(0)).0, [Sec::List { first: 0, part: 0, parts: 1 }]);
		assert_eq!(sections(&plain(7)), (vec![Sec::List { first: 0, part: 0, parts: 1 }], 7));
		assert_eq!(sections(&plain(8)).0.len(), 2);
		assert_eq!((row(&plain(3), 0, 2), row(&plain(3), 0, 3)), (Row::Plugin(2), Row::Blank));
		assert_eq!((row(&plain(8), 0, 0), row(&plain(8), 0, 1), row(&plain(8), 1, 1), row(&plain(8), 1, 3)), (Row::Blank, Row::Plugin(0), Row::Plugin(6), Row::Blank));
		let many: Vec<SInfo> = (0..8).map(|_| quality(0)).collect();
		let m = vec![plugin("A", false, false, Vec::new()), plugin("B", true, false, vec![quality(1)]), plugin("C", false, true, many)];
		let (secs, per) = sections(&m);
		assert_eq!(per, 6);
		assert_eq!(
			secs,
			[
				Sec::List { first: 0, part: 0, parts: 1 },
				Sec::Settings { plugin: 1, first: 0, part: 0, parts: 1 },
				Sec::Settings { plugin: 2, first: 0, part: 0, parts: 2 },
				Sec::Settings { plugin: 2, first: 6, part: 1, parts: 2 }
			]
		);
		assert_eq!((row(&m, 0, 0), row(&m, 0, 3), row(&m, 0, 4)), (Row::Blank, Row::Plugin(2), Row::Blank));
		assert_eq!((row(&m, 1, 1), row(&m, 1, 2), row(&m, 3, 2), row(&m, 3, 3)), (Row::Setting(1, 0), Row::Blank, Row::Setting(2, 7), Row::Blank));
	}

	#[test]
	fn tables_follow_the_section() {
		assert_eq!(table(&[], 0), [ID_NONE, PBASE + 1, ID_BLANK, ID_BLANK, 0, 0]);
		let m = vec![plugin("RT", true, false, Vec::new()), plugin("Live", false, false, Vec::new())];
		assert_eq!(table(&m, 0), [ID_NAME, PBASE, ID_V[0], ID_V[1], 0, 0, ID_NAME + 1, PBASE + 1, ID_OFF, ID_ON, 0, 0]);
		let m = vec![plugin("A", false, false, Vec::new()), plugin("B", true, false, vec![quality(1), SInfo { name: "Mode", values: &["X", "Y"], at_start: 0 }])];
		let t = table(&m, 0);
		assert_eq!(&t[..6], [ID_PAGE, SEL, ID_SEL, ID_SEL, 0, 0]);
		assert_eq!(&t[6..], [ID_NAME + 1, PBASE + 1, ID_OFF, ID_ON, 0, 0, ID_NAME + 2, PBASE + 2, ID_V[0] + 2, ID_V[1] + 2, 0, 0]);
		let t = table(&m, 1);
		assert_eq!(&t[6..], [ID_NAME + 1, PBASE + 1, ID_V[0] + 1, ID_V[1] + 1, ID_V[2] + 1, 0, ID_NAME + 2, PBASE + 2, ID_V[0] + 2, ID_V[1] + 2, 0, 0]);
	}

	#[test]
	fn texts_name_the_section_and_mark_values_that_wait() {
		let m = vec![plugin("Ray-traced shadows", true, false, vec![quality(1)]), plugin("Other", false, false, Vec::new())];
		assert_eq!(text(ID_SEL, 0, 0, &m), b"< Plugins >");
		assert_eq!(text(ID_SEL, 0, 1, &m), b"< Ray-traced shadows >");
		assert_eq!(text(ID_NAME + 1, 0, 0, &m), b"Ray-traced shadows");
		assert_eq!(text(ID_V[1] + 1, 0, 0, &m), b"On after restart");
		assert_eq!(text(ID_V[0] + 1, 0, 0, &m), b"Off");
		assert_eq!(text(ID_V[0], 2, 0, &plain_restart_on()), b"Aus nach Neustart");
		assert_eq!(text(ID_NAME + 1, 0, 1, &m), b"Quality");
		assert_eq!((text(ID_V[1] + 1, 0, 1, &m), text(ID_V[2] + 1, 0, 1, &m), text(ID_V[3] + 1, 0, 1, &m)), (b"Medium".to_vec(), b"High after restart".to_vec(), Vec::new()));
		let many: Vec<SInfo> = (0..7).map(|_| quality(0)).collect();
		assert_eq!(text(ID_SEL, 0, 2, &[plugin("Big", false, false, many)]), b"< Big 2/2 >");
		for lang in 0..5 {
			for sec in 0..2 {
				for id in ID0..ID_END {
					let s = text(id, lang, sec, &m);
					assert!(s.len() < SLOT as usize - 1 && !s.contains(&b'%') && !s.contains(&b'+'), "{id:#x} {lang}");
					assert!(s.iter().all(|&b| (0x20..0x7f).contains(&b) || (0x80..=0xb0).contains(&b)), "{id:#x} {lang}: {s:?}");
				}
			}
		}
	}

	fn plain_restart_on() -> Vec<PInfo> {
		vec![plugin("RT", true, true, Vec::new())]
	}
}
