//! wgpu renderer: textured static mesh + HUD + PB ghost.

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
pub use hud::{menu_layout, MenuLayout, MenuPanel, PanelLayout};
pub use hud::{HudRenderer, HudState, HudTimerPhase, MenuHud, MenuRecentEntry, ShowKeysState};
pub use mesh::{GpuMaterials, GpuMesh, MeshVertex};
pub use offscreen::{render_to_png, Offscreen};
pub use pipeline::{Renderer, ViewParams};
pub use trail::TrailPoint;
