// Textured quad: sample a sub-region (src uv rect) and multiply by tint.

struct Screen {
    res: vec2<f32>,
    time: f32,
    _pad: f32,
};

struct DrawU {
    mx: vec2<f32>,
    my: vec2<f32>,
    t:  vec2<f32>,
    size: vec2<f32>,
    tint: vec4<f32>,
    src: vec4<f32>,        // uv sub-rect: x, y, w, h in 0..1
    _pad2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var<uniform> draw: DrawU;
@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

fn corner(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    return c[i];
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let q = corner(vi);
    let local = q * draw.size;
    let world = draw.mx * local.x + draw.my * local.y + draw.t;
    var out: VsOut;
    out.clip = vec4<f32>(
        world.x / screen.res.x * 2.0 - 1.0,
        1.0 - world.y / screen.res.y * 2.0,
        0.0, 1.0,
    );
    out.uv = draw.src.xy + q * draw.src.zw;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, in.uv);
    return c * draw.tint;
}
