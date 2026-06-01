//! Runnable demo for the wgpu draw-engine backend.
//!
//! Opens a winit window, builds a small retained `DrawScene`, flattens it with
//! `render_to_draws`, and paints it via `WgpuBackend`. Continuous redraw.
//!
//! NOTE: this exercises the public API. The draw_layer *bodies* may still be
//! `unimplemented!()` while that layer is in progress — in that case the demo
//! will panic at runtime, but it compiles against the frozen type signatures.

use std::sync::Arc;

use draw_engine::draw_layer::{
    render_to_draws, Color, DrawScene, FontHandle, Outline, Rect, TexHandle, Transform, Vec2,
};
use draw_engine::WgpuBackend;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const W: u32 = 960;
const H: u32 = 640;

/// Procedurally build an 8x8 RGBA checkerboard texture (no image crate).
fn checkerboard(cells: u32, px: u32) -> (Vec<u8>, u32) {
    let size = cells * px;
    let mut data = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let cx = x / px;
            let cy = y / px;
            let on = (cx + cy).is_multiple_of(2);
            let i = ((y * size + x) * 4) as usize;
            let (r, g, b) = if on {
                (230u8, 90u8, 60u8)
            } else {
                (40u8, 50u8, 90u8)
            };
            data[i] = r;
            data[i + 1] = g;
            data[i + 2] = b;
            data[i + 3] = 255;
        }
    }
    (data, size)
}

fn load_system_font(backend: &mut WgpuBackend) -> Option<FontHandle> {
    let candidates = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/Library/Fonts/Arial.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "C:/Windows/Fonts/arial.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(backend.load_font(&bytes));
        }
    }
    None
}

/// Build the demo scene: rounded rect, outlined rect, circle, a translucent
/// overlapping rect (z-order), a thin rect used as a "line", a textured quad,
/// and a couple text runs.
fn build_scene(tex: TexHandle, font: Option<FontHandle>) -> DrawScene {
    let mut scene = DrawScene::new();

    // 1. solid rounded rect (lower-left)
    let r1 = scene.create_rect(
        Vec2 { x: 220.0, y: 140.0 },
        Color { r: 0.20, g: 0.55, b: 0.85, a: 1.0 },
        24.0,
    );
    scene.set_transform(r1, tform(80.0, 380.0));
    scene.set_z_height(r1, 1.0);

    // 2. outlined (stroke) rect
    let r2 = scene.create_rect(
        Vec2 { x: 200.0, y: 120.0 },
        Color { r: 0.95, g: 0.85, b: 0.25, a: 1.0 },
        6.0,
    );
    scene.set_transform(r2, tform(360.0, 80.0));
    scene.set_z_height(r2, 1.0);
    scene.set_outline(
        r2,
        Outline {
            thickness: 6.0,
            color: Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 },
        },
    );

    // 3. circle
    let c = scene.create_circle(
        70.0,
        Color { r: 0.85, g: 0.30, b: 0.45, a: 1.0 },
    );
    scene.set_transform(c, tform(720.0, 180.0));
    scene.set_z_height(c, 1.0);

    // 4. translucent rect overlapping the circle (drawn on top -> z-order proof)
    let ov = scene.create_rect(
        Vec2 { x: 180.0, y: 180.0 },
        Color { r: 0.20, g: 0.95, b: 0.55, a: 0.45 },
        12.0,
    );
    scene.set_transform(ov, tform(640.0, 120.0));
    scene.set_z_height(ov, 3.0);

    // 5. a "line": a thin, long rect
    let line = scene.create_rect(
        Vec2 { x: 300.0, y: 4.0 },
        Color { r: 0.9, g: 0.9, b: 0.95, a: 1.0 },
        2.0,
    );
    scene.set_transform(line, tform(80.0, 320.0));
    scene.set_z_height(line, 2.0);

    // 6. textured quad (checkerboard), full-texture src region
    let t = scene.create_tex(
        tex,
        Vec2 { x: 160.0, y: 160.0 },
        Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 },
        Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
    );
    scene.set_transform(t, tform(360.0, 380.0));
    scene.set_z_height(t, 1.0);

    // 7. text
    if let Some(font) = font {
        let title = scene.create_text(
            FontHandle(font.0),
            "draw_engine — wgpu backend".to_string(),
            34.0,
            Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
        );
        scene.set_transform(title, tform(70.0, 24.0));
        scene.set_z_height(title, 5.0);

        let sub = scene.create_text(
            FontHandle(font.0),
            "rects · circles · stroke · texture · z-order".to_string(),
            20.0,
            Color { r: 0.75, g: 0.85, b: 0.95, a: 1.0 },
        );
        scene.set_transform(sub, tform(600.0, 380.0));
        scene.set_z_height(sub, 5.0);
    }

    scene
}

fn tform(x: f32, y: f32) -> Transform {
    Transform {
        translation: Vec2 { x, y },
        rotation_rad: 0.0,
        scale: Vec2 { x: 1.0, y: 1.0 },
    }
}

struct App {
    window: Option<Arc<Window>>,
    backend: Option<WgpuBackend>,
    tex: Option<TexHandle>,
    font: Option<FontHandle>,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            backend: None,
            tex: None,
            font: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("draw_engine demo")
            .with_inner_size(LogicalSize::new(W as f64, H as f64));
        let window = Arc::new(event_loop.create_window(attrs).unwrap());

        let mut backend = pollster::block_on(WgpuBackend::new(window.clone()));
        backend.set_clear_color(Color { r: 0.06, g: 0.07, b: 0.10, a: 1.0 });

        let (rgba, dim) = checkerboard(8, 16);
        let tex = backend.load_texture(&rgba, dim, dim);
        let font = load_system_font(&mut backend);

        self.tex = Some(tex);
        self.font = font;
        self.backend = Some(backend);
        self.window = Some(window.clone());
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                backend.resize(size.width, size.height);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let tex = TexHandle(self.tex.as_ref().unwrap().0);
                let font = self.font.as_ref().map(|f| FontHandle(f.0));
                let scene = build_scene(tex, font);
                let draws = render_to_draws(&scene);
                backend.render(&draws);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    event_loop.run_app(&mut App::new()).unwrap();
}
