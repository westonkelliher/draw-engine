# wgpu Draw Engine — Tutorial Takeaways (Reference)

Source: sotrh learn-wgpu beginner tutorials 1–3 (window, surface, pipeline).

## Tutorial 1 — Window & Init Lifecycle
- **Objects**: `winit::EventLoop`, `Window`, `ApplicationHandler` trait, a `State` struct holding all engine resources.
- **Init flow**: Window can only be created in the `resumed` event → engine `State::new(window)` must run there. `State::new` is **async** (adapter/device requests are futures) → wrap in `pollster::block_on()` on desktop.
- **Render trigger**: winit only redraws on `RedrawRequested` or resize. Call `window.request_redraw()` to drive a continuous loop. Your `display()` lives behind `RedrawRequested`.
- **Gotchas**:
  - Surface lifetime is tied to the Window — keep Window alive for the whole `State` (use `Arc<Window>` to satisfy `Surface<'static>` lifetime).
  - Init `env_logger` early or wgpu errors are silent.

## Tutorial 2 — Surface, Render Pass, Present (the `display()` core)
- **Objects & purpose**:
  - `Instance` — entry point; creates `Surface` + enumerates `Adapter`s.
  - `Adapter` — handle to a physical GPU; used to create the Device.
  - `Device` — creates all resources (pipelines, buffers, textures, encoders).
  - `Queue` — submits command buffers; also `write_buffer`/`write_texture` for uploads.
  - `Surface` — the drawable window region; hands out per-frame textures.
  - `SurfaceConfiguration` — format (prefer sRGB), width/height (must be **non-zero**), `present_mode` (`Fifo` = vsync, universally supported), `usage: RENDER_ATTACHMENT`.
- **Per-frame loop (this is `display()`)**:
  1. `surface.get_current_texture()` → `SurfaceTexture` (or `SurfaceError`).
  2. `texture.create_view(...)` → the `TextureView` you render into.
  3. `device.create_command_encoder(...)`.
  4. `encoder.begin_render_pass(...)` with a `RenderPassColorAttachment` (set `LoadOp::Clear(color)` on the first pass to clear, `StoreOp::Store`).
  5. Record all queued draws inside the pass (each draw = `set_pipeline` + bind groups + `draw`).
  6. Drop the pass, `queue.submit([encoder.finish()])`.
  7. `output.present()` ← **the framebuffer swap**.
- **Resize / reconfigure**: on resize update `config.width/height` (guard against 0) and call `surface.configure(&device, &config)`. Skip rendering when minimized (zero size).
- **SurfaceError handling**:
  - `Lost` / `Outdated` → reconfigure surface, skip frame.
  - `OutOfMemory` → fatal, exit.
  - `Timeout` → skip frame.
- **Engine mapping**: queued `draw_*` calls accumulate state; `display()` opens one encoder/pass, replays them, submits, presents.

## Tutorial 3 — RenderPipeline & Shaders (foundation for every `draw_*`, esp. `draw_shader`)
- **Objects**:
  - `ShaderModule` — `device.create_shader_module(include_wgsl!("x.wgsl"))`; holds compiled WGSL.
  - `PipelineLayout` — declares bind group layouts (empty `&[]` when no uniforms/textures yet; you'll need entries for `draw_tex`/`draw_text`/`draw_shader` uniforms).
  - `RenderPipeline` — `device.create_render_pipeline(...)`: binds vertex+fragment entry points, layout, primitive state, and color target.
- **WGSL**: `@vertex` and `@fragment` entry points **must have different names** (newer spec). Fragment returns `@location(0)` color matching the surface format.
- **Pipeline config**: `PrimitiveTopology::TriangleList`, `FrontFace::Ccw`, optional `cull_mode: Back`. Color target = `ColorTargetState { format: config.format, blend: …, write_mask: ALL }`. Use `BlendState::REPLACE` for opaque; **use `BlendState::ALPHA_BLENDING`** for the engine (circles/text/textures with alpha).
- **draw_shader (fullscreen quad) pattern**:
  - No vertex buffer needed. Generate verts from `@builtin(vertex_index)` and `draw(0..3, 0..1)` — a single oversized triangle covering the viewport is the standard fullscreen trick (cheaper than a 6-vertex quad).
  - For `draw_shader` at a given size/location: either scale/offset the generated positions via a uniform, or draw a 2-triangle quad covering the target rect; pass user fragment shader + uniforms (resolution, time, rect) via a bind group.
  - Each `draw_*` primitive = its own pipeline (circle SDF, rect, line, textured quad, text-atlas quad, user shader).
- **Cost gotcha**: pipeline creation compiles shaders — **expensive**. Create all pipelines **once at init**, cache them, reuse every frame. Never build pipelines inside `display()`. For user-supplied `draw_shader` sources, compile+cache per unique shader (keyed by source).

### Design implications
- Init (once): Instance → Surface → Adapter → Device/Queue → configure Surface → build & cache all RenderPipelines + bind group layouts.
- Per `display()`: acquire frame texture → encoder → single (or batched) render pass → replay queued draws (each sets cached pipeline + per-draw bind group, issues `draw`) → submit → present.
- Keep one shared clear/load on the first pass; batch same-pipeline draws to minimize `set_pipeline` switches.
