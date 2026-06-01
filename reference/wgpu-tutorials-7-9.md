# wgpu Tutorials 7-9: Takeaways for a 2D Draw Engine

Source: sotrh learn-wgpu beginner tutorials 7–9 (instancing, depth, models).

## Instancing (the core batching mechanism)
- **One draw call per primitive type**: batch all rects in one `draw_indexed(0..6, 0, 0..N)` where N = instance count. Same for circles, textures. The `0..N` instance range is what multiplies the unit quad.
- **Two-struct pattern**: a logical `Instance` (ergonomic CPU side) and a `#[repr(C)] InstanceRaw` (GPU-ready, `bytemuck::Pod + Zeroable`). Build `Vec<InstanceRaw>`, upload via `create_buffer_init` with `BufferUsages::VERTEX` (or `VERTEX | COPY_DST` if you re-upload each frame).
- **Separate vertex buffer at slot 1**: pipeline gets `buffers: &[Quad::desc(), InstanceRaw::desc()]`. Bind unit-quad verts at slot 0, instance buffer at slot 1 via two `set_vertex_buffer` calls.
- **`step_mode: VertexStepMode::Instance`** on the instance layout — GPU advances instance data once per instance, not per vertex. `array_stride = size_of::<InstanceRaw>()`.
- **Attribute locations continue past vertex attrs**: tutorial uses locations 5-8 for a mat4 (four `Float32x4` rows, since attributes are max 16 bytes). For 2D you likely don't need a full mat4.
- **Recommended per-instance data for 2D** (pack into a tight `InstanceRaw`):
  - `pos: [f32;2]`, `size: [f32;2]` (or a 2D affine: `[f32;4]` = 2x2 + translation in a separate vec2) — covers position, scale, **rotation** (encode rotation in the 2x2 matrix, or store `rotation: f32` and build it in the shader).
  - `color: [f32;4]` (tint / fill).
  - `uv_rect: [f32;4]` (x, y, w, h) — atlas sub-region for `draw_tex`/`draw_text`; shader maps unit-quad UV into this rect. Lets all textures from one atlas batch together.
  - `z: f32` (depth/ordering, see below) and optional `radius`/`corner` params for circles/rounded rects.
  - Vertex shader reassembles transform from instance attributes (mirror of the tutorial's `mat4x4(row0,row1,...)`).
- **Batching boundary = bind group switches**: instances sharing the same texture-atlas bind group and pipeline batch into one call. Different atlas = new batch. Minimize atlases to maximize batching.

## Depth buffer (z-ordering for 2D)
- **Two valid strategies**:
  1. **Painter's algorithm** (no depth buffer): execute queued draws in insertion order; later draws paint over earlier. Simplest for a 2D engine; ordering is implicit in the queue. But forces strict ordering and breaks if you reorder draws to batch by texture.
  2. **Depth test** (`Depth32Float`): assign each draw an increasing `z`, let GPU sort per-pixel. **This decouples draw/submission order from visual order**, so you can freely reorder/batch draws by pipeline/atlas without breaking layering — valuable for a batched engine.
- **Recommendation**: use a depth buffer if you batch aggressively (you will). Assign monotonically increasing z per `draw_*` call so call order still defines layering, but GPU handles correctness regardless of batch reordering.
- **Setup specifics**:
  - `const DEPTH_FORMAT = TextureFormat::Depth32Float;`
  - Depth texture: same size as surface config, usage `RENDER_ATTACHMENT` (add `TEXTURE_BINDING` only if you'll sample it).
  - Pipeline: `depth_stencil: Some(DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(true), depth_compare: Some(CompareFunction::Less), stencil: default(), bias: default() })`.
  - Render pass: add `depth_stencil_attachment` with `depth_ops: Operations { load: LoadOp::Clear(1.0), store: Store }`.
- **`display()` implications**:
  - Clear depth to 1.0 each frame alongside the color clear.
  - **Recreate depth texture in `resize()` AFTER reconfiguring the surface** (dimension-mismatch crash otherwise).
  - **Alpha caveat**: depth test + alpha blending is order-dependent for transparent pixels. With translucent fills/text, you may still need painter's ordering for transparent draws, or draw opaque-first then translucent back-to-front. Pure painter's algorithm sidesteps this.

## Resource management (transferable from model loading)
- **Index-not-embed**: tutorial's `Mesh.material: usize` indexes a shared `Vec<Material>`. Mirror this: store textures/atlases centrally, have each queued draw hold a lightweight **handle/index** into a resource table, not an owned texture. Enables batching draws that share a handle.
- **Material = texture + bind group together**: cache the `BindGroup` alongside each atlas/texture so `draw_tex` doesn't rebuild bind groups per call. Bind-group creation is the expensive part — do it at load time.
- **`Vertex` trait with `desc()`**: centralize each primitive's `VertexBufferLayout` behind a trait/`desc()` fn rather than hardcoding — clean way to support rect/circle/line/tex vertex+instance layouts uniformly.
- **`DrawModel`-style extension trait**: the tutorial extends `RenderPass` with `draw_*` helpers that bind the right bind group then issue the instanced draw. Good pattern for your internal flush: one helper per primitive that sets pipeline + atlas bind group + instance buffer, then one instanced draw.
- **Atlas grouping**: pack glyphs (text) and small images into shared atlases so `draw_text`/`draw_tex` resolve to the same bind group → one batched call. The `uv_rect` per-instance field is what makes this work.

## Gotchas
- **Instance layout**: each vertex attribute ≤ 16 bytes, so a mat4 needs 4 separate `Float32x4` slots; keep `shader_location`s sequential and non-colliding with vertex attrs. Ensure `#[repr(C)]` + `bytemuck` and that struct field order exactly matches the attribute offsets.
- **Instance buffer sizing**: if instance count grows per frame, either reallocate the buffer when it grows or pre-size to a max; `BufferUsages` must include `COPY_DST` for per-frame `queue.write_buffer`.
- **Depth format consistency**: pipeline `depth_stencil.format` must equal the depth texture's format and every pipeline used in the pass must declare the same depth-stencil state (or all `None`).
- **Depth clear value 1.0** (far plane) with `CompareFunction::Less` — getting these inverted makes everything fail the test or draw in reverse.
- **Resize order**: surface reconfigure → recreate depth texture. Stale depth view = validation error / crash.
- **Resource lifetime**: textures/bind groups referenced by a render pass must outlive the pass; with a deferred draw queue, keep resources in a central store that lives across `display()`, and have draw commands hold handles, not borrows.
