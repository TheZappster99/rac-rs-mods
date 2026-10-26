//! [PC] Ray-traced sun rays (the built-in mod Ray-traced sun rays, plugins.rs: Mods in the launcher, read at start-up; RAC_RT_RAYS=1 forces it on).
//! No retail counterpart. They take the place of the screen-space Sun rays (post.rs `fs_rmask` / `fs_rays`) while the level's
//! ray-tracing structure (rtshadow.rs) is built, with Lighting Enhanced, Dynamic shadows and Sun rays on; otherwise the
//! screen-space ones draw as before.
//!
//! - `cs_march`: per pixel of the half-size linear depth (post.rs `lin`), the view ray is marched from the eye to the surface it
//!   sees (the sky: `REACH` world units) in `steps` samples, denser near the eye and staggered per pixel. Each sample traces one
//!   ray towards the sun through the static world (rtshadow.rs `rt_ray`, see-through leaves included); the lit ones scatter the
//!   key light towards the eye, through air of density `SIGMA` and a mostly forward phase (`G`), so the shafts are strongest
//!   looking towards the sun but stand in the air from every side. Written into post.rs's ray mask target.
//! - `cs_blur`: a depth-aware 5x5 blur of that into post.rs's rays target, which the composite adds over the picture as the
//!   screen-space rays did. Mobies cast no shafts yet (rtshadow.rs phase 4).
//!
//! The ray-tracing device and structure come up when either this mod or Ray-traced shadows is on at start-up; this mod
//! alone leaves the shadows to the cascades (rtshadow.rs `shadows`).

use super::*;

/// The Ray-traced sun rays built-in mod (read at start-up: the ray-tracing device feature is requested when the window opens).
pub(super) fn wanted() -> bool {
	crate::plugins::on_at_start("rt-sun-rays")
}

/// How far the view ray is marched into the sky (world units).
const REACH: f32 = 120.0;

/// Samples per pixel by the Graphics Sun rays level (1 Low, 2 Medium, otherwise High).
const fn steps(level: u8) -> u32 {
	match level {
		1 => 8,
		2 => 12,
		_ => 16,
	}
}

/// The march's camera and light (`RaysU` in the shaders): eye (w: the march's reach), forward, right x horizontal tan, up x
/// vertical tan (w: samples), the light's travel direction (w: its strength 0..1), its colour, (width, height) of the target.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RaysU {
	eye: [f32; 4],
	f: [f32; 4],
	r: [f32; 4],
	u: [f32; 4],
	sun: [f32; 4],
	col: [f32; 4],
	size: [f32; 4],
}

const RAYS_HEAD: &str = r#"
struct RaysU { eye: vec4<f32>, f: vec4<f32>, r: vec4<f32>, u: vec4<f32>, sun: vec4<f32>, col: vec4<f32>, size: vec4<f32> };
@group(0) @binding(0) var lin: texture_2d<f32>;
@group(0) @binding(1) var<uniform> ru: RaysU;
"#;

const RAYS_MARCH: &str = r#"
@group(0) @binding(2) var rays_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var sun_tlas: acceleration_structure;
@group(0) @binding(4) var<storage, read> rt_tris: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read> rt_bits: array<u32>;

// The air's density (per world unit) and how forward it scatters (Henyey-Greenstein g; 0 = the same every way).
// (First playtest, Kerwan towards the sun, 2026-10-25: SIGMA 0.006 / GAIN 1.6 / no cap washed the whole view out to white and
// drowned the shafts; thinner air, a lower gain and a soft cap keep the picture and let the shadowed gaps read.)
// (Second playtest, Kerwan in the sun: SIGMA 0.003 / FORWARD 0.7 / CAP 0.35 still laid a flat yellow veil over the far
// buildings in every direction; thinner air, a narrower same-every-way share and a lower cap keep the shafts towards the sun.)
const SIGMA: f32 = 0.0015;
const G: f32 = 0.6;
// The share of the light scattered by the forward lobe (the rest the same every way), the shafts' overall gain, and the soft
// cap the added light approaches (linear).
const FORWARD: f32 = 0.85;
const GAIN: f32 = 0.6;
const CAP: f32 = 0.2;

fn ign(x: vec2<f32>) -> f32 {
	return fract(52.9829189 * fract(dot(x, vec2<f32>(0.06711056, 0.00583715))));
}

// Henyey-Greenstein over the isotropic phase (1 = scatters as much as the same-every-way air would).
fn phase(c: f32) -> f32 {
	let hg = (1.0 - G * G) / pow(max(1.0 + G * G - 2.0 * G * c, 1e-4), 1.5);
	return mix(1.0, hg, FORWARD);
}

// [PC] Ray-traced sun rays (rtrays.rs): the key light scattered towards the eye by the lit air along this pixel's view ray.
@compute @workgroup_size(8, 8) fn cs_march(@builtin(global_invocation_id) id: vec3<u32>) {
	let wh = vec2<u32>(ru.size.xy);
	if (id.x >= wh.x || id.y >= wh.y) { return; }
	let z = textureLoad(lin, vec2<i32>(id.xy), 0).r;
	let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(wh);
	// The world step per unit of view depth: the march runs to the seen surface, the sky to the reach.
	let d = ru.f.xyz + (uv.x * 2.0 - 1.0) * ru.r.xyz + (1.0 - uv.y * 2.0) * ru.u.xyz;
	let len = length(d);
	let dir = d / len;
	let reach = min(z * len, ru.eye.w);
	let l = -ru.sun.xyz;
	let n = u32(ru.u.w);
	let jit = ign(vec2<f32>(id.xy));
	var acc = 0.0;
	for (var i = 0u; i < n; i++) {
		// Sample i of n sits at reach * ((i + jit) / n)^2 and stands for the segment between (i / n)^2 and ((i + 1) / n)^2.
		let a = (f32(i) + jit) / f32(n);
		let t = reach * a * a;
		let dt = reach * (2.0 * f32(i) + 1.0) / f32(n * n);
		let lit = 1.0 - rt_ray(ru.eye.xyz + dir * t, l, 0.0);
		acc += lit * exp(-SIGMA * t) * SIGMA * dt;
	}
	var c = ru.col.rgb * (acc * phase(dot(dir, l)) * ru.sun.w * GAIN);
	// Soft cap: bright haze rolls off towards CAP instead of burning the picture to white.
	c = c / (1.0 + max(c.r, max(c.g, c.b)) / CAP);
	textureStore(rays_out, vec2<i32>(id.xy), vec4<f32>(c, 1.0));
}
"#;

const RAYS_BLUR: &str = r#"
@group(0) @binding(2) var rays_in: texture_2d<f32>;
@group(0) @binding(3) var rays_out: texture_storage_2d<rgba16float, write>;

// [PC] Ray-traced sun rays (rtrays.rs): the march's per-pixel stagger smoothed over 5x5, weighed by how close each neighbour's
// depth is, so a shaft in front of a wall does not bleed onto the sky beside it.
@compute @workgroup_size(8, 8) fn cs_blur(@builtin(global_invocation_id) id: vec3<u32>) {
	let wh = vec2<i32>(ru.size.xy);
	let c = vec2<i32>(id.xy);
	if (c.x >= wh.x || c.y >= wh.y) { return; }
	let z = textureLoad(lin, c, 0).r;
	let tol = 0.05 * z + 0.1;
	var s = vec3<f32>(0.0);
	var ws = 0.0;
	for (var dy = -2; dy <= 2; dy++) {
		for (var dx = -2; dx <= 2; dx++) {
			let q = clamp(c + vec2<i32>(dx, dy), vec2<i32>(0), wh - vec2<i32>(1));
			let zq = textureLoad(lin, q, 0).r;
			let w = exp(-f32(dx * dx + dy * dy) * 0.25) * exp(-abs(zq - z) / tol);
			s += textureLoad(rays_in, q, 0).rgb * w;
			ws += w;
		}
	}
	textureStore(rays_out, c, vec4<f32>(s / max(ws, 1e-4), 1.0));
}
"#;

fn march_shader() -> String {
	format!("enable wgpu_ray_query;\n{RAYS_HEAD}{RAYS_MARCH}{}", super::rtshadow::rt_fns())
}

fn blur_shader() -> String {
	format!("{RAYS_HEAD}{RAYS_BLUR}")
}

/// The colour of the key light `sunshadow::key_light` picks from slot 0 of `rows` (as `Gpu::light_rows` gives them), white
/// without a lit direction.
pub(super) fn key_colour(rows: &[[[f32; 4]; 4]; 16]) -> [f32; 3] {
	let r = rows[0];
	let luma = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
	let lit = |to: [f32; 4]| to[0] * to[0] + to[1] * to[1] + to[2] * to[2] > 1e-6;
	[(r[0], r[1]), (r[2], r[3])].into_iter().filter(|(_, to)| lit(*to)).max_by(|x, y| luma(x.0).total_cmp(&luma(y.0))).map_or([1.0; 3], |(c, _)| [c[0], c[1], c[2]])
}

pub(super) struct RtRays {
	ubuf: wgpu::Buffer,
	march: wgpu::ComputePipeline,
	blur: wgpu::ComputePipeline,
	/// The bind groups for post.rs's targets of this generation.
	binds: Option<(u64, wgpu::BindGroup, wgpu::BindGroup)>,
}

impl RtRays {
	pub(super) fn new(device: &wgpu::Device) -> RtRays {
		let pipeline = |label, src: String, entry| {
			let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(crate::devtune::wgsl(label, src.into())) });
			device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(label), layout: None, module: &module, entry_point: Some(entry), compilation_options: Default::default(), cache: None })
		};
		let ubuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt rays"), size: std::mem::size_of::<RaysU>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
		eprintln!("rt rays: on (ray-traced sun rays)");
		RtRays { ubuf, march: pipeline("rt rays march", march_shader(), "cs_march"), blur: pipeline("rt rays blur", blur_shader(), "cs_blur"), binds: None }
	}

	/// This frame's sun rays into post.rs's rays target, after `Post::encode` (which then left its own ray passes out) and before
	/// the composite. `rt` is the level's structure (built), `cam` / `aspect` the level pass's camera, `f` the frame's key light,
	/// `col` its colour, `level` the Graphics Sun rays level.
	#[allow(clippy::too_many_arguments)]
	pub(super) fn run(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, rt: &super::rtshadow::RtShadow, post: &super::post::Post, cam: &Camera, aspect: f32, f: &super::sunshadow::SunFrame, col: [f32; 3], level: u8) {
		let Some((lin, rays, generation)) = post.rays_io() else { return };
		let _s = super::profile::span("rt_rays");
		let (w, h) = (lin.texture().width(), lin.texture().height());
		let fw = forward(cam);
		let r = normalise(cross(fw, cam.up));
		let up = cross(r, fw);
		let (tx, ty) = (cam.tan_half * aspect, cam.tan_half);
		let v = RaysU {
			eye: [cam.pos[0], cam.pos[1], cam.pos[2], REACH],
			f: [fw[0], fw[1], fw[2], 0.0],
			r: [r[0] * tx, r[1] * tx, r[2] * tx, 0.0],
			u: [up[0] * ty, up[1] * ty, up[2] * ty, steps(level) as f32],
			sun: [f.dir[0], f.dir[1], f.dir[2], (f.strength / super::sunshadow::STRENGTH).clamp(0.0, 1.0)],
			col: [col[0], col[1], col[2], 0.0],
			size: [w as f32, h as f32, 0.0, 0.0],
		};
		queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&v));
		if self.binds.as_ref().is_none_or(|b| b.0 != generation) {
			fn tv(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
				wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
			}
			let [a, b, c] = rt.trace_entries();
			let march = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("rt rays march"), layout: &self.march.get_bind_group_layout(0), entries: &[tv(0, lin), wgpu::BindGroupEntry { binding: 1, resource: self.ubuf.as_entire_binding() }, tv(2, &rays[0]), a, b, c] });
			let blur = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("rt rays blur"), layout: &self.blur.get_bind_group_layout(0), entries: &[tv(0, lin), wgpu::BindGroupEntry { binding: 1, resource: self.ubuf.as_entire_binding() }, tv(2, &rays[0]), tv(3, &rays[1])] });
			self.binds = Some((generation, march, blur));
		}
		let Some((_, march, blur)) = &self.binds else { return };
		let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("rt rays"), timestamp_writes: None });
		pass.set_pipeline(&self.march);
		pass.set_bind_group(0, march, &[]);
		pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
		pass.set_pipeline(&self.blur);
		pass.set_bind_group(0, blur, &[]);
		pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn ray_traced_sun_ray_shaders_validate() {
		for s in [super::march_shader(), super::blur_shader()] {
			let m = naga::front::wgsl::parse_str(&s).expect("rt rays wgsl parse");
			naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).expect("rt rays wgsl validate");
		}
	}
}
