// Shape pipeline: rounded-rect & circle SDF, fill or stroke.
// One pipeline branches on `kind`/`style` from the per-draw uniform.

struct Screen {
    res: vec2<f32>,
    time: f32,
    _pad: f32,
};

struct DrawU {
    // baked affine: columns mx, my and translation t (pixel space)
    mx: vec2<f32>,
    my: vec2<f32>,
    t:  vec2<f32>,
    size: vec2<f32>,        // local quad size in pixels
    color: vec4<f32>,
    // x = shape kind (0 = rect, 1 = circle)
    // y = style (0 = fill, 1 = stroke)
    // z = corner_radius (rect) or radius (circle)
    // w = stroke thickness
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var<uniform> draw: DrawU;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,   // local pixel coord within the quad (origin top-left)
};

// unit quad corners (two triangles) in 0..1
fn corner(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    return c[i];
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let uv = corner(vi);
    let local = uv * draw.size;                 // local pixel coord
    // apply affine (linear part as columns) then translation
    let world = draw.mx * local.x + draw.my * local.y + draw.t;
    var out: VsOut;
    out.clip = vec4<f32>(
        world.x / screen.res.x * 2.0 - 1.0,
        1.0 - world.y / screen.res.y * 2.0,
        0.0, 1.0,
    );
    out.local = local;
    return out;
}

// signed distance to a rounded box centered at origin, half-extents b, radius r
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let kind = draw.params.x;
    let style = draw.params.y;
    let half = draw.size * 0.5;
    let p = in.local - half;        // center-relative

    var d: f32;
    if (kind < 0.5) {
        // rounded rect
        let r = min(draw.params.z, min(half.x, half.y));
        d = sd_round_box(p, half, r);
    } else {
        // circle
        d = length(p) - draw.params.z;
    }

    let aa = fwidth(d);
    var alpha: f32;
    if (style < 0.5) {
        // fill: inside (d<0)
        alpha = 1.0 - smoothstep(-aa, aa, d);
    } else {
        // stroke: band of thickness centered on the edge
        let th = draw.params.w * 0.5;
        let band = abs(d) - th;
        alpha = 1.0 - smoothstep(-aa, aa, band);
    }

    var col = draw.color;
    col.a = col.a * alpha;
    if (col.a <= 0.0) {
        discard;
    }
    return col;
}
