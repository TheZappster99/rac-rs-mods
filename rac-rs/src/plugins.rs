//! [PC] Plugins: optional features that are not part of the retail game and that the player switches On / Off in the game's own
//! pause menu (Pause -> Options -> Plugins, retail/pcplug.rs). No retail counterpart.
//!
//! A plugin is a Rust type that implements `Plugin` and is listed in `PLUGINS`. It is compiled into the game like the rest of the
//! PC code: it may live anywhere in the crate (the renderer, the retail driver, ...) and only asks this module whether it is on.
//! Two kinds:
//! - Live plugins (`needs_restart` false) follow the row at once: `on` changes in the same field, `on_change` is called, and
//!   `pre_field` runs before each field of a level while the plugin is on.
//! - Restart plugins (`needs_restart` true) are read once at start-up (`on_at_start`), for things that are fixed when the window or
//!   the GPU device is made (the ray-traced shadows need a device feature). The row still flips and is remembered; the menu says
//!   "after restart" on the value that is not the running one.
//!
//! Settings: a plugin may list settings (`Plugin::settings`), each a row of two to four named values. The Plugins page gives such a
//! plugin a page of its own after the On / Off list (the Page stepper), so the plugin never builds a menu itself; it reads its
//! values with `value` (live plugins, `on_setting` tells of a change) or `value_at_start` (restart plugins).
//!
//! The choices are remembered in the PC options file (retail/pcopts.rs keeps the keys it does not model; pcext.rs writes them):
//! - `plugins=rt_shadows:on,other:off`: the plugins whose value differs from their default; entries for plugins this build does
//!   not have are kept.
//! - `plugin.<id>=<setting>:<value>,...`: a plugin's settings that differ from their default, by value name (lower case); unknown
//!   settings are kept, and the keys of plugins this build does not have are never touched.
//!
//! A plugin's environment switch (`env_force`, set to 1) forces it on whatever the menu says, as the dev switches always did.
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

/// One setting of a plugin: a menu row whose value cycles through `values`.
pub struct Setting {
	/// The key in the options file: lower case, no spaces, ',' or ':'.
	pub id: &'static str,
	/// The row's label: plain ASCII letters, digits, spaces and '-' (the game's fonts), at most 24 characters.
	pub name: &'static str,
	/// Two to four value names (the retail row holds four choices), same characters, at most 16 each.
	pub values: &'static [&'static str],
	/// The value a new player has (index into `values`).
	pub default: u8,
}

pub trait Plugin: Sync {
	/// The key in the options file: lower case, no spaces, ',' or ':'.
	fn id(&self) -> &'static str;
	/// The menu row's label: plain ASCII letters, digits, spaces and '-' (the game's fonts), at most 24 characters.
	fn name(&self) -> &'static str;
	/// The value a new player has.
	fn default_on(&self) -> bool {
		false
	}
	/// True when the plugin is only read at start-up (module doc).
	fn needs_restart(&self) -> bool {
		false
	}
	/// An environment variable that forces the plugin on when it is "1".
	fn env_force(&self) -> Option<&'static str> {
		None
	}
	/// The plugin's settings, in menu order (none: the plugin has no page of its own).
	fn settings(&self) -> &'static [Setting] {
		&[]
	}
	/// A live plugin was switched by the player (worker thread).
	fn on_change(&self, _on: bool) {}
	/// A live plugin's setting `id` was changed by the player to value `value` (worker thread).
	fn on_setting(&self, _id: &'static str, _value: u8) {}
	/// Worker, before each field of a level while the plugin is on (after the PC camera and aim code).
	fn pre_field(&self, _m: &mut ee_oracle::Machine) {}
}

// ---- the plugins of this build ----------------------------------------------------------------------------------------------

/// Ray-traced sun shadows (viewer/rtshadow.rs): a Vulkan adapter with hardware ray queries, Lighting Enhanced and Dynamic shadows
/// on. The device feature is requested when the window opens, so it is a restart plugin.
struct RtShadows;

impl Plugin for RtShadows {
	fn id(&self) -> &'static str {
		"rt_shadows"
	}
	fn name(&self) -> &'static str {
		"Ray-traced shadows"
	}
	fn needs_restart(&self) -> bool {
		true
	}
	fn env_force(&self) -> Option<&'static str> {
		Some("RAC_RT_SHADOWS")
	}
}

/// Ray-traced sun rays (viewer/rtrays.rs): light shafts in the air, traced through the same structure as the shadows (either
/// plugin brings it up), in place of the screen-space Sun rays. Lighting Enhanced, Dynamic shadows and Sun rays on; the Sun rays
/// level picks how many samples each pixel takes. A restart plugin for the same reason as the shadows.
struct RtSunRays;

impl Plugin for RtSunRays {
	fn id(&self) -> &'static str {
		"rt_sun_rays"
	}
	fn name(&self) -> &'static str {
		"Ray-traced sun rays"
	}
	fn needs_restart(&self) -> bool {
		true
	}
	fn env_force(&self) -> Option<&'static str> {
		Some("RAC_RT_RAYS")
	}
}

/// Every plugin, in menu order. A new plugin is a type implementing `Plugin` and one entry here.
static PLUGINS: &[&dyn Plugin] = &[&RtShadows, &RtSunRays];

// ---- state ------------------------------------------------------------------------------------------------------------------

struct State {
	/// The player's value per plugin (`PLUGINS` order).
	on: Vec<AtomicBool>,
	/// The values the game started with (restart plugins read these).
	at_start: Vec<bool>,
	/// Entries of the `plugins=` line for plugins this build does not have, kept when it is rewritten.
	unknown: Vec<(String, bool)>,
	/// Per plugin: its settings' values, the values the game started with, and the `plugin.<id>=` entries it does not know.
	values: Vec<Vec<AtomicU8>>,
	values_at_start: Vec<Vec<u8>>,
	unknown_settings: Vec<Vec<(String, String)>>,
}

fn state() -> &'static State {
	static ST: OnceLock<State> = OnceLock::new();
	ST.get_or_init(|| {
		// Read straight from the file, so the renderer may ask before the retail driver has loaded its options.
		let opts = crate::retail::pcopts::PcOptions::load(&crate::retail::pcopts::file());
		let saved = parse(opts.extra_get("plugins").unwrap_or(""));
		let at_start: Vec<bool> = PLUGINS.iter().map(|p| saved.iter().find(|(id, _)| id == p.id()).map_or(p.default_on(), |e| e.1)).collect();
		let unknown = saved.into_iter().filter(|(id, _)| !PLUGINS.iter().any(|p| p.id() == id)).collect();
		let on = at_start.iter().map(|&v| AtomicBool::new(v)).collect();
		let mut values_at_start = Vec::new();
		let mut unknown_settings = Vec::new();
		for p in PLUGINS {
			let pairs = parse_pairs(opts.extra_get(&file_key(*p)).unwrap_or(""));
			values_at_start.push(p.settings().iter().map(|s| pairs.iter().find(|(id, _)| id == s.id).and_then(|(_, v)| value_index(s, v)).unwrap_or(s.default)).collect::<Vec<u8>>());
			unknown_settings.push(pairs.into_iter().filter(|(id, _)| !p.settings().iter().any(|s| s.id == id)).collect());
		}
		let values = values_at_start.iter().map(|v| v.iter().map(|&x| AtomicU8::new(x)).collect()).collect();
		for (i, p) in PLUGINS.iter().enumerate() {
			let set: Vec<String> = p.settings().iter().zip(&values_at_start[i]).map(|(s, &v)| format!(", {} {}", s.id, s.values[v as usize])).collect();
			eprintln!("plugins: {} {}{}{}", p.id(), if at_start[i] { "on" } else { "off" }, set.concat(), if forced(*p) { " (forced on by the environment)" } else { "" });
		}
		State { on, at_start, unknown, values, values_at_start, unknown_settings }
	})
}

/// `a:b,c:d` -> the trimmed, lower-case pairs; entries without ':' or with an empty key are skipped.
fn parse_pairs(v: &str) -> Vec<(String, String)> {
	v.split(',')
		.filter_map(|e| {
			let (k, val) = e.split_once(':')?;
			Some((k.trim().to_ascii_lowercase(), val.trim().to_ascii_lowercase()))
		})
		.filter(|(k, _)| !k.is_empty())
		.collect()
}

/// `id:on,id:off` -> the pairs; anything else is skipped.
fn parse(v: &str) -> Vec<(String, bool)> {
	parse_pairs(v)
		.into_iter()
		.filter_map(|(id, val)| match val.as_str() {
			"on" => Some((id, true)),
			"off" => Some((id, false)),
			_ => None,
		})
		.collect()
}

/// The index of the value named `name` (any case) in setting `s`.
fn value_index(s: &Setting, name: &str) -> Option<u8> {
	s.values.iter().position(|v| v.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

fn file_key(p: &dyn Plugin) -> String {
	format!("plugin.{}", p.id())
}

fn forced(p: &dyn Plugin) -> bool {
	p.env_force().is_some_and(|k| std::env::var(k).is_ok_and(|v| v == "1"))
}

fn index(id: &str) -> Option<usize> {
	PLUGINS.iter().position(|p| p.id() == id)
}

fn setting_index(i: usize, id: &str) -> Option<usize> {
	PLUGINS[i].settings().iter().position(|s| s.id == id)
}

// ---- the API ----------------------------------------------------------------------------------------------------------------

/// Every plugin, in menu order.
pub fn list() -> &'static [&'static dyn Plugin] {
	PLUGINS
}

/// The player's current value of plugin `i` (`list` order), without the environment switch: what the menu row shows.
pub fn chosen(i: usize) -> bool {
	state().on.get(i).is_some_and(|v| v.load(Ordering::Relaxed))
}

/// The value plugin `i` had when the game started.
pub fn chosen_at_start(i: usize) -> bool {
	state().at_start.get(i).copied().unwrap_or(false)
}

/// Whether plugin `id` is on now (live plugins). The environment switch forces it on.
#[allow(dead_code)] // No live plugin in this build yet.
pub fn on(id: &str) -> bool {
	index(id).is_some_and(|i| chosen(i) || forced(PLUGINS[i]))
}

/// Whether plugin `id` was on when the game started (restart plugins). The environment switch forces it on.
pub fn on_at_start(id: &str) -> bool {
	index(id).is_some_and(|i| chosen_at_start(i) || forced(PLUGINS[i]))
}

/// The current value of setting `k` of plugin `i` (`list` and `settings` order): what the menu row shows.
pub fn setting(i: usize, k: usize) -> u8 {
	state().values.get(i).and_then(|v| v.get(k)).map_or(0, |v| v.load(Ordering::Relaxed))
}

/// The value setting `k` of plugin `i` had when the game started.
pub fn setting_at_start(i: usize, k: usize) -> u8 {
	state().values_at_start.get(i).and_then(|v| v.get(k)).copied().unwrap_or(0)
}

/// The current value of setting `setting` of plugin `id` (live plugins); 0 when either is unknown.
#[allow(dead_code)] // No plugin with settings in this build yet.
pub fn value(id: &str, setting_id: &str) -> u8 {
	index(id).and_then(|i| Some(setting(i, setting_index(i, setting_id)?))).unwrap_or(0)
}

/// The value setting `setting` of plugin `id` had when the game started (restart plugins); 0 when either is unknown.
#[allow(dead_code)] // No plugin with settings in this build yet.
pub fn value_at_start(id: &str, setting_id: &str) -> u8 {
	index(id).and_then(|i| Some(setting_at_start(i, setting_index(i, setting_id)?))).unwrap_or(0)
}

/// The player switched plugin `i`: true when the value changed (the caller remembers it, `file_values`).
pub fn set(i: usize, on: bool) -> bool {
	let Some(v) = state().on.get(i) else { return false };
	if v.swap(on, Ordering::Relaxed) == on {
		return false;
	}
	let p = PLUGINS[i];
	let note = match (p.needs_restart(), forced(p)) {
		(_, true) => " (forced on by the environment)",
		(true, _) if on != chosen_at_start(i) => " (after a restart)",
		_ => "",
	};
	eprintln!("plugins: {} {}{note}", p.id(), if on { "on" } else { "off" });
	if !p.needs_restart() {
		p.on_change(on);
	}
	true
}

/// The player changed setting `k` of plugin `i` to `value` (clamped to its values): true when it changed (the caller remembers it,
/// `file_values`).
pub fn set_setting(i: usize, k: usize, value: u8) -> bool {
	let Some(s) = PLUGINS.get(i).and_then(|p| p.settings().get(k)) else { return false };
	let value = value.min(s.values.len().saturating_sub(1) as u8);
	let Some(v) = state().values.get(i).and_then(|v| v.get(k)) else { return false };
	if v.swap(value, Ordering::Relaxed) == value {
		return false;
	}
	let p = PLUGINS[i];
	let note = if p.needs_restart() && value != setting_at_start(i, k) { " (after a restart)" } else { "" };
	eprintln!("plugins: {} {} {}{note}", p.id(), s.id, s.values[value as usize]);
	if !p.needs_restart() {
		p.on_setting(s.id, value);
	}
	true
}

/// The options file's keys and their values (None: remove the key): `plugins=` (the plugins off their default, then the kept
/// unknown entries) and `plugin.<id>=` for every plugin with settings (the settings off their default, then the kept unknown ones).
pub fn file_values() -> Vec<(String, Option<String>)> {
	let st = state();
	let join = |all: Vec<String>| (!all.is_empty()).then(|| all.join(","));
	let own = PLUGINS.iter().enumerate().filter(|(i, p)| chosen(*i) != p.default_on()).map(|(i, p)| (p.id().to_string(), chosen(i)));
	let mut out = vec![("plugins".to_string(), join(own.chain(st.unknown.iter().cloned()).map(|(id, v)| format!("{id}:{}", if v { "on" } else { "off" })).collect()))];
	for (i, p) in PLUGINS.iter().enumerate().filter(|(_, p)| !p.settings().is_empty()) {
		let own = p.settings().iter().enumerate().filter(|(k, s)| setting(i, *k) != s.default).map(|(k, s)| (s.id.to_string(), s.values[setting(i, k) as usize].to_ascii_lowercase()));
		let all = own.chain(st.unknown_settings[i].iter().cloned()).map(|(k, v)| format!("{k}:{v}")).collect();
		out.push((file_key(*p), join(all)));
	}
	out
}

/// Worker, before each field of a level: the live plugins that are on.
pub fn pre_field(m: &mut ee_oracle::Machine) {
	for (i, p) in PLUGINS.iter().enumerate() {
		if !p.needs_restart() && (chosen(i) || forced(*p)) {
			p.pre_field(m);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_file_value_parses_leniently() {
		assert_eq!(parse("rt_shadows:on, Foo:OFF,bad,x:maybe,:on"), [("rt_shadows".to_string(), true), ("foo".to_string(), false)]);
		assert!(parse("").is_empty());
		assert_eq!(parse_pairs("Quality: High ,bad,reach:far"), [("quality".to_string(), "high".to_string()), ("reach".to_string(), "far".to_string())]);
		let s = Setting { id: "quality", name: "Quality", values: &["Low", "Medium", "High"], default: 1 };
		assert_eq!((value_index(&s, "high"), value_index(&s, "LOW"), value_index(&s, "ultra")), (Some(2), Some(0), None));
	}

	/// Ids, labels and values fit the options file and the menu (the game's font, a row's width, the retail row's four choices).
	#[test]
	fn plugin_ids_and_names_fit_the_file_and_the_menu() {
		let id_ok = |id: &str| !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
		let text_ok = |t: &str, max: usize| !t.is_empty() && t.len() <= max && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-');
		for p in list() {
			assert!(id_ok(p.id()) && text_ok(p.name(), 24), "{}", p.id());
			for s in p.settings() {
				assert!(id_ok(s.id) && text_ok(s.name, 24), "{} {}", p.id(), s.id);
				assert!((2..=4).contains(&s.values.len()) && (s.default as usize) < s.values.len(), "{} {}", p.id(), s.id);
				assert!(s.values.iter().all(|v| text_ok(v, 16)), "{} {}", p.id(), s.id);
			}
			let mut sids: Vec<&str> = p.settings().iter().map(|s| s.id).collect();
			sids.sort();
			sids.dedup();
			assert_eq!(sids.len(), p.settings().len(), "{}: no setting id twice", p.id());
		}
		let mut ids: Vec<&str> = list().iter().map(|p| p.id()).collect();
		ids.sort();
		ids.dedup();
		assert_eq!(ids.len(), list().len(), "no id twice");
	}
}
