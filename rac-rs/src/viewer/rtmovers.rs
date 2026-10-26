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
}

impl RtMovers {
	/// Builds this frame's structure (submitted on its own, ahead of the frame's) from the scene's moving casters around
	/// `eye`; None when nothing moves near enough to cast. The previous frame's structure stays alive until this returns.
	pub(super) fn build(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene, eye: [f32; 3], which: Which) -> Option<&wgpu::Blas> {
		let _s = super::profile::span("rt_movers");
		let t = std::time::Instant::now();
		let solid = |d: &Draw| !scene.clear_tex.get(d.texture).copied().unwrap_or(false);
		// The props slots that cast, posed into `scratch` at their vertex ranges (the ranges are uploaded one by one).
		let mut runs: Vec<(u32, u32)> = Vec::new();
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
					if scene.props.draws.get(d as usize).is_some_and(|d| solid(d)) {
						runs.push((first, count));
					}
				}
				if runs.len() > before {
					slots.push(i);
				}
			}
		}
		let props_idx: u32 = runs.iter().map(|r| r.1).sum();
		// The world-space batches: the player, the clipped props, the debris (their solid draws).
		let mut whole: Vec<(&Batch, Vec<(u32, u32)>)> = Vec::new();
		let add = |b: &Batch| b.draws.iter().filter(|d| solid(d)).map(|d| (d.start, d.count)).collect::<Vec<_>>();
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
			self.ibuf = Some((device.create_buffer(&wgpu::BufferDescriptor { label: Some("rt movers indices"), size: (cap * 4) as u64, usage: wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }), cap));
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
		let mut at = 0u32;
		let copy = |enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, runs: &[(u32, u32)], at: &mut u32| {
			for &(s, n) in runs {
				enc.copy_buffer_to_buffer(src, s as u64 * 4, ibuf, *at as u64 * 4, Some(n as u64 * 4));
				*at += n;
			}
		};
		if let Some((pbuf, cap)) = posed.as_ref().filter(|_| props_idx > 0) {
			copy(&mut enc, &scene.props.ibuf, &runs, &mut at);
			geos.push(Geo { vbuf: pbuf, stride: 12, verts: (*cap).min(nverts) as u32, first: 0, count: props_idx });
		}
		for (b, r) in &whole {
			let first = at;
			copy(&mut enc, &b.ibuf, r, &mut at);
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
		queue.submit([enc.finish()]);
		self.blas = Some(blas);
		self.builds += 1;
		if self.builds % LOG_EVERY == 1 {
			eprintln!("rt movers: {} prop slots ({} triangles), {} other triangles, {:.2} ms", slots.len(), props_idx / 3, (total - props_idx) / 3, t.elapsed().as_secs_f64() * 1e3);
		}
		self.blas.as_ref()
	}
}
