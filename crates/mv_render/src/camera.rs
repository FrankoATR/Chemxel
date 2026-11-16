//! Perspective camera.
//!
//! Positions are in microvoxels and stored as `f64` so precision does not degrade
//! far from the origin; the GPU only ever sees positions relative to the rendered
//! volume, which are small and fit comfortably in `f32`.

use glam::{DVec3, Vec3};

/// First-person camera. `yaw = 0, pitch = 0` looks down `-Z`; `+Y` is up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Eye position in world microvoxels.
    pub position: DVec3,
    /// Rotation around +Y in radians (positive turns left).
    pub yaw: f32,
    /// Rotation around the camera's right axis in radians (positive looks up).
    pub pitch: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
}

impl Camera {
    /// Pitch is clamped just short of straight up/down to keep the basis valid.
    pub const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.001;

    pub fn new(position: DVec3, yaw: f32, pitch: f32, fov_y: f32) -> Self {
        Self {
            position,
            yaw,
            pitch: pitch.clamp(-Self::MAX_PITCH, Self::MAX_PITCH),
            fov_y,
        }
    }

    /// Unit view direction.
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    /// Unit vector to the right of the view direction, parallel to the ground.
    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cy, 0.0, -sy)
    }

    /// Unit up vector of the view (orthogonal to forward and right).
    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward())
    }

    /// Forward direction projected onto the ground plane (for walking/flying).
    pub fn flat_forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(-sy, 0.0, -cy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn default_orientation_looks_down_negative_z() {
        let c = Camera::new(DVec3::ZERO, 0.0, 0.0, 1.0);
        assert!(approx(c.forward(), Vec3::NEG_Z));
        assert!(approx(c.right(), Vec3::X));
        assert!(approx(c.up(), Vec3::Y));
    }

    #[test]
    fn basis_is_orthonormal() {
        let c = Camera::new(DVec3::ZERO, 0.7, -0.4, 1.0);
        let (f, r, u) = (c.forward(), c.right(), c.up());
        for v in [f, r, u] {
            assert!((v.length() - 1.0).abs() < 1e-5);
        }
        assert!(f.dot(r).abs() < 1e-5 && f.dot(u).abs() < 1e-5 && r.dot(u).abs() < 1e-5);
    }

    #[test]
    fn positive_yaw_turns_left() {
        let c = Camera::new(DVec3::ZERO, std::f32::consts::FRAC_PI_2, 0.0, 1.0);
        assert!(approx(c.forward(), Vec3::NEG_X));
    }

    #[test]
    fn pitch_is_clamped() {
        let c = Camera::new(DVec3::ZERO, 0.0, 10.0, 1.0);
        assert!(c.pitch <= Camera::MAX_PITCH);
    }
}
