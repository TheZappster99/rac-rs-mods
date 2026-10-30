//! [PC] Ray-traced reflections (the Ray tracing built-in mod's Reflections, plugins.rs: Mods in the launcher, read at start-up;
//! RAC_RT_REFLECTIONS=1 forces it on at Performance). No retail counterpart. RT_REFLECTIONS_PLAN.md has the plan and the phases.
//!
//! Phase 1: the switch and its quality. With Reflections on, the ray-tracing device and the level's structure (rtshadow.rs)
//! come up as they do for the other parts of the mod, so the cost of the structure alone can be measured (RAC_GPU_PASSES,
//! passtime.rs) before anything traces a reflection.
//!
//! Phase 2: what a ray sees where it hits the static world (`RT_HIT`, `rt_hit_colour`): the triangle's uv and baked vertex colour
//! from the batch it came from (rtshadow.rs `Hit`), its texture from the bindless array at the mip its footprint covers,
//! texel x colour as the level shader's `fs` (no sun, no fog). The dev view RAC_POST_VIEW=rtrefl (`Debug`) traces the camera's
//! own view rays through it at half size and shows the result alone, so the traced picture can be held against the drawn one.
//!
//! Phase 3 (`Water`, `rt_water`): the water's shading pass traces one ray where the screen-space reflection is unsure and no
//! planar mirror covers the pixel, in place of the far-air guess. Dev switches: RAC_RT_REFL_SHOW=1, RAC_RT_REFL_FORCE=1.
//!
//! Phase 4 (`rt_water`): a traced hit the picture shows (on screen, at the picture's depth there) takes the picture's colour, so
//! it carries the sun, the shadows and the fog; the moving things are traced too and show where the picture has them.
//!
//! Phase 5 (Balanced and up): every water pixel off the mirrored planes traces, and its hit wins over the screen-space one (which
//! stays where the ray finds nothing within reach); a static-world hit off screen gets the sun's shadow (one ray to the light);
//! a moving thing off screen gets its texture's mean colour (rtmovers.rs `tex_table`); lava, sludge and mud trace their gloss.
//! Picture hits are moved by the ripples as the screen-space ones are.
//!
//! Phase 6 (`Metal`, Balanced and up): the props' chrome and glass (gpu/metal.rs) are drawn by a copy of the level pipelines
//! whose texel goes through `rt_metal`: one ray along the mirror of the view, its hit shaded as off-screen water hits are, laid
//! over the retail environment texture (mixed at Balanced, alone at Ultra). A miss keeps the env texture.

use super::*;

/// The Reflections setting's On values (plugins.rs `RayTracing::settings`, after Off, in the same order). Each has a GPU time
/// budget the reflection passes must stay under (RT_REFLECTIONS_PLAN.md): Performance 0.5 ms, Balanced 1.5 ms, Ultra 3.5 ms on an
/// RTX 3080 Ti at 1440p.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Quality {
	/// Default: traces only where the screen-space reflection and the planar mirror found nothing.
	Performance,
	/// Every water pixel off the mirrored planes, a sun shadow ray at the hit, moving things off screen, the lava, sludge and mud
	/// gloss (and chrome and glass, phase 6).
	Balanced,
	/// Full size, 2 rays on rough surfaces, the mirrored planes too (the planar mirror's redraw off).
	Ultra,
}

/// The Reflections setting at start-up: 0 Off, 1.. the qualities.
fn start_setting() -> u8 {
	super::rtshadow::part("reflections", "RAC_RT_REFLECTIONS")
}

/// The Reflections setting the Insert key (`cycle`) switched to, u8::MAX until it is pressed.
static LIVE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(u8::MAX);

/// The Reflections setting now: the start-up one until the Insert key changes it.
fn setting() -> u8 {
	match LIVE.load(std::sync::atomic::Ordering::Relaxed) {
		u8::MAX => start_setting(),
		v => v,
	}
}

/// Reflections were on at start-up (the ray-tracing device feature is requested when the window opens, and the level's
/// structure and the traced passes exist from then on, whatever the Insert key switches to later).
pub(super) fn wanted() -> bool {
	start_setting() > 0
}

/// Reflections trace now (Off by the Insert key: the water and the metal draw as they do without the mod).
fn tracing() -> bool {
	setting() > 0
}

/// [PC] Insert (while Reflections were on at start-up): the next Reflections setting, Off -> Performance -> Balanced -> Ultra, for
/// this session only, to see the qualities side by side. The traced pipelines are built again for it on their next use (a
/// short hitch while the shaders compile); the window title shows it (`title_suffix`).
pub(in crate::viewer) fn cycle() {
	if !wanted() || !super::rtshadow::on() {
		return;
	}
	let next = (setting() + 1) % 4;
	LIVE.store(next, std::sync::atomic::Ordering::Relaxed);
	eprintln!("rt reflections: Insert key: {}", setting_name(next));
}

fn setting_name(s: u8) -> &'static str {
	match s {
		0 => "Off",
		1 => "Performance",
		2 => "Balanced",
		_ => "Ultra",
	}
}

/// The window title's note once the Insert key has switched the Reflections setting (nothing before that).
pub(in crate::viewer) fn title_suffix() -> String {
	match LIVE.load(std::sync::atomic::Ordering::Relaxed) {
		u8::MAX => String::new(),
		v => format!(" - RT reflections {}", setting_name(v)),
	}
}

/// The quality now (Performance while Reflections are off).
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
		true => eprintln!("rt reflections: on, quality {:?} (phase 6: {})", quality(), if movers_shaded() { "water, lava, sludge, mud, chrome, glass" } else { "water" }),
		false => eprintln!("rt reflections: switched on, but the adapter has no ray queries on Vulkan: off"),
	}
}

/// Reflections at Balanced and up: the whole water traces, with the sun's shadow at a hit and the moving things shaded off
/// screen (rtmovers.rs keeps their texture table for it).
pub(super) fn movers_shaded() -> bool {
	wanted() && tracing() && quality() != Quality::Performance
}

/// Reflections at Ultra: the flat water planes trace too, so the planar mirror (mirror.rs) draws nothing and its second pass over
/// the level is saved (gpu/draw/passes.rs, once the traced water can run). Rippled water traces a second ray (`rt_water`).
pub(in crate::viewer) fn mirrors_traced() -> bool {
	wanted() && tracing() && quality() == Quality::Ultra
}

/// Dev only: RAC_POST_VIEW=rtrefl shows what the camera's own view rays hit, traced and shaded as reflections will be (`Debug`).
pub(super) fn debug_view() -> bool {
	static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
	*V.get_or_init(|| std::env::var("RAC_POST_VIEW").is_ok_and(|v| v == "rtrefl"))
}

/// The hit data's bindings (rtshadow.rs `Hit`, then the movers' texture table, rtmovers.rs `tex_table`) in `group` from binding
/// `base` on (`HIT_BINDINGS`), and the bindless texture array as group `tex_group` (bindless.rs, its layout), with the shading of
/// a hit.
fn hit_wgsl(group: u32, base: u32, tex_group: u32) -> String {
	let mut s = String::new();
	for (k, decl) in ["var<storage, read> rt_idx: array<u32>", "var<storage, read> rt_tex: array<u32>", "var<uniform> rt_geo: array<vec4<u32>, 8>", "var<storage, read> rt_v0: array<f32>", "var<storage, read> rt_v1: array<f32>", "var<storage, read> rt_v2: array<f32>", "var<storage, read> rt_v3: array<f32>", "var<storage, read> rt_mtex: array<u32>"].iter().enumerate() {
		s += &format!("@group({group}) @binding({}) {decl};\n", base + k as u32);
	}
	s += &format!("@group({tex_group}) @binding(0) var texs: binding_array<texture_2d<f32>, {}>;\n@group({tex_group}) @binding(1) var samp: sampler;\n", super::bindless::TEX_CAP);
	s + &RT_HIT.replace("TEX_LAST", &format!("{}u", super::bindless::TEX_CAP - 1))
}

/// Bindings `hit_wgsl` takes.
const HIT_BINDINGS: usize = 8;

/// The hit data's bind group entries for `hit_wgsl(base)`; `movers`: the movers' texture table (or a stand-in before there is one).
fn hit_entries<'a>(h: &'a super::rtshadow::Hit, movers: &'a wgpu::Buffer, base: u32) -> [wgpu::BindGroupEntry<'a>; HIT_BINDINGS] {
	let bufs = [&h.idx, &h.tex, &h.geo, &h.verts[0], &h.verts[1], &h.verts[2], &h.verts[3], movers];
	std::array::from_fn(|k| wgpu::BindGroupEntry { binding: base + k as u32, resource: bufs[k].as_entire_binding() })
}

/// The movers' texture table of `rt` and its generation, else `none` (a stand-in: Performance, or nothing has moved near yet).
fn movers_tex<'a>(rt: &'a super::rtshadow::RtShadow, none: &'a wgpu::Buffer) -> (&'a wgpu::Buffer, u64) {
	rt.movers_tex().unwrap_or((none, 0))
}

/// The stand-in movers' texture table: the 4 geometry starts, all 0.
fn movers_none(device: &wgpu::Device) -> wgpu::Buffer {
	device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt movers textures none"), size: 16, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false })
}

/// The hit data's layout entries for `hit_wgsl(base)`.
fn hit_layout(base: u32, visibility: wgpu::ShaderStages) -> [wgpu::BindGroupLayoutEntry; HIT_BINDINGS] {
	let storage = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None };
	let uniform = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None };
	std::array::from_fn(|k| wgpu::BindGroupLayoutEntry { binding: base + k as u32, visibility, ty: if k == 2 { uniform } else { storage }, count: None })
}

const RT_HIT: &str = r#"
// One float of vertex `k / 16` of source batch `src` (world, terrain, ties, shrubs; `Vertex`: pos 0..2, uv 3..4, normal 5..7,
// colour 8..11, light 12..15).
fn rt_vf(src: u32, k: u32) -> f32 {
	switch src {
		case 0u: { return rt_v0[k]; }
		case 1u: { return rt_v1[k]; }
		case 2u: { return rt_v2[k]; }
		default: { return rt_v3[k]; }
	}
}

fn rt_vpos(src: u32, b: u32) -> vec3<f32> {
	return vec3<f32>(rt_vf(src, b), rt_vf(src, b + 1u), rt_vf(src, b + 2u));
}

// [PC] Ray-traced reflections (rtrefl.rs): the colour a ray sees where it hits triangle `prim` of geometry `geo` of the static world
// at barycentrics `bc`, `foot` world units per pixel there: texel x baked vertex colour as the level shader's `fs` draws the world
// (no fog), under a sun shadow of strength `shadow` taken as its `sun_baked` takes it (by how lit the baked colour is: a painted
// shadow takes none). The mip is the one the footprint covers, from the triangle's own texels per world unit.
fn rt_hit_colour(geo: u32, prim: u32, bc: vec2<f32>, foot: f32, shadow: f32) -> vec3<f32> {
	let g = rt_geo[min(geo, 7u)];
	let first = g.x + prim * 3u;
	let bw = array<f32, 3>(1.0 - bc.x - bc.y, bc.x, bc.y);
	var b: array<u32, 3>;
	var uvs: array<vec2<f32>, 3>;
	var uv = vec2<f32>(0.0);
	var c = vec3<f32>(0.0);
	for (var k = 0u; k < 3u; k++) {
		b[k] = rt_idx[first + k] * 16u;
		uvs[k] = vec2<f32>(rt_vf(g.y, b[k] + 3u), rt_vf(g.y, b[k] + 4u));
		uv += uvs[k] * bw[k];
		c += vec3<f32>(rt_vf(g.y, b[k] + 8u), rt_vf(g.y, b[k] + 9u), rt_vf(g.y, b[k] + 10u)) * bw[k];
	}
	let slot = min(rt_tex[first / 3u], TEX_LAST);
	let dim = vec2<f32>(textureDimensions(texs[slot]));
	let p0 = rt_vpos(g.y, b[0]);
	let world = length(cross(rt_vpos(g.y, b[1]) - p0, rt_vpos(g.y, b[2]) - p0));
	let du = (uvs[1] - uvs[0]) * dim;
	let dv = (uvs[2] - uvs[0]) * dim;
	let texels = abs(du.x * dv.y - du.y * dv.x);
	let per_unit = sqrt(texels / max(world, 1e-8));
	let lod = log2(max(foot * per_unit, 1.0));
	let t = textureSampleLevel(texs[slot], samp, uv, lod);
	let shade = min(c, vec3<f32>(255.0 / 128.0));
	let lum = dot(shade, vec3<f32>(0.2126, 0.7152, 0.0722));
	return t.rgb * shade * (1.0 - shadow * smoothstep(0.3, 0.8, lum));
}

// [PC] Ray-traced reflections (rtrefl.rs, Balanced and up): the colour of a moving thing a ray hit (triangle `prim` of geometry `geo`
// of the movers' structure; rtmovers.rs `tex_table`: the geometries' starts, then each triangle's texture slot): its texture's
// mean (the last mip) under a plain light. Their poses and lighting rig are not at hand, and a flat colour is enough for
// something only seen in a reflection and off screen.
fn rt_mover_colour(geo: u32, prim: u32) -> vec3<f32> {
	let slot = min(rt_mtex[rt_mtex[min(geo, 3u)] + prim], TEX_LAST);
	return textureSampleLevel(texs[slot], samp, vec2<f32>(0.5), 16.0).rgb * 0.8;
}
"#;

const RT_DEBUG: &str = r#"
struct DbgU { eye: vec4<f32>, f: vec4<f32>, r: vec4<f32>, u: vec4<f32>, size: vec4<f32>, sky: vec4<f32> };
@group(0) @binding(0) var<uniform> du: DbgU;
@group(0) @binding(1) var dbg_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var sun_tlas: acceleration_structure;
@group(0) @binding(3) var<storage, read> rt_tris: array<vec4<u32>>;
@group(0) @binding(4) var<storage, read> rt_bits: array<u32>;

// [PC] RAC_POST_VIEW=rtrefl (rtrefl.rs `Debug`): the camera's own view ray per pixel of the half-size target, to the nearest hit
// (see-through texels skipped as the shadows skip them); the static world shaded as reflections will shade it, the moving things
// flat magenta, the sky flat.
@compute @workgroup_size(8, 8) fn cs_debug(@builtin(global_invocation_id) id: vec3<u32>) {
	let wh = vec2<u32>(du.size.xy);
	if (id.x >= wh.x || id.y >= wh.y) { return; }
	let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(wh);
	let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
	let d = normalize(du.f.xyz + du.r.xyz * ndc.x + du.u.xyz * ndc.y);
	var rq: ray_query;
	rayQueryInitialize(&rq, sun_tlas, RayDesc(RAY_FLAG_NONE, 0xffu, 0.05, du.eye.w, du.eye.xyz, d));
	while (rayQueryProceed(&rq)) {
		if (rt_texel_casts(rayQueryGetCandidateIntersection(&rq))) {
			rayQueryConfirmIntersection(&rq);
		}
	}
	let h = rayQueryGetCommittedIntersection(&rq);
	var col = du.sky.rgb;
	if (h.kind != RAY_QUERY_INTERSECTION_NONE) {
		if (h.instance_custom_data == 0u) {
			col = rt_hit_colour(h.geometry_index, h.primitive_index, h.barycentrics, h.t * du.size.z, 0.0);
		} else {
			col = vec3<f32>(0.6, 0.15, 0.6);
		}
	}
	textureStore(dbg_out, vec2<i32>(id.xy), vec4<f32>(col, 1.0));
}
"#;

/// The dev view's shader: `RT_DEBUG`, the hit data from binding 5 and the shared ray code (rtshadow.rs `rt_fns`).
fn debug_shader() -> String {
	format!("enable wgpu_ray_query;\nenable wgpu_binding_array;\n{RT_DEBUG}{}{}", hit_wgsl(0, 5, 1), super::rtshadow::rt_fns())
}

/// The dev view's camera (`DbgU` in the shader): eye (w: the reach), forward, right x horizontal tan, up x vertical tan,
/// (width, height, world units per pixel at distance 1), the sky colour.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DbgU {
	eye: [f32; 4],
	f: [f32; 4],
	r: [f32; 4],
	u: [f32; 4],
	size: [f32; 4],
	sky: [f32; 4],
}

/// How far the dev view's rays look (world units).
const DEBUG_REACH: f32 = 400.0;

/// RAC_POST_VIEW=rtrefl: the camera's view rays traced and shaded into post.rs's rays target, which the composite then shows alone.
pub(super) struct Debug {
	ubuf: wgpu::Buffer,
	layout: wgpu::BindGroupLayout,
	pipeline: wgpu::ComputePipeline,
	/// The stand-in movers' texture table (`movers_tex`).
	none: wgpu::Buffer,
	/// The bind group, for (post targets' generation, structure's hit generation, movers' texture table's generation).
	bind: Option<((u64, u64, u64), wgpu::BindGroup)>,
}

impl Debug {
	/// `array_layout`: the bindless texture array's (bindless.rs), group 1 of the pass.
	pub(super) fn new(device: &wgpu::Device, array_layout: &wgpu::BindGroupLayout) -> Debug {
		let cs = wgpu::ShaderStages::COMPUTE;
		let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: cs, ty, count: None };
		let storage = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None };
		let mut entries = vec![
			entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
			entry(1, wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: wgpu::TextureFormat::Rgba16Float, view_dimension: wgpu::TextureViewDimension::D2 }),
			entry(2, wgpu::BindingType::AccelerationStructure { vertex_return: false }),
			entry(3, storage),
			entry(4, storage),
		];
		entries.extend(hit_layout(5, cs));
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("rt refl debug"), entries: &entries });
		let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("rt refl debug"), bind_group_layouts: &[Some(&layout), Some(array_layout)], immediate_size: 0 });
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rt refl debug"), source: wgpu::ShaderSource::Wgsl(crate::devtune::wgsl("rt refl debug", debug_shader().into())) });
		let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("rt refl debug"), layout: Some(&pl), module: &module, entry_point: Some("cs_debug"), compilation_options: Default::default(), cache: None });
		let ubuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt refl debug"), size: std::mem::size_of::<DbgU>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
		eprintln!("rt reflections: dev view RAC_POST_VIEW=rtrefl (the camera's rays traced and shaded)");
		Debug { ubuf, layout, pipeline, none: movers_none(device), bind: None }
	}

	/// After the sun rays and before the composite: the view rays of `cam` / `aspect` into post.rs's rays target. Nothing until the
	/// level's structure has hit data; `array` is the scene's bindless texture group.
	#[allow(clippy::too_many_arguments)]
	pub(super) fn run(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, rt: &super::rtshadow::RtShadow, post: &super::post::Post, cam: &Camera, aspect: f32, array: &wgpu::BindGroup) {
		let (Some((_, rays, post_gen)), Some((hit, hit_gen))) = (post.rays_io(), rt.hit()) else { return };
		let (w, h) = (rays[1].texture().width(), rays[1].texture().height());
		let fw = forward(cam);
		let r = normalise(cross(fw, cam.up));
		let up = cross(r, fw);
		let (tx, ty) = (cam.tan_half * aspect, cam.tan_half);
		let v = DbgU {
			eye: [cam.pos[0], cam.pos[1], cam.pos[2], DEBUG_REACH],
			f: [fw[0], fw[1], fw[2], 0.0],
			r: [r[0] * tx, r[1] * tx, r[2] * tx, 0.0],
			u: [up[0] * ty, up[1] * ty, up[2] * ty, 0.0],
			size: [w as f32, h as f32, 2.0 * ty / h as f32, 0.0],
			sky: [0.35, 0.5, 0.7, 0.0],
		};
		queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&v));
		let (mtex, mgen) = movers_tex(rt, &self.none);
		if self.bind.as_ref().is_none_or(|b| b.0 != (post_gen, hit_gen, mgen)) {
			// trace_entries are bindings 3..5 of the mask pass's layout; here the structure is 2 and the alpha tables 3 and 4.
			let trace = rt.trace_entries().map(|e| wgpu::BindGroupEntry { binding: e.binding - 1, resource: e.resource });
			let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: self.ubuf.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&rays[1]) }];
			entries.extend(trace);
			entries.extend(hit_entries(hit, mtex, 5));
			self.bind = Some(((post_gen, hit_gen, mgen), device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("rt refl debug"), layout: &self.layout, entries: &entries })));
		}
		let Some((_, bind)) = &self.bind else { return };
		let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("rt refl debug"), timestamp_writes: crate::viewer::passtime::cw("rt refl debug") });
		pass.set_pipeline(&self.pipeline);
		pass.set_bind_group(0, bind, &[]);
		pass.set_bind_group(1, array, &[]);
		pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
	}
}

/// How far a reflected ray looks for something to reflect (world units), by quality: beyond it the far-air guess stays.
fn reach() -> f32 {
	match quality() {
		Quality::Performance => 60.0,
		Quality::Balanced => 120.0,
		Quality::Ultra => 200.0,
	}
}

/// The water shader's line after which the traced reflection replaces the far-air guess (waterfx/shader.rs `fs_water`).
/// (Since v0.4.2 the screen-space march runs only off the mirrored planes; `rt_water` skips those itself, `rt_on_mirror`.)
const WATER_HOOK: &str = "\tif (ml < 0) { hit = reflect_ray(p + gsm * 0.02, rr, jit); }\n\tvar refl = env(r, base);\n";

const RT_WATER: &str = r#"
// [PC] Ray-traced reflections (rtrefl.rs `Water`): the pixel lies on a plane the planar mirror draws (mirror.rs), as `fs_water`
// picks its layer; the mirror's image wins there, so nothing is traced.
fn rt_on_mirror(pf: vec3<f32>, gz: f32) -> bool {
	if (w.mirror.y < 0.5 || gz <= 0.96) { return false; }
	return abs(pf.z - w.mirror.x) < 0.4 || (w.mirror2.z > 1.5 && abs(pf.z - w.mirror2.x) < 0.4) || (w.mirror2.z > 2.5 && abs(pf.z - w.mirror2.y) < 0.4);
}

// [PC] Ray-traced reflections (rtrefl.rs, Balanced and up: RT_BALANCED): the sun's shadow at a traced hit `q` of the static
// world, `along` the whole path from the eye: one ray to the key light (static world and moving things), at the strength the
// level shader's shadows take (sunshadow.rs `strength`, here from the key light's glint, waterfx/passes.rs: both ramp with the
// light's brightness from 0.1, the glint over 0.5 and the shadow over 0.4; and the shadow fades as the light nears the horizon).
fn rt_sun_shadow(q: vec3<f32>, along: f32) -> f32 {
	if (RT_BALANCED < 0.5 || w.sun.w <= 0.0) { return 0.0; }
	let strength = RT_STRENGTH * clamp(w.sun.w / RT_GLINT * 1.25, 0.0, 1.0) * clamp(w.sun.z / 0.15, 0.0, 1.0);
	return strength * rt_ray(q, w.sun.xyz, 0.03 + 0.004 * along);
}

// [PC] Ray-traced reflections (rtrefl.rs): the reflected ray `r` from `o` traced through the static world and the moving things,
// up to RT_REACH: (colour in linear light, how much of it the trace decided). A miss keeps `fallback` (decided 0).
// A hit the picture shows (on screen, not hidden behind something nearer: its depth matches the picture's there) takes the
// picture's colour, softened as the screen-space reflection softens it and moved by the ripples as it is (`kb` x the hit's
// distance, up to 4), so it has the sun, the shadows and the fog the picture has. A hit off screen or hidden is shaded: the
// static world by `rt_hit_colour` (with the sun's shadow from Balanced on), a moving thing by its texture's mean from Balanced
// on (at Performance it keeps `fallback`). Far hits fade back into `fallback`. `rough` widens the softening and the texture
// footprint (water 1; the gloss of lava, sludge and mud more); `dist`: the eye's distance to the surface (the footprint at the
// hit grows with the whole path); `jit`: the pixel's dither.
fn rt_trace(o: vec3<f32>, r: vec3<f32>, fallback: vec3<f32>, dist: f32, rough: f32, kb: vec2<f32>, jit: f32) -> vec4<f32> {
	var rq: ray_query;
	rayQueryInitialize(&rq, sun_tlas, RayDesc(RAY_FLAG_NONE, 0xffu, 0.05, RT_REACH, o, r));
	while (rayQueryProceed(&rq)) {
		if (rt_texel_casts(rayQueryGetCandidateIntersection(&rq))) {
			rayQueryConfirmIntersection(&rq);
		}
	}
	let h = rayQueryGetCommittedIntersection(&rq);
	if (h.kind == RAY_QUERY_INTERSECTION_NONE) { return vec4<f32>(rt_shown(fallback, vec3<f32>(0.0, 0.2, 1.0)), 0.0); }
	// How much of the hit the picture shows: on screen, at the picture's depth there (the edges fade as the screen-space ones do).
	let q = o + r * h.t;
	let s = project(q);
	var seen = 0.0;
	if (on_screen(s) && abs(s.z - scene_z(s.xy)) < 0.05 + 0.02 * s.z) {
		seen = edge_fade(s.xy);
	}
	var col = fallback;
	var known = seen;
	if (seen < 0.999) {
		if (h.instance_custom_data == 0u) {
			let pix = 2.0 * w.proj.y * w.size.w;
			col = to_linear(rt_hit_colour(h.geometry_index, h.primitive_index, h.barycentrics, (dist + h.t) * pix * rough, rt_sun_shadow(q, dist + h.t)));
			known = 1.0;
		} else if (RT_BALANCED > 0.5) {
			col = to_linear(rt_mover_colour(h.geometry_index, h.primitive_index));
			known = 1.0;
		}
	}
	if (seen > 0.0) {
		let bend = project(o + vec3<f32>(kb * min(h.t, 4.0), 0.0)).xy - project(o).xy;
		let uv = clamp(s.xy + bend, vec2<f32>(0.0), vec2<f32>(1.0));
		col = mix(col, soft_scene(uv, 0.002 * rough + 0.014 * h.t / (h.t + dist), jit), seen);
	}
	let far = smoothstep(RT_REACH * 0.5, RT_REACH, h.t);
	return vec4<f32>(rt_shown(mix(col, fallback, far), mix(vec3<f32>(1.0, 0.1, 0.0), vec3<f32>(1.0, 0.9, 0.0), seen)), known * (1.0 - far));
}

// [PC] Ray-traced reflections (rtrefl.rs `Water`): the water's reflection `fallback` (the far-air guess at Performance, where it
// runs before the screen-space reflection; the screen-space reflection over it from Balanced on, where it runs after) with the
// traced ray `r` from `o` laid over it (`rt_trace`). It traces where no mirrored plane lies under the pixel, and at
// Performance only where the screen-space march found nothing sure (`ssr` < 1). At Ultra (RT_ULTRA) the mirror draws nothing, and
// where the ripples turn the full ripple normal's mirror ray `rn` away from `r` a second ray halfway between the two is averaged
// in: rough water then blurs what it reflects as the ripples spread it, instead of showing one sharp image.
fn rt_water(o: vec3<f32>, r: vec3<f32>, fallback: vec3<f32>, ssr: f32, pf: vec3<f32>, gz: f32, dist: f32, jit: f32, kb: vec2<f32>, rn: vec3<f32>) -> vec4<f32> {
	if (RT_FORCE < 0.5 && ((RT_BALANCED < 0.5 && ssr > 0.99) || rt_on_mirror(pf, gz))) { return vec4<f32>(fallback, 0.0); }
	let a = rt_trace(o, r, fallback, dist, 1.0, kb, jit);
	if (RT_ULTRA < 0.5 || dot(r, rn) > 0.999) { return a; }
	let r2 = normalize(r + rn);
	return 0.5 * (a + rt_trace(o, normalize(vec3<f32>(r2.xy, max(r2.z, 0.04))), fallback, dist, 1.0, kb, 1.0 - jit));
}

// [PC] Ray-traced reflections (rtrefl.rs, Balanced and up): the gloss of lava, sludge and mud, `fallback` (their screen-space
// reflection or the far air) with the traced ray laid over it, softer by `rough`.
fn rt_gloss(o: vec3<f32>, r: vec3<f32>, fallback: vec3<f32>, dist: f32, rough: f32, jit: f32) -> vec3<f32> {
	return rt_trace(o, r, fallback, dist, rough, vec2<f32>(0.0), jit).rgb;
}

// RAC_RT_REFL_SHOW=1 (dev): the traced pixels tinted (RT_SHOW 1): yellow where the hit's colour came from the picture, red where
// it was shaded from the level's own data, blue where the ray found nothing.
fn rt_shown(c: vec3<f32>, tint: vec3<f32>) -> vec3<f32> {
	return mix(c, tint, RT_SHOW * 0.7);
}
"#;

/// Dev only: RAC_RT_REFL_SHOW=1 tints the water (and lava, sludge, mud) pixels that traced (`rt_shown`), to see where the traced
/// reflection applies.
fn show() -> bool {
	std::env::var("RAC_RT_REFL_SHOW").is_ok_and(|v| v == "1")
}

/// Dev only: RAC_RT_REFL_FORCE=1 traces every water pixel (`rt_water`), even where the screen-space reflection or the planar
/// mirror has it: the traced reflection can then be held against the mirror's on any water.
fn force() -> bool {
	std::env::var("RAC_RT_REFL_FORCE").is_ok_and(|v| v == "1")
}

/// The water shader's lines that end its screen-space reflection (`fs_water`): from Balanced on the traced one goes after them.
const WATER_SSR_END: &str = "\t\trefl = mix(refl, soft_scene(huv, rad, jit), hit.w);\n\t}\n";
/// How far the ripples move a screen-space hit in `fs_water`, per unit of the hit's distance (its `bend`).
const WATER_KB: &str = "-(rs + wsd * WAVE_BEND) * 0.5 * (1.0 - FAR_BEND * ds)";
/// The gloss lines of lava, sludge and mud (waterfx/shader.rs) and what they become from Balanced on (`rt_gloss`, softer the
/// rougher the surface: mud reflects nothing sharp).
const GLOSS_HOOKS: [(&str, &str); 3] = [
	("\tmolten += refl * fres * LAVA_GLOSS;\n", "\trefl = rt_gloss(p + vec3<f32>(0.0, 0.0, 0.02), r, refl, dist, 1.5, jit);\n\tmolten += refl * fres * LAVA_GLOSS;\n"),
	("\tcol = mix(col, refl, clamp(fres * SLUDGE_GLOSS", "\trefl = rt_gloss(p + vec3<f32>(0.0, 0.0, 0.02), r, refl, dist, 3.0, jit);\n\tcol = mix(col, refl, clamp(fres * SLUDGE_GLOSS"),
	("col += env(r, base) * fres * MUD_SHEEN", "col += rt_gloss(p + vec3<f32>(0.0, 0.0, 0.02), r, env(r, base), dist, 8.0, 0.5) * fres * MUD_SHEEN"),
];

/// The water shader `src` (waterfx/setup.rs) with `rt_water` in `fs_water`: at Performance after the far-air guess (the
/// screen-space reflection is laid over it), from Balanced on (`balanced`) after the screen-space reflection (the trace wins
/// where it hits), with the lava, sludge and mud gloss traced too. The structure and the alpha tables at group 2 bindings 0..2,
/// the hit data from binding 3, the bindless texture array as group 3. `ultra`: the mirrored planes trace too (the mirror draws
/// nothing then) and rippled water takes a second ray.
fn water_shader(src: &str, balanced: bool, ultra: bool) -> String {
	assert!(src.contains(WATER_HOOK) && src.contains(WATER_SSR_END), "rt reflections: fs_water has no reflect_ray / env / screen-space lines to hook");
	let mut src = match balanced {
		false => src.replacen(WATER_HOOK, &format!("{WATER_HOOK}\trefl = rt_water(p + gsm * 0.02, rr, refl, hit.w, pf, g.z, dist, jit, {WATER_KB}, r).rgb;\n"), 1),
		true => {
			let mut s = src.replacen(WATER_SSR_END, &format!("{WATER_SSR_END}\tlet rtw = rt_water(p + gsm * 0.02, rr, refl, hit.w, pf, g.z, dist, jit, {WATER_KB}, r);\n\trefl = rtw.rgb;\n\tsure = max(sure, rtw.a);\n"), 1);
			for (from, to) in GLOSS_HOOKS {
				assert!(s.contains(from), "rt reflections: the water shader has no gloss line {from:?}");
				s = s.replacen(from, to, 1);
			}
			s
		}
	};
	// RAC_RT_REFL_FORCE: the traced reflection alone, so neither the screen-space one nor the mirror's image is laid over it.
	if force() {
		const SSR: &str = "\tif (hit.w > 0.0) {\n\t\tlet bend";
		// No mirror layer: the mirror image is skipped (and the march runs, its hit then dropped by `SSR`).
		const MIRROR: &str = "\tlet ml = mirror_layer(pf.z, g.z);\n";
		assert!(src.contains(SSR) && src.contains(MIRROR), "rt reflections: fs_water has no screen-space / mirror lines to switch off");
		src = src.replacen(SSR, "\tif (false) {\n\t\tlet bend", 1).replacen(MIRROR, "\tlet ml = -1;\n", 1);
	}
	let decls = "@group(2) @binding(0) var sun_tlas: acceleration_structure;\n@group(2) @binding(1) var<storage, read> rt_tris: array<vec4<u32>>;\n@group(2) @binding(2) var<storage, read> rt_bits: array<u32>;\n";
	let flag = |on: bool| if on { "1.0" } else { "0.0" };
	let rt = RT_WATER
		.replace("RT_REACH", &format!("{:.1}", reach()))
		.replace("RT_SHOW", flag(show()))
		.replace("RT_FORCE", flag(force()))
		.replace("RT_BALANCED", flag(balanced))
		.replace("RT_ULTRA", flag(ultra))
		.replace("RT_STRENGTH", &format!("{:.3}", super::sunshadow::STRENGTH))
		.replace("RT_GLINT", &format!("{:.3}", super::waterfx::GLINT));
	format!("enable wgpu_ray_query;\nenable wgpu_binding_array;\n{src}\n{decls}{}{}{rt}", hit_wgsl(2, 3, 3), super::rtshadow::rt_fns())
}

/// The traced passes' group 2: the level's structure and its alpha tables at bindings 0..2, the hit data from binding 3
/// (`hit_wgsl(2, 3, 3)`), made again when the structure's hit data or the movers' texture table change.
struct HitGroup {
	layout: wgpu::BindGroupLayout,
	/// The stand-in movers' texture table (`movers_tex`).
	none: wgpu::Buffer,
	/// For (the structure's hit generation, the movers' texture table's generation).
	bind: std::cell::RefCell<Option<((u64, u64), wgpu::BindGroup)>>,
	label: &'static str,
}

impl HitGroup {
	fn new(device: &wgpu::Device, label: &'static str) -> HitGroup {
		let fs = wgpu::ShaderStages::FRAGMENT;
		let storage = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None };
		let mut entries = vec![
			wgpu::BindGroupLayoutEntry { binding: 0, visibility: fs, ty: wgpu::BindingType::AccelerationStructure { vertex_return: false }, count: None },
			wgpu::BindGroupLayoutEntry { binding: 1, visibility: fs, ty: storage, count: None },
			wgpu::BindGroupLayoutEntry { binding: 2, visibility: fs, ty: storage, count: None },
		];
		entries.extend(hit_layout(3, fs));
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries: &entries });
		HitGroup { layout, none: movers_none(device), bind: Default::default(), label }
	}

	/// The group for the level's structure `rt`, or None until it has hit data.
	fn group(&self, device: &wgpu::Device, rt: &super::rtshadow::RtShadow) -> Option<wgpu::BindGroup> {
		let (hit, hit_gen) = rt.hit()?;
		let (mtex, mgen) = movers_tex(rt, &self.none);
		let generation = (hit_gen, mgen);
		let mut b = self.bind.borrow_mut();
		if b.as_ref().is_none_or(|b| b.0 != generation) {
			// trace_entries are bindings 3..5 of the mask pass's layout; here 0..2.
			let mut entries: Vec<wgpu::BindGroupEntry> = rt.trace_entries().map(|e| wgpu::BindGroupEntry { binding: e.binding - 3, resource: e.resource }).into();
			entries.extend(hit_entries(hit, mtex, 3));
			*b = Some((generation, device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(self.label), layout: &self.layout, entries: &entries })));
		}
		b.as_ref().map(|b| b.1.clone())
	}
}

/// Phase 3: the water's shading pass with traced reflections (`water_shader`), used in place of waterfx's own while Reflections
/// are on and the level's structure has hit data.
pub(super) struct Water {
	hit: HitGroup,
	/// The pipeline layout, the water module's source and the post scene's format, to build the pipeline for a setting.
	pl: wgpu::PipelineLayout,
	src: String,
	format: wgpu::TextureFormat,
	built: ForSetting<wgpu::RenderPipeline>,
}

impl Water {
	/// None unless Reflections were on at start-up, the device traces and the bindless array exists (`array_layout`). `src`: the
	/// water module's source, `water_layout` its group 0, `format` the post scene's.
	pub(super) fn new(device: &wgpu::Device, src: &str, water_layout: &wgpu::BindGroupLayout, array_layout: Option<&wgpu::BindGroupLayout>, format: wgpu::TextureFormat) -> Option<Water> {
		let array_layout = array_layout.filter(|_| wanted() && super::rtshadow::on())?;
		let hit = HitGroup::new(device, "rt refl water");
		let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("rt refl water"), bind_group_layouts: &[Some(water_layout), None, Some(&hit.layout), Some(array_layout)], immediate_size: 0 });
		let w = Water { hit, pl, src: src.to_string(), format, built: ForSetting::default() };
		w.pipeline(device);
		Some(w)
	}

	/// The traced shading pipeline for the setting now, None while Reflections are Off (the water then shades as before).
	pub(super) fn pipeline(&self, device: &wgpu::Device) -> Option<wgpu::RenderPipeline> {
		self.built.get(|| tracing().then(|| self.make(device)))
	}

	fn make(&self, device: &wgpu::Device) -> wgpu::RenderPipeline {
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rt refl water"), source: wgpu::ShaderSource::Wgsl(crate::devtune::wgsl("rt refl water", water_shader(&self.src, movers_shaded(), mirrors_traced()).into())) });
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("water shade rt"),
			layout: Some(&self.pl),
			vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
			fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_water"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
			primitive: Default::default(),
			depth_stencil: None,
			multisample: Default::default(),
			multiview_mask: None,
			cache: None,
		});
		let where_ = match (force(), movers_shaded(), mirrors_traced()) {
			(true, _, _) => "everywhere (RAC_RT_REFL_FORCE)",
			(false, false, _) => "where the screen-space reflection misses",
			(false, true, false) => "off the mirrored planes, with sun shadows, moving things and the lava, sludge and mud gloss",
			(false, true, true) => "everywhere, the planar mirror off, 2 rays on ripples, with sun shadows, moving things and the lava, sludge and mud gloss",
		};
		eprintln!("rt reflections: water traced {where_} (reach {:.0}){}", reach(), if show() { ", traced pixels tinted (RAC_RT_REFL_SHOW)" } else { "" });
		pipeline
	}

	/// Group 2 for the level's structure `rt`, or None until it has hit data (the water then shades as before).
	pub(super) fn group(&self, device: &wgpu::Device, rt: &super::rtshadow::RtShadow) -> Option<wgpu::BindGroup> {
		self.hit.group(device, rt)
	}
}

/// A traced pass's pipelines and the Reflections setting they were built for: built again when the setting changes (`cycle`),
/// None where that setting does not trace the pass.
struct ForSetting<T>(std::cell::RefCell<Option<(u8, Option<T>)>>);

impl<T> Default for ForSetting<T> {
	fn default() -> Self {
		ForSetting(Default::default())
	}
}

impl<T: Clone> ForSetting<T> {
	fn get(&self, make: impl FnOnce() -> Option<T>) -> Option<T> {
		let s = setting();
		let mut b = self.0.borrow_mut();
		if b.as_ref().is_none_or(|b| b.0 != s) {
			*b = Some((s, make()));
		}
		b.as_ref().and_then(|b| b.1.clone())
	}
}

/// How much of the chrome and glass the traced hit takes over from the retail environment texture: Balanced mixes the two (the
/// env texture keeps the PS2's look of the metal), Ultra shows the hit alone.
fn metal_mix() -> f32 {
	match quality() {
		Quality::Ultra => 1.0,
		_ => 0.6,
	}
}

/// How much of its mirror hit the wrench's steel head takes (`rt_wrench`): less than the chrome, as the PS2's wrench is painted
/// steel, not a mirror.
fn wrench_mix() -> f32 {
	metal_mix() * 0.75
}

const RT_METAL: &str = r#"
// [PC] Ray-traced reflections (rtrefl.rs `Metal`): 0 for the chrome and glass, 1 for the wrench's gloss (the pipelines' constant).
override RT_GLOSS: f32 = 0.0;

// [PC] Ray-traced reflections (rtrefl.rs `Metal`, Balanced and up): what the fragment's mirror ray sees, (colour, how much of it
// holds; 0 for a miss). One ray along the mirror of the view finds what the surface really faces: the static world shaded as
// `rt_hit_colour` (with the sun's shadow at the hit), a moving thing by its texture's mean, both fogged over the ray's own length.
// The surface is drawn inside the level pass, so the picture is not there to take hits from. Far hits fade out.
fn rt_metal_hit(i: VOut) -> vec4<f32> {
	let ln = length(i.n);
	if (ln < 1e-4) { return vec4<f32>(0.0); }
	var n = i.n / ln;
	let to = i.w - u.eye.xyz;
	let dist = length(to);
	let v = to / max(dist, 1e-4);
	// A packet drawn without culling shows its back faces too: they mirror on their own side.
	if (dot(n, v) > 0.0) { n = -n; }
	let r = reflect(v, n);
	let o = i.w + n * (0.02 + 0.002 * dist);
	var rq: ray_query;
	rayQueryInitialize(&rq, sun_tlas, RayDesc(RAY_FLAG_NONE, 0xffu, 0.02, RT_REACH, o, r));
	while (rayQueryProceed(&rq)) {
		if (rt_texel_casts(rayQueryGetCandidateIntersection(&rq))) {
			rayQueryConfirmIntersection(&rq);
		}
	}
	let h = rayQueryGetCommittedIntersection(&rq);
	if (h.kind == RAY_QUERY_INTERSECTION_NONE) { return vec4<f32>(0.0); }
	var col: vec3<f32>;
	if (h.instance_custom_data == 0u) {
		// One pixel's footprint at the hit grows with the whole path (a curved mirror spreads it more; kept simple).
		let pix = 2.0 / max(abs(u.view_proj[1][1]), 1e-4) / RT_HEIGHT;
		var shadow = 0.0;
		if (u.sun.w > 0.0) {
			shadow = RT_STRENGTH * min(u.sun.w * 1.4, 1.0) * rt_ray(o + r * h.t, -normalize(u.sun.xyz), 0.03 + 0.004 * (dist + h.t));
		}
		col = rt_hit_colour(h.geometry_index, h.primitive_index, h.barycentrics, (dist + h.t) * pix, shadow);
	} else {
		col = rt_mover_colour(h.geometry_index, h.primitive_index);
	}
	// The level's fog over the ray's own length (the fragment's own fog over the eye's path is laid on after, as on any moby).
	let f = clamp(u.fog.z + (h.t - u.fog.x) * (u.fog.w - u.fog.z) / max(u.fog.y - u.fog.x, 0.001), min(u.fog.z, u.fog.w), 255.0) / 255.0;
	return vec4<f32>(mix(u.fog_colour.rgb, col, f), 1.0 - smoothstep(RT_REACH * 0.5, RT_REACH, h.t));
}

// Chrome and glass (the metal packets, NOV-35b): `env` is the retail environment texel (the PS2's fake chrome / glass picture); a
// hit takes RT_MIX of it, a miss keeps it.
fn rt_metal(i: VOut, env: vec3<f32>) -> vec3<f32> {
	let h = rt_metal_hit(i);
	if (h.a <= 0.0) { return rt_metal_shown(env, vec3<f32>(0.0, 0.2, 1.0)); }
	return rt_metal_shown(mix(env, h.rgb, RT_MIX * h.a), vec3<f32>(1.0, 0.1, 0.0));
}

// [PC] The held wrench's head (not retail: a plain textured moby there, class 71, rtrefl.rs `Metal`): where its texel is bright
// and grey (the steel, not the handle's colour or the dark grip) it takes RT_WRENCH of its mirror hit, tinted a little by the
// texel so the steel keeps its shade. Elsewhere nothing is traced.
fn rt_wrench(i: VOut, c: vec3<f32>) -> vec3<f32> {
	let hi = max(c.r, max(c.g, c.b));
	let sat = (hi - min(c.r, min(c.g, c.b))) / max(hi, 1e-3);
	let lum = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
	let steel = smoothstep(0.2, 0.45, lum) * (1.0 - smoothstep(0.12, 0.3, sat));
	if (steel < 0.02) { return c; }
	let h = rt_metal_hit(i);
	if (h.a <= 0.0) { return rt_metal_shown(c, vec3<f32>(0.0, 0.2, 1.0) * steel + c * (1.0 - steel)); }
	let tinted = h.rgb * mix(vec3<f32>(1.0), c / max(lum, 0.05), 0.35);
	return rt_metal_shown(mix(c, tinted, RT_WRENCH * steel * h.a), vec3<f32>(1.0, 0.1, 0.0) * steel + c * (1.0 - steel));
}

// The texel of a surface the metal module draws: the chrome and glass, or the wrench (RT_GLOSS).
fn rt_surface(i: VOut, c: vec3<f32>) -> vec3<f32> {
	if (RT_GLOSS > 0.5) { return rt_wrench(i, c); }
	return rt_metal(i, c);
}

// RAC_RT_REFL_SHOW=1 (dev): the traced surfaces tinted as the water is: red where a hit was shaded, blue where nothing was hit.
fn rt_metal_shown(c: vec3<f32>, tint: vec3<f32>) -> vec3<f32> {
	return mix(c, tint, RT_SHOW * 0.7);
}
"#;

/// The level shader lines where the metal faces' texel is read, the opaque `fs` and the blended `moby_rgb` (`fs_moby_a` / `_b`),
/// and what they become in the metal module (the texel's colour through `rt_metal`).
const METAL_HOOKS: [(&str, &str); 2] = [
	("\tif (i.c.a >= 0.0 && i.kmx.y >= 0.0) { c = textureSampleLevel(tex, samp, i.uv, gs_level(i)); }\n\tif (c.a < 0.25) { discard; }\n", "\tif (i.c.a >= 0.0 && i.kmx.y >= 0.0) { c = textureSampleLevel(tex, samp, i.uv, gs_level(i)); }\n\tif (c.a < 0.25) { discard; }\n\tc = vec4<f32>(rt_metal(i, c.rgb), c.a);\n"),
	("fn moby_rgb(i: VOut, c: vec4<f32>) -> vec3<f32> {\n\tvar rgb = c.rgb * i.c.rgb * sun_lit(i);\n", "fn moby_rgb(i: VOut, c: vec4<f32>) -> vec3<f32> {\n\tvar rgb = rt_surface(i, c.rgb) * i.c.rgb * sun_lit(i);\n"),
];

/// The level shader `src` (as the device runs it, rtshadow.rs `level`) for the metal faces: `rt_metal` on their texel. The level's
/// structure and alpha tables are group 2 bindings 0..2, unless the shadow receivers' shader already declares them in group 0
/// (the bind group carries them either way), the hit data from group 2 binding 3, the bindless texture array as group 3.
/// `height`: the window's height in pixels (a ray's footprint).
fn metal_shader(src: &str, height: f32) -> String {
	let mut src = src.to_string();
	for (from, to) in METAL_HOOKS {
		assert!(src.contains(from), "rt reflections: the level shader has no metal texel line {from:?}");
		src = src.replacen(from, to, 1);
	}
	let flag = |on: bool| if on { "1.0" } else { "0.0" };
	let rt = RT_METAL
		.replace("RT_REACH", &format!("{:.1}", reach()))
		.replace("RT_SHOW", flag(show()))
		.replace("RT_MIX", &format!("{:.3}", metal_mix()))
		.replace("RT_WRENCH", &format!("{:.3}", wrench_mix()))
		.replace("RT_HEIGHT", &format!("{:.1}", height.max(1.0)))
		.replace("RT_STRENGTH", &format!("{:.3}", super::sunshadow::STRENGTH));
	// The shadow receivers' level shader already has the structure, the alpha tables and the ray code (rtshadow.rs `shader`).
	let (head, decls) = match src.starts_with("enable wgpu_ray_query;") {
		true => ("enable wgpu_binding_array;\n", String::new()),
		false => ("enable wgpu_ray_query;\nenable wgpu_binding_array;\n", format!("@group(2) @binding(0) var sun_tlas: acceleration_structure;\n@group(2) @binding(1) var<storage, read> rt_tris: array<vec4<u32>>;\n@group(2) @binding(2) var<storage, read> rt_bits: array<u32>;\n{}", super::rtshadow::rt_fns())),
	};
	// The level shader has its own `samp` (group 1): the bindless array's sampler goes by another name here.
	let hit = hit_wgsl(2, 3, 3).replace("var samp: sampler", "var rt_samp: sampler").replace("], samp,", "], rt_samp,");
	format!("{head}{src}\n{decls}{hit}{rt}")
}

/// Phase 6: the props' chrome and glass (gpu/metal.rs) drawn with traced reflections (`metal_shader`) in place of the level
/// pipelines they use otherwise, from Balanced on, once the level's structure has hit data.
pub(super) struct Metal {
	hit: HitGroup,
	/// The pipeline layout, the level shader as the device runs it, the level pass's targets and the window's height, to build
	/// the pipelines for a setting.
	pl: wgpu::PipelineLayout,
	src: String,
	format: wgpu::TextureFormat,
	depth: wgpu::TextureFormat,
	height: f32,
	built: ForSetting<MetalPipelines>,
}

#[derive(Clone)]
pub(super) struct MetalPipelines {
	/// The opaque pass's states (as `pipeline`, then `pipeline_cull` back and front).
	pub opaque: [wgpu::RenderPipeline; 3],
	/// The ABE pass's states (as `moby_pipelines`).
	pub blend: [wgpu::RenderPipeline; 2],
	/// The held wrench's opaque draws (`rt_wrench`; the opaque states, as `opaque`).
	pub gloss: [wgpu::RenderPipeline; 3],
}

impl Metal {
	/// None unless Reflections were on at start-up, the device traces and the bindless array exists (`array_layout`). `src`: the
	/// level shader as the device runs it, `ulayout` / `tex_layout` its groups 0 and 1, `format` / `depth` the level pass's
	/// targets, `height` the window's height.
	#[allow(clippy::too_many_arguments)]
	pub(super) fn new(device: &wgpu::Device, src: &str, ulayout: &wgpu::BindGroupLayout, tex_layout: &wgpu::BindGroupLayout, array_layout: Option<&wgpu::BindGroupLayout>, format: wgpu::TextureFormat, depth: wgpu::TextureFormat, height: f32) -> Option<Metal> {
		let array_layout = array_layout.filter(|_| wanted() && super::rtshadow::on())?;
		let hit = HitGroup::new(device, "rt refl metal");
		let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("rt refl metal"), bind_group_layouts: &[Some(ulayout), Some(tex_layout), Some(&hit.layout), Some(array_layout)], immediate_size: 0 });
		let m = Metal { hit, pl, src: src.to_string(), format, depth, height, built: ForSetting::default() };
		m.pipelines(device);
		Some(m)
	}

	/// The traced pipelines for the setting now, None below Balanced (the metal then draws as before).
	pub(super) fn pipelines(&self, device: &wgpu::Device) -> Option<MetalPipelines> {
		self.built.get(|| movers_shaded().then(|| self.make(device)))
	}

	fn make(&self, device: &wgpu::Device) -> MetalPipelines {
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rt refl metal"), source: wgpu::ShaderSource::Wgsl(crate::devtune::wgsl("rt refl metal", metal_shader(&self.src, self.height).into())) });
		let make = |fs: &str, blend: Option<wgpu::BlendState>, depth_write: bool, compare: wgpu::CompareFunction, cull: Option<wgpu::Face>, gloss: f64| {
			device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
				label: Some("metal rt"),
				layout: Some(&self.pl),
				vertex: wgpu::VertexState {
					module: &module,
					entry_point: Some("vs"),
					compilation_options: Default::default(),
					buffers: &[Some(wgpu::VertexBufferLayout {
						array_stride: std::mem::size_of::<Vertex>() as u64,
						step_mode: wgpu::VertexStepMode::Vertex,
						attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x3, 3 => Float32x4, 4 => Float32x4],
					})],
				},
				fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(fs), compilation_options: wgpu::PipelineCompilationOptions { constants: &[("RT_GLOSS", gloss)], ..Default::default() }, targets: &[Some(wgpu::ColorTargetState { format: self.format, blend, write_mask: wgpu::ColorWrites::ALL })] }),
				primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
				depth_stencil: Some(wgpu::DepthStencilState { format: self.depth, depth_write_enabled: Some(depth_write), depth_compare: Some(compare), stencil: Default::default(), bias: Default::default() }),
				multisample: wgpu::MultisampleState { count: MSAA, ..Default::default() },
				multiview_mask: None,
				cache: None,
			})
		};
		let z = super::gs_state::ztest();
		let culls = [None, Some(wgpu::Face::Back), Some(wgpu::Face::Front)];
		let opaque = culls.map(|c| make("fs", None, true, z, c, 0.0));
		let ab = Some(wgpu::BlendState::ALPHA_BLENDING);
		let blend = [make("fs_moby_a", ab, true, wgpu::CompareFunction::LessEqual, None, 0.0), make("fs_moby_b", ab, false, wgpu::CompareFunction::LessEqual, None, 0.0)];
		let gloss = culls.map(|c| make("fs", None, true, z, c, 1.0));
		eprintln!("rt reflections: chrome and glass traced ({:.0}% over the env texture, the wrench's steel {:.0}%, reach {:.0})", metal_mix() * 100.0, wrench_mix() * 100.0, reach());
		MetalPipelines { opaque, blend, gloss }
	}

	/// Group 2 for the level's structure `rt`, or None until it has hit data (the metal then draws as before).
	pub(super) fn group(&self, device: &wgpu::Device, rt: &super::rtshadow::RtShadow) -> Option<wgpu::BindGroup> {
		self.hit.group(device, rt)
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn ray_traced_reflection_shaders_validate() {
		let m = naga::front::wgsl::parse_str(&super::debug_shader()).expect("rt refl debug wgsl parse");
		naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).expect("rt refl debug wgsl validate");
		// The traced water at Performance and from Balanced on (its hooks found, its source whole).
		let src = super::super::waterfx::shade_source("");
		for (balanced, ultra) in [(false, false), (true, false), (true, true)] {
			let m = naga::front::wgsl::parse_str(&super::water_shader(&src, balanced, ultra)).unwrap_or_else(|e| panic!("rt refl water wgsl parse (balanced {balanced}, ultra {ultra}): {e}"));
			naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).unwrap_or_else(|e| panic!("rt refl water wgsl validate (balanced {balanced}, ultra {ultra}): {e:?}"));
		}
		// The traced chrome and glass, over the plain level shader and over the shadow receivers' one.
		let level = super::super::volfog::soft::level_source(super::super::SHADER, false);
		for shadows in [false, true] {
			let src = if shadows { super::super::rtshadow::shader(&level) } else { level.clone() };
			let m = naga::front::wgsl::parse_str(&super::metal_shader(&src, 1440.0)).unwrap_or_else(|e| panic!("rt refl metal wgsl parse (shadows {shadows}): {e}"));
			naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).unwrap_or_else(|e| panic!("rt refl metal wgsl validate (shadows {shadows}): {e:?}"));
		}
	}
}
