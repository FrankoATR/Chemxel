//! Free-flying first-person camera controller.
//!
//! `WASD` move on the horizontal plane, `Space`/`Ctrl` move up/down, `Shift`
//! speeds up, the mouse rotates (while the cursor is captured).

use glam::Vec3;
use mv_core::config::ControlsConfig;
use mv_core::consts::VOXELS_PER_BLOCK;
use mv_render::Camera;
use winit::keyboard::KeyCode;

use crate::input::InputState;

#[derive(Debug, Clone)]
pub struct FlyController {
    /// Base speed in microvoxels per second.
    speed: f32,
    fast_multiplier: f32,
    sensitivity: f32,
}

impl FlyController {
    pub fn new(controls: &ControlsConfig) -> Self {
        Self {
            speed: controls.fly_speed * VOXELS_PER_BLOCK as f32,
            fast_multiplier: controls.fast_multiplier,
            sensitivity: controls.mouse_sensitivity,
        }
    }

    /// Applies the mouse motion since the last call (if `look` is true).
    pub fn look(&self, camera: &mut Camera, input: &mut InputState, look: bool) {
        let mouse = input.take_mouse_delta();
        if look {
            camera.yaw -= mouse.x * self.sensitivity;
            camera.pitch = (camera.pitch - mouse.y * self.sensitivity)
                .clamp(-Camera::MAX_PITCH, Camera::MAX_PITCH);
            camera.yaw = camera.yaw.rem_euclid(std::f32::consts::TAU);
        }
    }

    /// Applies mouse look (if `look` is true) and keyboard movement for `dt` seconds.
    pub fn update(&self, camera: &mut Camera, input: &mut InputState, dt: f32, look: bool) {
        self.look(camera, input, look);

        let axis = |pos: KeyCode, neg: KeyCode| {
            f32::from(u8::from(input.is_held(pos))) - f32::from(u8::from(input.is_held(neg)))
        };
        let forward = axis(KeyCode::KeyW, KeyCode::KeyS);
        let strafe = axis(KeyCode::KeyD, KeyCode::KeyA);
        let vertical = axis(KeyCode::Space, KeyCode::ControlLeft);

        let wish = camera.flat_forward() * forward + camera.right() * strafe + Vec3::Y * vertical;
        if wish == Vec3::ZERO {
            return;
        }
        let fast = input.is_held(KeyCode::ShiftLeft) || input.is_held(KeyCode::ShiftRight);
        let speed = self.speed * if fast { self.fast_multiplier } else { 1.0 };
        camera.position += (wish.normalize() * speed * dt).as_dvec3();
    }
}
