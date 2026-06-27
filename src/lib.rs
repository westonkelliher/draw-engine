//! draw_engine — a 2D draw engine on wgpu.
//!
//! `DrawScene` --render_to_draws--> `Vec<DrawCall>` --> `WgpuBackend`.

pub mod draw_layer;
pub mod wgpu_backend;

pub use draw_layer::{
    render_to_draws, Affine2, Color, Content, DrawCall, DrawScene, DrawStyle, FontHandle, NodeId,
    Outline, Rect, Shape, ShaderHandle, TexHandle, Transform, Vec2,
};
pub use wgpu_backend::WgpuBackend;
