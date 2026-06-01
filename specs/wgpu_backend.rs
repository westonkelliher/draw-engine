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
//! back-to-front, this backend can simply paint in order. A depth buffer is
//! OPTIONAL here (paint order already yields correct layering, and per-draw
//! submission means no reordering happens). `DrawCall::z` may be ignored.

use crate::draw_layer::{DrawCall, Rect, TexHandle, FontHandle, ShaderHandle};

/// Owns surface/device/queue/config, one cached RenderPipeline per primitive
/// kind (shape-fill, shape-stroke, tex, text, user-shader), the bind group
/// layouts, and the resource tables (textures, fonts/glyph atlases, shaders).
pub struct WgpuBackend { /* surface, device, queue, config, pipelines, resources */ }

impl WgpuBackend {
    /// Create the backend for a window. Async: requests adapter+device,
    /// configures the surface (sRGB, Fifo/vsync, alpha blending), and builds &
    /// caches every pipeline + bind group layout ONCE. Call on window resume.
    pub async fn new(window: std::sync::Arc<winit::window::Window>) -> Self { unimplemented!() }

    /// Reconfigure the surface (and depth texture, if used) after a window
    /// resize. No-op when minimized (zero size). Run before the next render.
    pub fn resize(&mut self, width: u32, height: u32) { unimplemented!() }

    /// Background clear color used at the start of each frame.
    pub fn set_clear_color(&mut self, color: crate::draw_layer::Color) { unimplemented!() }

    /// Current drawable size in pixels.
    pub fn size(&self) -> (u32, u32) { unimplemented!() }

    // --- resource loading (do once at load time; returns cheap handles) ---

    /// Upload an RGBA8 image; returns a handle usable in `create_tex`. Builds the
    /// texture + sampler + bind group up front. Never call per frame.
    pub fn load_texture(&mut self, rgba: &[u8], width: u32, height: u32) -> TexHandle { unimplemented!() }

    /// Load a font and build its glyph atlas; returns a handle for `create_text`.
    /// The backend owns glyph layout (the draw layer passes strings through).
    pub fn load_font(&mut self, ttf_bytes: &[u8]) -> FontHandle { unimplemented!() }

    /// Compile a user WGSL fragment shader and cache its pipeline (keyed by
    /// source); returns a handle for `create_shader`. Compilation is expensive —
    /// call once up front.
    pub fn load_shader(&mut self, wgsl_source: &str) -> ShaderHandle { unimplemented!() }

    /// Measure a string without drawing (pixel width/height), for layout. Needs
    /// the font metrics this layer owns.
    pub fn measure_text(&self, font: FontHandle, text: &str, size_px: f32) -> crate::draw_layer::Vec2 { unimplemented!() }

    // --- the frame ---

    /// Render one frame from a flat, pre-sorted draw list and present it.
    ///
    /// Steps:
    ///   1. Acquire the surface texture + view (reconfigure & skip on Lost/Outdated).
    ///   2. Update the per-frame screen-size uniform (maps pixel coords → NDC) and
    ///      any time uniform.
    ///   3. Open one command encoder + one render pass (clear color).
    ///   4. For each `DrawCall` in order: set the matching cached pipeline, set
    ///      its bind group(s) (screen-size uniform; per-call texture/atlas/params),
    ///      upload its small geometry/uniforms, and issue a single `draw`. Text is
    ///      laid out into glyph quads here, one draw per call (or per glyph).
    ///   5. Submit the encoder; `present()` (the framebuffer swap).
    ///
    /// `draws` is consumed in the given order — that order IS the z-ordering.
    pub fn render(&mut self, draws: &[DrawCall]) { unimplemented!() }
}
