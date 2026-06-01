# draw-engine

A 2D draw engine, built on [wgpu](https://wgpu.rs) that will be utilized both in a game engine and webpage DOM.

> **Status:** v1 implemented. The `specs/` files are the original typed prototypes;
> the working crate lives in `src/` (`cargo test`, `cargo run --example demo`).

## Approach

Retained-mode scene graph compiled to GPU draw calls through pure, testable layers:

```
DrawScene  --render_to_draws-->  Vec<DrawCall>  -->  WgpuBackend
 (retained)        (pure)          (plain data)        (GPU)
```

- **DrawScene** — a tree of uniform **nodes**. Every node has a transform, a
  `z_height`, optional children, and a *content* (Rect / Circle / Tex / Text /
  Shader, or **Empty**). An `Empty` node draws nothing and acts as a pure
  transform/group container; any node may have children.
- **`render_to_draws`** — a **pure** function that flattens the tree into a flat,
  paint-ordered `Vec<DrawCall>` of plain data (no wgpu types; resources by
  handle). Easy to unit-test by asserting on the output.
- **WgpuBackend** — owns all GPU state and resources; issues one draw per
  `DrawCall`. Deliberately simple (no batching) to start.

## Z-ordering

Resolved entirely in `render_to_draws`, as a 3-tier key:

1. **Primary** — accumulated `z_height` (a node's effective z = its own + all ancestors').
2. **Secondary** — node order (stable pre-order traversal).
3. **Tertiary** — per-node sub-order (e.g. outline drawn after fill).

Higher effective z_height draws on top. Each `DrawCall` is stamped with a
normalized depth so a future depth-buffer backend can reorder freely.

## Layout

| Path | What |
|------|------|
| `src/draw_layer.rs` | Retained scene + pure `render_to_draws` + `DrawCall` (+ unit tests) |
| `src/wgpu_backend.rs` | wgpu 29 backend (one draw per call, no batcher) |
| `src/shaders/` | WGSL: SDF shape (rounded-rect/circle, fill+stroke), tex, user-shader prelude |
| `examples/demo.rs` | winit window drawing rects/circle/line/text/texture with z-ordering |
| `specs/`, `reference/` | Original typed prototypes + wgpu tutorial takeaways |
| `DRAW_ENGINE_SPEC.rs` | Earlier immediate-mode sketch (superseded by the retained approach) |

## Roadmap

- [x] Implement the three layers (pure scene, `render_to_draws`, wgpu backend).
- [ ] Stretch: insert a pure `batch()` pass and enable instanced + depth-tested rendering (see `specs/batcher_stretch.rs`).
