// Prelude prepended to every user fragment shader loaded via `load_shader`.
//
// The user supplies a fragment entry point named `fs_main` with signature:
//     @fragment fn fs_main(in: ShaderIn) -> @location(0) vec4<f32>
// where `in.uv` is the quad-local UV in 0..1 (origin top-left).
//
// Available uniforms:
//   screen.res   : vec2<f32>  surface resolution (px)
//   screen.time  : f32        seconds since start
//   shader_u.rect: vec4<f32>  dest rect in pixels (x, y, w, h)
//   shader_u.params: vec4<f32> first 16 bytes of the user `params` blob (as f32x4)

struct Screen {
    res: vec2<f32>,
    time: f32,
    _pad: f32,
};

struct ShaderU {
    mx: vec2<f32>,
    my: vec2<f32>,
    t:  vec2<f32>,
    size: vec2<f32>,
    rect: vec4<f32>,
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var<uniform> shader_u: ShaderU;

struct ShaderIn {
    @builtin(position) frag: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

fn _corner(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    return c[i];
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> ShaderIn {
    let q = _corner(vi);
    let local = q * shader_u.size;
    let world = shader_u.mx * local.x + shader_u.my * local.y + shader_u.t;
    var out: ShaderIn;
    out.frag = vec4<f32>(
        world.x / screen.res.x * 2.0 - 1.0,
        1.0 - world.y / screen.res.y * 2.0,
        0.0, 1.0,
    );
    out.uv = q;
    return out;
}

// ----- user fragment source appended below -----
