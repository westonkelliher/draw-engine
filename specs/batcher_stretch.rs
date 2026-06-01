//! STRETCH GOAL — Batcher layer.
//!
//! Inserts a PURE batching pass between `render_to_draws` and the wgpu backend:
//!
//!     scene --render_to_draws--> Vec<DrawCall> --batch--> Vec<Batch> --> backend
//!
//! Goal: collapse the per-call draws of the simple backend into a few instanced
//! draws by grouping consecutive calls that share a pipeline and bind group
//! (texture atlas). This is the main performance lever; it is intentionally
//! NOT part of the v1 specs.
//!
//! KEY CONSTRAINT — ordering. The simple backend relied on draw order for
//! z-ordering. Batching REORDERS calls (groups by pipeline/atlas), which would
//! break painter's ordering. The fix is the depth buffer + the normalized `z`
//! that `render_to_draws` already stamps on every DrawCall: visual order moves
//! from "submission order" into the `z` value, so the batcher is free to reorder.

use crate::draw_layer::{DrawCall, Affine2, Color, Rect, TexHandle, ShaderHandle};

// ---------------------------------------------------------------------------
// Batch representation
// ---------------------------------------------------------------------------

/// Which cached pipeline a batch targets.
pub enum PipelineKind { ShapeFill, ShapeStroke, Tex, Text, Shader }

/// Tightly-packed, GPU-ready per-instance record (one quad). `#[repr(C)] + Pod`
/// in the real impl. `z` goes into clip-space Z for the depth test. `uv_rect`
/// indexes the shared atlas so many textures batch under one bind group.
pub struct InstanceRaw {
    pub transform: Affine2,
    pub size: [f32; 2],
    pub color: Color,
    pub uv_rect: [f32; 4],
    pub z: f32,
}

/// One instanced draw: a run of instances sharing a pipeline and a bind group.
pub struct Batch {
    pub pipeline: PipelineKind,
    /// Atlas/texture (and thus bind group) shared by every instance, if any.
    pub tex: Option<TexHandle>,
    /// User shader for `PipelineKind::Shader` batches.
    pub shader: Option<ShaderHandle>,
    pub instances: Vec<InstanceRaw>,
}

// ---------------------------------------------------------------------------
// The pure batching pass
// ---------------------------------------------------------------------------

/// PURE: group a flat draw list into instanced batches.
///
/// Strategy (simple, order-independent thanks to depth):
///   1. Bucket each DrawCall by (PipelineKind, bind-group key) where the key is
///      the texture/atlas handle (or shader handle), translating each call into
///      one `InstanceRaw` (carrying its `z`).
///   2. Emit one `Batch` per bucket.
///   3. `Shader` calls with distinct params/shader generally can't share a bind
///      group — they fall back to one-instance batches (still uniform output).
///
/// Output feeds a batched backend that issues one instanced draw per Batch with
/// the depth test on. Deterministic and testable on the returned Vec.
pub fn batch(draws: &[DrawCall]) -> Vec<Batch> { unimplemented!() }

// ---------------------------------------------------------------------------
// How the other layers change to accommodate the batcher
// ---------------------------------------------------------------------------
//
// draw_layer (`render_to_draws`):  NO STRUCTURAL CHANGE.
//   - It already stamps a normalized, monotonic `z` per DrawCall — that is
//     exactly what the depth buffer needs once draw order is lost. Done for free.
//   - Only soft requirement: `z` must be strictly monotonic with the intended
//     paint order across ALL calls (including the fill/outline tertiary order),
//     so equal-z_height ties still resolve. The rank-based stamping already
//     guarantees this. No API change.
//
// wgpu_backend:  REPLACE the per-call loop with a per-Batch loop.
//   - `render(&[DrawCall])` becomes `render(&[Batch])` (or add an overload).
//   - ENABLE the depth buffer: create a Depth32Float texture sized to the
//     surface, recreate it in `resize()` AFTER reconfiguring the surface, and
//     add a depth-stencil attachment (clear 1.0, CompareFunction::Less,
//     depth_write enabled) to the pass. Every pipeline must declare the same
//     depth-stencil state.
//   - Pipelines gain a second vertex buffer at slot 1 (VertexStepMode::Instance)
//     for `InstanceRaw`; upload each Batch's instances to a reused, growable
//     instance buffer (COPY_DST) and issue `draw_indexed(0..6, 0, 0..N)`.
//   - The vertex shader writes `z` into clip Z so the depth test enforces the
//     ordering the batcher discarded.
//
//   ALPHA CAVEAT: depth test + alpha blending is order-dependent for
//   translucent pixels. If translucency artifacts appear, keep opaque batches
//   depth-tested/reordered but draw translucent calls in their original `z`
//   order (e.g. a second, un-reordered pass), or document the limitation.
