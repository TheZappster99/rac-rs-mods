//! [PC] Built-in mods: optional PC features compiled into the game that the player switches On / Off with the game's mods, in the
//! launcher's Mods page (the in-game Mods pages, retail/pcmods.rs, are off: retail/pcext.rs `MODS_PAGE`). No retail counterpart.
//!
//! A built-in mod is a Rust type that implements `Plugin` and is listed in `PLUGINS`. It is compiled into the game like the rest of
//! the PC code: it may live anywhere in the crate (the renderer, the retail driver, ...) and only asks this module whether it is on.
//!
//! The mods folder (omni/load.rs `mods_dir`) is where every mod lives, and these too: at the first question the game writes each
//! built-in mod's `<id>/omni-mod.json` there (rewritten only when it differs), so the launcher lists it with
//! its settings like any script mod. The manifest says `"type": "script"` (the launcher accepts only script and native mods) with no
//! scripts and `"builtin": true`; omni/load.rs never hands a built-in id to OmniLoader (`is_builtin`). So:
//! - On / Off is the mod's line in `mods/active.txt` under the "Load mods" master tick (omni/plugins.rs `active_ids`: no line = off),
//!   written by the launcher (from the profile, at Play).
//! - Settings are the mod's entry in `mods/settings.json`, by choice value (`value_name`); a missing or unknown value is the default.
//! - Both are read once, when the game starts (the mods' rule: "Mods change at the next start"). `pre_field` runs for the mods that
//!   were on at the start.
//!
//! A mod's environment switch (`env_force`, set to 1) forces it on whatever the files say, as the dev switches always did.
use crate::omni::json::Json;
use std::sync::OnceLock;

/// One setting of a built-in mod: a choice of two to four named values (a title menu row holds four).
pub struct Setting {
	/// The key in settings.json: lower case letters, digits and '_'.
	pub id: &'static str,
	/// The row's label: plain ASCII letters, digits, spaces and '-' (the game's fonts), at most 24 characters.
	pub name: &'static str,
	/// Two to four value names, same characters, at most 16 each (settings.json stores `value_name` of each).
	pub values: &'static [&'static str],
	/// The value a new player has (index into `values`).
	pub default: u8,
}

pub trait Plugin: Sync {
	/// The mod id (folder name, active.txt line, settings.json key): lower case letters and digits joined by single hyphens.
	fn id(&self) -> &'static str;
	/// The name the launcher and the Mods page show: plain ASCII letters, digits, spaces and '-' (the game's fonts), at most 24.
	fn name(&self) -> &'static str;
	/// One or two plain sentences for the launcher's Mods page.
	fn description(&self) -> &'static str;
	/// An environment variable that forces the mod on when it is "1".
	fn env_force(&self) -> Option<&'static str> {
		None
	}
	/// The mod's settings, in menu order.
	fn settings(&self) -> &'static [Setting] {
		&[]
	}
	/// Worker, before each field of a level while the mod is on (after the PC camera and aim code).
	fn pre_field(&self, _m: &mut ee_oracle::Machine) {}
}

// ---- the built-in mods of this build ----------------------------------------------------------------------------------------

/// Ray-traced sun shadows (viewer/rtshadow.rs): a Vulkan adapter with hardware ray queries, Lighting Enhanced and Dynamic shadows
/// on. The device feature is requested when the window opens.
struct RtShadows;

impl Plugin for RtShadows {
	fn id(&self) -> &'static str {
		"rt-shadows"
	}
	fn name(&self) -> &'static str {
		"Ray-traced shadows"
	}
	fn description(&self) -> &'static str {
		"Sharper, more exact sun shadows traced by the graphics card. Needs Vulkan and a graphics card with ray tracing, and Lighting Enhanced with Dynamic shadows on. Built into the game."
	}
	fn env_force(&self) -> Option<&'static str> {
		Some("RAC_RT_SHADOWS")
	}
}

/// Ray-traced sun rays (viewer/rtrays.rs): light shafts in the air, traced through the same structure as the shadows (either mod
/// brings it up), in place of the screen-space Sun rays. Lighting Enhanced, Dynamic shadows and Sun rays on; the Sun rays level
/// picks how many samples each pixel takes.
struct RtSunRays;

impl Plugin for RtSunRays {
	fn id(&self) -> &'static str {
		"rt-sun-rays"
	}
	fn name(&self) -> &'static str {
		"Ray-traced sun rays"
	}
	fn description(&self) -> &'static str {
		"Shafts of sunlight in the air, traced by the graphics card, in place of the usual Sun rays. Needs Vulkan and a graphics card with ray tracing, and Lighting Enhanced with Dynamic shadows and Sun rays on. Built into the game."
	}
	fn env_force(&self) -> Option<&'static str> {
		Some("RAC_RT_RAYS")
	}
}

/// Ragdoll Ratchet (retail/ragdoll.rs, viewer/ragdoll.rs; RAGDOLL_PLAN.md): a big fall, L3 in the air or an enemy hit makes Ratchet go limp
/// and flop; he gets up after the limp time. Only the picture and the pad while he is limp change.
struct RagdollRatchet;

impl Plugin for RagdollRatchet {
	fn id(&self) -> &'static str {
		crate::retail::ragdoll::ID
	}
	fn name(&self) -> &'static str {
		"Ragdoll Ratchet"
	}
	fn description(&self) -> &'static str {
		"Ratchet goes limp and flops when he falls from a big height without the Heli-Pack, the Thruster-Pack or a wrench slam. Press L3 (Left Shift) in the air to go limp at once. Enemy hits can knock him down too. You have no control until he gets up. Built into the game."
	}
	fn env_force(&self) -> Option<&'static str> {
		Some("RAC_RAGDOLL")
	}
	fn settings(&self) -> &'static [Setting] {
		&[
			Setting { id: "height", name: "Fall height", values: &["Low", "Medium", "High"], default: 1 },
			Setting { id: "button", name: "Ragdoll button", values: &["Off", "L3"], default: 1 },
			Setting { id: "time", name: "Limp time", values: &["Short", "Medium", "Long"], default: 1 },
			Setting { id: "flop", name: "Floppiness", values: &["Stiff", "Loose", "Noodle"], default: 1 },
			Setting { id: "bounce", name: "Bounciness", values: &["Off", "Some", "Lots"], default: 1 },
			Setting { id: "hits", name: "Enemy hits", values: &["Off", "On"], default: 1 },
			Setting { id: "camera", name: "Camera follow", values: &["Off", "On"], default: 1 },
		]
	}
	fn pre_field(&self, m: &mut ee_oracle::Machine) {
		crate::retail::ragdoll::pre_field(m);
	}
}

/// Every built-in mod, in menu order. A new one is a type implementing `Plugin` and one entry here.
static PLUGINS: &[&dyn Plugin] = &[&RtShadows, &RtSunRays, &RagdollRatchet];

// ---- state ------------------------------------------------------------------------------------------------------------------

struct State {
	/// Per mod (`PLUGINS` order): on at the start, and its settings' values.
	on: Vec<bool>,
	values: Vec<Vec<u8>>,
}

fn state() -> &'static State {
	static ST: OnceLock<State> = OnceLock::new();
	ST.get_or_init(|| {
		let dir = crate::omni::load::mods_dir();
		for p in PLUGINS {
			if let Err(e) = write_manifest(&dir, *p) {
				eprintln!("plugins: cannot write the {} manifest in {}: {e}", p.id(), dir.display());
			}
		}
		let found = crate::omni::load::find_mods(&dir);
		let active = crate::omni::plugins::active_ids(&dir, &found);
		let doc = crate::omni::plugins::read_settings(&dir);
		let on: Vec<bool> = PLUGINS.iter().map(|p| active.iter().any(|id| id == p.id())).collect();
		let values: Vec<Vec<u8>> = PLUGINS.iter().map(|p| p.settings().iter().map(|s| stored(&doc, *p, s)).collect()).collect();
		for (i, p) in PLUGINS.iter().enumerate() {
			let set: Vec<String> = p.settings().iter().zip(&values[i]).map(|(s, &v)| format!(", {} {}", s.id, s.values[v as usize])).collect();
			eprintln!("plugins: {} {}{}{}", p.id(), if on[i] { "on" } else { "off" }, set.concat(), if forced(*p) { " (forced on by the environment)" } else { "" });
		}
		State { on, values }
	})
}

/// How settings.json stores value `v`: its name in lower case with '-' for spaces ("Very high" -> "very-high").
fn value_name(v: &str) -> String {
	v.to_ascii_lowercase().replace(' ', "-")
}

/// Setting `s` of mod `p` as settings.json holds it (index into `values`), else its default.
fn stored(doc: &Json, p: &dyn Plugin, s: &Setting) -> u8 {
	let v = crate::omni::plugins::value_of(doc, p.id(), s.id).and_then(Json::str);
	v.and_then(|v| s.values.iter().position(|n| value_name(n) == v)).map_or(s.default, |i| i as u8)
}

/// The omni-mod.json text of mod `p` (format 2, the loader's and the launcher's).
fn manifest(p: &dyn Plugin) -> String {
	let s = |t: &str| Json::Str(t.to_string());
	let settings = p
		.settings()
		.iter()
		.map(|st| {
			let choices = st.values.iter().map(|v| Json::Obj(vec![("value".into(), Json::Str(value_name(v))), ("name".into(), s(v))])).collect();
			Json::Obj(vec![("key".into(), s(st.id)), ("name".into(), s(st.name)), ("type".into(), s("choice")), ("default".into(), Json::Str(value_name(st.values[st.default as usize]))), ("choices".into(), Json::Arr(choices))])
		})
		.collect();
	Json::Obj(vec![
		("format".into(), Json::Num(2.0)),
		("id".into(), s(p.id())),
		("name".into(), s(p.name())),
		("version".into(), s(env!("CARGO_PKG_VERSION"))),
		("author".into(), s("rac-rs")),
		("description".into(), s(p.description())),
		("type".into(), s("script")),
		("builtin".into(), Json::Bool(true)),
		("scripts".into(), Json::Arr(Vec::new())),
		("games".into(), Json::Obj(vec![("rac1".into(), Json::Arr(vec![s("pal"), s("ntsc")]))])),
		("settings".into(), Json::Arr(settings)),
	])
	.to_text()
}

/// Writes `<dir>/<id>/omni-mod.json` for mod `p` when it is missing or differs.
fn write_manifest(dir: &std::path::Path, p: &dyn Plugin) -> std::io::Result<()> {
	let (folder, text) = (dir.join(p.id()), manifest(p));
	let file = folder.join("omni-mod.json");
	if std::fs::read_to_string(&file).is_ok_and(|t| t == text) {
		return Ok(());
	}
	std::fs::create_dir_all(&folder)?;
	std::fs::write(&file, text)
}

fn forced(p: &dyn Plugin) -> bool {
	p.env_force().is_some_and(|k| std::env::var(k).is_ok_and(|v| v == "1"))
}

fn index(id: &str) -> Option<usize> {
	PLUGINS.iter().position(|p| p.id() == id)
}

// ---- the API ----------------------------------------------------------------------------------------------------------------

/// Whether `id` is a built-in mod's (omni/load.rs keeps it from OmniLoader: it has no scripts).
pub fn is_builtin(id: &str) -> bool {
	index(id).is_some()
}

/// Whether mod `id` was on when the game started. The environment switch forces it on.
pub fn on_at_start(id: &str) -> bool {
	index(id).is_some_and(|i| state().on[i] || forced(PLUGINS[i]))
}

/// The value setting `setting_id` of mod `id` had when the game started (index into its `values`); 0 when either is unknown.
pub fn value_at_start(id: &str, setting_id: &str) -> u8 {
	let Some(i) = index(id) else { return 0 };
	PLUGINS[i].settings().iter().position(|s| s.id == setting_id).map_or(0, |k| state().values[i][k])
}

/// Worker, before each field of a level: the built-in mods that are on.
pub fn pre_field(m: &mut ee_oracle::Machine) {
	for p in PLUGINS {
		if on_at_start(p.id()) {
			p.pre_field(m);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::omni::json;

	/// Ids, names and values fit the loader's and the launcher's rules and the menu (the game's font, a row's width, four choices).
	#[test]
	fn plugin_ids_and_names_fit_the_file_and_the_menu() {
		let id_ok = |id: &str| id.len() <= 64 && id.split('-').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
		let key_ok = |id: &str| !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
		let text_ok = |t: &str, max: usize| !t.is_empty() && t.len() <= max && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-');
		for p in PLUGINS {
			assert!(id_ok(p.id()) && text_ok(p.name(), 24) && !p.description().is_empty(), "{}", p.id());
			for s in p.settings() {
				assert!(key_ok(s.id) && text_ok(s.name, 24), "{} {}", p.id(), s.id);
				assert!((2..=4).contains(&s.values.len()) && (s.default as usize) < s.values.len(), "{} {}", p.id(), s.id);
				assert!(s.values.iter().all(|v| text_ok(v, 16)), "{} {}", p.id(), s.id);
				let mut names: Vec<String> = s.values.iter().map(|v| value_name(v)).collect();
				names.sort();
				names.dedup();
				assert_eq!(names.len(), s.values.len(), "{} {}: no stored value twice", p.id(), s.id);
			}
			let mut sids: Vec<&str> = p.settings().iter().map(|s| s.id).collect();
			sids.sort();
			sids.dedup();
			assert_eq!(sids.len(), p.settings().len(), "{}: no setting id twice", p.id());
		}
		let mut ids: Vec<&str> = PLUGINS.iter().map(|p| p.id()).collect();
		ids.sort();
		ids.dedup();
		assert_eq!(ids.len(), PLUGINS.len(), "no id twice");
	}

	/// The manifest is one the game's mods code reads as a script mod with this id, name and settings.
	#[test]
	fn the_manifest_reads_as_a_mod() {
		static S: [Setting; 1] = [Setting { id: "quality", name: "Quality", values: &["Low", "Very high"], default: 1 }];
		struct T;
		impl Plugin for T {
			fn id(&self) -> &'static str {
				"test-mod"
			}
			fn name(&self) -> &'static str {
				"Test mod"
			}
			fn description(&self) -> &'static str {
				"A test."
			}
			fn settings(&self) -> &'static [Setting] {
				&S
			}
		}
		let j = json::parse(&manifest(&T)).unwrap();
		assert_eq!((j.get("format").and_then(Json::as_num), j.get("id").and_then(Json::str), j.get("type").and_then(Json::str)), (Some(2.0), Some("test-mod"), Some("script")));
		assert!(!crate::omni::load::is_native_manifest(&j));
		let st = &j.get("settings").unwrap().arr()[0];
		assert_eq!((st.get("default").and_then(Json::str), st.get("choices").unwrap().arr().len()), (Some("very-high"), 2));
		let doc = json::parse(r#"{"format":1,"mods":{"test-mod":{"quality":"low"}}}"#).unwrap();
		assert_eq!(stored(&doc, &T, &S[0]), 0);
		let doc = json::parse(r#"{"format":1,"mods":{"test-mod":{"quality":"ultra"}}}"#).unwrap();
		assert_eq!(stored(&doc, &T, &S[0]), 1, "an unknown value is the default");
	}
}
