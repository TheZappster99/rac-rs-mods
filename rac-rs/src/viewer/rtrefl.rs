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
		true => eprintln!("rt reflections: on, quality {:?} (phase 5: {})", quality(), if movers_shaded() { "water, lava, sludge, mud" } else { "water" }),
		false => eprintln!("rt reflections: switched on, but the adapter has no ray queries on Vulkan: off"),
	}
}

/// Reflections at Balanced and up: the whole water traces, with the sun's shadow at a hit and the moving things shaded off
/// screen (rtmovers.rs keeps their texture table for it).
pub(super) fn movers_shaded() -> bool {
	wanted() && quality() != Quality::Performance
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
const WATER_HOOK: &str = "\tlet hit = reflect_ray(p + gsm * 0.02, rr, jit);\n\tvar refl = env(r, base);\n";

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
// Performance only where the screen-space march found nothing sure (`ssr` < 1).
fn rt_water(o: vec3<f32>, r: vec3<f32>, fallback: vec3<f32>, ssr: f32, pf: vec3<f32>, gz: f32, dist: f32, jit: f32, kb: vec2<f32>) -> vec4<f32> {
	if (RT_FORCE < 0.5 && ((RT_BALANCED < 0.5 && ssr > 0.99) || rt_on_mirror(pf, gz))) { return vec4<f32>(fallback, 0.0); }
	return rt_trace(o, r, fallback, dist, 1.0, kb, jit);
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
/// the hit data from binding 3, the bindless texture array as group 3.
fn water_shader(src: &str, balanced: bool) -> String {
	assert!(src.contains(WATER_HOOK) && src.contains(WATER_SSR_END), "rt reflections: fs_water has no reflect_ray / env / screen-space lines to hook");
	let mut src = match balanced {
		false => src.replacen(WATER_HOOK, &format!("{WATER_HOOK}\trefl = rt_water(p + gsm * 0.02, rr, refl, hit.w, pf, g.z, dist, jit, {WATER_KB}).rgb;\n"), 1),
		true => {
			let mut s = src.replacen(WATER_SSR_END, &format!("{WATER_SSR_END}\tlet rtw = rt_water(p + gsm * 0.02, rr, refl, hit.w, pf, g.z, dist, jit, {WATER_KB});\n\trefl = rtw.rgb;\n\tsure = max(sure, rtw.a);\n"), 1);
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
		const MIRROR: &str = "if (w.mirror.y > 0.5 && g.z > 0.96) {";
		assert!(src.contains(SSR) && src.contains(MIRROR), "rt reflections: fs_water has no screen-space / mirror lines to switch off");
		src = src.replacen(SSR, "\tif (false) {\n\t\tlet bend", 1).replacen(MIRROR, "if (false) {", 1);
	}
	let decls = "@group(2) @binding(0) var sun_tlas: acceleration_structure;\n@group(2) @binding(1) var<storage, read> rt_tris: array<vec4<u32>>;\n@group(2) @binding(2) var<storage, read> rt_bits: array<u32>;\n";
	let flag = |on: bool| if on { "1.0" } else { "0.0" };
	let rt = RT_WATER
		.replace("RT_REACH", &format!("{:.1}", reach()))
		.replace("RT_SHOW", flag(show()))
		.replace("RT_FORCE", flag(force()))
		.replace("RT_BALANCED", flag(balanced))
		.replace("RT_STRENGTH", &format!("{:.3}", super::sunshadow::STRENGTH))
		.replace("RT_GLINT", &format!("{:.3}", super::waterfx::GLINT));
	format!("enable wgpu_ray_query;\nenable wgpu_binding_array;\n{src}\n{decls}{}{}{rt}", hit_wgsl(2, 3, 3), super::rtshadow::rt_fns())
}

/// Phase 3: the water's shading pass with traced reflections (`water_shader`), used in place of waterfx's own while Reflections
/// are on and the level's structure has hit data.
pub(super) struct Water {
	layout: wgpu::BindGroupLayout,
	pub pipeline: wgpu::RenderPipeline,
	/// The stand-in movers' texture table (`movers_tex`).
	none: wgpu::Buffer,
	/// Group 2, for (the structure's hit generation, the movers' texture table's generation).
	bind: std::cell::RefCell<Option<((u64, u64), wgpu::BindGroup)>>,
}

impl Water {
	/// None unless Reflections are on, the device traces and the bindless array exists (`array_layout`). `src`: the water
	/// module's source, `water_layout` its group 0, `format` the post scene's.
	pub(super) fn new(device: &wgpu::Device, src: &str, water_layout: &wgpu::BindGroupLayout, array_layout: Option<&wgpu::BindGroupLayout>, format: wgpu::TextureFormat) -> Option<Water> {
		let array_layout = array_layout.filter(|_| wanted() && super::rtshadow::on())?;
		let fs = wgpu::ShaderStages::FRAGMENT;
		let storage = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None };
		let mut entries = vec![
			wgpu::BindGroupLayoutEntry { binding: 0, visibility: fs, ty: wgpu::BindingType::AccelerationStructure { vertex_return: false }, count: None },
			wgpu::BindGroupLayoutEntry { binding: 1, visibility: fs, ty: storage, count: None },
			wgpu::BindGroupLayoutEntry { binding: 2, visibility: fs, ty: storage, count: None },
		];
		entries.extend(hit_layout(3, fs));
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("rt refl water"), entries: &entries });
		let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("rt refl water"), bind_group_layouts: &[Some(water_layout), None, Some(&layout), Some(array_layout)], immediate_size: 0 });
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rt refl water"), source: wgpu::ShaderSource::Wgsl(crate::devtune::wgsl("rt refl water", water_shader(src, movers_shaded()).into())) });
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("water shade rt"),
			layout: Some(&pl),
			vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
			fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_water"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
			primitive: Default::default(),
			depth_stencil: None,
			multisample: Default::default(),
			multiview_mask: None,
			cache: None,
		});
		let where_ = match (force(), movers_shaded()) {
			(true, _) => "everywhere (RAC_RT_REFL_FORCE)",
			(false, false) => "where the screen-space reflection misses",
			(false, true) => "off the mirrored planes, with sun shadows, moving things and the lava, sludge and mud gloss",
		};
		eprintln!("rt reflections: water traced {where_} (reach {:.0}){}", reach(), if show() { ", traced pixels tinted (RAC_RT_REFL_SHOW)" } else { "" });
		Some(Water { layout, pipeline, none: movers_none(device), bind: Default::default() })
	}

	/// Group 2 for the level's structure `rt`, or None until it has hit data (the water then shades as before).
	pub(super) fn group(&self, device: &wgpu::Device, rt: &super::rtshadow::RtShadow) -> Option<wgpu::BindGroup> {
		let (hit, hit_gen) = rt.hit()?;
		let (mtex, mgen) = movers_tex(rt, &self.none);
		let generation = (hit_gen, mgen);
		let mut b = self.bind.borrow_mut();
		if b.as_ref().is_none_or(|b| b.0 != generation) {
			// trace_entries are bindings 3..5 of the mask pass's layout; here 0..2.
			let mut entries: Vec<wgpu::BindGroupEntry> = rt.trace_entries().map(|e| wgpu::BindGroupEntry { binding: e.binding - 3, resource: e.resource }).into();
			entries.extend(hit_entries(hit, mtex, 3));
			*b = Some((generation, device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("rt refl water"), layout: &self.layout, entries: &entries })));
		}
		b.as_ref().map(|b| b.1.clone())
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
		for balanced in [false, true] {
			let m = naga::front::wgsl::parse_str(&super::water_shader(&src, balanced)).unwrap_or_else(|e| panic!("rt refl water wgsl parse (balanced {balanced}): {e}"));
			naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).unwrap_or_else(|e| panic!("rt refl water wgsl validate (balanced {balanced}): {e:?}"));
		}
	}
}
