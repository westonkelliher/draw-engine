//! wgpu backend layer (NO batcher).
//!
//! Consumes the `Vec<DrawCall>` produced by `render_to_draws` and executes it on
//! the GPU. This version is deliberately simple: it issues **one GPU draw per
//! DrawCall**, in the order given (painter's algorithm). No instancing, no
//! batching, no atlas packing. Correctness over performance.
//!
//! It owns all GPU state and all real resources; the draw layer only holds the
//! handles this layer hands out (`TexHandle` / `FontHandle` / `ShaderHandle`).
//!
//! Z-ordering: because `render_to_draws` returns the calls already sorted
//! back-to-front, this backend simply paints in order. No depth buffer is used
//! (paint order already yields correct layering). `DrawCall::z` is ignored.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::draw_layer::{
    Color, DrawCall, DrawStyle, FontHandle, Shape, ShaderHandle, TexHandle, Vec2,
};

// ---------------------------------------------------------------------------
// Per-frame and per-draw uniform layouts (std140-friendly: vec2 pairs / vec4s).
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ScreenUniform {
    res: [f32; 2],
    time: f32,
    _pad: f32,
}

/// 32 floats = 128 bytes; shared layout used by shape, tex and shader pipelines
/// so a single per-draw uniform buffer + dynamic offset works for all of them.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniform {
    mx: [f32; 2],
    my: [f32; 2],
    t: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
    // shape: [kind, style, radius/corner, thickness]; tex: src uv rect; shader: rect
    a: [f32; 4],
    b: [f32; 4],
}

impl DrawUniform {
    const SIZE: u64 = std::mem::size_of::<DrawUniform>() as u64;
}

// ---------------------------------------------------------------------------
// Owned GPU resources.
// ---------------------------------------------------------------------------

struct TextureRes {
    bind_group: wgpu::BindGroup,
}

struct ShaderRes {
    pipeline: wgpu::RenderPipeline,
}

struct FontRes {
    /// font id within glyphon's font database (cosmic-text fontdb).
    id: glyphon::fontdb::ID,
}

pub struct WgpuBackend {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    window: Arc<winit::window::Window>,

    clear_color: wgpu::Color,
    start: Instant,
    uniform_align: u64,

    // bind group layouts
    screen_bgl: wgpu::BindGroupLayout,
    draw_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,

    // cached pipelines
    shape_pipeline: wgpu::RenderPipeline,
    tex_pipeline: wgpu::RenderPipeline,

    // per-frame uniform resources
    screen_buffer: wgpu::Buffer,
    screen_bind_group: wgpu::BindGroup,

    // a growable per-draw uniform buffer + its dynamic-offset bind group
    draw_buffer: wgpu::Buffer,
    draw_buffer_cap: u64,
    draw_bind_group: wgpu::BindGroup,

    // resource tables
    textures: Vec<TextureRes>,
    shaders: Vec<ShaderRes>,
    shader_cache: HashMap<String, u32>,
    fonts: Vec<FontRes>,

    // text rendering (glyphon)
    font_system: glyphon::FontSystem,
    swash_cache: glyphon::SwashCache,
    // kept alive: shared by viewport/atlas/text_renderer internally.
    _glyphon_cache: glyphon::Cache,
    viewport: glyphon::Viewport,
    atlas: glyphon::TextAtlas,
    text_renderer: glyphon::TextRenderer,
}

impl WgpuBackend {
    pub async fn new(window: Arc<winit::window::Window>) -> Self {
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("request adapter");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("draw_engine device"),
                ..Default::default()
            })
            .await
            .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let uniform_align = device.limits().min_uniform_buffer_offset_alignment as u64;

        // --- bind group layouts ---
        let screen_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(DrawUniform::SIZE),
                },
                count: None,
            }],
        });

        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tex bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // --- pipelines ---
        let blend = wgpu::BlendState::ALPHA_BLENDING;

        let shape_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shape.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shape.wgsl").into()),
        });
        let shape_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shape layout"),
            bind_group_layouts: &[Some(&screen_bgl), Some(&draw_bgl)],
            immediate_size: 0,
        });
        let shape_pipeline =
            Self::make_pipeline(&device, &shape_layout, &shape_shader, format, blend);

        let tex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tex.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/tex.wgsl").into()),
        });
        let tex_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tex layout"),
            bind_group_layouts: &[Some(&screen_bgl), Some(&draw_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let tex_pipeline = Self::make_pipeline(&device, &tex_layout, &tex_shader, format, blend);

        // --- per-frame screen uniform ---
        let screen_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screen uniform"),
            size: std::mem::size_of::<ScreenUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("screen bg"),
            layout: &screen_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buffer.as_entire_binding(),
            }],
        });

        // --- per-draw uniform buffer (initial capacity; grows as needed) ---
        let draw_buffer_cap = (uniform_align.max(DrawUniform::SIZE)) * 256;
        let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("draw uniform"),
            size: draw_buffer_cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draw_bind_group = Self::make_draw_bind_group(&device, &draw_bgl, &draw_buffer);

        // --- glyphon text stack ---
        let font_system = glyphon::FontSystem::new();
        let swash_cache = glyphon::SwashCache::new();
        let glyphon_cache = glyphon::Cache::new(&device);
        let viewport = glyphon::Viewport::new(&device, &glyphon_cache);
        let mut atlas = glyphon::TextAtlas::new(&device, &queue, &glyphon_cache, format);
        let text_renderer =
            glyphon::TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Self {
            surface,
            device,
            queue,
            config,
            window,
            clear_color: wgpu::Color {
                r: 0.05,
                g: 0.05,
                b: 0.08,
                a: 1.0,
            },
            start: Instant::now(),
            uniform_align,
            screen_bgl,
            draw_bgl,
            tex_bgl,
            shape_pipeline,
            tex_pipeline,
            screen_buffer,
            screen_bind_group,
            draw_buffer,
            draw_buffer_cap,
            draw_bind_group,
            textures: Vec::new(),
            shaders: Vec::new(),
            shader_cache: HashMap::new(),
            fonts: Vec::new(),
            font_system,
            swash_cache,
            _glyphon_cache: glyphon_cache,
            viewport,
            atlas,
            text_renderer,
        }
    }

    fn make_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        blend: wgpu::BlendState,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pipeline"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    fn make_draw_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draw bg"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(DrawUniform::SIZE),
                }),
            }],
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_clear_color(&mut self, color: Color) {
        self.clear_color = wgpu::Color {
            r: color.r as f64,
            g: color.g as f64,
            b: color.b as f64,
            a: color.a as f64,
        };
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    // --- resource loading ---

    pub fn load_texture(&mut self, rgba: &[u8], width: u32, height: u32) -> TexHandle {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("user texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tex sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tex bg"),
            layout: &self.tex_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let idx = self.textures.len() as u32;
        self.textures.push(TextureRes { bind_group });
        TexHandle(idx)
    }

    pub fn load_font(&mut self, ttf_bytes: &[u8]) -> FontHandle {
        let ids = self
            .font_system
            .db_mut()
            .load_font_source(glyphon::fontdb::Source::Binary(Arc::new(
                ttf_bytes.to_vec(),
            )));
        let id = ids.into_iter().next().expect("font contained no faces");
        let idx = self.fonts.len() as u32;
        self.fonts.push(FontRes { id });
        FontHandle(idx)
    }

    pub fn load_shader(&mut self, wgsl_source: &str) -> ShaderHandle {
        if let Some(&idx) = self.shader_cache.get(wgsl_source) {
            return ShaderHandle(idx);
        }
        let prelude = include_str!("shaders/shader_prelude.wgsl");
        let full = format!("{prelude}\n{wgsl_source}");
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("user shader"),
            source: wgpu::ShaderSource::Wgsl(full.into()),
        });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("user shader layout"),
                bind_group_layouts: &[Some(&self.screen_bgl), Some(&self.draw_bgl)],
                immediate_size: 0,
            });
        let pipeline = Self::make_pipeline(
            &self.device,
            &layout,
            &module,
            self.config.format,
            wgpu::BlendState::ALPHA_BLENDING,
        );
        let idx = self.shaders.len() as u32;
        self.shaders.push(ShaderRes { pipeline });
        self.shader_cache.insert(wgsl_source.to_string(), idx);
        ShaderHandle(idx)
    }

    pub fn measure_text(&self, font: FontHandle, text: &str, size_px: f32) -> Vec2 {
        // FontSystem mutation is required by cosmic-text shaping; do it on a clone
        // of the shared db so `&self` stays honest.
        let mut fs = glyphon::FontSystem::new_with_locale_and_db(
            self.font_system.locale().to_string(),
            self.font_system.db().clone(),
        );
        let mut buffer =
            glyphon::Buffer::new(&mut fs, glyphon::Metrics::new(size_px, size_px * 1.25));
        let attrs = self.attrs_for(font);
        buffer.set_text(&mut fs, text, &attrs, glyphon::Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut fs, false);
        measure_buffer(&buffer)
    }

    /// Build cosmic-text `Attrs` that pin to the loaded font face (by id) when
    /// available, otherwise fall back to a sans-serif family.
    fn attrs_for(&self, font: FontHandle) -> glyphon::Attrs<'static> {
        if let Some(res) = self.fonts.get(font.0 as usize) {
            if let Some(face) = self.font_system.db().face(res.id) {
                // leak a stable family name string for 'static attrs lifetime
                let name: &'static str =
                    Box::leak(face.families[0].0.clone().into_boxed_str());
                return glyphon::Attrs::new().family(glyphon::Family::Name(name));
            }
        }
        glyphon::Attrs::new().family(glyphon::Family::SansSerif)
    }

    // --- the frame ---

    pub fn render(&mut self, draws: &[DrawCall]) {
        let (w, h) = (self.config.width, self.config.height);
        if w == 0 || h == 0 {
            return;
        }

        // 1. update screen uniform
        let time = self.start.elapsed().as_secs_f32();
        let screen = ScreenUniform {
            res: [w as f32, h as f32],
            time,
            _pad: 0.0,
        };
        self.queue
            .write_buffer(&self.screen_buffer, 0, bytemuck::bytes_of(&screen));

        // 2. build the per-draw uniform array; record the kind & dynamic offset
        //    for each non-text draw so we can play them back in the render pass.
        let stride = align_up(DrawUniform::SIZE, self.uniform_align);
        enum Op {
            Shape { offset: u64 },
            Tex { offset: u64, tex: u32 },
            Shader { offset: u64, shader: u32 },
        }
        let mut ops: Vec<Op> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();

        let push = |u: &DrawUniform, bytes: &mut Vec<u8>| -> u64 {
            let offset = bytes.len() as u64;
            bytes.extend_from_slice(bytemuck::bytes_of(u));
            bytes.resize(align_up(bytes.len() as u64, stride) as usize, 0);
            offset
        };

        // text areas are gathered separately for one glyphon prepare/render.
        let mut text_buffers: Vec<(glyphon::Buffer, [f32; 2], Color)> = Vec::new();

        for call in draws {
            match call {
                DrawCall::Shape {
                    transform,
                    shape,
                    style,
                    color,
                    ..
                } => {
                    let (kind, radius, size) = match shape {
                        Shape::Rect {
                            size,
                            corner_radius,
                        } => (0.0_f32, *corner_radius, [size.x, size.y]),
                        Shape::Circ { radius } => {
                            (1.0_f32, *radius, [radius * 2.0, radius * 2.0])
                        }
                    };
                    let (style_f, thickness) = match style {
                        DrawStyle::Fill => (0.0_f32, 0.0_f32),
                        DrawStyle::Stroke { thickness } => (1.0_f32, *thickness),
                    };
                    // For circle the quad's local origin must be the top-left of
                    // the bounding box; the baked transform's translation refers
                    // to the circle center, so shift back by radius.
                    let t = if matches!(shape, Shape::Circ { .. }) {
                        offset_translation(transform, -radius, -radius)
                    } else {
                        [transform.t[0], transform.t[1]]
                    };
                    let u = DrawUniform {
                        mx: [transform.m[0][0], transform.m[1][0]],
                        my: [transform.m[0][1], transform.m[1][1]],
                        t,
                        size,
                        color: [color.r, color.g, color.b, color.a],
                        a: [kind, style_f, radius, thickness],
                        b: [0.0; 4],
                    };
                    let offset = push(&u, &mut bytes);
                    ops.push(Op::Shape { offset });
                }
                DrawCall::Tex {
                    transform,
                    size,
                    tex,
                    src,
                    tint,
                    ..
                } => {
                    let u = DrawUniform {
                        mx: [transform.m[0][0], transform.m[1][0]],
                        my: [transform.m[0][1], transform.m[1][1]],
                        t: [transform.t[0], transform.t[1]],
                        size: [size.x, size.y],
                        color: [tint.r, tint.g, tint.b, tint.a],
                        a: [src.x, src.y, src.w, src.h],
                        b: [0.0; 4],
                    };
                    let offset = push(&u, &mut bytes);
                    ops.push(Op::Tex {
                        offset,
                        tex: tex.0,
                    });
                }
                DrawCall::Shad {
                    transform,
                    size,
                    shader,
                    params,
                    ..
                } => {
                    let p = params_to_vec4(params);
                    let u = DrawUniform {
                        mx: [transform.m[0][0], transform.m[1][0]],
                        my: [transform.m[0][1], transform.m[1][1]],
                        t: [transform.t[0], transform.t[1]],
                        size: [size.x, size.y],
                        color: [0.0; 4],
                        a: [transform.t[0], transform.t[1], size.x, size.y], // dest rect
                        b: p,
                    };
                    let offset = push(&u, &mut bytes);
                    ops.push(Op::Shader {
                        offset,
                        shader: shader.0,
                    });
                }
                DrawCall::Writ {
                    transform,
                    font,
                    text,
                    size_px,
                    color,
                    ..
                } => {
                    let mut buffer = glyphon::Buffer::new(
                        &mut self.font_system,
                        glyphon::Metrics::new(*size_px, *size_px * 1.25),
                    );
                    let attrs = self.attrs_for(*font);
                    buffer.set_text(
                        &mut self.font_system,
                        text,
                        &attrs,
                        glyphon::Shaping::Advanced,
                        None,
                    );
                    buffer.shape_until_scroll(&mut self.font_system, false);
                    text_buffers.push((buffer, [transform.t[0], transform.t[1]], *color));
                }
            }
        }

        // 3. ensure the draw uniform buffer is large enough, then upload.
        if !bytes.is_empty() {
            if bytes.len() as u64 > self.draw_buffer_cap {
                let new_cap = (bytes.len() as u64).next_power_of_two();
                self.draw_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("draw uniform"),
                    size: new_cap,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.draw_buffer_cap = new_cap;
                self.draw_bind_group =
                    Self::make_draw_bind_group(&self.device, &self.draw_bgl, &self.draw_buffer);
            }
            self.queue.write_buffer(&self.draw_buffer, 0, &bytes);
        }

        // 4. prepare text (glyphon) for the same pass.
        self.viewport.update(
            &self.queue,
            glyphon::Resolution {
                width: w,
                height: h,
            },
        );
        let text_areas: Vec<glyphon::TextArea> = text_buffers
            .iter()
            .map(|(buf, pos, color)| glyphon::TextArea {
                buffer: buf,
                left: pos[0],
                top: pos[1],
                scale: 1.0,
                bounds: glyphon::TextBounds::default(),
                default_color: glyphon::Color::rgba(
                    (color.r * 255.0) as u8,
                    (color.g * 255.0) as u8,
                    (color.b * 255.0) as u8,
                    (color.a * 255.0) as u8,
                ),
                custom_glyphs: &[],
            })
            .collect();
        let _ = self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            text_areas,
            &mut self.swash_cache,
        );

        // 5. acquire surface and record the pass.
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return;
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => panic!("surface validation error"),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_bind_group(0, &self.screen_bind_group, &[]);
            for op in &ops {
                match op {
                    Op::Shape { offset } => {
                        pass.set_pipeline(&self.shape_pipeline);
                        pass.set_bind_group(1, &self.draw_bind_group, &[*offset as u32]);
                        pass.draw(0..6, 0..1);
                    }
                    Op::Tex { offset, tex } => {
                        if let Some(t) = self.textures.get(*tex as usize) {
                            pass.set_pipeline(&self.tex_pipeline);
                            pass.set_bind_group(1, &self.draw_bind_group, &[*offset as u32]);
                            pass.set_bind_group(2, &t.bind_group, &[]);
                            pass.draw(0..6, 0..1);
                        }
                    }
                    Op::Shader { offset, shader } => {
                        if let Some(s) = self.shaders.get(*shader as usize) {
                            pass.set_pipeline(&s.pipeline);
                            pass.set_bind_group(1, &self.draw_bind_group, &[*offset as u32]);
                            pass.draw(0..6, 0..1);
                        }
                    }
                }
            }

            // text last (within this pass)
            let _ = self
                .text_renderer
                .render(&self.atlas, &self.viewport, &mut pass);
        }

        self.queue.submit(Some(encoder.finish()));
        self.window.pre_present_notify();
        frame.present();
        self.atlas.trim();
    }
}

fn align_up(v: u64, align: u64) -> u64 {
    if align == 0 {
        return v;
    }
    v.div_ceil(align) * align
}

fn offset_translation(a: &crate::draw_layer::Affine2, dx: f32, dy: f32) -> [f32; 2] {
    // world translation of a local point (dx,dy) offset: t + M * (dx,dy)
    [
        a.t[0] + a.m[0][0] * dx + a.m[0][1] * dy,
        a.t[1] + a.m[1][0] * dx + a.m[1][1] * dy,
    ]
}

fn params_to_vec4(params: &[u8]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for (i, chunk) in params.chunks(4).take(4).enumerate() {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        out[i] = f32::from_le_bytes(b);
    }
    out
}

fn measure_buffer(buffer: &glyphon::Buffer) -> Vec2 {
    let mut w = 0.0f32;
    let mut h = 0.0f32;
    for run in buffer.layout_runs() {
        w = w.max(run.line_w);
        h += run.line_height;
    }
    Vec2 { x: w, y: h }
}
