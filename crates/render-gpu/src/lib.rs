//! Experimental GPU canvas renderer (a spike): the scene graph as a [vello] scene, rasterised by
//! compute shaders on wgpu.
//!
//! The CPU renderer (`figcraft-render`) stays the reference, the default and the only renderer of
//! exports. [`Canvas::draw`] returns the reason ([`Fallback`]) when a frame holds something this
//! doesn't draw yet (effects, masks, pattern fills, outline views); the caller then renders that
//! frame on the CPU.
//!
//! The scene is built in page coordinates and kept per top-level layer: a pan or zoom only
//! re-renders it through a new transform, an edit rebuilds the top-level layers it touched.

pub mod scene;

use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

use figcraft_doc::{Document, NodeId, NodeKind};
use figcraft_geom::Rect;
use figcraft_render::{Options, Pixmap, View};
pub use scene::{Builder, Fallback, Images};
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene, wgpu};

/// More changed layers than this rebuild the whole scene.
const MAX_CHANGED: usize = 4096;

/// A wgpu device with a vello renderer and a render target (resized as needed) it reads back.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    target: Option<Target>,
    /// The adapter's name and backend (for reports).
    pub adapter: String,
}

struct Target {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Rows padded to wgpu's copy alignment.
    readback: wgpu::Buffer,
    row: u32,
}

impl Gpu {
    /// A device of its own on the default (high-performance) adapter.
    pub fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }),
        )
        .map_err(|e| e.to_string())?;
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("figcraft-gpu-canvas"), ..Default::default() }))
                .map_err(|e| e.to_string())?;
        // wgpu panics on device errors by default; a canvas frame must not take the app down.
        device.on_uncaptured_error(Arc::new(|e| log::error!("gpu canvas: {e}")));
        let renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { device, queue, renderer, target: None, adapter: format!("{} ({:?})", info.name, info.backend) })
    }

    /// Render `scene` into the `width`×`height` target over `base` (submitted, not waited for).
    pub fn render(&mut self, scene: &Scene, width: u32, height: u32, base: figcraft_color::Color) -> Result<(), Fallback> {
        let max = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            return Err("view larger than the GPU's textures");
        }
        if self.target.as_ref().is_none_or(|t| t.width != width || t.height != height) {
            self.target = Some(self.target(width, height));
        }
        let Some(t) = &self.target else { return Err("no render target") };
        let params = RenderParams {
            base_color: vello::peniko::Color::new([base.r, base.g, base.b, base.a]),
            width,
            height,
            antialiasing_method: AaConfig::Area,
        };
        self.renderer.render_to_texture(&self.device, &self.queue, scene, &t.view, &params).map_err(|e| {
            log::warn!("gpu canvas: {e}");
            "GPU render failed"
        })
    }

    fn target(&self, width: u32, height: u32) -> Target {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("figcraft-gpu-canvas-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("figcraft-gpu-canvas-readback"),
            size: u64::from(row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Target { width, height, texture, view, readback, row }
    }

    /// Block until the GPU has finished what was submitted.
    pub fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// The last render as a premultiplied pixmap (copied back from the GPU; blocks).
    pub fn read(&self) -> Option<Pixmap> {
        let t = self.target.as_ref()?;
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("figcraft-gpu-canvas-read") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &t.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(t.row), rows_per_image: None },
            },
            wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = t.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| _ = tx.send(r.is_ok()));
        self.wait();
        if rx.recv() != Ok(true) {
            return None;
        }
        let mut pix = Pixmap::new(t.width, t.height)?;
        {
            let Ok(data) = slice.get_mapped_range() else {
                t.readback.unmap();
                return None;
            };
            let w = t.width as usize * 4;
            for (dst, src) in pix.data_mut().chunks_exact_mut(w).zip(data.chunks(t.row as usize)) {
                let Some(src) = src.get(..w) else { break };
                // vello writes straight alpha; pixmaps are premultiplied.
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    let a = u32::from(s.get(3).copied().unwrap_or(0));
                    if a == 255 {
                        d.copy_from_slice(s);
                    } else {
                        for (dc, sc) in d.iter_mut().zip(s).take(3) {
                            *dc = ((u32::from(*sc) * a + 127) / 255) as u8;
                        }
                        if let Some(da) = d.get_mut(3) {
                            *da = a as u8;
                        }
                    }
                }
            }
        }
        t.readback.unmap();
        Some(pix)
    }
}

/// A top-level layer's scene and the page area it paints (`None`: anywhere).
type Part = Result<(Scene, Option<Rect>), Fallback>;

/// What the kept scene was built from.
struct Kept {
    doc: Arc<Document>,
    page: NodeId,
    skip: Vec<NodeId>,
    fonts: u64,
    /// Each top-level layer's scene (or why it can't be drawn), in page order.
    parts: Vec<(NodeId, Part)>,
}

/// The GPU canvas: a device plus the scene kept between frames.
pub struct Canvas {
    pub gpu: Gpu,
    images: Images,
    kept: Option<Kept>,
    frame: Scene,
}

impl Canvas {
    pub fn new() -> Result<Self, String> {
        Ok(Self { gpu: Gpu::new()?, images: Images::default(), kept: None, frame: Scene::new() })
    }

    /// Drop the kept scene (the next frame builds it from scratch).
    pub fn forget(&mut self) {
        self.kept = None;
    }

    /// Render `view` of `page` on the GPU (submitted, not read back), or say why it can't.
    pub fn draw(&mut self, doc: &Arc<Document>, page: NodeId, view: &View, opts: &Options) -> Result<(), Fallback> {
        if opts.outline_mode || !opts.outlined.is_empty() {
            return Err("outline view");
        }
        let base = opts
            .background
            .or_else(|| match doc.node(page).map(|p| &p.kind) {
                Some(NodeKind::Page { background, .. }) => Some(*background),
                _ => None,
            })
            .unwrap_or(figcraft_color::Color::TRANSPARENT);
        self.frame.reset();
        let xf = view.transform();
        if opts.solo.is_some() {
            let mut sc = Scene::new();
            Builder { doc, skip: &opts.skip, images: &mut self.images }.page(&mut sc, page, opts.solo)?;
            self.frame.append(&sc, Some(xf));
        } else {
            self.update(doc, page, &opts.skip)?;
            // Only the top-level layers in view go to the GPU.
            let shown = view.page_rect();
            for (sc, at) in self.kept.iter().flat_map(|k| k.parts.iter()).filter_map(|(_, p)| p.as_ref().ok()) {
                if at.is_none_or(|r| r.overlaps(shown)) {
                    self.frame.append(sc, Some(xf));
                }
            }
        }
        self.gpu.render(&self.frame, view.width, view.height, base)
    }

    /// [`Self::draw`], then read the image back.
    pub fn render(&mut self, doc: &Arc<Document>, page: NodeId, view: &View, opts: &Options) -> Result<Pixmap, Fallback> {
        self.draw(doc, page, view, opts)?;
        self.gpu.read().ok_or("GPU readback failed")
    }

    /// The kept page scene brought up to date with `doc`.
    fn update(&mut self, doc: &Arc<Document>, page: NodeId, skip: &[NodeId]) -> Result<(), Fallback> {
        let fonts = figcraft_text::layout_generation();
        let old = self.kept.take().filter(|k| k.page == page && k.skip == skip && k.fonts == fonts && k.doc.color_profile == doc.color_profile);
        let kept = match old {
            Some(k) if Arc::ptr_eq(&k.doc, doc) => k,
            old => {
                // Top-level layers to rebuild; `None`: all of them.
                let dirty = old.as_ref().and_then(|k| dirty_tops(&k.doc, doc, page));
                let mut reuse: HashMap<NodeId, Part> = match (old, &dirty) {
                    (Some(k), Some(_)) => k.parts.into_iter().collect(),
                    _ => HashMap::new(),
                };
                let mut b = Builder { doc, skip, images: &mut self.images };
                let kids = doc.node(page).map(|p| p.children.as_slice()).unwrap_or_default();
                let parts = kids
                    .iter()
                    .map(|c| {
                        let cached = if dirty.as_ref().is_some_and(|d| !d.contains(c)) { reuse.remove(c) } else { None };
                        let part = cached.unwrap_or_else(|| {
                            let mut sc = Scene::new();
                            // A page unit of margin for anti-aliasing.
                            b.node(&mut sc, *c, figcraft_geom::Affine::IDENTITY, 0)
                                .map(|()| (sc, figcraft_render::paint_bounds(doc, *c).map(|r| r.inflate(1.0, 1.0))))
                        });
                        (*c, part)
                    })
                    .collect();
                Kept { doc: doc.clone(), page, skip: skip.to_vec(), fonts, parts }
            }
        };
        let kept = self.kept.insert(kept);
        if let Some((_, Err(why))) = kept.parts.iter().find(|(_, p)| p.is_err()) {
            return Err(why);
        }
        Ok(())
    }
}

/// The top-level layers of `page` an edit from `old` to `new` touched; `None` when the whole page
/// scene has to be rebuilt (the page itself changed, or too much did).
fn dirty_tops(old: &Document, new: &Document, page: NodeId) -> Option<HashSet<NodeId>> {
    let mut out = HashSet::new();
    for id in new.changed_ids(old, MAX_CHANGED)? {
        if id == page {
            return None;
        }
        for d in [old, new] {
            if d.page_of(id) != Some(page) {
                continue;
            }
            // The ancestor right under the page.
            let mut top = id;
            for a in d.ancestors(id) {
                if a == page {
                    break;
                }
                top = a;
            }
            out.insert(top);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
