//! wgpu renderer: textured static mesh + HUD + PB ghost.

mod camera;
mod ghost;
mod hud;
mod mesh;
mod pipeline;
mod skybox;
mod trail;

pub use camera::Camera;
pub use ghost::GhostPose;
pub use hud::{
    HudRenderer, HudState, HudTimerPhase, MenuHud, MenuRecentEntry, ShowKeysState,
};
pub use mesh::{GpuMaterials, GpuMesh, MeshVertex};
pub use pipeline::{Renderer, ViewParams};
pub use trail::TrailPoint;
