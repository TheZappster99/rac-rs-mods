//! [PC] Ray-traced sun shadows (the Ray tracing built-in mod's Shadows, or RAC_RT_SHADOWS=1; Lighting Enhanced and Dynamic shadows on; a Vulkan adapter
//! with hardware ray queries, wgpu's EXPERIMENTAL_RAY_QUERY). No retail counterpart. Without the switch or the feature nothing
//! here runs and the frame is the shadow-map one (sunshadow.rs).
//!
//! - Casters: the level's static world in one bottom-level structure, built once per level from the batches' own vertex buffers
//!   and a compact copy of the chosen index ranges: the merged world batch, terrain fragments with their near mesh, ties with
//!   LOD 0 and the shrubs. The things that move (mobies, the player, debris) are rtmovers.rs's, in the
//!   top level's slot 1, rebuilt every frame.
//! - Texel alpha: as the cascades cut them (sunshadow.rs `shrub_shader`), a terrain texel below alpha 0x60, a tie texel below
//!   0x40 and a shrub texel below 0x30 casts nothing. A draw whose texture keeps every texel is opaque geometry; one that keeps
//!   none is left out; the rest are non-opaque geometry whose triangles carry their uvs and a 1-bit mask of their texture
//!   (`rt_tris`, `rt_bits`), so a ray takes a candidate hit only on a kept texel (vines and leaves cast their holes).
//! - Soft shadows: after the level pass, `mask` traces `RAYS` rays over the sun's disc for every pixel of its depth into a
//!   shadow mask, with the camera it used. The static world's shadow at a world point holds from frame to frame, so the rays turn
//!   each frame and the world's share is blended with the last frame's (`HISTORY`); the moving casters' share is the frame's own.
//!   A moving caster's shadow in the mask lags one frame.
//! - Receivers: `sun_occ` (viewer.rs SHADER, rewritten by `shader`) projects its world point into the last frame's mask and
//!   takes the mean of the 3x3 pixels whose depth matches (within the surface's depth slope); with none (off screen, newly
//!   revealed, a see-through layer in front) it traces one ray of its own, out to `REACH`. Until the level's structure is built (`rt_view.sun.w` 0) it reads the
//!   cascades, which still draw for the shaders that do not trace.

use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How far a shadow ray looks for a caster (world units; the cascades reach as far towards the light, sunshadow.rs `REACH`).
const REACH: f32 = 200.0;
/// Room in the texel-alpha tables (bound once, at device setup): triangles of the non-opaque geometries, and mask words.
const TRI_CAP: usize = 2 << 20;
const BITS_CAP: usize = 8 << 20;
/// `rt_tris` header: the first cut triangle of each geometry (vec4 0 and 1); a triangle's record follows at 2 + 2 * index.
const MAX_GEOS: usize = 8;

/// Set once the device has ray queries and a part of the Ray tracing mod is on (`enable`); batches created after it carry `usage`.
static ON: AtomicBool = AtomicBool::new(false);
/// Set with `ON` when the Ray tracing mod's Shadows part is on: the receivers trace (`shadows`).
static SHADOWS: AtomicBool = AtomicBool::new(false);

/// A part of the Ray tracing built-in mod (plugins.rs `RayTracing`, read at start-up): its setting's value (index into its values;
/// 0 = Off), or 0 while the mod is off. The part's old dev switch `env` set to 1 forces it to at least its first On value.
pub(super) fn part(setting: &str, env: &str) -> u8 {
	let id = crate::plugins::RAY_TRACING;
	let v = if crate::plugins::on_at_start(id) { crate::plugins::value_at_start(id, setting) } else { 0 };
	match std::env::var(env).is_ok_and(|v| v == "1") {
		true => v.max(1),
		false => v,
	}
}

/// The Shadows part of the Ray tracing mod (RAC_RT_SHADOWS=1 still forces it on).
fn wanted_shadows() -> bool {
	part("shadows", "RAC_RT_SHADOWS") > 0
}

/// The level's structure is wanted by the shadows, the sun rays (rtrays.rs) or the reflections (rtrefl.rs).
fn wanted() -> bool {
	wanted_shadows() || super::rtrays::wanted() || super::rtrefl::wanted()
}

/// The device features to request: ray queries when wanted and the adapter traces in hardware (Vulkan only in wgpu 30).
pub(super) fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
	let f = wgpu::Features::EXPERIMENTAL_RAY_QUERY;
	match wanted() {
		true if adapter.features().contains(f) && adapter.get_info().backend == wgpu::Backend::Vulkan => f,
		true => {
			eprintln!("rt shadows: switched on, but the adapter has no ray queries on Vulkan: shadow maps");
			wgpu::Features::empty()
		}
		false => wgpu::Features::empty(),
	}
}

/// After the device is made: on when it got the features.
pub(super) fn enable(device: &wgpu::Device) {
	let on = device.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY);
	ON.store(on, Ordering::Relaxed);
	SHADOWS.store(on && wanted_shadows(), Ordering::Relaxed);
	if on {
		eprintln!("rt shadows: {} (hardware ray queries)", if shadows() { "on" } else { "structure only, the shadows stay shadow maps" });
	}
	super::rtrefl::announce(on);
}

/// The device traces and the level's structure is built (for the shadows, the sun rays or both).
pub(super) fn on() -> bool {
	ON.load(Ordering::Relaxed)
}

/// The shadow receivers trace (the level shader variant, its bindings 4..8 and the shadow mask pass).
pub(super) fn shadows() -> bool {
	SHADOWS.load(Ordering::Relaxed)
}

/// Vertex and index buffers also feed the structure builds (and the index copies) while ray tracing is on, and the reflections
/// read the static batches' vertices at a ray's hit (`Hit`, rtrefl.rs).
pub(super) fn usage(u: wgpu::BufferUsages) -> wgpu::BufferUsages {
	match on() && u.intersects(wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX) {
		true => u | wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::STORAGE,
		false => u,
	}
}

/// The level's group-0 layout entries past the cascades and the soft fog buffer (bindings 4..8; 3 is volfog/soft.rs), none while
/// ray tracing is off.
pub(super) fn layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
	if !shadows() {
		return Vec::new();
	}
	let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None };
	let storage = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None };
	let uniform = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None };
	vec![entry(4, wgpu::BindingType::AccelerationStructure { vertex_return: false }), entry(5, storage), entry(6, storage), entry(7, uniform), entry(8, storage)]
}

/// The level shader with `sun_occ` reading the shadow mask and tracing where it has none (the cascade read stays as
/// `sun_occ_map`, the fallback until the level's structure is built).
pub(super) fn shader(src: &str) -> String {
	const OCC: &str = "fn sun_occ(p: vec3<f32>) -> f32 {";
	// `sun_lit` (lit mobies and the player) takes `sun_occ_moby`.
	const MOBY: &str = "sun_occ(i.w)";
	assert!(src.contains(OCC) && src.contains(MOBY), "rt shadows: the level shader has no sun_occ or sun_lit");
	let src = src.replacen(OCC, "fn sun_occ_map(p: vec3<f32>) -> f32 {", 1).replacen(MOBY, "sun_occ_moby(i.w)", 1);
	let show = std::env::var("RAC_RT_SHADOW_SHOW").is_ok_and(|v| v == "1");
	format!("enable wgpu_ray_query;\n{}\n{}{}{}{}", src, RT_LEVEL, rt_fns(), RT_LOOKUP, RT_OCC.replace("RT_SHOW", if show { "true" } else { "false" }))
}

/// The level shader `src` as the device runs it: `shader(src)` while the receivers trace, else unchanged.
pub(super) fn level(src: String) -> String {
	if shadows() { shader(&src) } else { src }
}

/// The mask pass's shader (`cs_mask`).
fn mask_shader() -> String {
	let mask = RT_MASK.replace("REACH", &format!("{REACH:.1}")).replace("SUN_DISC", &format!("{SUN_DISC:.4}")).replace("RAYS", &format!("{RAYS}u")).replace("HISTORY", &format!("{HISTORY:.3}"));
	format!("enable wgpu_ray_query;\n{}{}{}", mask, rt_fns(), RT_LOOKUP)
}

/// The ray code the shaders share (these two and rtrays.rs's march), with this file's constants.
pub(super) fn rt_fns() -> String {
	RT_FNS.replace("REACH", &format!("{REACH:.1}")).replace("SUN_DISC", &format!("{SUN_DISC:.4}")).replace("RAYS", &format!("{RAYS}u"))
}

/// The sun's disc as the soft shadows see it: the tangent of its radius (Earth's sun is 0.0047; wider reads softer, as the
/// cascades' 4x4 filter did).
const SUN_DISC: f32 = 0.012;
/// Rays per pixel of the shadow mask.
const RAYS: u32 = 4;
/// Room in the shadow mask (pixels; bound once at device setup): a bigger window traces in the fragment shaders instead.
const MASK_CAP: usize = 3840 * 2160;

/// The camera of the shadow mask (`RtView` in the shaders): eye (w: 1 = the mask holds this level), forward (w: near), right
/// (w: far), up, (width, height, tan_half x aspect, tan_half), the light's travel direction.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RtView {
	eye: [f32; 4],
	f: [f32; 4],
	r: [f32; 4],
	u: [f32; 4],
	size: [f32; 4],
	sun: [f32; 4],
}

const RT_LEVEL: &str = r#"
@group(0) @binding(4) var sun_tlas: acceleration_structure;
// Texel-alpha casters (rtshadow.rs): header (first cut triangle per geometry), then per triangle (uv0, uv1), (uv2, mask word
// offset, mask w | h << 16); the masks, one bit per texel (1 = casts).
@group(0) @binding(5) var<storage, read> rt_tris: array<vec4<u32>>;
@group(0) @binding(6) var<storage, read> rt_bits: array<u32>;
// The shadow mask of the last frame and its camera (rtshadow.rs `mask`): per pixel the shadow of the world and of the moving
// casters, and the view depth (`rt_unpack`).
struct RtView { eye: vec4<f32>, f: vec4<f32>, r: vec4<f32>, u: vec4<f32>, size: vec4<f32>, sun: vec4<f32> };
@group(0) @binding(7) var<uniform> rt_view: RtView;
@group(0) @binding(8) var<storage, read> rt_mask: array<u32>;
"#;

const RT_MASK: &str = r#"
struct RtView { eye: vec4<f32>, f: vec4<f32>, r: vec4<f32>, u: vec4<f32>, size: vec4<f32>, sun: vec4<f32> };
@group(0) @binding(0) var depth_ms: texture_depth_multisampled_2d;
@group(0) @binding(1) var<uniform> rt_cam: RtView;
@group(0) @binding(2) var<storage, read_write> rt_out: array<u32>;
@group(0) @binding(3) var sun_tlas: acceleration_structure;
@group(0) @binding(4) var<storage, read> rt_tris: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read> rt_bits: array<u32>;
// The last frame's mask and its camera, under the names the receivers read them by (`rt_mask_ch`).
@group(0) @binding(6) var<uniform> rt_view: RtView;
@group(0) @binding(7) var<storage, read> rt_mask: array<u32>;

fn ign(x: vec2<f32>) -> f32 {
	return fract(52.9829189 * fract(dot(x, vec2<f32>(0.06711056, 0.00583715))));
}

// One shadow ray as `rt_ray`: 0 lit, 1 a caster of the static world, 2 a moving caster (the top level's instance with custom
// data 1, rtshadow.rs `ready`). The first hit found ends it, so a ray that both block may say either.
fn rt_ray_kind(p: vec3<f32>, l: vec3<f32>, t0: f32) -> u32 {
	var rq: ray_query;
	rayQueryInitialize(&rq, sun_tlas, RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, 0xffu, t0, REACH, p, l));
	while (rayQueryProceed(&rq)) {
		if (rt_texel_casts(rayQueryGetCandidateIntersection(&rq))) {
			rayQueryConfirmIntersection(&rq);
		}
	}
	let h = rayQueryGetCommittedIntersection(&rq);
	if (h.kind == RAY_QUERY_INTERSECTION_NONE) { return 0u; }
	return select(1u, 2u, h.instance_custom_data == 1u);
}

// [PC] The shadow mask (rtshadow.rs): per pixel of the level pass's depth, its world point (as post.rs `view_pos`) and RAYS
// rays over the sun's disc, turned per pixel and per frame; the sky stores no depth a receiver can match.
// The static world's share is blended into the last frame's at the same world point (`HISTORY` of it kept): 4 rays give a
// penumbra only 5 steps, and the smallest camera motion moved pixels across them every frame, so the soft shadows far from
// their casters shimmered. Turning the rays each frame, the blend sees many more directions over the sun's disc. The moving
// casters' share is this frame's only (a blend would trail behind them); a ray they stop says nothing of the world, so it
// does not count towards the world's share.
@compute @workgroup_size(8, 8) fn cs_mask(@builtin(global_invocation_id) id: vec3<u32>) {
	let wh = vec2<u32>(rt_cam.size.xy);
	if (id.x >= wh.x || id.y >= wh.y) { return; }
	let d = textureLoad(depth_ms, vec2<i32>(id.xy), 0);
	var m = rt_pack(0.0, 0.0, 65504.0);
	if (d < 1.0) {
		let n = rt_cam.f.w;
		let fa = rt_cam.r.w;
		let z = n * fa / (fa - d * (fa - n));
		// Where sample 0 lies in the pixel (the standard 4x MSAA pattern), as its depth is that point's, not the middle's.
		let uv = (vec2<f32>(id.xy) + vec2<f32>(0.375, 0.125)) / vec2<f32>(wh);
		let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
		let p = rt_cam.eye.xyz + (rt_cam.f.xyz + rt_cam.r.xyz * (ndc.x * rt_cam.size.z) + rt_cam.u.xyz * (ndc.y * rt_cam.size.w)) * z;
		// As `rt_soft`, the rays sorted by what stops them.
		let l = -normalize(rt_cam.sun.xyz);
		let a = normalize(cross(l, select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(l.z) > 0.9)));
		let b = cross(l, a);
		let spin = fract(ign(vec2<f32>(id.xy)) + rt_cam.u.w * 0.618034) * 6.2831853;
		var world = 0.0;
		var known = 0.0;
		var movers = 0.0;
		for (var i = 0u; i < RAYS; i++) {
			let ang = spin + f32(i) * 2.3999632;
			let rad = SUN_DISC * sqrt((f32(i) + 0.5) / f32(RAYS));
			let k = rt_ray_kind(p, normalize(l + (a * cos(ang) + b * sin(ang)) * rad), 0.03 + 0.004 * z);
			if (k == 2u) {
				movers += 1.0;
			} else {
				world += f32(k);
				known += 1.0;
			}
		}
		let h = rt_mask_ch(p);
		var w = world / max(known, 1.0);
		if (h.z >= 0.5) {
			// The last frame's world share at p: its own pixel when that matches (no blur over frames), else the neighbours'.
			let last = select(h.x, h.w, h.w >= 0.0);
			w = mix(last, w, (1.0 - HISTORY) * known / f32(RAYS));
		}
		m = rt_pack(w, movers / f32(RAYS), min(z, 65000.0));
	}
	rt_out[id.y * wh.x + id.x] = m;
}
"#;

/// The share of the last frame's static-world shadow the mask keeps at a point (`cs_mask`): about 1 / (1 - HISTORY) frames of
/// rays are blended.
const HISTORY: f32 = 0.9;

const RT_FNS: &str = r#"
// A candidate hit on a non-opaque caster: does its texel cast? (the uv repeats, nearest texel of level 0)
fn rt_texel_casts(c: RayIntersection) -> bool {
	let g = min(c.geometry_index, 7u);
	let t = 2u + 2u * (rt_tris[g >> 2u][g & 3u] + c.primitive_index);
	let a = rt_tris[t];
	let b = rt_tris[t + 1u];
	let bc = c.barycentrics;
	let uv = bitcast<vec2<f32>>(a.xy) * (1.0 - bc.x - bc.y) + bitcast<vec2<f32>>(a.zw) * bc.x + bitcast<vec2<f32>>(b.xy) * bc.y;
	let wh = vec2<u32>(b.w & 0xffffu, b.w >> 16u);
	let xy = min(vec2<u32>(fract(uv) * vec2<f32>(wh)), wh - 1u);
	let i = xy.y * wh.x + xy.x;
	return ((rt_bits[b.z + (i >> 5u)] >> (i & 31u)) & 1u) != 0u;
}

// One shadow ray from p along l (towards the light): 1 when a caster within REACH casts there, the static world or the moving
// casters. It starts t0 off the surface, more far from the eye (a far fragment is drawn with a coarser mesh than the traced near
// one, and lies off it).
fn rt_ray(p: vec3<f32>, l: vec3<f32>, t0: f32) -> f32 {
	return rt_ray_masked(p, l, t0, 0xffu);
}

// `rt_ray` through the top-level instances in `cull` only (rtshadow.rs `MASK_WORLD`: 1 the static world, 2 the moving casters).
fn rt_ray_masked(p: vec3<f32>, l: vec3<f32>, t0: f32, cull: u32) -> f32 {
	var rq: ray_query;
	rayQueryInitialize(&rq, sun_tlas, RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, cull, t0, REACH, p, l));
	while (rayQueryProceed(&rq)) {
		if (rt_texel_casts(rayQueryGetCandidateIntersection(&rq))) {
			rayQueryConfirmIntersection(&rq);
		}
	}
	return select(0.0, 1.0, rayQueryGetCommittedIntersection(&rq).kind != RAY_QUERY_INTERSECTION_NONE);
}

// RAYS rays over the sun's disc (tangent radius SUN_DISC), spread evenly over its area and turned by `spin`: the shadowed share.
fn rt_soft(p: vec3<f32>, l: vec3<f32>, t0: f32, spin: f32) -> f32 {
	let a = normalize(cross(l, select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(l.z) > 0.9)));
	let b = cross(l, a);
	var occ = 0.0;
	for (var i = 0u; i < RAYS; i++) {
		let ang = spin + f32(i) * 2.3999632;
		let rad = SUN_DISC * sqrt((f32(i) + 0.5) / f32(RAYS));
		occ += rt_ray(p, normalize(l + (a * cos(ang) + b * sin(ang)) * rad), t0);
	}
	return occ / f32(RAYS);
}
"#;

const RT_LOOKUP: &str = r#"
// A shadow mask pixel (rtshadow.rs `cs_mask`): the static world's shadowed share (unorm8), the moving casters' (unorm8 << 8),
// the view depth (the high f16).
fn rt_pack(world: f32, movers: f32, z: f32) -> u32 {
	let s = vec2<u32>(round(saturate(vec2<f32>(world, movers)) * 255.0));
	return s.x | (s.y << 8u) | (pack2x16float(vec2<f32>(0.0, z)) & 0xffff0000u);
}

fn rt_unpack(v: u32) -> vec3<f32> {
	return vec3<f32>(f32(v & 0xffu) / 255.0, f32((v >> 8u) & 0xffu) / 255.0, unpack2x16float(v).y);
}

// The shadow mask at world point p (last frame's, re-projected with its camera): the means of the world's and the moving
// casters' shares over the 3x3 pixels around it whose depth matches p's, their total weight (under 0.5: none hold p; off
// screen, hidden last frame, a see-through layer in front of the mask's) and the world share of p's own pixel (-1 when that
// one does not match).
// A neighbour's depth may differ from p's by the surface's depth slope on screen times its distance in pixels from p: far
// ground seen at a low angle changes depth by more than 1% per pixel, and with a fixed tolerance its neighbours dropped in and
// out from frame to frame, so the far shadows flickered between the mask and the hard ray. The slope on each axis is the
// smaller of the two one-sided differences at the centre pixel (a plane gives both the same; at an object's edge the other
// side is another object). A match counts less the nearer it is to the tolerance, so a neighbour drifting out fades out.
fn rt_mask_ch(p: vec3<f32>) -> vec4<f32> {
	let none = vec4<f32>(0.0, 0.0, 0.0, -1.0);
	if (rt_view.eye.w < 0.5) { return none; }
	let v = p - rt_view.eye.xyz;
	let z = dot(v, rt_view.f.xyz);
	if (z <= rt_view.f.w) { return none; }
	let nd = vec2<f32>(dot(v, rt_view.r.xyz) / (z * rt_view.size.z), dot(v, rt_view.u.xyz) / (z * rt_view.size.w));
	if (any(abs(nd) >= vec2<f32>(1.0))) { return none; }
	let wh = vec2<i32>(rt_view.size.xy);
	let px = vec2<f32>(nd.x * 0.5 + 0.5, 0.5 - nd.y * 0.5) * rt_view.size.xy;
	// A mask pixel holds the point of its depth sample 0, at (0.375, 0.125) in the pixel (`cs_mask`): the centre pixel is the
	// one whose sample is nearest p, and f is p's offset from that sample.
	let c = vec2<i32>(floor(px + vec2<f32>(0.125, 0.375)));
	let f = px - vec2<f32>(c) - vec2<f32>(0.375, 0.125);
	var ms: array<vec3<f32>, 9>;
	for (var i = 0; i < 9; i++) {
		let q = clamp(c + vec2<i32>(i % 3 - 1, i / 3 - 1), vec2<i32>(0), wh - vec2<i32>(1));
		ms[i] = rt_unpack(rt_mask[u32(q.y * wh.x + q.x)]);
	}
	let zc = ms[4].z;
	let g = min(vec2<f32>(min(abs(zc - ms[3].z), abs(ms[5].z - zc)), min(abs(zc - ms[1].z), abs(ms[7].z - zc))), vec2<f32>(0.1 * z));
	let base = 0.02 + 0.01 * z;
	var s = vec2<f32>(0.0);
	var n = 0.0;
	var own = -1.0;
	for (var i = 0; i < 9; i++) {
		let o = abs(vec2<f32>(f32(i % 3 - 1), f32(i / 3 - 1)) - f);
		let w = 1.0 - abs(ms[i].z - z) / (base + 1.25 * dot(g, o));
		// The sky stores 65504, where no receiver is.
		if (w > 0.0 && ms[i].z < 65000.0) {
			s += ms[i].xy * w;
			n += w;
			if (i == 4) { own = ms[i].x; }
		}
	}
	return vec4<f32>(s / max(n, 1e-4), n, own);
}
"#;

const RT_OCC: &str = r#"
// [PC] Ray-traced sun shadows (rtshadow.rs; rt_view.sun.w = 1 once the level's structure is built; u.sun_q.w is texfilter.wgsl's):
// the soft shadow of the mask where it holds p (the moving casters' share, and the world's over the rest), else one ray of
// its own.
fn sun_occ(p: vec3<f32>) -> f32 {
	if (rt_view.sun.w < 0.5) { return sun_occ_map(p); }
	let m = rt_mask_ch(p);
	if (m.z >= 0.5) { return m.y + (1.0 - m.y) * m.x; }
	// RAC_RT_SHADOW_SHOW=1 (dev): the receivers that trace this ray get dark stripes.
	if (RT_SHOW && fract(dot(p, vec3<f32>(0.5))) < 0.5) { return 1.0; }
	return rt_ray(p, -normalize(u.sun.xyz), 0.03 + 0.004 * distance(p, u.eye.xyz));
}

// A moving receiver (a lit moby or the player, `sun_lit`): one ray of its own through the static world only. The shadow mask and
// the moving casters hold its own shape, and the moby lighting's wrap lights the far side the sun does not reach, which its
// own shadow took away (the player turned dark all over in the sun).
fn sun_occ_moby(p: vec3<f32>) -> f32 {
	if (rt_view.sun.w < 0.5) { return sun_occ_map(p); }
	return rt_ray_masked(p, -normalize(u.sun.xyz), 0.03 + 0.004 * distance(p, u.eye.xyz), 1u);
}
"#;

/// A slot texture's level-0 alpha (logged at upload while `build_scene` runs, gpu.rs `rt_log`).
pub(super) struct Alpha {
	w: usize,
	h: usize,
	a: Vec<u8>,
}

impl Alpha {
	pub(super) fn of(t: &crate::level::Texture) -> Arc<Alpha> {
		Arc::new(Alpha { w: t.w, h: t.h, a: t.rgba.iter().map(|c| c[3]).collect() })
	}
}

/// How a batch's texels cast: the alpha (x 128, as the cascades read it) below which a texel casts nothing.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Cut {
	Whole,
	Tfrag,
	Tie,
	Shrub,
}

impl Cut {
	fn floor(self) -> f32 {
		match self {
			Cut::Whole => 0.0,
			Cut::Tfrag => 95.5,
			Cut::Tie => 63.5,
			Cut::Shrub => 48.0,
		}
	}
}

/// RAC_RT_TUNE=<file> (dev): `cut=tfrag,tie,shrub` (the kinds traced by texel alpha; a terrain or tie draw left out casts whole,
/// a shrub left out casts nothing), `solid=0.98` (a texture with at least that share of casting texels casts whole) and
/// `movers=0` (no moving casters; or a list of `props,player,clip,debris`, rtmovers.rs), `world=0` (debug: only the moving casters). The file is read again when
/// it changes, and the level's structure rebuilt.
#[derive(Clone, PartialEq, Debug)]
struct Tune {
	cut: [bool; 3],
	solid: f32,
	/// Which moving casters cast (rtmovers.rs `Which`): `movers=0` none, `movers=props,player,clip,debris` those.
	movers: super::rtmovers::Which,
	/// `world=0` leaves the static world out (debug: only the movers cast).
	world: bool,
}

impl Default for Tune {
	fn default() -> Tune {
		Tune { cut: [true; 3], solid: 1.0, movers: super::rtmovers::Which::ALL, world: true }
	}
}

impl Tune {
	fn parse(s: &str) -> Tune {
		let mut t = Tune::default();
		for w in s.split_whitespace() {
			match w.split_once('=') {
				Some(("cut", v)) => t.cut = ["tfrag", "tie", "shrub"].map(|k| v.split(',').any(|x| x == k)),
				Some(("solid", v)) => t.solid = v.parse().unwrap_or(1.0),
				Some(("movers", v)) => t.movers = super::rtmovers::Which::parse(v),
				Some(("world", v)) => t.world = v != "0",
				_ => {}
			}
		}
		t
	}

	fn traced(&self, c: Cut) -> bool {
		match c {
			Cut::Whole => false,
			Cut::Tfrag => self.cut[0],
			Cut::Tie => self.cut[1],
			Cut::Shrub => self.cut[2],
		}
	}
}

/// What a draw's texture makes of it.
#[derive(Clone, Copy)]
enum Mask {
	/// Every texel casts (or no texture to cut by).
	Solid,
	/// No texel casts.
	Clear,
	/// Some do: the mask at this word offset.
	Cut { off: u32, w: u32, h: u32 },
}

/// The 1-bit masks of the level, one per (slot, cut).
#[derive(Default)]
struct Masks {
	of: HashMap<(usize, Cut), Mask>,
	bits: Vec<u32>,
}

impl Masks {
	fn get(&mut self, alpha: &[Option<Arc<Alpha>>], slot: usize, cut: Cut, tune: &Tune) -> Mask {
		if cut == Cut::Whole {
			return Mask::Solid;
		}
		if !tune.traced(cut) {
			return if cut == Cut::Shrub { Mask::Clear } else { Mask::Solid };
		}
		if let Some(&m) = self.of.get(&(slot, cut)) {
			return m;
		}
		let m = match alpha.get(slot).and_then(|a| a.as_ref()).filter(|a| a.w > 0 && a.h > 0 && a.w < 1 << 16 && a.h < 1 << 16) {
			None => Mask::Solid,
			Some(a) => {
				let casts = |v: u8| v as f32 / 255.0 * 128.0 >= cut.floor();
				let n = a.a.iter().filter(|&&v| casts(v)).count();
				if n == a.a.len() || n as f32 >= tune.solid * a.a.len() as f32 {
					Mask::Solid
				} else if n == 0 {
					Mask::Clear
				} else {
					let off = self.bits.len();
					self.bits.resize(off + a.a.len().div_ceil(32), 0);
					for (i, &v) in a.a.iter().enumerate() {
						if casts(v) {
							self.bits[off + i / 32] |= 1 << (i % 32);
						}
					}
					Mask::Cut { off: off as u32, w: a.w as u32, h: a.h as u32 }
				}
			}
		};
		self.of.insert((slot, cut), m);
		m
	}
}

/// One geometry of the static structure: its batch, its index runs (start, count, mask of a cut run) and where they land in the
/// compact index buffer.
struct Geo<'a> {
	batch: &'a Batch,
	verts: &'a [Vertex],
	/// The source batch (`Hit::verts` index: world, terrain, ties, shrubs) and each triangle's texture slot, in run order.
	src: u32,
	tex: Vec<u32>,
	cut: bool,
	runs: Vec<(u32, u32, Mask)>,
	first: u32,
	count: u32,
}

/// The level's structure: the top level (one instance, identity), bound at the level's group 0 binding 4, the texel-alpha tables at 5 and 6,
/// and the static world's bottom level.
pub(super) struct RtShadow {
	pub tlas: wgpu::Tlas,
	tris: wgpu::Buffer,
	bits: wgpu::Buffer,
	blas: Option<wgpu::Blas>,
	/// The scene the structure was built for (its world index buffer).
	key: Option<wgpu::Buffer>,
	/// RAC_RT_TUNE (dev): the settings, and when the file was last looked at.
	tune: Tune,
	tune_seen: Option<(std::time::Instant, Option<std::time::SystemTime>)>,
	/// The shadow mask (`rt_pack`, `MASK_CAP` pixels), the camera it was traced with as the level shaders read it (`view`,
	/// copied from `view_next` after the frame's level pass) and as the mask pass reads it (`view_next`); the mask pass blends
	/// in a copy of the last frame's (`mask_last`, with `view`). `frame` turns the rays from frame to frame.
	mask: wgpu::Buffer,
	mask_last: wgpu::Buffer,
	frame: u32,
	view: wgpu::Buffer,
	view_next: wgpu::Buffer,
	mask_pipeline: wgpu::ComputePipeline,
	/// The mask pass's bind group and the depth view it reads (a new window size makes a new depth target).
	mask_bind: Option<(wgpu::TextureView, wgpu::BindGroup)>,
	/// Phase 4 (rtmovers.rs): the moving casters, rebuilt every frame into the top level's slot 1.
	movers: super::rtmovers::RtMovers,
	/// What a ray that hits the static world needs to shade the hit (rtrefl.rs), and a number that changes with every level's
	/// structure (so the passes that bind it rebind). None until a level is built, or when a buffer is past the binding limit.
	hit: Option<Hit>,
	hit_gen: u64,
}

/// The static world's hit data (rtrefl.rs `RT_HIT`): the compact index buffer the structure was built from, each triangle's
/// texture slot (the bindless array's index, `Draw::texture`) in the structure's order, per geometry (first index, source batch)
/// as `MAX_GEOS` vec4<u32>, and the source batches' vertex buffers (world, terrain, ties, shrubs; `Vertex`, 16 floats each).
pub(super) struct Hit {
	pub idx: wgpu::Buffer,
	pub tex: wgpu::Buffer,
	pub geo: wgpu::Buffer,
	pub verts: [wgpu::Buffer; 4],
}

/// The hit data of a structure built from `geos` over the compact index buffer `idx`, with `batches` (world, terrain, ties,
/// shrubs) as the sources. None when a buffer is past the device's storage binding limit (the reflections then trace nothing).
fn hit_data(device: &wgpu::Device, geos: &[Geo], idx: wgpu::Buffer, batches: [&Batch; 4]) -> Option<Hit> {
	use wgpu::util::DeviceExt;
	let tex: Vec<u32> = geos.iter().flat_map(|g| g.tex.iter().copied()).collect();
	let mut geo = [[0u32; 4]; MAX_GEOS];
	for (k, g) in geos.iter().enumerate() {
		geo[k] = [g.first, g.src, 0, 0];
	}
	let limit = device.limits().max_storage_buffer_binding_size as u64;
	let over: Vec<String> = batches.iter().map(|b| b.vbuf.size()).chain([idx.size(), tex.len() as u64 * 4]).filter(|&s| s > limit).map(|s| format!("{} MB", s >> 20)).collect();
	if !over.is_empty() {
		eprintln!("rt reflections: hit data past the storage binding limit ({} MB): {}; nothing traced", limit >> 20, over.join(", "));
		return None;
	}
	let mk = |label, contents: &[u8], usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage });
	let tex = mk("rt hit textures", bytemuck::cast_slice(&tex), wgpu::BufferUsages::STORAGE);
	let geo = mk("rt hit geometries", bytemuck::cast_slice(&geo), wgpu::BufferUsages::UNIFORM);
	Some(Hit { idx, tex, geo, verts: batches.map(|b| b.vbuf.clone()) })
}

const IDENTITY: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
/// The top level's instance masks: the static world (slot 0) and the moving casters (slot 1, rtmovers.rs). A ray with cull mask
/// 0xff sees both; the moving receivers trace `MASK_WORLD` only (`sun_occ_moby`).
const MASK_WORLD: u8 = 0x01;
const MASK_MOVERS: u8 = 0x02;

impl RtShadow {
	/// The top level, built empty so the level's bind groups can hold it before any level loads, the alpha tables and the
	/// shadow mask.
	pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> RtShadow {
		let tlas = device.create_tlas(&wgpu::CreateTlasDescriptor { label: Some("rt shadows"), max_instances: 2, flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE, update_mode: wgpu::AccelerationStructureUpdateMode::Build });
		let table = |label, size: usize| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
		let (tris, bits, mask, mask_last) = (table("rt shadow cut triangles", (2 + 2 * TRI_CAP) * 16), table("rt shadow masks", BITS_CAP * 4), table("rt shadow mask", MASK_CAP * 4), table("rt shadow mask last", MASK_CAP * 4));
		let uniform = |label, extra| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: std::mem::size_of::<RtView>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | extra, mapped_at_creation: false });
		let (view, view_next) = (uniform("rt shadow view", wgpu::BufferUsages::empty()), uniform("rt shadow view next", wgpu::BufferUsages::COPY_SRC));
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rt shadow mask"), source: wgpu::ShaderSource::Wgsl(crate::devtune::wgsl("rt shadow mask", mask_shader().into())) });
		let mask_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("rt shadow mask"), layout: None, module: &module, entry_point: Some("cs_mask"), compilation_options: Default::default(), cache: None });
		let mut enc = device.create_command_encoder(&Default::default());
		enc.build_acceleration_structures(std::iter::empty(), std::iter::once(&tlas));
		queue.submit([enc.finish()]);
		RtShadow { tlas, tris, bits, blas: None, key: None, tune: Tune::default(), tune_seen: None, mask, mask_last, frame: 0, view, view_next, mask_pipeline, mask_bind: None, movers: Default::default(), hit: None, hit_gen: 0 }
	}

	/// The frame's shadow mask, after the level pass (`depth` its depth, `cam` / `aspect` / `range` its camera, `dir` the light's
	/// travel direction, `size` the target's): traced into the mask, whose camera the next frame's level shaders then read.
	/// A window past `MASK_CAP` traces in the fragment shaders instead.
	#[allow(clippy::too_many_arguments)]
	pub(super) fn mask(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, cam: &Camera, aspect: f32, range: (f32, f32), dir: [f32; 3], size: (u32, u32)) {
		if self.blas.is_none() || (size.0 as usize) * (size.1 as usize) > MASK_CAP {
			// No mask (eye.w 0), but a built structure still traces in the fragment shaders (sun.w).
			let v = RtView { sun: [0.0, 0.0, 0.0, self.built() as u8 as f32], ..RtView::zeroed() };
			queue.write_buffer(&self.view, 0, bytemuck::bytes_of(&v));
			return;
		}
		let _s = super::profile::span("rt_mask");
		let f = forward(cam);
		let r = normalise(cross(f, cam.up));
		let u = cross(r, f);
		self.frame = (self.frame + 1) % 1024;
		let v = RtView {
			eye: [cam.pos[0], cam.pos[1], cam.pos[2], 1.0],
			f: [f[0], f[1], f[2], range.0],
			r: [r[0], r[1], r[2], range.1],
			// w: the frame, which turns the mask's rays (the receivers do not read it).
			u: [u[0], u[1], u[2], self.frame as f32],
			size: [size.0 as f32, size.1 as f32, cam.tan_half * aspect, cam.tan_half],
			// w: the structure is built (the receivers trace; it is, past the check above).
			sun: [dir[0], dir[1], dir[2], 1.0],
		};
		queue.write_buffer(&self.view_next, 0, bytemuck::bytes_of(&v));
		if self.mask_bind.as_ref().is_none_or(|(d, _)| d != depth) {
			let depth_only = depth.texture().create_view(&wgpu::TextureViewDescriptor { aspect: wgpu::TextureAspect::DepthOnly, ..Default::default() });
			let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
				label: Some("rt shadow mask"),
				layout: &self.mask_pipeline.get_bind_group_layout(0),
				entries: &[
					wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth_only) },
					wgpu::BindGroupEntry { binding: 1, resource: self.view_next.as_entire_binding() },
					wgpu::BindGroupEntry { binding: 2, resource: self.mask.as_entire_binding() },
					wgpu::BindGroupEntry { binding: 3, resource: self.tlas.as_binding() },
					wgpu::BindGroupEntry { binding: 4, resource: self.tris.as_entire_binding() },
					wgpu::BindGroupEntry { binding: 5, resource: self.bits.as_entire_binding() },
					wgpu::BindGroupEntry { binding: 6, resource: self.view.as_entire_binding() },
					wgpu::BindGroupEntry { binding: 7, resource: self.mask_last.as_entire_binding() },
				],
			});
			self.mask_bind = Some((depth.clone(), bind));
		}
		let Some((_, bind)) = &self.mask_bind else { return };
		// The last frame's mask, as its camera (`view`, not yet replaced) saw it.
		let last = (size.0 as u64 * size.1 as u64 * 4).max(4);
		enc.copy_buffer_to_buffer(&self.mask, 0, &self.mask_last, 0, Some(last));
		{
			let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("rt shadow mask"), timestamp_writes: crate::viewer::passtime::cw("rt shadow mask") });
			pass.set_pipeline(&self.mask_pipeline);
			pass.set_bind_group(0, bind, &[]);
			pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
		}
		// The next frame's level shaders read this mask with this camera.
		enc.copy_buffer_to_buffer(&self.view_next, 0, &self.view, 0, Some(std::mem::size_of::<RtView>() as u64));
	}

	/// RAC_RT_TUNE (dev): every half second, the file's settings; a change rebuilds the level's structure.
	fn retune(&mut self) {
		let Some(path) = std::env::var_os("RAC_RT_TUNE") else { return };
		let now = std::time::Instant::now();
		if self.tune_seen.is_some_and(|(t, _)| now.duration_since(t).as_secs_f32() < 0.5) {
			return;
		}
		let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
		let old = self.tune_seen.replace((now, stamp));
		if old.is_some_and(|(_, s)| s == stamp) {
			return;
		}
		let t = Tune::parse(&std::fs::read_to_string(&path).unwrap_or_default());
		if t != self.tune {
			eprintln!("rt shadows: tune {t:?}");
			self.tune = t;
			self.key = None;
		}
	}

	/// The level's group-0 entries for bindings 4..8 (3 is volfog/soft.rs).
	pub(super) fn entries(&self) -> [wgpu::BindGroupEntry<'_>; 5] {
		[
			wgpu::BindGroupEntry { binding: 4, resource: self.tlas.as_binding() },
			wgpu::BindGroupEntry { binding: 5, resource: self.tris.as_entire_binding() },
			wgpu::BindGroupEntry { binding: 6, resource: self.bits.as_entire_binding() },
			wgpu::BindGroupEntry { binding: 7, resource: self.view.as_entire_binding() },
			wgpu::BindGroupEntry { binding: 8, resource: self.mask.as_entire_binding() },
		]
	}

	/// The structure and its texel-alpha tables at bindings 3, 4 and 5, for another pass that traces (rtrays.rs).
	pub(super) fn trace_entries(&self) -> [wgpu::BindGroupEntry<'_>; 3] {
		[
			wgpu::BindGroupEntry { binding: 3, resource: self.tlas.as_binding() },
			wgpu::BindGroupEntry { binding: 4, resource: self.tris.as_entire_binding() },
			wgpu::BindGroupEntry { binding: 5, resource: self.bits.as_entire_binding() },
		]
	}

	/// The level's structure is built (`ready` returned true for the current scene).
	/// Phase 4 (rtmovers.rs): this frame's moving casters around `eye` in the top level's slot 1 (the static world is slot 0),
	/// and the top level rebuilt (its own submit, ahead of the frame's). Nothing until the level's structure is built.
	pub(super) fn movers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene, eye: [f32; 3]) {
		if self.blas.is_none() {
			return;
		}
		let blas = self.movers.build(device, queue, scene, eye, self.tune.movers);
		self.tlas[0] = self.blas.as_ref().filter(|_| self.tune.world).map(|b| wgpu::TlasInstance::new(b, IDENTITY, 0, MASK_WORLD));
		self.tlas[1] = blas.map(|b| wgpu::TlasInstance::new(b, IDENTITY, 1, MASK_MOVERS));
		let mut enc = device.create_command_encoder(&Default::default());
		enc.build_acceleration_structures(std::iter::empty(), std::iter::once(&self.tlas));
		queue.submit([enc.finish()]);
	}

	pub(super) fn built(&self) -> bool {
		self.blas.is_some()
	}

	/// Builds the structure for `scene` when it is new (submits of its own, ahead of the frame's); true once it is built.
	pub(super) fn ready(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene) -> bool {
		self.retune();
		if self.key.as_ref() == Some(&scene.world.ibuf) {
			return self.blas.is_some();
		}
		let _s = super::profile::span("rt_build");
		let t = std::time::Instant::now();
		self.key = Some(scene.world.ibuf.clone());
		self.blas = None;
		self.hit = None;
		// The last mask belongs to the old level.
		queue.write_buffer(&self.view, 0, bytemuck::bytes_of(&RtView::zeroed()));
		self.tlas[0] = None;
		// The casting draws per batch: every world draw (whole), the near mesh (2) of each terrain fragment, LOD 0 of each tie and
		// every shrub draw, each by its texel alpha.
		let mut masks = Masks::default();
		let tf: Vec<&Draw> = scene.tfrag_ranges.iter().flat_map(|r| scene.tfrag_draws[r[2].start as usize..r[2].end as usize].iter()).collect();
		let ti: Vec<&Draw> = scene.tie_ranges.iter().flat_map(|r| scene.tie_draws[r[0].start as usize..r[0].end as usize].iter()).collect();
		let sources: [(&Batch, &[Vertex], Vec<&Draw>, Cut); 4] = [
			(&scene.world, &scene.world_verts, scene.world.draws.iter().collect(), Cut::Whole),
			(&scene.tfrags, &scene.tfrag_verts, tf, Cut::Tfrag),
			(&scene.ties, &scene.tie_verts, ti, Cut::Tie),
			(&scene.shrubs, &scene.shrub_verts, scene.shrubs.draws.iter().collect(), Cut::Shrub),
		];
		// Opaque geometries first, then the cut ones; adjacent runs of one mesh merge (a cut run only with the same mask).
		let mut opaque = Vec::new();
		let mut cut = Vec::new();
		for (src, (batch, verts, draws, c)) in sources.iter().enumerate() {
			let geo = |cut| Geo { batch: *batch, verts: *verts, src: src as u32, tex: Vec::new(), cut, runs: Vec::new(), first: 0, count: 0 };
			let (mut o, mut k) = (geo(false), geo(true));
			for d in draws {
				let m = masks.get(&scene.rt_alpha, d.texture, *c, &self.tune);
				let g = match m {
					Mask::Clear => continue,
					Mask::Solid => &mut o,
					Mask::Cut { .. } => &mut k,
				};
				// Runs merge across draws, so each triangle's texture is kept here, in the same order.
				g.tex.extend(std::iter::repeat_n(d.texture as u32, d.count as usize / 3));
				match g.runs.last_mut() {
					Some((s, n, Mask::Solid)) if *s + *n == d.start && matches!(m, Mask::Solid) => *n += d.count,
					Some((s, n, Mask::Cut { off, .. })) if *s + *n == d.start && matches!(m, Mask::Cut { off: o2, .. } if o2 == *off) => *n += d.count,
					_ => g.runs.push((d.start, d.count, m)),
				}
			}
			opaque.push(o);
			cut.push(k);
		}
		let mut geos: Vec<Geo> = opaque.into_iter().chain(cut).filter(|g| !g.runs.is_empty()).collect();
		let mut at = 0u32;
		for g in &mut geos {
			g.first = at;
			g.count = g.runs.iter().map(|r| r.1).sum();
			at += g.count;
		}
		let total = at;
		let cut_from = geos.iter().find(|g| g.cut).map_or(total, |g| g.first);
		let cut_tris = (total - cut_from) as usize / 3;
		if total == 0 || geos.len() > MAX_GEOS {
			return false;
		}
		if cut_tris > TRI_CAP || masks.bits.len() > BITS_CAP {
			eprintln!("rt shadows: {cut_tris} see-through triangles or {} mask words past the tables: they cast whole", masks.bits.len());
			for g in &mut geos {
				g.cut = false;
			}
		}
		let ibuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt shadow indices"), size: total as u64 * 4, usage: wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
		let mut enc = device.create_command_encoder(&Default::default());
		for g in &geos {
			let mut at = g.first as u64;
			for &(s, n, _) in &g.runs {
				enc.copy_buffer_to_buffer(&g.batch.ibuf, s as u64 * 4, &ibuf, at * 4, Some(n as u64 * 4));
				at += n as u64;
			}
		}
		// The cut triangles' records need their indices on the CPU: read the cut part of the compact buffer back once.
		if geos.iter().any(|g| g.cut) {
			let n = (total - cut_from) as u64 * 4;
			let staging = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt shadow index readback"), size: n, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
			enc.copy_buffer_to_buffer(&ibuf, cut_from as u64 * 4, &staging, 0, Some(n));
			queue.submit([std::mem::replace(&mut enc, device.create_command_encoder(&Default::default())).finish()]);
			staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
			if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
				eprintln!("rt shadows: index readback failed");
				return false;
			}
			let Ok(view) = staging.slice(..).get_mapped_range() else {
				eprintln!("rt shadows: index readback not mapped");
				return false;
			};
			let idx: Vec<u32> = bytemuck::cast_slice(&view).to_vec();
			drop(view);
			staging.unmap();
			let mut head = [0u32; MAX_GEOS];
			let mut recs: Vec<[u32; 8]> = Vec::with_capacity(cut_tris);
			for (gi, g) in geos.iter().enumerate().filter(|(_, g)| g.cut) {
				head[gi] = (g.first - cut_from) / 3;
				let mut k = (g.first - cut_from) as usize;
				for &(_, n, m) in &g.runs {
					let Mask::Cut { off, w, h } = m else { continue };
					for tri in idx[k..k + n as usize].chunks_exact(3) {
						let uv = |i: u32| g.verts.get(i as usize).map_or([0.0; 2], |v| v.uv).map(f32::to_bits);
						let (a, b, c) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
						recs.push([a[0], a[1], b[0], b[1], c[0], c[1], off, w | h << 16]);
					}
					k += n as usize;
				}
			}
			queue.write_buffer(&self.tris, 0, bytemuck::cast_slice(&head));
			queue.write_buffer(&self.tris, 32, bytemuck::cast_slice(&recs));
			queue.write_buffer(&self.bits, 0, bytemuck::cast_slice(&masks.bits));
		}
		let sizes: Vec<wgpu::BlasTriangleGeometrySizeDescriptor> = geos
			.iter()
			.map(|g| wgpu::BlasTriangleGeometrySizeDescriptor {
				vertex_format: wgpu::VertexFormat::Float32x3,
				vertex_count: (g.batch.vbuf.size() / std::mem::size_of::<Vertex>() as u64) as u32,
				index_format: Some(wgpu::IndexFormat::Uint32),
				index_count: Some(g.count),
				flags: if g.cut { wgpu::AccelerationStructureGeometryFlags::NO_DUPLICATE_ANY_HIT_INVOCATION } else { wgpu::AccelerationStructureGeometryFlags::OPAQUE },
			})
			.collect();
		let blas = device.create_blas(
			&wgpu::CreateBlasDescriptor { label: Some("rt shadow world"), flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE, update_mode: wgpu::AccelerationStructureUpdateMode::Build },
			wgpu::BlasGeometrySizeDescriptors::Triangles { descriptors: sizes.clone() },
		);
		let geometry = geos
			.iter()
			.zip(&sizes)
			.map(|(g, size)| wgpu::BlasTriangleGeometry { size, vertex_buffer: &g.batch.vbuf, first_vertex: 0, vertex_stride: std::mem::size_of::<Vertex>() as u64, index_buffer: Some(&ibuf), first_index: Some(g.first), transform_buffer: None, transform_buffer_offset: None })
			.collect();
		self.tlas[0] = Some(wgpu::TlasInstance::new(&blas, IDENTITY, 0, MASK_WORLD));
		enc.build_acceleration_structures(std::iter::once(&wgpu::BlasBuildEntry { blas: &blas, geometry: wgpu::BlasGeometries::TriangleGeometries(geometry) }), std::iter::once(&self.tlas));
		queue.submit([enc.finish()]);
		eprintln!(
			"rt shadows: level structure, {} triangles ({cut_tris} see-through, {} masks, {} KB) in {} geometries, {:.1} ms",
			total / 3,
			masks.of.values().filter(|m| matches!(m, Mask::Cut { .. })).count(),
			masks.bits.len() * 4 / 1024,
			geos.len(),
			t.elapsed().as_secs_f64() * 1e3
		);
		self.blas = Some(blas);
		self.hit = hit_data(device, &geos, ibuf, [&scene.world, &scene.tfrags, &scene.ties, &scene.shrubs]);
		self.hit_gen += 1;
		true
	}

	/// The static world's hit data with a number that changes with each level's structure (None: not built, or not bindable).
	pub(super) fn hit(&self) -> Option<(&Hit, u64)> {
		self.hit.as_ref().filter(|_| self.blas.is_some()).map(|h| (h, self.hit_gen))
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn traced_level_shader_validates() {
		// As gpu/setup.rs builds it: the soft fog's level source (its binding 3), then the traced variant (4..8).
		let level = super::super::volfog::soft::level_source(super::super::SHADER, false);
		let s = super::shader(&level);
		assert!(s.contains("fn sun_occ_map(p: vec3<f32>)") && s.contains("RayDesc(") && s.contains(", 200.0, p,"));
		let m = naga::front::wgsl::parse_str(&s).expect("rt wgsl parse");
		naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).expect("rt wgsl validate");
		let b = super::shader(&super::super::bindless::shader_source(&level));
		naga::front::wgsl::parse_str(&b).expect("rt bindless wgsl parse");
	}

	#[test]
	fn masks_keep_the_texels_that_cast() {
		use super::*;
		let t = crate::level::Texture { w: 2, h: 2, rgba: vec![[0, 0, 0, 255], [0, 0, 0, 0], [0, 0, 0, 100], [0, 0, 0, 128]] };
		let alpha = vec![Some(Alpha::of(&t))];
		let mut m = Masks::default();
		// Ties cast from 63.5 / 128: alpha 255 and 128 cast, 0 and 100 (49.8) do not.
		assert!(matches!(m.get(&alpha, 0, Cut::Tie, &Tune::default()), Mask::Cut { off: 0, w: 2, h: 2 }));
		assert_eq!(m.bits, vec![0b1001]);
		assert!(matches!(m.get(&alpha, 0, Cut::Whole, &Tune::default()), Mask::Solid));
		assert!(matches!(m.get(&alpha, 1, Cut::Tie, &Tune::default()), Mask::Solid), "no texture: whole");
	}
}
