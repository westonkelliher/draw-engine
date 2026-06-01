# wgpu Draw Engine — Reference Takeaways

Source: sotrh learn-wgpu beginner tutorials 4–6 (buffers, textures, uniforms).

## Vertex/Index Buffers (draw_rect / draw_line / draw_circle)
- **Vertex struct**: `#[repr(C)] #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)] struct Vertex { position: [f32;2 or 3], color/uv: [...] }`. `#[repr(C)]` is mandatory for GPU layout.
- **VertexBufferLayout**: `{ array_stride: size_of::<Vertex>(), step_mode: VertexStepMode::Vertex, attributes: &[...] }`. Each `VertexAttribute { offset, shader_location, format }`; offset = cumulative `size_of` of prior fields; format e.g. `Float32x2`, `Float32x3`.
- **Geometry generation (CPU-side)**:
  - `draw_rect`: 4 vertices + index buffer `[0,1,2, 0,2,3]` (2 tris).
  - `draw_line`: expand to a quad (rectangle) along the line direction with a thickness offset (`perp * width/2`), then same 4-vert/6-index pattern. Lines aren't a primitive worth using here.
  - `draw_circle`: triangle-fan — center vertex + N rim vertices; indices `[0,i,i+1]`. Pick N from radius. Alternatively draw a quad and do the circle in a fragment shader (cheaper geometry, see draw_shader).
- **Buffers**: `device.create_buffer_init(&BufferInitDescriptor { contents: bytemuck::cast_slice(&verts), usage: BufferUsages::VERTEX })` (needs `use wgpu::util::DeviceExt;`). Index buffer same with `BufferUsages::INDEX`, data `&[u16]`.
- **Render**: `set_vertex_buffer(0, buf.slice(..))`, `set_index_buffer(buf.slice(..), IndexFormat::Uint16)`, `draw_indexed(0..n, 0, 0..1)`. Only one index buffer bound at a time.
- **Shader linkage**: `shader_location` ↔ `@location(n)` in WGSL `VertexInput`.

## Textures / Samplers (draw_tex, draw_text)
- **Upload**: load → `to_rgba8()` → `create_texture(&TextureDescriptor { format: Rgba8UnormSrgb, usage: TEXTURE_BINDING | COPY_DST, ... })` → `queue.write_texture(... bytes_per_row: Some(4*width), rows_per_image: Some(height) ...)`.
- **View + Sampler**: `texture.create_view(default)`; `create_sampler(&SamplerDescriptor { address_mode_*: ClampToEdge, mag/min_filter: Linear (or Nearest for pixel-art/text atlas) })`.
- **BindGroupLayout** (2 entries, `visibility: FRAGMENT`): entry0 `BindingType::Texture { sample_type: Float{filterable:true}, view_dimension: D2 }`; entry1 `BindingType::Sampler(Filtering)`.
- **BindGroup**: pairs layout with `BindingResource::TextureView(&view)` and `BindingResource::Sampler(&sampler)`.
- **WGSL**: `@group(N) @binding(0) var t: texture_2d<f32>; @binding(1) var s: sampler;` → `textureSample(t, s, uv)`.
- **Tex coords**: origin `(0,0)` = top-left, `(1,1)` = bottom-right. Y is inverted vs. world-up; flip with `1 - y` if your world is Y-up.
- **draw_text**: same path — glyphs as a texture atlas; each glyph is a UV-subregion quad. Use `Nearest` or a dedicated text crate (e.g. glyphon) if available.

## Uniforms (camera/screen-size, draw_shader)
- **Struct**: `#[repr(C)] ... struct Uniform { ... }`, e.g. `view_proj: [[f32;4];4]` or `screen_size: [f32;2]`.
- **Screen-size uniform = pixel coords**: pass `[width, height]` so vertex shader maps pixel coords → NDC: `clip = vec4(pos.x/w*2-1, 1-pos.y/h*2, 0, 1)`. Lets all draw calls submit geometry in pixel space. Alternative: a 4x4 projection matrix uniform.
- **Buffer**: `create_buffer_init(usage: UNIFORM | COPY_DST)`. Update each frame via `queue.write_buffer(&buf, 0, bytemuck::cast_slice(&[uniform]))` (no staging buffer; cheap).
- **BindGroupLayout**: `BindingType::Buffer { ty: Uniform, has_dynamic_offset: false, min_binding_size: None }`, visibility VERTEX (and/or FRAGMENT).
- **draw_shader uniforms** (size/location/time): pack into one uniform struct visible to FRAGMENT; draw a quad sized/positioned via the screen-size mapping, fragment shader uses `time`/`resolution`/`uv` for the effect. Time and per-draw params change every call → use `queue.write_buffer` per draw, or dynamic offsets (`has_dynamic_offset: true`) to pack many draws in one buffer.
- **Group indexing**: `@group(0)`=textures, `@group(1)`=uniforms etc.; index = order in PipelineLayout's `bind_group_layouts`. `set_bind_group(i, &group, &[])` (3rd arg = dynamic offsets).

## Batching Implications
- **Forces a new draw call**: vertex/index buffer switch, bind-group switch, pipeline switch.
- **Forces a new pipeline**: different shader (circle vs rect vs custom shader vs textured), different blend state, different vertex layout, different primitive topology.
- **Forces a new bind group**: different texture (each `draw_tex` of a distinct image), or different per-draw uniform values (unless using dynamic offsets / instancing).
- **Batch wins**:
  - Group all solid-color shapes (rect/line/circle-as-geometry) into one vertex/index buffer + one pipeline → single `draw_indexed`.
  - Use a **texture atlas** so many `draw_tex`/`draw_text` calls share one bind group → one draw.
  - Use **instancing** (`step_mode: Instance`, instance buffer) for many identical quads with per-instance transform/color/uv-rect — collapses N draws into one.
  - Sort/queue draws by pipeline then bind group to minimize switches before flushing in `display()`.

## Gotchas
- **Alignment**: uniform structs must respect std140-ish rules — fields align to 16 bytes; a `vec3`/`[f32;3]` pads to 16. A trailing scalar (e.g. `time: f32`) after a `vec2` can misalign; add explicit `_pad` fields and keep total a multiple of 16. Matrices (`[[f32;4];4]`) are fine.
- **bytemuck/Pod**: every field must be Pod; padding fields included. `#[repr(C)]` required or layout is UB. Use `cast_slice(&[uniform])` (slice of one), not `cast_slice(&uniform)`.
- **Coordinate systems**: NDC is Y-up, X/Y in `[-1,1]`, Z in `[0,1]`. Texture UV is Y-down, top-left origin. Pixel space is your choice — bridge via the screen-size uniform. Be consistent or things flip.
- **Buffer creation cost**: `create_buffer_init` allocates GPU memory — do NOT create per-draw per-frame. Either (a) pre-allocate large dynamic buffers and `write_buffer` updated regions, or (b) accumulate all queued geometry CPU-side and upload once in `display()`.
- **Dynamic vs static**: static geometry (a glyph atlas quad, unit circle) → create once. Per-frame batched geometry → one growable buffer with `COPY_DST`, re-`write_buffer` each frame; only recreate when it must grow.
- **`bytes_per_row`** in `write_texture` must be a multiple of 256 for buffer-to-texture copies (not for `write_texture`'s direct path, but keep in mind if you switch to `copy_buffer_to_texture`).
- **sRGB**: `Rgba8UnormSrgb` does gamma correction in-shader; if colors look washed/dark, match surface format and texture format sRGB-ness.
