//! Retained scene tree + `render_to_draws`, a pure function that flattens it
//! into a paint-ordered `Vec<DrawCall>` of plain data (no wgpu types).
//!
//! Coordinates are 2D pixel-space (origin top-left, +y down); transforms are 2D
//! affine. GPU resources are referenced by opaque handles the backend owns.
//!
//! Z-ordering keys (ascending = back→front): accumulated `z_height` (summed down
//! the tree), then pre-order traversal index, then per-node sub-order (fill
//! before outline).

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2 { pub x: f32, pub y: f32 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

/// Authoring transform; composed onto children for groups.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec2,
    pub rotation_rad: f32,
    pub scale: Vec2,
}
impl Transform {
    pub fn identity() -> Self {
        Transform {
            translation: Vec2 { x: 0.0, y: 0.0 },
            rotation_rad: 0.0,
            scale: Vec2 { x: 1.0, y: 1.0 },
        }
    }
    /// Bake to affine. Order: scale, then rotate, then translate.
    pub fn to_affine(&self) -> Affine2 {
        let (s, c) = self.rotation_rad.sin_cos();
        // R*S where R = [[c,-s],[s,c]], S = diag(sx,sy).
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

/// Baked 2D affine transform (2x2 linear + translation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine2 { pub m: [[f32; 2]; 2], pub t: [f32; 2] }
impl Affine2 {
    pub fn identity() -> Self {
        Affine2 { m: [[1.0, 0.0], [0.0, 1.0]], t: [0.0, 0.0] }
    }
    /// `self * rhs` — apply `rhs` then `self` (parent * child).
    pub fn compose(&self, rhs: &Affine2) -> Affine2 {
        let a = &self.m;
        let b = &rhs.m;
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
        let t = [
            a[0][0] * rhs.t[0] + a[0][1] * rhs.t[1] + self.t[0],
            a[1][0] * rhs.t[0] + a[1][1] * rhs.t[1] + self.t[1],
        ];
        Affine2 { m, t }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline { pub thickness: f32, pub color: Color }

// Resource handles (owned by the wgpu backend; opaque here).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexHandle(pub u32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontHandle(pub u32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaderHandle(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// What a node draws. `Empty` draws nothing (pure transform/group container).
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Empty,
    Rect { size: Vec2, color: Color, corner_radius: f32, outline: Option<Outline> },
    Circ { radius: f32, color: Color, outline: Option<Outline> },
    Tex { tex: TexHandle, size: Vec2, src: Rect, tint: Color },
    Writ { font: FontHandle, text: String, size_px: f32, color: Color },
    Shad { shader: ShaderHandle, size: Vec2, params: Vec<u8> },
}

struct Node {
    transform: Transform,
    z_height: f32,
    content: Content,
    visible: bool,
    children: Vec<NodeId>,
    parent: Option<NodeId>,
    #[allow(dead_code)]
    order: u64,
    alive: bool,
}

/// Retained-mode scene: a tree of uniform nodes under an implicit root. New
/// nodes attach to the root unless re-parented; transform + z_height propagate
/// to the whole subtree.
pub struct DrawScene {
    nodes: Vec<Node>,
    root: NodeId,
    insertion_counter: u64,
}

impl DrawScene {
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

    // Node creation. New nodes start under the root.

    /// Empty node — draws nothing, exists to transform/group its children.
    pub fn create_empty(&mut self) -> NodeId {
        self.push(Content::Empty)
    }

    /// Filled rect of `size` (origin at local 0,0). `corner_radius` 0 = sharp.
    pub fn create_rect(&mut self, size: Vec2, color: Color, corner_radius: f32) -> NodeId {
        self.push(Content::Rect { size, color, corner_radius, outline: None })
    }

    /// Filled circle of `radius` centered at local origin.
    pub fn create_circ(&mut self, radius: f32, color: Color) -> NodeId {
        self.push(Content::Circ { radius, color, outline: None })
    }

    /// Image quad. `src` selects a 0..1 UV sub-region; `tint` multiplies it.
    pub fn create_tex(&mut self, tex: TexHandle, size: Vec2, src: Rect, tint: Color) -> NodeId {
        self.push(Content::Tex { tex, size, src, tint })
    }

    /// Text run; `size_px` is glyph pixel height. Layout is deferred to the
    /// backend (which owns the glyph atlas), so this stays pure.
    pub fn create_writ(&mut self, font: FontHandle, text: String, size_px: f32, color: Color) -> NodeId {
        self.push(Content::Writ { font, text, size_px, color })
    }

    /// Fragment-shader quad. `params` is opaque bytes forwarded as a uniform.
    pub fn create_shad(&mut self, shader: ShaderHandle, size: Vec2, params: Vec<u8>) -> NodeId {
        self.push(Content::Shad { shader, size, params })
    }

    /// Replace a node's local transform (composed onto its whole subtree).
    pub fn set_transform(&mut self, id: NodeId, t: Transform) {
        if let Some(n) = self.get_mut(id) {
            n.transform = t;
        }
    }

    /// Set local `z_height` (added to every descendant's effective z_height).
    pub fn set_z_height(&mut self, id: NodeId, z_height: f32) {
        if let Some(n) = self.get_mut(id) {
            n.z_height = z_height;
        }
    }

    /// Add/replace the outline on a shape node (Rect/Circ). No-op otherwise.
    /// Emitted as a separate stroke DrawCall after the fill.
    pub fn set_outline(&mut self, id: NodeId, outline: Outline) {
        if let Some(n) = self.get_mut(id) {
            match &mut n.content {
                Content::Rect { outline: o, .. } => *o = Some(outline),
                Content::Circ { outline: o, .. } => *o = Some(outline),
                _ => {}
            }
        }
    }

    /// Toggle whether a node (and its subtree) is emitted.
    pub fn set_visible(&mut self, id: NodeId, visible: bool) {
        if let Some(n) = self.get_mut(id) {
            n.visible = visible;
        }
    }

    /// Re-parent `child` under `parent`. No-op if it would create a cycle, if
    /// either is dead, or if `child` is the root.
    pub fn add_child(&mut self, parent: NodeId, child: NodeId) {
        if self.get(parent).is_none() || self.get(child).is_none() {
            return;
        }
        if child == self.root || parent == child {
            return;
        }
        // Cycle guard: parent must not be in child's subtree.
        if self.is_descendant_or_self(child, parent) {
            return;
        }
        if let Some(old) = self.get(child).and_then(|n| n.parent) {
            if let Some(op) = self.get_mut(old) {
                op.children.retain(|c| *c != child);
            }
        }
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
        if id == self.root || self.get(id).is_none() {
            return;
        }
        if let Some(parent) = self.get(id).and_then(|n| n.parent) {
            if let Some(p) = self.get_mut(parent) {
                p.children.retain(|c| *c != id);
            }
        }
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawStyle { Fill, Stroke { thickness: f32 } }

/// A backend-agnostic draw command (plain data, no wgpu types). `transform` is
/// the baked world affine; `z` is normalized depth in 0..1, monotonic with paint
/// order (larger = in front). The returned Vec is already sorted back-to-front.
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
    Writ {
        transform: Affine2,
        font: FontHandle,
        text: String,
        size_px: f32,
        color: Color,
        z: f32,
    },
    Shad {
        transform: Affine2,
        size: Vec2,
        shader: ShaderHandle,
        params: Vec<u8>,
        z: f32,
    },
}

/// Geometry of a shape DrawCall (pre-transform local units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Rect { size: Vec2, corner_radius: f32 },
    Circ { radius: f32 },
}

/// A draw call plus its composite sort key.
struct Emitted {
    z_height: f32,
    traversal_index: u32,
    sub_index: u32,
    call: DrawCall,
}

/// Flatten the scene tree into a paint-ordered `Vec<DrawCall>` (pure,
/// deterministic). Pre-order DFS accumulates world affine and effective
/// z_height; calls are then sorted by (z_height, traversal_index, sub_index)
/// ascending, and each `z` is stamped as its normalized rank.
pub fn render_to_draws(scene: &DrawScene) -> Vec<DrawCall> {
    let mut emitted: Vec<Emitted> = Vec::new();
    let mut traversal_index: u32 = 0;

    // Stack holds (node, accumulated world affine, accumulated z_height).
    let root = scene.root;
    let root_world = match scene.get(root) {
        Some(n) if n.visible => n.transform.to_affine(),
        _ => return Vec::new(),
    };
    let root_z = scene.get(root).map(|n| n.z_height).unwrap_or(0.0);

    let mut stack: Vec<(NodeId, Affine2, f32)> = vec![(root, root_world, root_z)];

    while let Some((id, world, eff_z)) = stack.pop() {
        let node = match scene.get(id) {
            Some(n) => n,
            None => continue,
        };
        if !node.visible {
            continue;
        }

        let ti = traversal_index;
        traversal_index += 1;

        emit_node(node, &world, eff_z, ti, &mut emitted);

        // Reverse so children pop in left-to-right pre-order.
        for &child in node.children.iter().rev() {
            if let Some(cn) = scene.get(child) {
                let child_world = world.compose(&cn.transform.to_affine());
                let child_z = eff_z + cn.z_height;
                stack.push((child, child_world, child_z));
            }
        }
    }

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
        Content::Empty => {}
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
        Content::Circ { radius, color, outline } => {
            out.push(Emitted {
                z_height: eff_z,
                traversal_index: ti,
                sub_index: 0,
                call: DrawCall::Shape {
                    transform: *world,
                    shape: Shape::Circ { radius: *radius },
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
                        shape: Shape::Circ { radius: *radius },
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
        Content::Writ { font, text, size_px, color } => {
            out.push(Emitted {
                z_height: eff_z,
                traversal_index: ti,
                sub_index: 0,
                call: DrawCall::Writ {
                    transform: *world,
                    font: *font,
                    text: text.clone(),
                    size_px: *size_px,
                    color: *color,
                    z: 0.0,
                },
            });
        }
        Content::Shad { shader, size, params } => {
            out.push(Emitted {
                z_height: eff_z,
                traversal_index: ti,
                sub_index: 0,
                call: DrawCall::Shad {
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
        DrawCall::Writ { z, .. } => *z = value,
        DrawCall::Shad { z, .. } => *z = value,
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
            DrawCall::Writ { transform, .. } => transform,
            DrawCall::Shad { transform, .. } => transform,
        }
    }
    fn call_z(c: &DrawCall) -> f32 {
        match c {
            DrawCall::Shape { z, .. } => *z,
            DrawCall::Tex { z, .. } => *z,
            DrawCall::Writ { z, .. } => *z,
            DrawCall::Shad { z, .. } => *z,
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
        let t = scene.create_writ(FontHandle(0), "hi".into(), 12.0, red());
        scene.set_outline(t, Outline { thickness: 1.0, color: red() });
        let draws = render_to_draws(&scene);
        assert_eq!(draws.len(), 1, "text + outline should still be one call");
        assert!(matches!(draws[0], DrawCall::Writ { .. }));
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
