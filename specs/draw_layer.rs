//! Draw-calls engine — retained object layer + `render_to_draws`.
//!
//! Two sub-layers:
//!   1. Retained scene: register/manipulate draw objects and groups (a tree).
//!   2. `render_to_draws`: a PURE function that flattens the tree into a flat,
//!      paint-ordered `Vec<DrawCall>` of plain data (no wgpu types).
//!
//! Coordinates are 2D pixel-space; transforms are 2D affine. The retained layer
//! holds no GPU state — textures/fonts/shaders are referenced by opaque handles
//! that the wgpu backend (separate spec) owns.
//!
//! Z-ORDERING (resolved entirely here, see `render_to_draws`):
//!   primary   = accumulated `z_height` (group z_height sums into children),
//!   secondary = object order (stable pre-order traversal of the tree),
//!   tertiary  = per-object sub-order (fill before outline).
//! Convention: higher effective z_height draws on top / in front.

// ---------------------------------------------------------------------------
// Shared value types
// ---------------------------------------------------------------------------

/// RGBA color, linear 0.0..=1.0 components.
pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }

/// Pixel-space 2D vector / point (origin top-left, +y down).
pub struct Vec2 { pub x: f32, pub y: f32 }

/// Pixel-space axis-aligned size or rect region.
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

/// Authoring transform for an object or group. Composed onto children for groups.
pub struct Transform {
    pub translation: Vec2,
    pub rotation_rad: f32,
    pub scale: Vec2,
}
impl Transform {
    /// Identity transform (no translation/rotation, unit scale).
    pub fn identity() -> Self { unimplemented!() }
    /// Collapse to a baked affine matrix for flattening.
    pub fn to_affine(&self) -> Affine2 { unimplemented!() }
}

/// Baked 2D affine transform (2x2 linear + translation). Output-side only;
/// `render_to_draws` multiplies these down the tree so each DrawCall is absolute.
pub struct Affine2 { pub m: [[f32; 2]; 2], pub t: [f32; 2] }
impl Affine2 {
    pub fn identity() -> Self { unimplemented!() }
    /// `self * rhs` — apply `rhs` then `self` (parent * child).
    pub fn compose(&self, rhs: &Affine2) -> Affine2 { unimplemented!() }
}

/// Outline/stroke styling for a shape object.
pub struct Outline { pub thickness: f32, pub color: Color }

// ---------------------------------------------------------------------------
// Resource handles (owned by the wgpu backend; opaque here)
// ---------------------------------------------------------------------------

pub struct TexHandle(pub u32);
pub struct FontHandle(pub u32);
pub struct ShaderHandle(pub u32);

/// Handle to a registered object in the scene.
pub struct ObjectId(pub u32);
/// Handle to a registered group in the scene.
pub struct GroupId(pub u32);

// ---------------------------------------------------------------------------
// Retained scene
// ---------------------------------------------------------------------------

/// Retained-mode draw scene: a tree of objects and groups under an implicit
/// root group. All `create_*` calls attach to the root unless re-parented.
pub struct DrawScene { /* arena of objects/groups, root, insertion counter */ }

impl DrawScene {
    /// Create an empty scene with an implicit root group at identity / z_height 0.
    pub fn new() -> Self { unimplemented!() }

    // --- object registration (returns a handle; objects start at the root) ---

    /// Register a filled rectangle of `size` (pixels, pre-transform, origin at
    /// its local 0,0). `corner_radius` rounds corners (0.0 = sharp).
    pub fn create_rect(&mut self, size: Vec2, color: Color, corner_radius: f32) -> ObjectId { unimplemented!() }

    /// Register a filled circle of `radius` (pixels) centered at its local origin.
    pub fn create_circle(&mut self, radius: f32, color: Color) -> ObjectId { unimplemented!() }

    /// Register an image quad of `size` (pixels). `src` selects a 0..1 UV
    /// sub-region of the texture; `tint` multiplies the sampled color.
    pub fn create_tex(&mut self, tex: TexHandle, size: Vec2, src: Rect, tint: Color) -> ObjectId { unimplemented!() }

    /// Register a text run. `size_px` is glyph pixel height. Layout into glyph
    /// quads is deferred to the backend (which owns the glyph atlas); this layer
    /// stores the string + font + size verbatim to stay pure.
    pub fn create_text(&mut self, font: FontHandle, text: String, size_px: f32, color: Color) -> ObjectId { unimplemented!() }

    /// Register a user fragment-shader quad of `size` (pixels). `params` is
    /// opaque bytes forwarded to the shader as a uniform block.
    pub fn create_shader(&mut self, shader: ShaderHandle, size: Vec2, params: Vec<u8>) -> ObjectId { unimplemented!() }

    /// Create an empty group (own transform + z_height + children).
    pub fn create_group(&mut self) -> GroupId { unimplemented!() }

    // --- manipulation (objects and groups share these via Node addressing) ---

    /// Replace an object's local transform.
    pub fn set_transform(&mut self, id: ObjectId, t: Transform) { unimplemented!() }
    /// Replace a group's local transform (composed onto all descendants).
    pub fn set_group_transform(&mut self, id: GroupId, t: Transform) { unimplemented!() }

    /// Set an object's local `z_height` (primary ordering key, accumulated with
    /// ancestor groups' z_height).
    pub fn set_z_height(&mut self, id: ObjectId, z_height: f32) { unimplemented!() }
    /// Set a group's local `z_height` (added to every descendant's effective z).
    pub fn set_group_z_height(&mut self, id: GroupId, z_height: f32) { unimplemented!() }

    /// Add/replace an outline on a shape object (rect/circle). No-op for
    /// tex/text/shader objects. Emitted as a separate stroke DrawCall after fill.
    pub fn set_outline(&mut self, id: ObjectId, outline: Outline) { unimplemented!() }

    /// Toggle whether an object is emitted by `render_to_draws`.
    pub fn set_visible(&mut self, id: ObjectId, visible: bool) { unimplemented!() }

    // --- tree structure ---

    /// Move an object under a group (re-parent). Affects transform composition,
    /// accumulated z_height, and traversal (secondary) order.
    pub fn add_object_to_group(&mut self, group: GroupId, child: ObjectId) { unimplemented!() }
    /// Nest a group under another group.
    pub fn add_group_to_group(&mut self, parent: GroupId, child: GroupId) { unimplemented!() }

    /// Remove an object from the scene (invalidates its handle).
    pub fn remove_object(&mut self, id: ObjectId) { unimplemented!() }
    /// Remove a group and all its descendants.
    pub fn remove_group(&mut self, id: GroupId) { unimplemented!() }
}

// ---------------------------------------------------------------------------
// Flattened output: DrawCall
// ---------------------------------------------------------------------------

/// Fill vs. stroke for a shape DrawCall.
pub enum DrawStyle { Fill, Stroke { thickness: f32 } }

/// A single backend-agnostic draw command. Plain data — NO wgpu types.
/// `transform` is the fully-baked world affine; `z` is the resolved normalized
/// depth in 0.0..1.0 (monotonic with final paint order; larger = in front).
/// The returned Vec is already sorted back-to-front, so a no-batcher backend can
/// paint in order and ignore `z` if it wishes.
pub enum DrawCall {
    Shape {
        transform: Affine2,
        shape: Shape,
        style: DrawStyle,
        color: Color,
        z: f32,
    },
    Tex {
        transform: Affine2,
        size: Vec2,
        tex: TexHandle,
        src: Rect,
        tint: Color,
        z: f32,
    },
    Text {
        transform: Affine2,
        font: FontHandle,
        text: String,
        size_px: f32,
        color: Color,
        z: f32,
    },
    Shader {
        transform: Affine2,
        size: Vec2,
        shader: ShaderHandle,
        params: Vec<u8>,
        z: f32,
    },
}

/// Geometry of a shape DrawCall (size/radius are pre-transform local units).
pub enum Shape {
    Rect { size: Vec2, corner_radius: f32 },
    Circle { radius: f32 },
}

// ---------------------------------------------------------------------------
// The pure flattening function
// ---------------------------------------------------------------------------

/// PURE: flatten the scene tree into a paint-ordered `Vec<DrawCall>`.
///
/// Algorithm:
///   1. Pre-order traversal of the tree, accumulating world affine
///      (parent.compose(child)) and effective z_height (sum down the path).
///      The traversal index is each node's secondary key.
///   2. For each visible object emit its fill call; if it has an outline emit a
///      stroke call too, with a higher tertiary sub-index (outline after fill).
///   3. Sort all emitted calls by the composite key
///      (effective_z_height, traversal_index, sub_index), ascending = back→front.
///   4. Stamp each call's `z` as its normalized rank (rank / count) so the value
///      is a valid monotonic depth for any future depth-buffer/batcher backend.
///
/// No GPU, no I/O, deterministic — testable by asserting on the returned Vec.
pub fn render_to_draws(scene: &DrawScene) -> Vec<DrawCall> { unimplemented!() }
