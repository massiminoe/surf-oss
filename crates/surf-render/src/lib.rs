//! wgpu renderer: textured static mesh + HUD + PB ghost.

mod backdrop;
mod camera;
mod ghost;
mod hud;
mod mesh;
mod offscreen;
mod pipeline;
mod skybox;
mod trail;

pub use camera::Camera;
pub use ghost::GhostPose;
pub use hud::{
    format_hud_time, layout_for, page_layout, row_height, PageLayout, PageSpec, PanelLayout, Rect,
};
pub use hud::{HudRenderer, HudState, HudTimerPhase, ReplayHud, ShowKeysState};
pub use hud::{MenuPage, MenuPanel, MenuRow, RowKind, RowTone, PANEL_ROWS_MAX};
pub use mesh::{GpuMaterials, GpuMesh, MeshVertex};
pub use offscreen::{render_to_png, Offscreen};
pub use pipeline::{Renderer, ViewParams};
pub use trail::TrailPoint;
