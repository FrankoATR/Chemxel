//! Keyboard and mouse state accumulated from winit events.

use std::collections::HashSet;

use glam::Vec2;
use winit::keyboard::KeyCode;

/// Snapshot of held keys plus mouse motion accumulated since the last frame.
#[derive(Debug, Default)]
pub struct InputState {
    held: HashSet<KeyCode>,
    mouse_delta: Vec2,
}

impl InputState {
    pub fn set_key(&mut self, key: KeyCode, pressed: bool) {
        if pressed {
            self.held.insert(key);
        } else {
            self.held.remove(&key);
        }
    }

    pub fn is_held(&self, key: KeyCode) -> bool {
        self.held.contains(&key)
    }

    pub fn add_mouse_motion(&mut self, dx: f64, dy: f64) {
        self.mouse_delta += Vec2::new(dx as f32, dy as f32);
    }

    /// Returns the accumulated mouse motion and resets it.
    pub fn take_mouse_delta(&mut self) -> Vec2 {
        std::mem::take(&mut self.mouse_delta)
    }

    /// Forgets every held key (e.g. when the window loses focus, so keys released
    /// elsewhere do not stay stuck).
    pub fn release_all(&mut self) {
        self.held.clear();
        self.mouse_delta = Vec2::ZERO;
    }
}
