//! [PC] Ray-traced reflections (the Ray tracing built-in mod's Reflections, plugins.rs: Mods in the launcher, read at start-up;
//! RAC_RT_REFLECTIONS=1 forces it on at Performance). No retail counterpart. RT_REFLECTIONS_PLAN.md has the plan and the phases.
//!
//! Phase 1 (this): the switch and its quality only. With Reflections on, the ray-tracing device and the level's structure
//! (rtshadow.rs) come up as they do for the other parts of the mod, so the cost of the structure alone can be measured
//! (RAC_GPU_PASSES, passtime.rs) before anything traces a reflection. Nothing on screen changes yet.

/// The Reflections setting's On values (plugins.rs `RayTracing::settings`, after Off, in the same order). Each has a GPU time
/// budget the reflection passes must stay under (RT_REFLECTIONS_PLAN.md): Performance 0.5 ms, Balanced 1.5 ms, Ultra 3.5 ms on an
/// RTX 3080 Ti at 1440p.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Quality {
	/// Default: traces only where the screen-space reflection and the planar mirror found nothing, at a quarter of the pixels.
	Performance,
	/// Every water pixel off the mirrored planes at half size, a sun shadow ray at the hit, chrome and glass.
	Balanced,
	/// Full size, 2 rays on rough surfaces, the mirrored planes too (the planar mirror's redraw off).
	Ultra,
}

/// The Reflections setting at start-up: 0 Off, 1.. the qualities.
fn setting() -> u8 {
	super::rtshadow::part("reflections", "RAC_RT_REFLECTIONS")
}

/// Reflections are on (read at start-up: the ray-tracing device feature is requested when the window opens).
pub(super) fn wanted() -> bool {
	setting() > 0
}

/// The quality the game started with (Performance while Reflections are off).
pub(super) fn quality() -> Quality {
	match setting() {
		0 | 1 => Quality::Performance,
		2 => Quality::Balanced,
		_ => Quality::Ultra,
	}
}

/// After the device is made (rtshadow.rs `enable`): says in the run log whether Reflections are on and at which quality.
pub(super) fn announce(traces: bool) {
	if !wanted() {
		return;
	}
	match traces {
		true => eprintln!("rt reflections: on, quality {:?} (phase 1: the structure only, nothing traced yet)", quality()),
		false => eprintln!("rt reflections: switched on, but the adapter has no ray queries on Vulkan: off"),
	}
}
