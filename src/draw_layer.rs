//! Draw-calls engine — retained object layer + `render_to_draws`.
//!
//! Two sub-layers:
//!   1. Retained scene: a tree of uniform nodes (each draws something, or
//!      nothing — an "empty" node is a pure transform/group container).
//!   2. `render_to_draws`: a PURE function that flattens the tree into a flat,
//!      paint-ordered `Vec<DrawCall>` of plain data (no wgpu types).
//!
//! Coordinates are 2D pixel-space; transforms are 2D affine. The retained layer
//! holds no GPU state — textures/fonts/shaders are referenced by opaque handles
//! that the wgpu backend (separate spec) owns.
//!
//! Z-ORDERING (resolved entirely here, see `render_to_draws`):
//!   primary   = accumulated `z_height` (ancestor z_height sums into children),
//!   secondary = node order (stable pre-order traversal of the tree),
//!   tertiary  = per-node sub-order (fill before outline).
//! Convention: higher effective z_height draws on top / in front.

// ---------------------------------------------------------------------------
// Shared value types
// ---------------------------------------------------------------------------

/// RGBA color, linear 0.0..=1.0 components.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }

/// Pixel-space 2D vector / point (origin top-left, +y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2 { pub x: f32, pub y: f32 }

/// Pixel-space axis-aligned size or rect region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

/// Authoring transform for an object or group. Composed onto children for groups.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec2,
    pub rotation_rad: f32,
    pub scale: Vec2,
}
impl Transform {
    /// Identity transform (no translation/rotation, unit scale).
    pub fn identity() -> Self {
        Transform {
            translation: Vec2 { x: 0.0, y: 0.0 },
            rotation_rad: 0.0,
            scale: Vec2 { x: 1.0, y: 1.0 },
        }
    }
    /// Collapse to a baked affine matrix for flattening.
    ///
    /// Order: scale, then rotate, then translate (translation * rotation * scale).
    pub fn to_affine(&self) -> Affine2 {
        let (s, c) = self.rotation_rad.sin_cos();
        // rotation * scale, as a column-major 2x2 stored row-by-row in `m`.
        // rotation R = [[c, -s], [s, c]]; scale S = diag(sx, sy).
        // R*S = [[c*sx, -s*sy], [s*sx, c*sy]].
        let sx = self.scale.x;
        let sy = self.scale.y;
        Affine2 {
            m: [
                [c * sx, -s * sy],
                [s * sx, c * sy],
            ],
            t: [self.translation.x, self.translation.y],
        }
    }
}

/// Baked 2D affine transform (2x2 linear + translation). Output-side only;
/// `render_to_draws` multiplies these down the tree so each DrawCall is absolute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine2 { pub m: [[f32; 2]; 2], pub t: [f32; 2] }
impl Affine2 {
    pub fn identity() -> Self {
        Affine2 { m: [[1.0, 0.0], [0.0, 1.0]], t: [0.0, 0.0] }
    }
    /// `self * rhs` — apply `rhs` then `self` (parent * child).
    ///
    /// As homogeneous 3x3 matrices: result = self * rhs, so a point `p` maps to
    /// `self * (rhs * p)`. Linear part = self.m * rhs.m; translation =
    /// self.m * rhs.t + self.t.
    pub fn compose(&self, rhs: &Affine2) -> Affine2 {
        let a = &self.m;
        let b = &rhs.m;
        // m = a * b (row-major matrix product)
        let m = [
            [
                a[0][0] * b[0][0] + a[0][1] * b[1][0],
                a[0][0] * b[0][1] + a[0][1] * b[1][1],
            ],
            [
                a[1][0] * b[0][0] + a[1][1] * b[1][0],
                a[1][0] * b[0][1] + a[1][1] * b[1][1],
            ],
        ];
        // t = a * rhs.t + self.t
        let t = [
            a[0][0] * rhs.t[0] + a[0][1] * rhs.t[1] + self.t[0],
            a[1][0] * rhs.t[0] + a[1][1] * rhs.t[1] + self.t[1],
        ];
        Affine2 { m, t }
    }
}

/// Outline/stroke styling for a shape object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline { pub thickness: f32, pub color: Color }

// ---------------------------------------------------------------------------
// Resource handles (owned by the wgpu backend; opaque here)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexHandle(pub u32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontHandle(pub u32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaderHandle(pub u32);

/// Handle to any node in the scene. There is a single node type: every node has
/// a transform, a z_height, a content (what it draws, possibly nothing), and a
/// list of children. A "group" is just a node whose content is `Content::Empty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// What a node draws. `Empty` draws nothing — it is a pure transform/grouping
/// container (the "group"). Shape content (Rect/Circle) may carry an outline.
/// Stored internally per node; constructed via the `create_*` methods.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Empty,
    Rect { size: Vec2, color: Color, corner_radius: f32, outline: Option<Outline> },
    Circle { radius: f32, color: Color, outline: Option<Outline> },
    Tex { tex: TexHandle, size: Vec2, src: Rect, tint: Color },
    Text { font: FontHandle, text: String, size_px: f32, color: Color },
    Shader { shader: ShaderHandle, size: Vec2, params: Vec<u8> },
}

// ---------------------------------------------------------------------------
// Retained scene
// ---------------------------------------------------------------------------

/// Internal per-node record stored in the arena.
struct Node {
    transform: Transform,
    z_height: f32,
    content: Content,
    visible: bool,
    children: Vec<NodeId>,
    parent: Option<NodeId>,
    /// Monotonically increasing insertion order index (assigned at creation).
    /// Retained per spec; traversal order is also fixed by `children` ordering.
    #[allow(dead_code)]
    order: u64,
    /// Whether this arena slot is live (false once removed).
    alive: bool,
}

/// Retained-mode draw scene: a tree of uniform nodes under an implicit root
/// node. All `create_*` calls attach to the root unless re-parented. Every node
/// can have children (even shape/tex/etc. nodes), and its transform + z_height
/// propagate to its whole subtree.
pub struct DrawScene {
    nodes: Vec<Node>,
    root: NodeId,
    insertion_counter: u64,
}

impl DrawScene {
    /// Create an empty scene with an implicit root node (Empty content, identity
    /// transform, z_height 0).
    pub fn new() -> Self {
        let root = Node {
            transform: Transform::identity(),
            z_height: 0.0,
            content: Content::Empty,
            visible: true,
            children: Vec::new(),
            parent: None,
            order: 0,
            alive: true,
        };
        DrawScene {
            nodes: vec![root],
            root: NodeId(0),
            insertion_counter: 1,
        }
    }

    fn idx(id: NodeId) -> usize { id.0 as usize }

    fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(Self::idx(id)).filter(|n| n.alive)
    }
    fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(Self::idx(id)).filter(|n| n.alive)
    }

    /// Insert a freshly built node under the root and return its handle.
    fn push(&mut self, content: Content) -> NodeId {
        let order = self.insertion_counter;
        self.insertion_counter += 1;
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node {
            transform: Transform::identity(),
            z_height: 0.0,
            content,
            visible: true,
            children: Vec::new(),
            parent: Some(self.root),
            order,
            alive: true,
        });
        let root = self.root;
        self.nodes[Self::idx(root)].children.push(id);
        id
    }

    // --- node creation (returns a NodeId; new nodes start under the root) ---

    /// Create an empty node — draws nothing, exists to transform/group its
    /// children. This is the "pure group". (Any node can have children; this is
    /// just the contentless one.)
    pub fn create_empty(&mut self) -> NodeId {
        self.push(Content::Empty)
    }

    /// Create a filled-rectangle node of `size` (pixels, pre-transform, origin at
    /// its local 0,0). `corner_radius` rounds corners (0.0 = sharp).
    pub fn create_rect(&mut self, size: Vec2, color: Color, corner_radius: f32) -> NodeId {
        self.push(Content::Rect { size, color, corner_radius, outline: None })
    }

    /// Create a filled-circle node of `radius` (pixels) centered at local origin.
    pub fn create_circle(&mut self, radius: f32, color: Color) -> NodeId {
        self.push(Content::Circle { radius, color, outline: None })
    }

    /// Create an image-quad node of `size` (pixels). `src` selects a 0..1 UV
    /// sub-region of the texture; `tint` multiplies the sampled color.
    pub fn create_tex(&mut self, tex: TexHandle, size: Vec2, src: Rect, tint: Color) -> NodeId {
        self.push(Content::Tex { tex, size, src, tint })
    }

    /// Create a text-run node. `size_px` is glyph pixel height. Layout into glyph
    /// quads is deferred to the backend (which owns the glyph atlas); this layer
    /// stores the string + font + size verbatim to stay pure.
    pub fn create_text(&mut self, font: FontHandle, text: String, size_px: f32, color: Color) -> NodeId {
        self.push(Content::Text { font, text, size_px, color })
    }

    /// Create a user fragment-shader-quad node of `size` (pixels). `params` is
    /// opaque bytes forwarded to the shader as a uniform block.
    pub fn create_shader(&mut self, shader: ShaderHandle, size: Vec2, params: Vec<u8>) -> NodeId {
        self.push(Content::Shader { shader, size, params })
    }

    // --- manipulation (uniform over all nodes) ---

    /// Replace a node's local transform (composed onto its whole subtree).
    pub fn set_transform(&mut self, id: NodeId, t: Transform) {
        if let Some(n) = self.get_mut(id) {
            n.transform = t;
        }
    }

    /// Set a node's local `z_height` (primary ordering key; added to every
    /// descendant's effective z_height).
    pub fn set_z_height(&mut self, id: NodeId, z_height: f32) {
        if let Some(n) = self.get_mut(id) {
            n.z_height = z_height;
        }
    }

    /// Add/replace the outline on a shape node (Rect/Circle). No-op if the node's
    /// content is Empty/Tex/Text/Shader. Emitted as a separate stroke DrawCall
    /// after the fill.
    pub fn set_outline(&mut self, id: NodeId, outline: Outline) {
        if let Some(n) = self.get_mut(id) {
            match &mut n.content {
                Content::Rect { outline: o, .. } => *o = Some(outline),
                Content::Circle { outline: o, .. } => *o = Some(outline),
                _ => {} // no-op for Empty/Tex/Text/Shader
            }
        }
    }

    /// Toggle whether a node (and thus its subtree) is emitted by
    /// `render_to_draws`.
    pub fn set_visible(&mut self, id: NodeId, visible: bool) {
        if let Some(n) = self.get_mut(id) {
            n.visible = visible;
        }
    }

    // --- tree structure ---

    /// Re-parent `child` under `parent`. Affects transform composition,
    /// accumulated z_height, and traversal (secondary) order. Any node may be a
    /// parent. Errors/no-ops if it would create a cycle.
    pub fn add_child(&mut self, parent: NodeId, child: NodeId) {
        // Both must be live, and child must not be the root.
        if self.get(parent).is_none() || self.get(child).is_none() {
            return;
        }
        if child == self.root {
            return; // root cannot be re-parented
        }
        if parent == child {
            return; // trivial cycle
        }
        // Cycle guard: child must not be an ancestor of parent (i.e. parent must
        // not be in child's subtree).
        if self.is_descendant_or_self(child, parent) {
            return;
        }
        // Detach from old parent.
        if let Some(old) = self.get(child).and_then(|n| n.parent) {
            if let Some(op) = self.get_mut(old) {
                op.children.retain(|c| *c != child);
            }
        }
        // Attach to new parent.
        if let Some(n) = self.get_mut(child) {
            n.parent = Some(parent);
        }
        if let Some(p) = self.get_mut(parent) {
            p.children.push(child);
        }
    }

    /// True if `maybe_ancestor` == `node` or is an ancestor of `node`.
    fn is_descendant_or_self(&self, maybe_ancestor: NodeId, node: NodeId) -> bool {
        let mut cur = Some(node);
        while let Some(c) = cur {
            if c == maybe_ancestor {
                return true;
            }
            cur = self.get(c).and_then(|n| n.parent);
        }
        false
    }

    /// Remove a node and its entire subtree (invalidates those handles).
    pub fn remove(&mut self, id: NodeId) {
        if id == self.root {
            return; // never remove the implicit root
        }
        if self.get(id).is_none() {
            return;
        }
        // Detach from parent.
        if let Some(parent) = self.get(id).and_then(|n| n.parent) {
            if let Some(p) = self.get_mut(parent) {
                p.children.retain(|c| *c != id);
            }
        }
        // Kill the subtree.
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            if let Some(n) = self.get_mut(cur) {
                n.alive = false;
                let kids = std::mem::take(&mut n.children);
                n.parent = None;
                stack.extend(kids);
            }
        }
    }
}

impl Default for DrawScene {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Flattened output: DrawCall
// ---------------------------------------------------------------------------

/// Fill vs. stroke for a shape DrawCall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawStyle { Fill, Stroke { thickness: f32 } }

/// A single backend-agnostic draw command. Plain data — NO wgpu types.
/// `transform` is the fully-baked world affine; `z` is the resolved normalized
/// depth in 0.0..1.0 (monotonic with final paint order; larger = in front).
/// The returned Vec is already sorted back-to-front, so a no-batcher backend can
/// paint in order and ignore `z` if it wishes.
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Rect { size: Vec2, corner_radius: f32 },
    Circle { radius: f32 },
}

// ---------------------------------------------------------------------------
// The pure flattening function
// ---------------------------------------------------------------------------

/// Intermediate record carrying a draw call plus its composite sort key.
struct Emitted {
    z_height: f32,
    traversal_index: u32,
    sub_index: u32,
    call: DrawCall,
}

/// PURE: flatten the scene tree into a paint-ordered `Vec<DrawCall>`.
///
/// Algorithm:
///   1. Pre-order traversal of the tree, accumulating world affine
///      (parent.compose(child)) and effective z_height (sum down the path).
///      The traversal index is each node's secondary key.
///   2. For each visible node emit its fill call (Empty nodes emit nothing but
///      still propagate transform + z_height); if it has an outline emit a
///      stroke call too, with a higher tertiary sub-index (outline after fill).
///   3. Sort all emitted calls by the composite key
///      (effective_z_height, traversal_index, sub_index), ascending = back→front.
///   4. Stamp each call's `z` as its normalized rank (rank / count) so the value
///      is a valid monotonic depth for any future depth-buffer/batcher backend.
///
/// No GPU, no I/O, deterministic — testable by asserting on the returned Vec.
pub fn render_to_draws(scene: &DrawScene) -> Vec<DrawCall> {
    let mut emitted: Vec<Emitted> = Vec::new();
    let mut traversal_index: u32 = 0;

    // Iterative pre-order DFS. Stack holds (node, accumulated world affine,
    // accumulated z_height). We push children in reverse so they pop in order.
    let root = scene.root;
    let root_world = match scene.get(root) {
        Some(n) if n.visible => n.transform.to_affine(),
        _ => return Vec::new(), // root missing or invisible -> nothing
    };
    let root_z = scene.get(root).map(|n| n.z_height).unwrap_or(0.0);

    let mut stack: Vec<(NodeId, Affine2, f32)> = vec![(root, root_world, root_z)];

    while let Some((id, world, eff_z)) = stack.pop() {
        let node = match scene.get(id) {
            Some(n) => n,
            None => continue,
        };
        // Invisible nodes (and their subtrees) are skipped entirely.
        if !node.visible {
            continue;
        }

        let ti = traversal_index;
        traversal_index += 1;

        emit_node(node, &world, eff_z, ti, &mut emitted);

        // Push children in reverse for left-to-right pre-order.
        for &child in node.children.iter().rev() {
            if let Some(cn) = scene.get(child) {
                let child_world = world.compose(&cn.transform.to_affine());
                let child_z = eff_z + cn.z_height;
                stack.push((child, child_world, child_z));
            }
        }
    }

    // Sort back->front by (z_height, traversal_index, sub_index) ascending.
    emitted.sort_by(|a, b| {
        a.z_height
            .total_cmp(&b.z_height)
            .then(a.traversal_index.cmp(&b.traversal_index))
            .then(a.sub_index.cmp(&b.sub_index))
    });

    let count = emitted.len();
    emitted
        .into_iter()
        .enumerate()
        .map(|(rank, mut e)| {
            let z = if count == 0 { 0.0 } else { rank as f32 / count as f32 };
            set_z(&mut e.call, z);
            e.call
        })
        .collect()
}

/// Emit fill (and optional stroke) draw calls for a single visible node.
fn emit_node(node: &Node, world: &Affine2, eff_z: f32, ti: u32, out: &mut Vec<Emitted>) {
    match &node.content {
        Content::Empty => { /* pure group: emits nothing */ }
        Content::Rect { size, color, corner_radius, outline } => {
            out.push(Emitted {
                z_height: eff_z,
                traversal_index: ti,
                sub_index: 0,
                call: DrawCall::Shape {
                    transform: *world,
                    shape: Shape::Rect { size: *size, corner_radius: *corner_radius },
                    style: DrawStyle::Fill,
                    color: *color,
                    z: 0.0,
                },
            });
            if let Some(o) = outline {
                out.push(Emitted {
                    z_height: eff_z,
                    traversal_index: ti,
                    sub_index: 1,
                    call: DrawCall::Shape {
                        transform: *world,
                        shape: Shape::Rect { size: *size, corner_radius: *corner_radius },
                        style: DrawStyle::Stroke { thickness: o.thickness },
                        color: o.color,
                        z: 0.0,
                    },
                });
            }
        }
        Content::Circle { radius, color, outline } => {
            out.push(Emitted {
                z_height: eff_z,
                traversal_index: ti,
                sub_index: 0,
                call: DrawCall::Shape {
                    transform: *world,
                    shape: Shape::Circle { radius: *radius },
                    style: DrawStyle::Fill,
                    color: *color,
                    z: 0.0,
                },
            });
            if let Some(o) = outline {
                out.push(Emitted {
                    z_height: eff_z,
                    traversal_index: ti,
                    sub_index: 1,
                    call: DrawCall::Shape {
                        transform: *world,
                        shape: Shape::Circle { radius: *radius },
                        style: DrawStyle::Stroke { thickness: o.thickness },
                        color: o.color,
                        z: 0.0,
                    },
                });
            }
        }
        Content::Tex { tex, size, src, tint } => {
            out.push(Emitted {
                z_height: eff_z,
                traversal_index: ti,
                sub_index: 0,
                call: DrawCall::Tex {
                    transform: *world,
                    size: *size,
                    tex: *tex,
                    src: *src,
                    tint: *tint,
                    z: 0.0,
                },
            });
        }
        Content::Text { font, text, size_px, color } => {
            out.push(Emitted {
                z_height: eff_z,
                traversal_index: ti,
                sub_index: 0,
                call: DrawCall::Text {
                    transform: *world,
                    font: *font,
                    text: text.clone(),
                    size_px: *size_px,
                    color: *color,
                    z: 0.0,
                },
            });
        }
        Content::Shader { shader, size, params } => {
            out.push(Emitted {
                z_height: eff_z,
                traversal_index: ti,
                sub_index: 0,
                call: DrawCall::Shader {
                    transform: *world,
                    size: *size,
                    shader: *shader,
                    params: params.clone(),
                    z: 0.0,
                },
            });
        }
    }
}

fn set_z(call: &mut DrawCall, value: f32) {
    match call {
        DrawCall::Shape { z, .. } => *z = value,
        DrawCall::Tex { z, .. } => *z = value,
        DrawCall::Text { z, .. } => *z = value,
        DrawCall::Shader { z, .. } => *z = value,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    fn assert_affine(got: &Affine2, m: [[f32; 2]; 2], t: [f32; 2]) {
        for i in 0..2 {
            for j in 0..2 {
                assert!(
                    approx(got.m[i][j], m[i][j]),
                    "m[{i}][{j}]: got {} expected {}",
                    got.m[i][j],
                    m[i][j]
                );
            }
            assert!(approx(got.t[i], t[i]), "t[{i}]: got {} expected {}", got.t[i], t[i]);
        }
    }

    fn red() -> Color { Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 } }
    fn sz(x: f32, y: f32) -> Vec2 { Vec2 { x, y } }

    fn call_transform(c: &DrawCall) -> &Affine2 {
        match c {
            DrawCall::Shape { transform, .. } => transform,
            DrawCall::Tex { transform, .. } => transform,
            DrawCall::Text { transform, .. } => transform,
            DrawCall::Shader { transform, .. } => transform,
        }
    }
    fn call_z(c: &DrawCall) -> f32 {
        match c {
            DrawCall::Shape { z, .. } => *z,
            DrawCall::Tex { z, .. } => *z,
            DrawCall::Text { z, .. } => *z,
            DrawCall::Shader { z, .. } => *z,
        }
    }

    #[test]
    fn identity_transform_to_affine() {
        let a = Transform::identity().to_affine();
        assert_affine(&a, [[1.0, 0.0], [0.0, 1.0]], [0.0, 0.0]);
    }

    #[test]
    fn affine_compose_translation_then_scale() {
        // parent translates by (10, 20), child scales by 2.
        let parent = Transform {
            translation: sz(10.0, 20.0),
            rotation_rad: 0.0,
            scale: sz(1.0, 1.0),
        }
        .to_affine();
        let child = Transform {
            translation: sz(0.0, 0.0),
            rotation_rad: 0.0,
            scale: sz(2.0, 3.0),
        }
        .to_affine();
        let world = parent.compose(&child);
        // A local point (1,1) -> scale -> (2,3) -> translate -> (12,23).
        assert_affine(&world, [[2.0, 0.0], [0.0, 3.0]], [10.0, 20.0]);
    }

    #[test]
    fn empty_scene_emits_nothing() {
        let scene = DrawScene::new();
        let draws = render_to_draws(&scene);
        assert!(draws.is_empty(), "root-only scene should emit no draw calls");
    }

    #[test]
    fn single_rect_emits_one_fill() {
        let mut scene = DrawScene::new();
        scene.create_rect(sz(4.0, 4.0), red(), 0.0);
        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 1);
        match &draws[0] {
            DrawCall::Shape { style: DrawStyle::Fill, color, .. } => {
                assert_eq!(*color, red());
            }
            other => panic!("expected fill shape, got {other:?}"),
        }
        // single call -> rank 0 / count 1 = 0.0
        assert!(approx(call_z(&draws[0]), 0.0));
    }

    #[test]
    fn z_height_ordering_overrides_insertion() {
        let mut scene = DrawScene::new();
        // Insert high-z first, low-z second. Lower z must come first (back).
        let high = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        let low = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.set_z_height(high, 10.0);
        scene.set_z_height(low, 1.0);

        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 2);
        // back (index 0) = lower z. Its normalized z must be <= the next.
        assert!(call_z(&draws[0]) <= call_z(&draws[1]));
        // The first drawn (back) should be the low one: distinguishable by z value.
        assert!(approx(call_z(&draws[0]), 0.0));
        assert!(approx(call_z(&draws[1]), 0.5));
    }

    #[test]
    fn accumulated_z_height_inherits_ancestor() {
        let mut scene = DrawScene::new();
        // group with high z, containing a child; a sibling rect with mid z.
        let group = scene.create_empty();
        scene.set_z_height(group, 100.0);
        let child = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.add_child(group, child);

        let sibling = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.set_z_height(sibling, 50.0);

        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 2);
        // child effective z = 100 (inherited) > sibling 50, so sibling is back.
        // back call (index 0) = sibling. We can't read z_height directly, but
        // ordering proves inheritance: child draws on top.
        // distinguish by size won't help (same), so use color tagging instead:
        // re-run with distinct colors.
        drop(draws);

        let mut scene = DrawScene::new();
        let group = scene.create_empty();
        scene.set_z_height(group, 100.0);
        let blue = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
        let child = scene.create_rect(sz(1.0, 1.0), blue, 0.0);
        scene.add_child(group, child);
        let sibling = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.set_z_height(sibling, 50.0);

        let draws = render_to_draws(&scene);
        // index 0 (back) should be red sibling; index 1 (front) blue child.
        match &draws[0] {
            DrawCall::Shape { color, .. } => assert_eq!(*color, red()),
            o => panic!("{o:?}"),
        }
        match &draws[1] {
            DrawCall::Shape { color, .. } => assert_eq!(*color, blue),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn child_world_transform_composes_parent() {
        let mut scene = DrawScene::new();
        let parent = scene.create_empty();
        scene.set_transform(
            parent,
            Transform {
                translation: sz(10.0, 20.0),
                rotation_rad: 0.0,
                scale: sz(2.0, 2.0),
            },
        );
        let child = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.set_transform(
            child,
            Transform {
                translation: sz(5.0, 0.0),
                rotation_rad: 0.0,
                scale: sz(1.0, 1.0),
            },
        );
        scene.add_child(parent, child);

        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 1);
        // world = parent(T10,20 * S2) ∘ child(T5,0).
        // linear = 2*I; translation = parent.m * child.t + parent.t
        //        = [2*5, 2*0] + [10,20] = [20, 20].
        assert_affine(call_transform(&draws[0]), [[2.0, 0.0], [0.0, 2.0]], [20.0, 20.0]);
    }

    #[test]
    fn rotation_world_transform() {
        let mut scene = DrawScene::new();
        let n = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.set_transform(
            n,
            Transform {
                translation: sz(0.0, 0.0),
                rotation_rad: std::f32::consts::FRAC_PI_2, // 90deg
                scale: sz(1.0, 1.0),
            },
        );
        let draws = render_to_draws(&scene);
        // R(90) = [[0,-1],[1,0]]
        assert_affine(call_transform(&draws[0]), [[0.0, -1.0], [1.0, 0.0]], [0.0, 0.0]);
    }

    #[test]
    fn outline_emits_stroke_after_fill() {
        let mut scene = DrawScene::new();
        let r = scene.create_rect(sz(2.0, 2.0), red(), 0.0);
        let outline_color = Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
        scene.set_outline(r, Outline { thickness: 3.0, color: outline_color });

        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 2);
        // fill first
        match &draws[0] {
            DrawCall::Shape { style: DrawStyle::Fill, color, .. } => assert_eq!(*color, red()),
            o => panic!("expected fill first, got {o:?}"),
        }
        // stroke second, same node
        match &draws[1] {
            DrawCall::Shape { style: DrawStyle::Stroke { thickness }, color, .. } => {
                assert!(approx(*thickness, 3.0));
                assert_eq!(*color, outline_color);
            }
            o => panic!("expected stroke second, got {o:?}"),
        }
    }

    #[test]
    fn set_outline_noop_on_non_shape() {
        let mut scene = DrawScene::new();
        let t = scene.create_text(FontHandle(0), "hi".into(), 12.0, red());
        scene.set_outline(t, Outline { thickness: 1.0, color: red() });
        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 1, "text + outline should still be one call");
        assert!(matches!(draws[0], DrawCall::Text { .. }));
    }

    #[test]
    fn invisible_node_and_subtree_emit_nothing() {
        let mut scene = DrawScene::new();
        let group = scene.create_empty();
        let child = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.add_child(group, child);
        let visible_rect = scene.create_rect(sz(1.0, 1.0), red(), 0.0);

        scene.set_visible(group, false);
        let draws = render_to_draws(&scene);
        // only the standalone visible_rect remains
        assert_eq!(draws.len(), 1);
        let _ = visible_rect;
    }

    #[test]
    fn stable_secondary_order_for_equal_z() {
        let mut scene = DrawScene::new();
        let a = scene.create_rect(sz(1.0, 1.0), Color { r: 0.1, g: 0.0, b: 0.0, a: 1.0 }, 0.0);
        let b = scene.create_rect(sz(1.0, 1.0), Color { r: 0.2, g: 0.0, b: 0.0, a: 1.0 }, 0.0);
        let c = scene.create_rect(sz(1.0, 1.0), Color { r: 0.3, g: 0.0, b: 0.0, a: 1.0 }, 0.0);
        let _ = (a, b, c);

        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 3);
        let reds: Vec<f32> = draws
            .iter()
            .map(|d| match d {
                DrawCall::Shape { color, .. } => color.r,
                _ => panic!(),
            })
            .collect();
        // insertion/traversal order preserved
        assert!(approx(reds[0], 0.1));
        assert!(approx(reds[1], 0.2));
        assert!(approx(reds[2], 0.3));
    }

    #[test]
    fn normalized_z_is_monotonic_nondecreasing() {
        let mut scene = DrawScene::new();
        for i in 0..5 {
            let n = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
            scene.set_z_height(n, i as f32);
        }
        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 5);
        let mut prev = f32::NEG_INFINITY;
        for d in &draws {
            let z = call_z(d);
            assert!(z >= prev, "z {z} < prev {prev}");
            assert!((0.0..=1.0).contains(&z), "z {z} out of [0,1]");
            prev = z;
        }
    }

    #[test]
    fn add_child_rejects_cycle() {
        let mut scene = DrawScene::new();
        let a = scene.create_empty();
        let b = scene.create_empty();
        scene.add_child(a, b);
        // attempting to make a a child of b would form a cycle -> no-op
        scene.add_child(b, a);
        // a should still be under root, b under a. Render to ensure no panic /
        // infinite loop and both empties emit nothing.
        let draws = render_to_draws(&scene);
        assert!(draws.is_empty());
    }

    #[test]
    fn remove_drops_subtree() {
        let mut scene = DrawScene::new();
        let group = scene.create_empty();
        let child = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.add_child(group, child);
        let keep = scene.create_rect(sz(1.0, 1.0), Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }, 0.0);

        scene.remove(group);
        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 1);
        match &draws[0] {
            DrawCall::Shape { color, .. } => assert_eq!(*color, Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }),
            o => panic!("{o:?}"),
        }
        let _ = keep;
    }

    #[test]
    fn reparent_detaches_from_old_parent() {
        let mut scene = DrawScene::new();
        let p1 = scene.create_empty();
        let p2 = scene.create_empty();
        let child = scene.create_rect(sz(1.0, 1.0), red(), 0.0);
        scene.add_child(p1, child);
        scene.add_child(p2, child);
        // removing p1 must NOT remove child (it's now under p2).
        scene.remove(p1);
        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 1, "child should survive under p2");
    }
}
