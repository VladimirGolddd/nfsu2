//! The renderer: the page's WebGPU renderer (web/src/gpu/index.ts and
//! encoder.ts) on wgpu, which runs it on Direct3D 12 or Vulkan.  It reads the
//! game's records (runtime/web/gpu_web.c, layout runtime/web/gpu_proto.h)
//! straight from its memory, runs the same WGSL (the runtime's translator and
//! wgsl.rs), and composites the game's back buffer into the window.
//!
//! What it leaves out of the page's: frame dumps (autotest), pipeline warm-up
//! across sessions, GPU timing and the per-minute statistics.  Semantics kept
//! from the Mac port are listed at the top of encoder.ts and hold here too.

use crate::dxt;
use crate::ffi::{self, Shm};
use crate::wgsl;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use wgpu::util::DeviceExt as _;

// ---- runtime/web/gpu_proto.h (byte offsets) ----
const GW_MAGIC: u32 = 0x5550_4757;
const GW_VERSION: u32 = 5;
const DRAW_BYTES: usize = 156;
const FRAME_BYTES: usize = 156;
const CTL_BYTES: usize = 96 + 2 * FRAME_BYTES;
const LAYOUT: u32 = ((DRAW_BYTES as u32) | (FRAME_BYTES as u32) << 8 | ((CTL_BYTES as u32) & 0xFFF) << 16) ^ (GW_VERSION << 28);
mod c {
    pub const MAGIC: usize = 0; pub const LAYOUT: usize = 8; pub const STATE: usize = 16; pub const BELL: usize = 20;
    pub const ATTACHED: usize = 24; pub const RQ_HEAD: usize = 28; pub const RQ_TAIL: usize = 32; pub const RQ_PTR: usize = 36;
    pub const RQ_CAP: usize = 40; pub const FRAMES_DONE: usize = 44; pub const VIEW_W: usize = 48; pub const VIEW_H: usize = 52;
    pub const DPR: usize = 56; pub const FEATURES: usize = 60; pub const ST_DRAWS: usize = 64; pub const ST_PASSES: usize = 68;
    pub const ST_ERRORS: usize = 76; pub const ST_PIPES: usize = 84; pub const RB_WANT: usize = 88; pub const RB_DONE: usize = 92;
    pub const FRAME: usize = 96;
}
mod f {
    pub const DN: usize = 4; pub const VN: usize = 8; pub const RVN: usize = 12; pub const KN: usize = 16;
    pub const DRAWS: usize = 24; pub const TV: usize = 28; pub const RAW: usize = 32; pub const K: usize = 36; pub const PSC: usize = 40;
    pub const RQ_END: usize = 44; pub const BB_W: usize = 48; pub const BB_H: usize = 52; pub const CLEAR: usize = 56;
    pub const RB_ID: usize = 136; pub const RB_TEX: usize = 140; pub const RB_DST: usize = 144; pub const RB_PITCH: usize = 148;
    pub const RB_ROWS: usize = 152;
}
mod d {   // GwDraw word indices
    pub const FIRST: usize = 0; pub const COUNT: usize = 1; pub const TEX: usize = 2; pub const TARGET: usize = 3;
    pub const KIND: usize = 4; pub const BLEND: usize = 5; pub const AREF: usize = 6; pub const DEPTH: usize = 7;
    pub const TEX1: usize = 8; pub const TEX2: usize = 9; pub const TEX3: usize = 10; pub const PS: usize = 11;
    pub const PSCI: usize = 12; pub const VSID: usize = 13; pub const KOFF: usize = 15; pub const PROJ: usize = 16;
    pub const SMP: usize = 20; pub const BLIT_SRC: usize = 24; pub const BCOL: usize = 25; pub const CULL: usize = 26;
    pub const FOGV: usize = 27; pub const CLR_ARGB: usize = 31; pub const CLR_FLAGS: usize = 32; pub const CLR_Z: usize = 33;
    pub const IX_ISZ: usize = 35; pub const IX_IB: usize = 36; pub const IX_BASE: usize = 37; pub const CLIP: usize = 38;
}
const KIND_CLEAR: u32 = 1;
const KIND_BLIT: u32 = 2;
const ATTACHED: i32 = 1;
const DETACHED: i32 = 2;
const RB_FLAG: u32 = 0x8000_0000;
const FEAT_BC: u32 = 1;
const CMD_HDR: usize = 32;
const TF_RT: u32 = 1;
const TF_CUBE: u32 = 2;
const TVERT_BYTES: u64 = 112;
const K_HL: u64 = 256;
const K_TONE: usize = 1552;
const TONE_MAGIC: u32 = 0x454E_4F54;
// docs/web/gpu-abi.md / shader-abi.ts
const VSPAR_SIZE: u32 = 4112;
const KWIN_SIZE: u64 = 65536;
const PSPAR_SIZE: u64 = 560;
const HLPAR_SIZE: u64 = 656;
const PSCUBE: i32 = 256;
const PSLOT: u64 = 768;
// D3DFMT
const FMT_A8R8G8B8: u32 = 21;
const FMT_X8R8G8B8: u32 = 22;
const FMT_A8: u32 = 28;
const FMT_P8: u32 = 41;
const FMT_DXT3: u32 = 0x3354_5844;

use wgpu::{BlendFactor as BF, BlendOperation as BO, CompareFunction as CF, AddressMode as AM};
const BLEND: [BF; 16] = [BF::One, BF::Zero, BF::One, BF::Src, BF::OneMinusSrc, BF::SrcAlpha, BF::OneMinusSrcAlpha, BF::DstAlpha,
    BF::OneMinusDstAlpha, BF::Dst, BF::OneMinusDst, BF::SrcAlphaSaturated, BF::SrcAlpha, BF::OneMinusSrcAlpha, BF::Constant, BF::OneMinusConstant];
const BLENDOP: [BO; 6] = [BO::Add, BO::Add, BO::Subtract, BO::ReverseSubtract, BO::Min, BO::Max];
const CMP: [CF; 9] = [CF::Always, CF::Never, CF::Less, CF::Equal, CF::LessEqual, CF::Greater, CF::NotEqual, CF::GreaterEqual, CF::Always];
const ADDR: [AM; 6] = [AM::Repeat, AM::Repeat, AM::MirrorRepeat, AM::ClampToEdge, AM::ClampToEdge, AM::MirrorRepeat];

/// What the window shares with the renderer: its size, and where the picture went (for the mouse).
pub struct View {
    pub w: AtomicU32, pub h: AtomicU32, pub resized: AtomicBool,
    /// the picture's rectangle in the window, x y w h
    pub img: Mutex<[f32; 4]>,
    pub quit: AtomicBool,
}

struct Tex {
    serial: u32, tex: wgpu::Texture, fmt: u32, w: u32, h: u32, levels: u32, cube: bool, rt: bool,
    view: wgpu::TextureView, rt_views: Vec<wgpu::TextureView>, expand_a8: bool, bc: bool, unpack: bool,
    face0: Option<wgpu::TextureView>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Src { TVert = 0, Raw = 1, Pool = 2 }

pub struct Renderer {
    dev: wgpu::Device, queue: wgpu::Queue, surface: wgpu::Surface<'static>, sfmt: wgpu::TextureFormat,
    scfg: wgpu::SurfaceConfiguration, view: Arc<View>,
    ctl: Shm, mem: Shm, bc: bool, errors: Arc<AtomicU32>,
    tex: Vec<Option<Tex>>, serial: u32, white: Tex, white_cube: Tex,
    ztex: Vec<Option<wgpu::Texture>>, zview: Vec<Option<wgpu::TextureView>>, zpend: [bool; 64], zpend_v: [f32; 64],
    scratch_z: HashMap<(u32, u32), wgpu::TextureView>,
    pool: wgpu::Buffer, vs_mods: HashMap<i32, wgpu::ShaderModule>, ps_mods: HashMap<i32, wgpu::ShaderModule>,
    samplers: HashMap<u32, wgpu::Sampler>, pipes: HashMap<u64, Option<wgpu::RenderPipeline>>,
    bg1: HashMap<(bool, [u32; 4], [u32; 4]), wgpu::BindGroup>,
    tv_buf: wgpu::Buffer, raw_buf: wgpu::Buffer, k_buf: wgpu::Buffer, p_buf: wgpu::Buffer,
    buf_gen: u32, bg0: [Option<wgpu::BindGroup>; 3], bg0_gen: u32, p_stage: Vec<f32>,
    bgl0: wgpu::BindGroupLayout, bgl1: wgpu::BindGroupLayout, bgl1c: wgpu::BindGroupLayout,
    pl2d: wgpu::PipelineLayout, pl_cube: wgpu::PipelineLayout,
    tvert_mod: wgpu::ShaderModule, ff_mod: wgpu::ShaderModule, ff_cube_mod: wgpu::ShaderModule,
    copy_mod: wgpu::ShaderModule, copy_bgl: wgpu::BindGroupLayout, copy_pl: wgpu::PipelineLayout,
    copy_pipes: HashMap<(wgpu::TextureFormat, bool), wgpu::RenderPipeline>,
    tone_mod: wgpu::ShaderModule, tone_bgl: wgpu::BindGroupLayout, tone_pl: wgpu::PipelineLayout, tone_buf: wgpu::Buffer,
    tone_pipes: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    linear: wgpu::Sampler, nearest: wgpu::Sampler,
    bb: Option<wgpu::Texture>, bb_view: Option<wgpu::TextureView>,
    destroy_later: Vec<wgpu::Texture>, unpack_buf: Vec<u8>,
    frames: u32, total_draws: u32, total_passes: u32, pipes_made: u32,
    missing_target: u32, no_vs: u32, said: std::collections::HashSet<String>,
}

fn log(s: &str) { crate::ffi::log(s); }

impl Renderer {
    /// Opens the GPU for the window and attaches to the game's control block (waiting for the game to register it).
    /// The GPU instance and the window's surface: made on the window's thread (winit gives the window's handle
    /// only there), then handed to the renderer's.
    pub fn surface(window: Arc<winit::window::Window>) -> Result<(wgpu::Instance, wgpu::Surface<'static>), String> {
        let mut idesc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        if let Ok(b) = std::env::var("NFSU2_GPU_BACKEND") {
            idesc.backends = match b.to_ascii_lowercase().as_str() {
                "dx12" | "d3d12" => wgpu::Backends::DX12, "vulkan" | "vk" => wgpu::Backends::VULKAN, _ => idesc.backends };
        }
        let instance = wgpu::Instance::new(idesc);
        let surface = instance.create_surface(window).map_err(|e| format!("no surface for the window: {e}"))?;
        Ok((instance, surface))
    }

    pub fn new(instance: wgpu::Instance, surface: wgpu::Surface<'static>, size: (u32, u32), view: Arc<View>) -> Result<Renderer, String> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), force_fallback_adapter: false, ..Default::default()
        })).map_err(|e| format!("no GPU adapter: {e}"))?;
        let info = adapter.get_info();
        let bc = adapter.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC) && std::env::var("NFSU2_NO_BC").is_err();
        let al = adapter.limits();
        let mut limits = wgpu::Limits::default();
        limits.max_buffer_size = al.max_buffer_size;
        limits.max_storage_buffer_binding_size = al.max_storage_buffer_binding_size;
        let (dev, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nfsu2"),
            required_features: if bc { wgpu::Features::TEXTURE_COMPRESSION_BC } else { wgpu::Features::empty() },
            required_limits: limits,
            ..Default::default()
        })).map_err(|e| format!("no GPU device: {e}"))?;
        let errors = Arc::new(AtomicU32::new(0));
        { let e = errors.clone();
          dev.on_uncaptured_error(Arc::new(move |err: wgpu::Error| {
              if e.fetch_add(1, Ordering::Relaxed) < 20 { log(&format!("[gpu] error: {err}")); } })); }
        let caps = surface.get_capabilities(&adapter);
        let sfmt = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm].into_iter()
            .find(|f| caps.formats.contains(f)).unwrap_or(caps.formats[0]);
        let vsync = std::env::var("NFSU2_VSYNC").map(|v| v != "0" && v != "off").unwrap_or(true);
        let present_mode = if vsync { wgpu::PresentMode::Fifo }
            else if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) { wgpu::PresentMode::Mailbox }
            else if caps.present_modes.contains(&wgpu::PresentMode::Immediate) { wgpu::PresentMode::Immediate }
            else { wgpu::PresentMode::Fifo };
        let scfg = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, format: sfmt, width: size.0.max(1), height: size.1.max(1),
            present_mode, alpha_mode: wgpu::CompositeAlphaMode::Auto, view_formats: vec![], desired_maximum_frame_latency: 2, color_space: Default::default(),
        };
        surface.configure(&dev, &scfg);
        log(&format!("[gpu] {} ({:?}, {:?}), surface {:?} {:?}{}", info.name, info.backend, info.device_type, sfmt, present_mode,
                     if bc { ", BC textures" } else { ", no BC: DXT unpacked on upload" }));

        // the game's control block
        let (ctl, _) = loop {
            if view.quit.load(Ordering::Relaxed) { return Err("quit before the game registered its graphics".into()); }
            if let Some(r) = ffi::lookup(ffi::KIND_GPU, 500) { break r; }
        };
        
        let ctl = Shm(ctl);
        if ctl.u32(c::MAGIC) != GW_MAGIC || ctl.u32(c::LAYOUT) != LAYOUT {
            return Err(format!("GPU control block mismatch: magic {:x} layout {:x} (host {LAYOUT:x})", ctl.u32(c::MAGIC), ctl.u32(c::LAYOUT)));
        }

        let s = |label, src: String| dev.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) });
        let (bgl0, bgl1, bgl1c) = abi_layouts(&dev);
        let pl = |l: &wgpu::BindGroupLayout| dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[Some(&bgl0), Some(l)], immediate_size: 0 });
        let pl2d = pl(&bgl1);
        let pl_cube = pl(&bgl1c);
        let fe = wgpu::ShaderStages::FRAGMENT;
        let copy_bgl = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("copy"), entries: &[
            tex_entry(0, fe, wgpu::TextureViewDimension::D2), smp_entry(1, fe)] });
        let copy_pl = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&copy_bgl)], immediate_size: 0 });
        let tone_bgl = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("tone"), entries: &[
            tex_entry(0, fe, wgpu::TextureViewDimension::D2), smp_entry(1, fe),
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: fe, ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }] });
        let tone_pl = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&tone_bgl)], immediate_size: 0 });
        let buf = |label, size, usage| dev.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        let su = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let uu = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        let white = make_white(&dev, &queue, false);
        let white_cube = make_white(&dev, &queue, true);
        let r = Renderer {
            tvert_mod: s("tvert", wgsl::tvert_vs()), ff_mod: s("ff", wgsl::ff_fs(false)), ff_cube_mod: s("ff-cube", wgsl::ff_fs(true)),
            copy_mod: s("copy", wgsl::COPY.to_string()), tone_mod: s("tone", wgsl::tone()),
            linear: dev.create_sampler(&wgpu::SamplerDescriptor { min_filter: wgpu::FilterMode::Linear, mag_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear, address_mode_u: AM::Repeat, address_mode_v: AM::Repeat, ..Default::default() }),
            nearest: dev.create_sampler(&wgpu::SamplerDescriptor::default()),
            tv_buf: buf("tvert", 1 << 20, su), raw_buf: buf("raw", 1 << 20, su), k_buf: buf("vsconst", 1 << 20, uu), p_buf: buf("pspar", 1 << 20, uu),
            pool: buf("pool", 1 << 16, su | wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_SRC),
            tone_buf: buf("tone", 16, uu),
            bgl0, bgl1, bgl1c, pl2d, pl_cube, copy_bgl, copy_pl, tone_bgl, tone_pl,
            dev, queue, surface, sfmt, scfg, view, ctl, mem: ffi::mem(), bc, errors,
            tex: Vec::new(), serial: 1, white, white_cube,
            ztex: (0..64).map(|_| None).collect(), zview: (0..64).map(|_| None).collect(), zpend: [false; 64], zpend_v: [1.0; 64],
            scratch_z: HashMap::new(), vs_mods: HashMap::new(), ps_mods: HashMap::new(), samplers: HashMap::new(), pipes: HashMap::new(),
            bg1: HashMap::new(), buf_gen: 0, bg0: [None, None, None], bg0_gen: u32::MAX, p_stage: Vec::new(),
            copy_pipes: HashMap::new(), tone_pipes: HashMap::new(), bb: None, bb_view: None,
            destroy_later: Vec::new(), unpack_buf: Vec::new(), frames: 0, total_draws: 0, total_passes: 0, pipes_made: 0,
            missing_target: 0, no_vs: 0, said: Default::default(),
        };
        r.publish_view();
        let won = r.ctl.cas(c::ATTACHED, 0, ATTACHED);
        if won != 0 {
            return Err(if won == DETACHED { "the game stopped waiting for a renderer".into() } else { "another renderer attached first".into() });
        }
        ffi::notify(r.ctl.atomic(c::ATTACHED));
        log("[gpu] renderer attached");
        Ok(r)
    }

    fn publish_view(&self) {
        let (w, h) = (self.view.w.load(Ordering::Relaxed).max(1), self.view.h.load(Ordering::Relaxed).max(1));
        self.ctl.set_u32(c::VIEW_W, w);
        self.ctl.set_u32(c::VIEW_H, h);
        self.ctl.set_u32(c::DPR, 1000);
        self.ctl.set_u32(c::FEATURES, FEAT_BC);   // DXT always goes up: as BC or unpacked here
    }

    /// The render loop (its own thread): the game's doorbell and handoffs until quit.
    pub fn run(mut self) {
        while !self.view.quit.load(Ordering::Relaxed) {
            if self.view.resized.swap(false, Ordering::Relaxed) {
                let (w, h) = (self.view.w.load(Ordering::Relaxed), self.view.h.load(Ordering::Relaxed));
                if w > 0 && h > 0 { self.scfg.width = w; self.scfg.height = h; self.surface.configure(&self.dev, &self.scfg); }
                self.publish_view();
            }
            let bell = self.ctl.load(c::BELL);
            self.pump();
            ffi::wait32(self.ctl.atomic(c::BELL), bell, 4.0);
        }
        self.ctl.store(c::ATTACHED, DETACHED);
        for w in [c::ATTACHED, c::STATE, c::RQ_TAIL, c::BELL] { ffi::notify(self.ctl.atomic(w)); }
    }

    fn pump(&mut self) {
        loop {
            let head = self.ctl.load(c::RQ_HEAD) as u32;
            let s = self.ctl.load(c::STATE);
            let (r, e) = (s & 3, (s >> 2) & 3);
            let take = r != 0 && e == 0;
            if take {
                if self.ctl.cas(c::STATE, s, s | (r << 2)) != s { continue; }
                ffi::notify(self.ctl.atomic(c::STATE));
            }
            let limit = if take { self.ctl.u32(c::FRAME + (r as usize - 1) * FRAME_BYTES + f::RQ_END) } else { head };
            self.drain(limit);
            if !take { break; }
            self.encode_and_present(r as usize - 1);
            self.ctl.store(c::STATE, 0);
            ffi::notify(self.ctl.atomic(c::STATE));
        }
    }

    // ------------------------------------------------------------ resource queue
    fn drain(&mut self, limit: u32) {
        let cap = self.ctl.u32(c::RQ_CAP);
        let base = self.ctl.u32(c::RQ_PTR);
        let mut tail = self.ctl.u32(c::RQ_TAIL);
        let mut n = 0u32;
        while (limit.wrapping_sub(tail) as i32) > 0 {
            let p = base.wrapping_add(tail & (cap - 1)) as usize;
            let m = self.mem;
            let (op, len) = (m.u32(p), m.u32(p + 4));
            if len < 16 || len > cap { log(&format!("[gpu] corrupt resource queue record op={op} len={len}")); tail = limit; break; }
            let a: [u32; 6] = std::array::from_fn(|i| m.u32(p + 8 + 4 * i));
            let data = p + CMD_HDR;
            match op {
                0 => {}
                1 => self.tex_create(a[0], a[1], a[2], a[3], a[4], a[5], false),
                2 => self.tex_upload(a[0], a[1] & 0xFFFF, a[1] >> 16, a[2], a[3], a[4], a[5], data),
                3 => self.tex_free(a[0]),
                4 => self.pool_grow(a[0] as u64),
                5 => if (a[0] as u64 + a[1] as u64) <= self.pool.size() && a[1] > 0 {
                    let n4 = (a[1] as usize) & !3;
                    if n4 > 0 { self.queue.write_buffer(&self.pool, a[0] as u64, m.bytes(data, n4)); }
                    if (a[1] as usize) & 3 != 0 {   // wgpu writes whole words: the tail padded from the pool's own copy is not kept, pad with the bytes there
                        let mut w = [0u8; 4]; let k = (a[1] as usize) & 3;
                        w[..k].copy_from_slice(m.bytes(data + n4, k));
                        if a[0] as u64 + n4 as u64 + 4 <= self.pool.size() { self.queue.write_buffer(&self.pool, a[0] as u64 + n4 as u64, &w); }
                    }
                },
                6 | 7 => {
                    let n = (a[1] as usize).saturating_sub(1);
                    let text = String::from_utf8_lossy(m.bytes(data, n)).into_owned();
                    let label = format!("{}_{}", if op == 6 { "vs" } else { "ps" }, a[0]);
                    let g = self.dev.push_error_scope(wgpu::ErrorFilter::Validation);
                    let md = self.dev.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(&label), source: wgpu::ShaderSource::Wgsl(text.into()) });
                    if let Some(e) = pollster::block_on(g.pop()) { log(&format!("[gpu] WGSL {label}: {e}")); }
                    if op == 6 { self.vs_mods.insert(a[0] as i32, md); } else { self.ps_mods.insert(a[0] as i32, md); }
                    self.pipes.retain(|_, p| p.is_some());   // a module that came late: its draws get a pipeline now
                }
                8 => if (a[0] as usize) < 64 { self.ztex[a[0] as usize] = None; self.zview[a[0] as usize] = None; },
                _ => log(&format!("[gpu] unknown resource op {op}")),
            }
            tail = tail.wrapping_add(len);
            n += 1;
            if n % 64 == 0 { self.ctl.store(c::RQ_TAIL, tail as i32); ffi::notify(self.ctl.atomic(c::RQ_TAIL)); }
        }
        self.ctl.store(c::RQ_TAIL, tail as i32);
        ffi::notify(self.ctl.atomic(c::RQ_TAIL));
    }

    #[allow(clippy::too_many_arguments)]
    fn tex_create(&mut self, id: u32, w: u32, h: u32, levels: u32, fmt: u32, flags: u32, force_unpack: bool) {
        self.tex_free(id);
        let (mut gf, mut bc, mut a8, mut unpack) = match fmt {
            FMT_A8R8G8B8 | FMT_X8R8G8B8 => (wgpu::TextureFormat::Bgra8Unorm, false, false, false),
            FMT_A8 => (wgpu::TextureFormat::Bgra8Unorm, false, true, false),
            FMT_P8 => (wgpu::TextureFormat::R8Unorm, false, false, false),
            dxt::DXT1 => (wgpu::TextureFormat::Bc1RgbaUnorm, true, false, false),
            FMT_DXT3 => (wgpu::TextureFormat::Bc2RgbaUnorm, true, false, false),
            dxt::DXT5 => (wgpu::TextureFormat::Bc3RgbaUnorm, true, false, false),
            _ => { log(&format!("[gpu] texture {id}: format {fmt} not supported")); return; }
        };
        // BC wants whole 4x4 blocks at level 0 (and a GPU that has it): else unpacked on upload
        if bc && (!self.bc || force_unpack || w & 3 != 0 || h & 3 != 0) { gf = wgpu::TextureFormat::Bgra8Unorm; bc = false; unpack = true; }
        let _ = &mut a8;
        let cube = flags & TF_CUBE != 0;
        let rt = flags & TF_RT != 0 && !bc && !unpack && fmt != FMT_A8 && fmt != FMT_P8;
        let maxlv = 32 - w.max(h).max(1).leading_zeros();
        let levels = levels.clamp(1, maxlv);
        let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC;
        if rt { usage |= wgpu::TextureUsages::RENDER_ATTACHMENT; }
        let tex = self.dev.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: if cube { 6 } else { 1 } },
            mip_level_count: levels, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: gf, usage, view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(if cube { wgpu::TextureViewDimension::Cube } else { wgpu::TextureViewDimension::D2 }), ..Default::default() });
        let mut rt_views = Vec::new();
        if rt {
            let one = |layer| tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer, array_layer_count: Some(1), base_mip_level: 0, mip_level_count: Some(1), ..Default::default() });
            if cube { for f in 0..6 { rt_views.push(one(f)); } } else { rt_views.push(one(0)); }
        }
        let i = id as usize;
        if self.tex.len() <= i { self.tex.resize_with(i + 1, || None); }
        self.tex[i] = Some(Tex { serial: self.serial, tex, fmt, w, h, levels, cube, rt, view, rt_views, expand_a8: a8, bc, unpack, face0: None });
        self.serial += 1;
    }

    fn tex_free(&mut self, id: u32) {
        if let Some(Some(t)) = self.tex.get_mut(id as usize).map(Option::take) {
            self.destroy_later.push(t.tex);
            if self.bg1.len() > 8192 { self.bg1.clear(); }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tex_upload(&mut self, id: u32, level: u32, face: u32, lw: u32, lh: u32, pitch: u32, rows: u32, data: usize) {
        let Some(Some(t)) = self.tex.get(id as usize) else { return };
        if level >= t.levels { return; }
        let dst = wgpu::TexelCopyTextureInfo { texture: &t.tex, mip_level: level,
            origin: wgpu::Origin3d { x: 0, y: 0, z: if t.cube { face.min(5) } else { 0 } }, aspect: wgpu::TextureAspect::All };
        let m = self.mem;
        if t.expand_a8 {
            let mut out = vec![0u8; (lw * lh * 4) as usize];
            for y in 0..lh as usize { for x in 0..lw as usize { out[(y * lw as usize + x) * 4 + 3] = m.bytes(data + y * pitch as usize + x, 1)[0]; } }
            self.queue.write_texture(dst, &out, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 4), rows_per_image: Some(lh) },
                wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 });
            return;
        }
        if t.unpack {
            let src = m.bytes(data, (pitch * rows) as usize);
            let mut out = std::mem::take(&mut self.unpack_buf);
            dxt::unpack(t.fmt, src, pitch as usize, lw as usize, lh as usize, &mut out);
            self.queue.write_texture(dst, &out, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 4), rows_per_image: Some(lh) },
                wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 });
            self.unpack_buf = out;
            return;
        }
        let (w, h) = if t.bc { ((lw + 3) & !3, (lh + 3) & !3) } else { (lw, lh) };
        self.queue.write_texture(dst, m.bytes(data, (pitch * rows) as usize),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(pitch), rows_per_image: Some(rows) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
    }

    fn pool_grow(&mut self, size: u64) {
        let size = (size + 3) & !3;
        let nb = self.dev.create_buffer(&wgpu::BufferDescriptor { label: Some("pool"), size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false });
        let mut e = self.dev.create_command_encoder(&Default::default());
        e.copy_buffer_to_buffer(&self.pool, 0, &nb, 0, self.pool.size().min(size));
        self.queue.submit([e.finish()]);
        self.pool = nb;
        self.buf_gen += 1;
    }

    // ------------------------------------------------------------------ helpers
    fn grow(&mut self, which: u8, need: u64) {
        let b = match which { 0 => &self.tv_buf, 1 => &self.raw_buf, 2 => &self.k_buf, _ => &self.p_buf };
        if b.size() >= need { return; }
        let size = ((need as f64 * 1.25 / 262144.0).ceil() as u64) * 262144;
        let usage = if which < 2 { wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST } else { wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST };
        let nb = self.dev.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage, mapped_at_creation: false });
        match which { 0 => self.tv_buf = nb, 1 => self.raw_buf = nb, 2 => self.k_buf = nb, _ => self.p_buf = nb }
        self.buf_gen += 1;
    }

    fn sampler(&mut self, k: u32) -> &wgpu::Sampler {
        if !self.samplers.contains_key(&k) {
            let (mn, mg, mp, au, av, al) = (k & 3, (k >> 2) & 3, (k >> 4) & 3, (k >> 6) & 7, (k >> 9) & 7, (k >> 12) & 7);
            let aniso = if mn == 3 || mg == 3 { (1u16 << al).min(16) } else { 1 };
            let fl = |v| if v == 1 { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
            let mut d = wgpu::SamplerDescriptor {
                min_filter: fl(mn), mag_filter: fl(mg),
                mipmap_filter: if mp == 2 { wgpu::MipmapFilterMode::Linear } else { wgpu::MipmapFilterMode::Nearest },
                address_mode_u: ADDR[if au < 6 { au as usize } else { 1 }], address_mode_v: ADDR[if av < 6 { av as usize } else { 1 }],
                lod_min_clamp: 0.0, lod_max_clamp: if mp == 0 { 0.0 } else { 32.0 }, ..Default::default()
            };
            if aniso > 1 {
                d.min_filter = wgpu::FilterMode::Linear; d.mag_filter = wgpu::FilterMode::Linear;
                d.mipmap_filter = wgpu::MipmapFilterMode::Linear; d.anisotropy_clamp = aniso;
            }
            let s = self.dev.create_sampler(&d);
            self.samplers.insert(k, s);
        }
        &self.samplers[&k]
    }

    fn copy_pipe(&mut self, format: wgpu::TextureFormat, opaque: bool) -> wgpu::RenderPipeline {
        if let Some(p) = self.copy_pipes.get(&(format, opaque)) { return p.clone(); }
        let p = self.dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("copy"), layout: Some(&self.copy_pl),
            vertex: wgpu::VertexState { module: &self.copy_mod, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &self.copy_mod, entry_point: Some(if opaque { "fs_opaque" } else { "fs_copy" }),
                compilation_options: Default::default(), targets: &[Some(format.into())] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        self.copy_pipes.insert((format, opaque), p.clone());
        p
    }

    /// draw `src` over `dst` (StretchRect between sizes, the composite), in the viewport `vp` if given
    #[allow(clippy::too_many_arguments)]
    fn copy_quad(&mut self, enc: &mut wgpu::CommandEncoder, src: &wgpu::TextureView, dst: &wgpu::TextureView, format: wgpu::TextureFormat,
                 linear: bool, vp: Option<[f32; 4]>, opaque: bool) {
        let pipe = self.copy_pipe(format, opaque);
        let smp = if linear { &self.linear } else { &self.nearest };
        let bg = self.dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.copy_bgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(smp) }] });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: dst, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            ..Default::default() });
        pass.set_pipeline(&pipe);
        pass.set_bind_group(0, &bg, &[]);
        if let Some(v) = vp { pass.set_viewport(v[0], v[1], v[2], v[3], 0.0, 1.0); }
        pass.draw(0..3, 0..1);
    }

    fn tone_quad(&mut self, enc: &mut wgpu::CommandEncoder, src: &wgpu::TextureView, dst: &wgpu::TextureView, linear: bool, tone: [f32; 3], vp: [f32; 4]) {
        let format = self.sfmt;
        if !self.tone_pipes.contains_key(&format) {
            let p = self.dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("tone"), layout: Some(&self.tone_pl),
                vertex: wgpu::VertexState { module: &self.tone_mod, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &self.tone_mod, entry_point: Some("fs_tone"),
                    compilation_options: Default::default(), targets: &[Some(format.into())] }),
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
            });
            self.tone_pipes.insert(format, p);
        }
        let mut b = [0u8; 16];
        for (i, v) in tone.iter().enumerate() { b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes()); }
        self.queue.write_buffer(&self.tone_buf, 0, &b);
        let smp = if linear { &self.linear } else { &self.nearest };
        let bg = self.dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.tone_bgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(smp) },
            wgpu::BindGroupEntry { binding: 2, resource: self.tone_buf.as_entire_binding() }] });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: dst, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            ..Default::default() });
        pass.set_pipeline(&self.tone_pipes[&format]);
        pass.set_bind_group(0, &bg, &[]);
        pass.set_viewport(vp[0], vp[1], vp[2], vp[3], 0.0, 1.0);
        pass.draw(0..3, 0..1);
    }

    #[allow(clippy::too_many_arguments)]
    fn pipeline(&mut self, key: u64, blend: u32, ps: i32, vs: i32, cull: u32, dm: u32, nd: bool) -> Option<wgpu::RenderPipeline> {
        if let Some(p) = self.pipes.get(&key) { return p.clone(); }
        let cube = ps >= PSCUBE;
        let vq = self.vs_mods.get(&vs);
        let pq = self.ps_mods.get(&ps);
        let vmod = vq.or(if vs == 0 { Some(&self.tvert_mod) } else { None });
        let fmod = pq.or(if ps == 0 { Some(&self.ff_mod) } else if ps == PSCUBE { Some(&self.ff_cube_mod) } else { None });
        let (Some(vmod), Some(fmod)) = (vmod, fmod) else { self.pipes.insert(key, None); return None; };
        let (en, sf, df, op, mask) = (blend & 1, (blend >> 1) & 31, (blend >> 6) & 31, (blend >> 11) & 7, (blend >> 14) & 15);
        let bs = if en != 0 {
            let o = *BLENDOP.get(op as usize).unwrap_or(&BO::Add);
            let mm = matches!(o, BO::Min | BO::Max);
            let mut s = *BLEND.get(sf as usize).unwrap_or(&BF::One);
            let mut d = *BLEND.get(df as usize).unwrap_or(&BF::One);
            if sf == 12 { d = BF::OneMinusSrcAlpha; }
            if sf == 13 { s = BF::OneMinusSrcAlpha; d = BF::SrcAlpha; }
            if mm { s = BF::One; d = BF::One; }
            let c = wgpu::BlendComponent { src_factor: s, dst_factor: d, operation: o };
            Some(wgpu::BlendState { color: c, alpha: c })
        } else { None };
        let consts = [("ALPHA_FUNC", if nd { 8.0 } else { 7.0 })];
        let g = self.dev.push_error_scope(wgpu::ErrorFilter::Validation);
        let p = self.dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None, layout: Some(if cube { &self.pl_cube } else { &self.pl2d }),
            vertex: wgpu::VertexState { module: vmod, entry_point: if vq.is_some() { None } else { Some("vs_tvert") },
                compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: fmod, entry_point: if pq.is_some() { None } else { Some("fs_ff") },
                compilation_options: wgpu::PipelineCompilationOptions { constants: &consts, zero_initialize_workgroup_memory: false },
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Bgra8Unorm, blend: bs,
                    write_mask: wgpu::ColorWrites::from_bits_truncate(mask) })] }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, front_face: wgpu::FrontFace::Cw,
                cull_mode: match cull { 1 => None, 2 => Some(wgpu::Face::Front), _ => Some(wgpu::Face::Back) }, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(dm & 2 != 0),
                depth_compare: Some(if dm & 1 != 0 { CMP[((dm >> 2) & 7) as usize] } else { CF::Always }),
                stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let ok = match pollster::block_on(g.pop()) {
            Some(e) => { log(&format!("[gpu] pipeline ps={ps} vs={vs} blend={blend:x}: {e}")); None }
            None => { self.pipes_made += 1; Some(p) }
        };
        self.pipes.insert(key, ok.clone());
        ok
    }

    fn group0(&mut self, src: Src) -> wgpu::BindGroup {
        if self.bg0_gen != self.buf_gen { self.bg0 = [None, None, None]; self.bg0_gen = self.buf_gen; }
        if let Some(g) = &self.bg0[src as usize] { return g.clone(); }
        let vb = match src { Src::TVert => &self.tv_buf, Src::Raw => &self.raw_buf, Src::Pool => &self.pool };
        let g = self.dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.bgl0, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: vb.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.k_buf, offset: 0, size: wgpu::BufferSize::new(KWIN_SIZE) }) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.p_buf, offset: 0, size: wgpu::BufferSize::new(PSPAR_SIZE) }) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.k_buf, offset: K_HL, size: wgpu::BufferSize::new(HLPAR_SIZE) }) },
        ] });
        self.bg0[src as usize] = Some(g.clone());
        g
    }

    fn depth_scratch(&mut self, w: u32, h: u32) -> wgpu::TextureView {
        self.scratch_z.entry((w, h)).or_insert_with(|| {
            self.dev.create_texture(&wgpu::TextureDescriptor { label: Some("scratch-z"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] }).create_view(&Default::default())
        }).clone()
    }

    fn ensure_back_buffer(&mut self, w: u32, h: u32) {
        if let Some(b) = &self.bb { if b.width() == w && b.height() == h { return; } }
        if let Some(b) = self.bb.take() { self.destroy_later.push(b); }
        let t = self.dev.create_texture(&wgpu::TextureDescriptor { label: Some("backbuffer"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[] });
        self.bb_view = Some(t.create_view(&Default::default()));
        self.bb = Some(t);
        log(&format!("[gpu] back buffer {w}x{h}"));
    }

    /// the texture's sampling view and serial, or white (missing, other dimension, or the pass's own target)
    fn tex_for(&self, id: u32, cube: bool, pass_tex: i64) -> (u32, wgpu::TextureView) {
        let w = if cube { &self.white_cube } else { &self.white };
        match self.tex.get(id as usize).and_then(Option::as_ref) {
            Some(t) if id != 0 && t.cube == cube && id as i64 != pass_tex => (t.serial, t.view.clone()),
            _ => (0, w.view.clone()),
        }
    }

    // -------------------------------------------------------------------- frame
    fn encode_and_present(&mut self, b: usize) {
        let fw = self.ctl.at(c::FRAME + b * FRAME_BYTES);
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.dev, &self.scfg); None }
            _ => None,
        };
        let mut enc = self.dev.create_command_encoder(&Default::default());
        self.encode_frame(fw, &mut enc);
        let (bbw, bbh) = (self.bb.as_ref().unwrap().width(), self.bb.as_ref().unwrap().height());
        if let Some(frame) = &frame {
            let cur = frame.texture.create_view(&Default::default());
            let (cw, ch) = (frame.texture.width() as f32, frame.texture.height() as f32);
            let sc = (cw / bbw as f32).min(ch / bbh as f32);
            let (w, h) = ((bbw as f32 * sc).round(), (bbh as f32 * sc).round());
            let rect = [((cw - w) / 2.0).floor(), ((ch - h) / 2.0).floor(), w, h];
            *self.view.img.lock().unwrap() = rect;
            let one = w as u32 == bbw && h as u32 == bbh;
            let src = self.bb_view.clone().unwrap();
            match self.tone_of(fw) {
                Some(t) => self.tone_quad(&mut enc, &src, &cur, !one, t, rect),
                None => { let f = self.sfmt; self.copy_quad(&mut enc, &src, &cur, f, !one, Some(rect), true); }
            }
        }
        // a texture the game waits for (gpu_web.c metal_read_texture), drawn by this very frame
        let rb_id = fw.u32(f::RB_ID);
        let rb = if rb_id != 0 { self.read_out(&mut enc, fw.u32(f::RB_TEX)) } else { None };
        self.queue.submit([enc.finish()]);
        if let Some(fr) = frame { self.queue.present(fr); }
        for t in self.destroy_later.drain(..) { t.destroy(); }
        if rb_id != 0 { self.answer_read(rb_id, rb, fw.u32(f::RB_DST), fw.u32(f::RB_PITCH), fw.u32(f::RB_ROWS)); }
        self.frames += 1;
        self.ctl.set_u32(c::FRAMES_DONE, self.frames);
        self.ctl.set_u32(c::ST_DRAWS, self.total_draws);
        self.ctl.set_u32(c::ST_PASSES, self.total_passes);
        self.ctl.set_u32(c::ST_ERRORS, self.errors.load(Ordering::Relaxed));
        self.ctl.set_u32(c::ST_PIPES, self.pipes_made);
    }

    /// Exposure and White Level (runtime/tonemenu.c): the frame's GwTone, or None while they leave the picture as drawn
    fn tone_of(&self, fw: Shm) -> Option<[f32; 3]> {
        let at = (fw.u32(f::K) as usize) + K_TONE;
        if self.mem.u32(at) != TONE_MAGIC { return None; }
        let t = [self.mem.f32(at + 4), self.mem.f32(at + 8), self.mem.f32(at + 12)];
        let on = t[0] > 0.0 && t[1] > 0.0 && t[2] >= 0.0 && t[2] < 1.0 && ((t[0] - 1.0).abs() > 1e-4 || t[1] < 0.9999);
        if on { Some(t) } else { None }
    }

    fn read_out(&mut self, enc: &mut wgpu::CommandEncoder, id: u32) -> Option<(wgpu::Buffer, u32, u32, u32)> {
        let (src, w, h) = {
            let t = self.tex.get(id as usize)?.as_ref()?;
            if t.tex.format() == wgpu::TextureFormat::Bgra8Unorm && !t.cube { (t.tex.clone(), t.w, t.h) }
            else {
                let out = self.dev.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: t.w, height: t.h, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
                let v = t.tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: 0,
                    array_layer_count: Some(1), base_mip_level: 0, mip_level_count: Some(1), ..Default::default() });
                let (w, h) = (t.w, t.h);
                let ov = out.create_view(&Default::default());
                self.copy_quad(enc, &v, &ov, wgpu::TextureFormat::Bgra8Unorm, false, None, true);
                (out, w, h)
            }
        };
        let bpr = (w * 4).div_ceil(256) * 256;
        let buf = self.dev.create_buffer(&wgpu::BufferDescriptor { label: None, size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        enc.copy_texture_to_buffer(src.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
        Some((buf, w, h, bpr))
    }

    fn answer_read(&mut self, id: u32, rb: Option<(wgpu::Buffer, u32, u32, u32)>, dst: u32, pitch: u32, rows: u32) {
        let mut ok = false;
        if let Some((buf, w, h, bpr)) = &rb {
            let s = buf.slice(..);
            s.map_async(wgpu::MapMode::Read, |_| {});
            let _ = self.dev.poll(wgpu::PollType::wait_indefinitely());
            if let Ok(v) = s.get_mapped_range() {
                if self.ctl.cas(c::RB_WANT, id as i32, (id | RB_FLAG) as i32) == id as i32 {
                    let n = (pitch as usize).min(*w as usize * 4);
                    for y in 0..(rows.min(*h) as usize) {
                        self.mem.bytes_mut(dst as usize + y * pitch as usize, n).copy_from_slice(&v[y * *bpr as usize..y * *bpr as usize + n]);
                    }
                    ok = true;
                } else { return; }
            }
            buf.unmap();
        }
        if !ok && self.ctl.cas(c::RB_WANT, id as i32, (id | RB_FLAG) as i32) != id as i32 { return; }
        self.ctl.store(c::RB_DONE, if ok { id as i32 } else { (id | RB_FLAG) as i32 });
        ffi::notify(self.ctl.atomic(c::RB_DONE));
    }

    /// Does the frame copy from the screen before anything draws to it? (it then reads the previous frame)
    fn encode_frame(&mut self, fw: Shm, enc: &mut wgpu::CommandEncoder) {
        let m = self.mem;
        let (dn, vn, rvn, kn) = (fw.u32(f::DN) as usize, fw.u32(f::VN) as u64, fw.u32(f::RVN) as u64, fw.u32(f::KN) as u64);
        let draws = fw.u32(f::DRAWS) as usize;
        let psc = fw.u32(f::PSC) as usize;
        let bbw = if fw.u32(f::BB_W) != 0 { fw.u32(f::BB_W) } else { 1280 };
        let bbh = if fw.u32(f::BB_H) != 0 { fw.u32(f::BB_H) } else { 720 };
        self.ensure_back_buffer(bbw, bbh);
        let screen = self.bb.clone().unwrap();
        let screen_view = self.bb_view.clone().unwrap();

        if vn > 0 { self.grow(0, vn * TVERT_BYTES); self.queue.write_buffer(&self.tv_buf, 0, m.bytes(fw.u32(f::TV) as usize, (vn * TVERT_BYTES) as usize)); }
        if rvn > 0 { let n = (rvn + 3) & !3; self.grow(1, n); self.queue.write_buffer(&self.raw_buf, 0, m.bytes(fw.u32(f::RAW) as usize, n as usize)); }
        if kn > 0 { let n = (kn + 3) & !3; self.grow(2, n + KWIN_SIZE); self.queue.write_buffer(&self.k_buf, 0, m.bytes(fw.u32(f::K) as usize, n as usize)); }
        self.grow(3, (dn as u64 + 1) * PSLOT);
        let need = (dn + 1) * (PSLOT as usize / 4);
        if self.p_stage.len() < need { self.p_stage.resize(need + 64 * (PSLOT as usize / 4), 0.0); }
        let mut pslots = 0usize;
        let mut last_psci = u32::MAX;
        let mut last_aref = f32::NAN;
        let mut last_pf = [f32::NAN; 8];

        let dw = |i: usize, k: usize| m.u32(draws + i * DRAW_BYTES + k * 4);
        let df = |i: usize, k: usize| m.f32(draws + i * DRAW_BYTES + k * 4);

        let mut pass: Option<wgpu::RenderPass<'static>> = None;
        let mut cur_t: i64 = -1;
        let mut cur_z: u32 = u32::MAX;
        let mut real_z = false;
        let mut pass_tex: i64 = -1;
        let mut screen_touched = false;
        let mut touched = std::collections::HashSet::new();
        let mut clear_argb = fw.u32(f::CLEAR);
        let (mut c_pipe, mut c_src, mut c_k, mut c_p, mut c_bcol, mut c_ix) = (None::<u64>, None::<Src>, -1i64, u64::MAX, u32::MAX, 0u32);
        let mut c_b1: Option<(bool, [u32; 4], [u32; 4])> = None;
        let (mut pass_w, mut pass_h, mut c_clip) = (0u32, 0u32, 0u32);
        let (mut draws_n, mut passes) = (0u32, 0u32);

        for i in 0..dn {
            let kind = dw(i, d::KIND);
            if kind == KIND_BLIT {
                let (sid, tgt) = (dw(i, d::BLIT_SRC), dw(i, d::TARGET));
                let (r0, r1) = (dw(i, d::FIRST), dw(i, d::COUNT));
                let s_tex = if sid != 0 { self.tex.get(sid as usize).and_then(|t| t.as_ref()).map(|t| t.tex.clone()) } else { Some(screen.clone()) };
                let d_t = if tgt != 0 { self.tex.get((tgt & 0xFFFF) as usize).and_then(|t| t.as_ref()) } else { None };
                let d_tex = if tgt != 0 { d_t.map(|t| t.tex.clone()) } else { Some(screen.clone()) };
                let (Some(s_tex), Some(d_tex)) = (s_tex, d_tex) else { continue };
                if s_tex == d_tex || (tgt != 0 && !d_t.unwrap().rt) { continue; }
                let dv = if tgt != 0 { d_t.unwrap().rt_views[0].clone() } else { screen_view.clone() };
                pass = None; cur_t = -1; cur_z = u32::MAX;
                if r1 != 0 && self.blit_rect(enc, &s_tex, r0, r1, &dv, sid == 0) { passes += 1; }
                else if s_tex.width() == d_tex.width() && s_tex.height() == d_tex.height() && s_tex.format() == d_tex.format()
                    && s_tex.depth_or_array_layers() == 1 && d_tex.depth_or_array_layers() == 1 {
                    enc.copy_texture_to_texture(s_tex.as_image_copy(), d_tex.as_image_copy(),
                        wgpu::Extent3d { width: s_tex.width(), height: s_tex.height(), depth_or_array_layers: 1 });
                } else {
                    let sv = if sid != 0 {
                        let t = self.tex[sid as usize].as_mut().unwrap();
                        if t.cube { t.face0.get_or_insert_with(|| t.tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2),
                            base_array_layer: 0, array_layer_count: Some(1), base_mip_level: 0, mip_level_count: Some(1), ..Default::default() })).clone() }
                        else { t.view.clone() }
                    } else { screen_view.clone() };
                    self.copy_quad(enc, &sv, &dv, wgpu::TextureFormat::Bgra8Unorm, true, None, false);
                    passes += 1;
                }
                if tgt != 0 { touched.insert((tgt & 0xFFFF) * 8 + ((tgt >> 16) & 7)); } else { screen_touched = true; }
                continue;
            }
            let depth = dw(i, d::DEPTH);
            let zid = depth >> 8;
            let target = dw(i, d::TARGET);
            if kind == KIND_CLEAR {
                let fl = dw(i, d::CLR_FLAGS);
                if fl & 1 != 0 { clear_argb = dw(i, d::CLR_ARGB); }
                if fl & 2 != 0 && zid != 0 && zid < 64 {
                    let z = df(i, d::CLR_Z);
                    self.zpend[zid as usize] = true; self.zpend_v[zid as usize] = z.clamp(0.0, 1.0);
                    if pass.is_some() && cur_z == zid { pass = None; cur_t = -1; }
                }
            }
            let zsplit = zid != cur_z && !(pass.is_some() && kind != KIND_CLEAR && (depth & 3) == 0);
            if cur_t == -1 || target as i64 != cur_t || zsplit {
                drop(pass.take());
                cur_t = target as i64; cur_z = zid;
                // open a pass on the target (and its Z surface)
                let opened = (|| -> Option<(wgpu::TextureView, u32, u32, u32, i64)> {
                    if target == 0 { return Some((screen_view.clone(), bbw, bbh, u32::MAX, -1)); }
                    let t = self.tex.get((target & 0xFFFF) as usize)?.as_ref()?;
                    if !t.rt { return None; }
                    let face = (target >> 16) & 7;
                    let v = if t.cube { t.rt_views[if face != 0 { (face as usize - 1).min(5) } else { 0 }].clone() } else { t.rt_views[0].clone() };
                    Some((v, t.w, t.h, (target & 0xFFFF) * 8 + face, (target & 0xFFFF) as i64))
                })();
                match opened {
                    None => { self.missing_target += 1; pass = None; }
                    Some((view, w, h, key, ptex)) => {
                        pass_w = w; pass_h = h; pass_tex = ptex;
                        let first = if key == u32::MAX { !screen_touched } else { !touched.contains(&key) };
                        let mut zv = None; let mut zclear = false; let mut zval = 1.0f32;
                        if zid != 0 && zid < 64 {
                            let zi = zid as usize;
                            let fits = self.ztex[zi].as_ref().map(|z| z.width() == w && z.height() == h);
                            if fits != Some(true) {
                                if fits.is_none() || zid == 1 {
                                    if let Some(z) = self.ztex[zi].take() { self.destroy_later.push(z); }
                                    let z = self.dev.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                                        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Depth32Float,
                                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] });
                                    self.zview[zi] = Some(z.create_view(&Default::default()));
                                    self.ztex[zi] = Some(z); zclear = true;
                                } else { self.zview[zi] = self.zview[zi].clone(); }
                            }
                            if self.ztex[zi].as_ref().map(|z| z.width() == w && z.height() == h) == Some(true) {
                                zv = self.zview[zi].clone();
                                if self.zpend[zi] { zclear = true; zval = self.zpend_v[zi]; self.zpend[zi] = false; }
                            }
                        }
                        real_z = zv.is_some();
                        let cc = clear_argb;
                        let col = wgpu::Color { r: ((cc >> 16) & 255) as f64 / 255.0, g: ((cc >> 8) & 255) as f64 / 255.0, b: (cc & 255) as f64 / 255.0, a: 1.0 };
                        let (zview, zload, zstore) = match zv {
                            Some(z) => (z, if zclear { wgpu::LoadOp::Clear(zval) } else { wgpu::LoadOp::Load }, wgpu::StoreOp::Store),
                            None => (self.depth_scratch(w, h), wgpu::LoadOp::Clear(1.0), wgpu::StoreOp::Discard),
                        };
                        let p = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None,
                                ops: wgpu::Operations { load: if first { wgpu::LoadOp::Clear(col) } else { wgpu::LoadOp::Load }, store: wgpu::StoreOp::Store } })],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &zview,
                                depth_ops: Some(wgpu::Operations { load: zload, store: zstore }), stencil_ops: None }),
                            ..Default::default() }).forget_lifetime();
                        if key == u32::MAX { screen_touched = true; } else { touched.insert(key); }
                        passes += 1;
                        c_pipe = None; c_src = None; c_k = -1; c_p = u64::MAX; c_b1 = None; c_bcol = u32::MAX; c_ix = 0; c_clip = 0;
                        pass = Some(p);
                    }
                }
            }
            if kind == KIND_CLEAR { continue; }
            let count = dw(i, d::COUNT);
            if pass.is_none() || count == 0 { continue; }

            // pipeline
            let psid = dw(i, d::PS) as i32;
            let vsid = dw(i, d::VSID) as i32;
            if vsid > 0 && !self.vs_mods.contains_key(&vsid) { self.no_vs += 1; continue; }
            let aref = df(i, d::AREF);
            let blend = dw(i, d::BLEND) & 0x3FFFF;
            let cull = dw(i, d::CULL) & 3;
            let dm = if real_z { depth & 63 } else { 0 };
            let t0 = dw(i, d::TEX);
            let mut ps_eff = if psid > 0 && self.ps_mods.contains_key(&psid) { psid } else if psid >= PSCUBE { PSCUBE } else { 0 };
            if ps_eff == 0 && t0 != 0 && self.tex.get(t0 as usize).and_then(|t| t.as_ref()).map(|t| t.cube) == Some(true) { ps_eff = PSCUBE; }
            let nd = !(aref > 0.0);
            let key = blend as u64 | (ps_eff as u64 & 0x3FF) << 18 | (vsid as u64 & 0xFFFF) << 28 | (cull as u64) << 44 | (dm as u64) << 46 | (nd as u64) << 52;
            let Some(pipe) = self.pipeline(key, blend, ps_eff, vsid, cull, dm, nd) else { continue };
            let p = pass.as_mut().unwrap();
            if c_pipe != Some(key) { p.set_pipeline(&pipe); c_pipe = Some(key); }
            let clip = if pass_w == bbw && pass_h == bbh { dw(i, d::CLIP) } else { 0 };
            if clip != c_clip {
                let (y0, y1) = ((clip & 0xFFFF).min(pass_h), (clip >> 16).min(pass_h));
                if clip != 0 { p.set_scissor_rect(0, y0, pass_w, y1.saturating_sub(y0)); } else { p.set_scissor_rect(0, 0, pass_w, pass_h); }
                c_clip = clip;
            }

            // PS parameters: a new slot only when they differ from the previous draw's
            let psci = dw(i, d::PSCI);
            let mut same = psci == last_psci && aref.to_bits() == last_aref.to_bits();
            if same { for q in 0..4 { if last_pf[q].to_bits() != df(i, d::PROJ + q).to_bits() || last_pf[4 + q].to_bits() != df(i, d::FOGV + q).to_bits() { same = false; break; } } }
            if !same {
                let o = pslots * (PSLOT as usize / 4);
                let ps = &mut self.p_stage;
                ps[o] = aref; ps[o + 1] = 0.0; ps[o + 2] = 0.0; ps[o + 3] = 0.0;
                for q in 0..4 {
                    last_pf[q] = df(i, d::PROJ + q); ps[o + 4 + q] = last_pf[q];
                    last_pf[4 + q] = df(i, d::FOGV + q); ps[o + 136 + q] = last_pf[4 + q];
                }
                let pb = psc + psci as usize * 512;
                for q in 0..128 { ps[o + 8 + q] = m.f32(pb + q * 4); }
                last_psci = psci; last_aref = aref; pslots += 1;
            }
            let poff = (pslots as u64 - 1) * PSLOT;

            // group 0: vertex source + VS constants (a 64 KB window, the draw's row as firstInstance) + PS parameters
            let isz = dw(i, d::IX_ISZ);
            let src = if isz != 0 { Src::Pool } else if vsid > 0 { Src::Raw } else { Src::TVert };
            let koff: i64 = if vsid > 0 { dw(i, d::KOFF) as i64 } else { -1 };
            if Some(src) != c_src || poff != c_p || (koff >= 0 && (c_k < 0 || koff < c_k || koff - c_k > (KWIN_SIZE as i64 - VSPAR_SIZE as i64))) {
                if koff >= 0 { c_k = koff & !255; } else if c_k < 0 { c_k = 0; }
                let g = self.group0(src);
                let p = pass.as_mut().unwrap();
                p.set_bind_group(0, &g, &[c_k as u32, poff as u32]);
                c_src = Some(src); c_p = poff;
            }
            let kb = if koff >= 0 { ((koff - c_k) >> 4) as u32 } else { 0 };

            // group 1: textures and samplers (a texture bound as this pass's target samples white)
            let cube = ps_eff >= PSCUBE;
            let ids = [t0, dw(i, d::TEX1), dw(i, d::TEX2), dw(i, d::TEX3)];
            let smp = [dw(i, d::SMP), dw(i, d::SMP + 1), dw(i, d::SMP + 2), dw(i, d::SMP + 3)];
            let tv: Vec<(u32, wgpu::TextureView)> = (0..4).map(|k| self.tex_for(ids[k], cube && k == 0, pass_tex)).collect();
            let k1 = (cube, [tv[0].0, tv[1].0, tv[2].0, tv[3].0], smp);
            if c_b1 != Some(k1) {
                if !self.bg1.contains_key(&k1) {
                    for s in smp { self.sampler(s); }
                    let g = self.dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: if cube { &self.bgl1c } else { &self.bgl1 }, entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tv[0].1) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tv[1].1) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&tv[2].1) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&tv[3].1) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.samplers[&smp[0]]) },
                        wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.samplers[&smp[1]]) },
                        wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.samplers[&smp[2]]) },
                        wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&self.samplers[&smp[3]]) }] });
                    self.bg1.insert(k1, g);
                }
                pass.as_mut().unwrap().set_bind_group(1, &self.bg1[&k1], &[]);
                c_b1 = Some(k1);
            }
            let p = pass.as_mut().unwrap();
            let bcol = dw(i, d::BCOL);
            if bcol != c_bcol {
                p.set_blend_constant(wgpu::Color { r: ((bcol >> 16) & 255) as f64 / 255.0, g: ((bcol >> 8) & 255) as f64 / 255.0,
                    b: (bcol & 255) as f64 / 255.0, a: (bcol >> 24) as f64 / 255.0 });
                c_bcol = bcol;
            }
            if isz != 0 {
                if isz != c_ix {
                    p.set_index_buffer(self.pool.slice(..), if isz == 4 { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 });
                    c_ix = isz;
                }
                let first = dw(i, d::IX_IB) / isz;
                p.draw_indexed(first..first + count, dw(i, d::IX_BASE) as i32, kb..kb + 1);
            } else if vsid > 0 {
                p.draw(0..count, kb..kb + 1);
            } else {
                let first = dw(i, d::FIRST);
                p.draw(first..first + count, 0..1);
            }
            draws_n += 1;
        }
        drop(pass);
        if !screen_touched {
            let cc = clear_argb;
            let col = wgpu::Color { r: ((cc >> 16) & 255) as f64 / 255.0, g: ((cc >> 8) & 255) as f64 / 255.0, b: (cc & 255) as f64 / 255.0, a: 1.0 };
            enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &screen_view, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(col), store: wgpu::StoreOp::Store } })], ..Default::default() });
            passes += 1;
        }
        if pslots > 0 {
            let n = pslots * (PSLOT as usize / 4);
            let bytes: Vec<u8> = self.p_stage[..n].iter().flat_map(|v| v.to_le_bytes()).collect();
            self.queue.write_buffer(&self.p_buf, 0, &bytes);
        }
        self.total_draws = self.total_draws.wrapping_add(draws_n);
        self.total_passes = self.total_passes.wrapping_add(passes);
        let _ = (self.missing_target, self.no_vs, &self.said);
    }

    /// StretchRect from a rectangle of `src`: copied into a texture of its own, then drawn over `dst`
    fn blit_rect(&mut self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Texture, r0: u32, r1: u32, dst: &wgpu::TextureView, opaque: bool) -> bool {
        let (x0, y0) = ((r0 & 0xFFFF).min(src.width()), (r0 >> 16).min(src.height()));
        let (x1, y1) = ((r1 & 0xFFFF).min(src.width()), (r1 >> 16).min(src.height()));
        if x1 <= x0 || y1 <= y0 || src.format() != wgpu::TextureFormat::Bgra8Unorm { return false; }
        let part = self.dev.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: x1 - x0, height: y1 - y0, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        enc.copy_texture_to_texture(wgpu::TexelCopyTextureInfo { texture: src, mip_level: 0, origin: wgpu::Origin3d { x: x0, y: y0, z: 0 }, aspect: wgpu::TextureAspect::All },
            part.as_image_copy(), wgpu::Extent3d { width: x1 - x0, height: y1 - y0, depth_or_array_layers: 1 });
        let pv = part.create_view(&Default::default());
        self.copy_quad(enc, &pv, dst, wgpu::TextureFormat::Bgra8Unorm, true, None, opaque);
        self.destroy_later.push(part);
        true
    }
}

fn tex_entry(binding: u32, vis: wgpu::ShaderStages, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: vis, ty: wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: dim, multisampled: false }, count: None }
}
fn smp_entry(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: vis, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }
}

/// The shader ABI's explicit layouts (docs/web/gpu-abi.md section 2, web/src/gpu/shader-abi.ts)
fn abi_layouts(dev: &wgpu::Device) -> (wgpu::BindGroupLayout, wgpu::BindGroupLayout, wgpu::BindGroupLayout) {
    let (v, fr) = (wgpu::ShaderStages::VERTEX, wgpu::ShaderStages::FRAGMENT);
    let ub = |binding, vis, dynamic, size| wgpu::BindGroupLayoutEntry { binding, visibility: vis, ty: wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: dynamic, min_binding_size: wgpu::BufferSize::new(size) }, count: None };
    let g0 = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("abi.group0"), entries: &[
        wgpu::BindGroupLayoutEntry { binding: 0, visibility: v, ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ub(1, v, true, KWIN_SIZE), ub(2, fr, true, PSPAR_SIZE), ub(3, fr, false, HLPAR_SIZE)] });
    let textures = |cube: bool| -> Vec<wgpu::BindGroupLayoutEntry> {
        let mut e: Vec<_> = (0..4).map(|k| tex_entry(k, fr, if cube && k == 0 { wgpu::TextureViewDimension::Cube } else { wgpu::TextureViewDimension::D2 })).collect();
        e.extend((0..4).map(|k| smp_entry(4 + k, fr)));
        e
    };
    let g1 = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("abi.group1"), entries: &textures(false) });
    let g1c = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("abi.group1.cube"), entries: &textures(true) });
    (g0, g1, g1c)
}

fn make_white(dev: &wgpu::Device, queue: &wgpu::Queue, cube: bool) -> Tex {
    let layers = if cube { 6 } else { 1 };
    let tex = dev.create_texture_with_data(queue, &wgpu::TextureDescriptor { label: Some("white"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] },
        wgpu::util::TextureDataOrder::LayerMajor, &vec![255u8; 4 * layers as usize]);
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(if cube { wgpu::TextureViewDimension::Cube } else { wgpu::TextureViewDimension::D2 }), ..Default::default() });
    Tex { serial: 0, tex, fmt: FMT_A8R8G8B8, w: 1, h: 1, levels: 1, cube, rt: false, view, rt_views: vec![], expand_a8: false, bc: false, unpack: false, face0: None }
}

/// The display clock (GwVsync), as the page fills it from requestAnimationFrame: a thread that waits for every
/// composition of the desktop (DwmFlush: the display's refresh) and publishes its time and the refresh rate.
pub fn vsync_thread(quit: Arc<View>) {
    let Some((p, _)) = ffi::lookup(ffi::KIND_VSYNC, 120_000) else { return };
    let v = Shm(p);
    const RATES: [f64; 16] = [24., 30., 48., 50., 60., 72., 75., 90., 96., 100., 120., 144., 165., 180., 240., 360.];
    let snap = |hz: f64| RATES.iter().copied().find(|r| (hz / r - 1.0).abs() < 0.025).unwrap_or(hz.round());
    let mut iv: std::collections::VecDeque<f64> = Default::default();
    let (mut last, mut hz, mut cand, mut cand_since) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    while !quit.quit.load(Ordering::Relaxed) {
        let t0 = ffi::now_ms();
        let ok = unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() } >= 0;
        // no composition, or a DwmFlush that does not wait for the display (some VMs, Wine): a 60 Hz clock
        if !ok || ffi::now_ms() - t0 < 2.0 {
            let next = (t0 / (1000.0 / 60.0)).floor() * (1000.0 / 60.0) + 1000.0 / 60.0;
            let d = next - ffi::now_ms();
            if d > 0.0 { std::thread::sleep(std::time::Duration::from_secs_f64(d / 1000.0)); }
        }
        let t = ffi::now_ms();
        if last > 0.0 { let d = t - last; if d > 2.0 && d < 100.0 { iv.push_back(d); if iv.len() > 120 { iv.pop_front(); } } }
        last = t;
        if iv.len() >= 30 {
            let mut sp: Vec<f64> = Vec::with_capacity(iv.len());
            for i in 3..iv.len() { sp.push((iv[i] + iv[i - 1] + iv[i - 2] + iv[i - 3]) / 4.0); }
            sp.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = sp[sp.len() / 2];
            let h = snap(1000.0 / med);
            if h != cand { cand = h; cand_since = t; }
            if cand != hz && (hz == 0.0 || t - cand_since >= 500.0) {
                log(&format!("[gpu] display {cand} Hz (median {med:.3} ms)"));
                hz = cand; v.store(24, hz as i32);
            }
            let per = if RATES.contains(&hz) { 1000.0 / hz } else { med };
            v.add(4, 1);
            v.set_f64(8, t); v.set_f64(16, per);
            v.add(4, 1);
            v.store(28, 1);
            v.add(0, 1);
            ffi::notify(v.atomic(0));
        }
    }
}
