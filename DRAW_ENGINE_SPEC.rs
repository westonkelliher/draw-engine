//! Draw Engine — interface specification (wgpu-backed 2D draw engine).
//!
//! Model: draw_* calls QUEUE retained commands; `display()` flushes them in one
//! render pass, submits, and presents. All coordinates are in PIXELS (origin
//! top-left, +y down), bridged to NDC by a per-frame screen-size uniform.
//! Layering: each draw_* gets a monotonically increasing z; a depth buffer
//! decouples draw order from batch order so commands can be reordered by
//! pipeline/atlas for batching. Pipelines, layouts, and atlas bind groups are
//! built once at init and cached; nothing expensive happens per draw call.

use wgpu;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// RGBA color, linear 0.0..=1.0 components.
pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }

/// Pixel-space point (origin top-left, +y down).
pub struct Vec2 { pub x: f32, pub y: f32 }

/// Pixel-space axis-aligned rectangle.
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

/// Opaque handle into the engine's texture/atlas table. Cheap to copy; draws
/// store handles, not borrows, so resources outlive the deferred queue.
pub struct TexHandle(u32);

/// Opaque handle to a compiled+cached user fragment shader (see `load_shader`).
pub struct ShaderHandle(u32);

/// Opaque handle to a loaded font (glyph atlas).
pub struct FontHandle(u32);

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

pub struct DrawEngine { /* surface, device, queue, config, pipelines, atlases, depth, queued cmds */ }

impl DrawEngine {
    /// Create the engine for a window. Async: requests adapter+device, configures
    /// the surface (sRGB, Fifo/vsync), and builds & caches every pipeline, bind
    /// group layout, and the depth texture. Call once on window resume.
    pub async fn new(window: std::sync::Arc<winit::window::Window>) -> Self { unimplemented!() }

    /// Reconfigure the surface and recreate the depth texture after a window
    /// resize. No-op when minimized (zero size). Must run before the next display().
    pub fn resize(&mut self, width: u32, height: u32) { unimplemented!() }

    // -----------------------------------------------------------------------
    // Draw commands (queue only; cheap; record in pixel space)
    // -----------------------------------------------------------------------

    /// Queue a filled circle centered at `center` with `radius` (pixels), filled
    /// with `color`. Rendered as an instanced quad with an SDF fragment (no fan
    /// geometry), so it stays crisp at any size.
    pub fn draw_circle(&mut self, center: Vec2, radius: f32, color: Color) { unimplemented!() }

    /// Queue a filled axis-aligned rectangle. `corner_radius` rounds corners
    /// (0.0 = sharp). Batched with all other rects into one instanced draw.
    pub fn draw_rect(&mut self, rect: Rect, color: Color, corner_radius: f32) { unimplemented!() }

    /// Queue a line segment from `a` to `b` of the given `thickness` (pixels).
    /// Expanded CPU-side to a thickness-quad; batched with rects/shapes.
    pub fn draw_line(&mut self, a: Vec2, b: Vec2, thickness: f32, color: Color) { unimplemented!() }

    /// Queue an image texture drawn into `dest` (pixel rect). `src` selects a
    /// sub-region in 0..1 UV space (use the full 0,0,1,1 for the whole image);
    /// `tint` multiplies the sampled color (use white for none). Draws sharing
    /// an atlas batch into a single instanced call via per-instance uv_rect.
    pub fn draw_tex(&mut self, tex: TexHandle, dest: Rect, src: Rect, tint: Color) { unimplemented!() }

    /// Queue a text string. `pos` is the top-left baseline origin (pixels),
    /// `size_px` the pixel height, `color` the fill. Glyphs come from the font's
    /// atlas, so an entire string is one batched instanced draw.
    pub fn draw_text(&mut self, font: FontHandle, text: &str, pos: Vec2, size_px: f32, color: Color) { unimplemented!() }

    /// Queue a user fragment shader rendered over `dest` (pixel rect). The shader
    /// receives a uniform block { resolution: dest size, time, rect, mouse? } and
    /// the quad's local uv in 0..1. Use for effects/procedural fills at a given
    /// size and location. `params` is opaque bytes uploaded as an extra uniform.
    pub fn draw_shader(&mut self, shader: ShaderHandle, dest: Rect, params: &[u8]) { unimplemented!() }

    // -----------------------------------------------------------------------
    // Frame
    // -----------------------------------------------------------------------

    /// Execute all queued draws and swap framebuffers. Acquires the surface
    /// texture, updates the screen-size uniform, opens one command encoder and
    /// render pass (clearing color + depth), uploads batched geometry/instances,
    /// replays draws grouped by pipeline+atlas to minimize state switches,
    /// submits, presents, and clears the queue. Handles Lost/Outdated by
    /// reconfiguring and skipping the frame.
    pub fn display(&mut self) { unimplemented!() }

    // -----------------------------------------------------------------------
    // Resource management (load once; expensive; returns cheap handles)
    // -----------------------------------------------------------------------

    /// Upload an RGBA image and return a handle. Internally placed into a texture
    /// atlas where possible so subsequent draw_tex calls can batch. Do at load
    /// time, never per frame.
    pub fn load_texture(&mut self, rgba: &[u8], width: u32, height: u32) -> TexHandle { unimplemented!() }

    /// Load a font and build/grow its glyph atlas; returns a handle for draw_text.
    pub fn load_font(&mut self, ttf_bytes: &[u8]) -> FontHandle { unimplemented!() }

    /// Compile a user WGSL fragment shader and cache its pipeline (keyed by
    /// source). Returns a handle for draw_shader. Compilation is expensive, so
    /// call once up front, not per frame.
    pub fn load_shader(&mut self, wgsl_source: &str) -> ShaderHandle { unimplemented!() }

    /// Free a texture/atlas slot. The handle becomes invalid; in-flight frames
    /// must have completed (or the call defers reclamation) so the GPU resource
    /// outlives any pass referencing it.
    pub fn unload_texture(&mut self, tex: TexHandle) { unimplemented!() }

    // -----------------------------------------------------------------------
    // Optional but useful
    // -----------------------------------------------------------------------

    /// Set the background clear color used at the start of each display().
    pub fn set_clear_color(&mut self, color: Color) { unimplemented!() }

    /// Current drawable size in pixels (surface config size).
    pub fn size(&self) -> (u32, u32) { unimplemented!() }

    /// Measure a string without drawing it (pixel width/height), for layout.
    pub fn measure_text(&self, font: FontHandle, text: &str, size_px: f32) -> Vec2 { unimplemented!() }
}
