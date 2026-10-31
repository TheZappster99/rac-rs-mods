//! [PC] Ray-traced shadows of the things that move (rtshadow.rs phase 4; no retail counterpart): the frame's mobies in a
//! bottom-level structure of their own, rebuilt every frame and placed in the top level next to the static world.
//!
//! - The props batch is posed in its vertex shader (gpuskin.rs `vs_props`), so its vertex buffer holds model space. The
//!   casters among its slots (shown, a prop or scene actor, not the purchase prompt, not faded below alpha 0x60: the same
//!   ones sunshadow.rs `vs_sh_props` lets cast) within `NEAR` of the eye are posed here on the CPU with gpuskin.rs
//!   `emulate_pos` into `posed` (by props vertex index, so the batch's own indices hold), and their solid index runs
//!   (not the see-through shells, `Scene::clear_tex`) are copied on the GPU into one compact index buffer.
//! - The player (`marker`), the clipped props (`clip_batch`) and the debris are written in world space every frame: their
//!   own vertex buffers go in as they are, with their solid draws copied into the same index buffer.
//! - Everything is opaque (no texel test) and built for speed (`PREFER_FAST_BUILD`). The shadow mask reads last frame's, so
//!   a moving shadow lags one frame behind its caster.
//! - For the ray-traced reflections at Balanced and up (rtrefl.rs `rt_mover_colour`): each triangle's texture slot
//!   (`tex_table`), so a reflected mover the picture does not show gets its texture's mean colour. At Ultra each triangle's
//!   three uvs go in the same table too (`UV_WGSL`, filled on the GPU from the movers' own vertex buffers, as the indices
//!   are only there), so the mover is textured: the water's shading pass has no room for more storage buffers.

use super::*;

/// How far from the eye a prop slot casts (world units, plus the slot's own radius): beyond it its shadow is left out.
const NEAR: f32 = 80.0;
/// Builds between two summaries in the run log (about 5 s at 60 fps).
const LOG_EVERY: u32 = 300;

/// Which groups of moving casters cast (RAC_RT_TUNE `movers=`, rtshadow.rs `Tune`): the props slots, the player, the clipped
/// props, the debris.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Which {
	pub props: bool,
	pub player: bool,
	pub clip: bool,
	pub debris: bool,
}

impl Which {
	pub(super) const ALL: Which = Which { props: true, player: true, clip: true, debris: true };

	/// `1` all, `0` none, else the named groups (`props,player,clip,debris`).
	pub(super) fn parse(v: &str) -> Which {
		match v {
			"1" => Which::ALL,
			_ => {
				let has = |k| v.split(',').any(|x| x == k);
				Which { props: has("props"), player: has("player"), clip: has("clip"), debris: has("debris") }
			}
		}
	}
}

/// One geometry of the movers' structure: a vertex buffer and its run in the compact index buffer.
struct Geo<'a> {
	vbuf: &'a wgpu::Buffer,
	stride: u64,
	verts: u32,
	first: u32,
	count: u32,
}

/// The movers' structure and the buffers it is built from, grown as needed.
#[derive(Default)]
pub(super) struct RtMovers {
	/// Posed positions of the props batch, by its vertex index (12 bytes each), and its room in vertices.
	posed: Option<(wgpu::Buffer, usize)>,
	/// The compact index buffer and its room in indices.
	ibuf: Option<(wgpu::Buffer, usize)>,
	blas: Option<wgpu::Blas>,
	/// Scratch: the posed positions of this frame, uploaded per slot range.
	scratch: Vec<[f32; 3]>,
	/// Builds so far (a summary goes to the run log every `LOG_EVERY`).
	builds: u32,
	/// Each triangle's texture slot (`tex_table`) and its room in u32s, and a number that changes when the buffer is remade.
	tex: Option<(wgpu::Buffer, usize)>,
	tex_gen: u64,
	/// Scratch: this frame's texture table.
	tex_scratch: Vec<u32>,
	/// The uv pass (`UV_WGSL`) and its layout, made on first use at Ultra.
	uv: Option<(wgpu::ComputePipeline, wgpu::BindGroupLayout)>,
}

/// The texture table's head (`tex_table`): the 4 geometries' starts, where the uvs start, the triangles, the geometries, 0.
const HEAD: usize = 8;

const UV_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read_write> tbl: array<u32>;
@group(0) @binding(1) var<storage, read> idx: array<u32>;
@group(0) @binding(2) var<storage, read> v0: array<f32>;
@group(0) @binding(3) var<storage, read> v1: array<f32>;
@group(0) @binding(4) var<storage, read> v2: array<f32>;
@group(0) @binding(5) var<storage, read> v3: array<f32>;

fn vf(g: u32, k: u32) -> f32 {
	switch g {
		case 0u: { return v0[k]; }
		case 1u: { return v1[k]; }
		case 2u: { return v2[k]; }
		default: { return v3[k]; }
	}
}

// [PC] Ray-traced reflections at Ultra (rtmovers.rs `tex_table`): triangle `t` of the movers' structure (compact indices 3t..3t+2,
// in the table's order) gets its three uvs, read from its geometry's `Vertex` buffer (uv at floats 3..4 of 16), at tbl[4] + 6t.
@compute @workgroup_size(64) fn cs_uv(@builtin(global_invocation_id) id: vec3<u32>) {
	let t = id.y * (65535u * 64u) + id.x;
	if (t >= tbl[5]) { return; }
	var g = 0u;
	for (var k = 1u; k < min(tbl[6], 4u); k++) {
		if (t + 8u >= tbl[k]) { g = k; }
	}
	let base = tbl[4] + t * 6u;
	for (var j = 0u; j < 3u; j++) {
		let b = idx[t * 3u + j] * 16u;
		tbl[base + j * 2u] = bitcast<u32>(vf(g, b + 3u));
		tbl[base + j * 2u + 1u] = bitcast<u32>(vf(g, b + 4u));
	}
}
"#;

impl RtMovers {
	/// Builds this frame's structure (submitted on its own, ahead of the frame's) from the scene's moving casters around
	/// `eye`; None when nothing moves near enough to cast. The previous frame's structure stays alive until this returns.
	pub(super) fn build(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene, eye: [f32; 3], which: Which) -> Option<&wgpu::Blas> {
		let _s = super::profile::span("rt_movers");
		let t = std::time::Instant::now();
		let solid = |d: &Draw| !scene.clear_tex.get(d.texture).copied().unwrap_or(false);
		// The props slots that cast, posed into `scratch` at their vertex ranges (the ranges are uploaded one by one).
		let mut runs: Vec<(u32, u32, u32)> = Vec::new();
		let mut slots: Vec<usize> = Vec::new();
		if scene.props_shown && which.props {
			for (i, s) in scene.props_inst.iter().enumerate() {
				let kind = s.k[0] & 0xff;
				let alpha = 128.0 - ((s.a[1] - (s.a[1] + 0.5).floor()).abs() * 512.0).round();
				if !scene.props_vis.get(i).copied().unwrap_or(false) || !(kind == gpuskin::KIND_PROP || kind == gpuskin::KIND_ACTOR) || s.k[0] & gpuskin::FLAG_PROMPT != 0 || alpha < 96.0 {
					continue;
				}
				let c = [s.m[12] - eye[0], s.m[13] - eye[1], s.m[14] - eye[2]];
				let reach = NEAR + scene.props_rad.get(i).copied().unwrap_or(0.0) * s.a[0].abs().max(1.0);
				if c[0] * c[0] + c[1] * c[1] + c[2] * c[2] > reach * reach {
					continue;
				}
				let before = runs.len();
				for &(d, first, count) in scene.slot_runs.get(i).map_or(&[][..], |r| &r[..]) {
					if let Some(d) = scene.props.draws.get(d as usize).filter(|d| solid(d)) {
						runs.push((first, count, d.texture as u32));
					}
				}
				if runs.len() > before {
					slots.push(i);
				}
			}
		}
		let props_idx: u32 = runs.iter().map(|r| r.1).sum();
		// The world-space batches: the player, the clipped props, the debris (their solid draws).
		let mut whole: Vec<(&Batch, Vec<(u32, u32, u32)>)> = Vec::new();
		let add = |b: &Batch| b.draws.iter().filter(|d| solid(d)).map(|d| (d.start, d.count, d.texture as u32)).collect::<Vec<_>>();
		if scene.marker_shown && which.player {
			whole.push((&scene.marker, add(&scene.marker)));
		}
		if scene.props_shown && which.clip {
			whole.push((&scene.clip_batch, add(&scene.clip_batch)));
		}
		if scene.props_shown && which.debris {
			if let Some(b) = &scene.debris {
				whole.push((b, add(b)));
			}
		}
		whole.retain(|(_, r)| !r.is_empty());
		let total = props_idx + whole.iter().flat_map(|(_, r)| r.iter()).map(|r| r.1).sum::<u32>();
		if total < 3 {
			self.blas = None;
			return None;
		}
		// Room: the posed props positions (the whole batch, by vertex index) and the compact indices.
		let nverts = scene.props_model.len();
		if props_idx > 0 && self.posed.as_ref().is_none_or(|p| p.1 < nverts) {
			let cap = nverts.next_power_of_two();
			self.posed = Some((device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt movers posed"), size: (cap * 12) as u64, usage: wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }), cap));
		}
		if self.ibuf.as_ref().is_none_or(|b| b.1 < total as usize) {
			let cap = (total as usize).next_power_of_two().max(1 << 16);
			self.ibuf = Some((device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt movers indices"), size: (cap * 4) as u64, usage: wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::STORAGE, mapped_at_creation: false }), cap));
		}
		let (Some((ibuf, _)), posed) = (&self.ibuf, &self.posed) else { return None };
		// Pose the casting slots (the shader's own operations, gpuskin.rs `emulate_pos`) and upload each slot's range.
		if let Some((pbuf, _)) = posed.as_ref().filter(|_| props_idx > 0) {
			for &i in &slots {
				let Some(r) = scene.props_ranges.get(i).filter(|r| r.end <= nverts) else { continue };
				let s = &scene.props_inst[i];
				self.scratch.clear();
				self.scratch.extend(scene.props_model[r.clone()].iter().enumerate().map(|(j, v)| gpuskin::emulate_pos(s, &scene.props_pose, v.pos, j)));
				queue.write_buffer(pbuf, (r.start * 12) as u64, bytemuck::cast_slice(&self.scratch));
			}
		}
		let mut enc = device.create_command_encoder(&Default::default());
		let mut geos: Vec<Geo> = Vec::new();
		// Each geometry's buffer with its uvs (`UV_WGSL`): the props batch's own for the posed props.
		let mut uv_src: Vec<&wgpu::Buffer> = Vec::new();
		let mut at = 0u32;
		let copy = |enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, runs: &[(u32, u32, u32)], at: &mut u32| {
			for &(s, n, _) in runs {
				enc.copy_buffer_to_buffer(src, s as u64 * 4, ibuf, *at as u64 * 4, Some(n as u64 * 4));
				*at += n;
			}
		};
		if let Some((pbuf, cap)) = posed.as_ref().filter(|_| props_idx > 0) {
			copy(&mut enc, &scene.props.ibuf, &runs, &mut at);
			geos.push(Geo { vbuf: pbuf, stride: 12, verts: (*cap).min(nverts) as u32, first: 0, count: props_idx });
			uv_src.push(&scene.props.vbuf);
		}
		for (b, r) in &whole {
			let first = at;
			copy(&mut enc, &b.ibuf, r, &mut at);
			uv_src.push(&b.vbuf);
			geos.push(Geo { vbuf: &b.vbuf, stride: std::mem::size_of::<Vertex>() as u64, verts: (b.vbuf.size() / std::mem::size_of::<Vertex>() as u64) as u32, first, count: at - first });
		}
		let sizes: Vec<wgpu::BlasTriangleGeometrySizeDescriptor> = geos
			.iter()
			.map(|g| wgpu::BlasTriangleGeometrySizeDescriptor { vertex_format: wgpu::VertexFormat::Float32x3, vertex_count: g.verts, index_format: Some(wgpu::IndexFormat::Uint32), index_count: Some(g.count), flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE })
			.collect();
		let blas = device.create_blas(
			&wgpu::CreateBlasDescriptor { label: Some("rt movers"), flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD, update_mode: wgpu::AccelerationStructureUpdateMode::Build },
			wgpu::BlasGeometrySizeDescriptors::Triangles { descriptors: sizes.clone() },
		);
		let geometry = geos
			.iter()
			.zip(&sizes)
			.map(|(g, size)| wgpu::BlasTriangleGeometry { size, vertex_buffer: g.vbuf, first_vertex: 0, vertex_stride: g.stride, index_buffer: Some(ibuf), first_index: Some(g.first), transform_buffer: None, transform_buffer_offset: None })
			.collect();
		enc.build_acceleration_structures(std::iter::once(&wgpu::BlasBuildEntry { blas: &blas, geometry: wgpu::BlasGeometries::TriangleGeometries(geometry) }), std::iter::empty());
		if super::rtrefl::movers_shaded() {
			// The geometries in the structure's order: the posed props, then the world-space batches. (Written before the submit,
			// so the uv pass in `enc` fills the table after it.)
			let order: Vec<&[(u32, u32, u32)]> = std::iter::once(&runs[..]).filter(|_| props_idx > 0).chain(whole.iter().map(|(_, r)| &r[..])).collect();
			let textured = super::rtrefl::movers_textured();
			tex_table(&mut self.tex, &mut self.tex_gen, &mut self.tex_scratch, device, queue, &order, textured);
			if let (true, Some((tbl, _))) = (textured, &self.tex) {
				let (pipeline, layout) = self.uv.get_or_insert_with(|| uv_pass(device));
				let src: [&wgpu::Buffer; 4] = std::array::from_fn(|k| uv_src.get(k).copied().unwrap_or(ibuf));
				let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: tbl.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: ibuf.as_entire_binding() }];
				entries.extend(src.iter().enumerate().map(|(k, b)| wgpu::BindGroupEntry { binding: 2 + k as u32, resource: b.as_entire_binding() }));
				let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("rt movers uvs"), layout, entries: &entries });
				let groups = (total / 3).div_ceil(64);
				let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("rt movers uvs"), timestamp_writes: None });
				pass.set_pipeline(pipeline);
				pass.set_bind_group(0, &bind, &[]);
				pass.dispatch_workgroups(groups.min(65535), groups.div_ceil(65535), 1);
			}
		}
		queue.submit([enc.finish()]);
		self.blas = Some(blas);
		self.builds += 1;
		if self.builds % LOG_EVERY == 1 {
			eprintln!("rt movers: {} prop slots ({} triangles), {} other triangles, {:.2} ms", slots.len(), props_idx / 3, (total - props_idx) / 3, t.elapsed().as_secs_f64() * 1e3);
		}
		self.blas.as_ref()
	}

	/// The reflections' texture table (`tex_table`) and a number that changes when its buffer is remade; None before the first.
	pub(super) fn tex(&self) -> Option<(&wgpu::Buffer, u64)> {
		self.tex.as_ref().map(|b| (&b.0, self.tex_gen))
	}
}

/// The reflections' texture table (`tex`, its room in u32s, remade larger as needed, `generation` bumped then) for this frame's
/// geometries (`geos`: each one's runs, in the compact index buffer's order): the head (`HEAD`: where each geometry's triangles
/// start in the table, where the uvs start, the triangles, the geometries), then each triangle's texture slot, then (`uvs`, at
/// Ultra) room for each triangle's 3 uvs, which the uv pass (`UV_WGSL`) fills on the GPU after this upload.
fn tex_table(tex: &mut Option<(wgpu::Buffer, usize)>, generation: &mut u64, t: &mut Vec<u32>, device: &wgpu::Device, queue: &wgpu::Queue, geos: &[&[(u32, u32, u32)]], uvs: bool) {
	t.clear();
	t.resize(HEAD, 0);
	for (k, runs) in geos.iter().take(4).enumerate() {
		t[k] = t.len() as u32;
		for &(_, n, tex) in runs.iter() {
			t.extend(std::iter::repeat_n(tex, (n / 3) as usize));
		}
	}
	let tris = t.len() - HEAD;
	t[4] = t.len() as u32;
	t[5] = tris as u32;
	t[6] = geos.len().min(4) as u32;
	let room = t.len() + if uvs { tris * 6 } else { 0 };
	if tex.as_ref().is_none_or(|b| b.1 < room) {
		let cap = room.next_power_of_two().max(1 << 14);
		*tex = Some((device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt movers textures"), size: (cap * 4) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }), cap));
		*generation += 1;
	}
	if let Some((b, _)) = tex {
		queue.write_buffer(b, 0, bytemuck::cast_slice(t));
	}
}

/// The uv pass (`UV_WGSL`) and its layout: the table read-write, the compact indices and the 4 geometries' vertex buffers read.
fn uv_pass(device: &wgpu::Device) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
	let entry = |binding, read_only| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }, count: None };
	let entries: Vec<wgpu::BindGroupLayoutEntry> = (0..6).map(|b| entry(b, b != 0)).collect();
	let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("rt movers uvs"), entries: &entries });
	let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("rt movers uvs"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
	let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rt movers uvs"), source: wgpu::ShaderSource::Wgsl(crate::devtune::wgsl("rt movers uvs", UV_WGSL.into())) });
	let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("rt movers uvs"), layout: Some(&pl), module: &module, entry_point: Some("cs_uv"), compilation_options: Default::default(), cache: None });
	eprintln!("rt reflections: moving things textured off screen (their uvs copied each frame)");
	(pipeline, layout)
}

#[cfg(test)]
mod tests {
	#[test]
	fn rt_movers_uv_shader_validates() {
		let m = naga::front::wgsl::parse_str(super::UV_WGSL).expect("rt movers uvs wgsl parse");
		naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&m).expect("rt movers uvs wgsl validate");
	}
}
