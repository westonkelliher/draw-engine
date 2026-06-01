# Draw Engine Design — What to Optimize For

**The whole design hinges on one choice: the format of the draw-call list `render_to_draws` emits.** Testability, performance, and modularity are all downstream of it. Optimize that representation first; everything else follows.

## Priorities (ranked)

### 1. Purity & inspectability of the scene→draws boundary *(the stated reason for the redesign)*
- `render_to_draws` must be a **pure fn**: `(scene) -> Vec<DrawCall>` — no GPU, no I/O, deterministic.
- **Why:** this is what makes it testable — you assert on the output list directly. If WGPU types leak in, you lose that.
- **Consequence:** `DrawCall` is a plain data enum with **no wgpu types** (no `Buffer`, no `BindGroup`). Textures referenced by **handle/index**, not GPU resources.

### 2. Backend-resolvability without backend coupling
- **Bake world transforms down the group tree** inside the pure fn; emit per-object calls with an absolute 2D affine + z. The backend stays "dumb."
- **Why:** the tree-walk (the interesting, bug-prone part) lives in the testable layer; the WGPU layer becomes a trivial, rarely-changing mapping.

### 3. Batch-friendliness
- Carry an explicit **z per call** so flattening the tree doesn't lose paint order, and so a batcher can freely reorder against a depth buffer.
- Shape the list so a **separate batching pass** can group by `(pipeline, atlas)` into instanced draws.
- Keep batching **out** of `render_to_draws` — it's its own pure, separately-testable pass over `Vec<DrawCall>`.

### 4. API ergonomics of the retained layer
- Objects/groups addressed by **handle**; mutate transform in place; group transform composes onto children.

## Concrete recommendation
- **Layers**, each pure-testable except the last:
  `scene (retained)` → `render_to_draws (pure)` → `batcher (pure)` → `wgpu backend (thin)`
- `render_to_draws`: scene tree → flat `Vec<DrawCall>`, transforms baked, z assigned, **fills and outlines emitted as separate calls**.
- `DrawCall = enum { Rect, Circle, Tex, Text, Shader }`, each with affine + z + color/handle, no wgpu types.

## Don't optimize yet
- **Dirty-tracking / incremental render:** re-walk the whole tree every frame first. It's premature, and it breaks purity. Add only if profiling demands.
