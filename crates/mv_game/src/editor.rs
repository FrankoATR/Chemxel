//! Voxel editing: what the crosshair points at and the spherical brush.

use std::time::Duration;

use glam::DVec3;
use mv_core::WorldPos;
use mv_core::config::EditorConfig;
use mv_materials::{MaterialId, MaterialRegistry, MatterState};
use mv_render::{BrushPreview, Camera};
use mv_world::World;
use mv_world::edit::fill_sphere;
use mv_world::raycast::{RayHit, raycast};

/// What a brush stroke does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushAction {
    /// Turn voxels into air.
    Remove,
    /// Add the selected material in front of the hit face, without overwriting
    /// solids or powders.
    Place,
}

/// What a brush stroke changed, for the simulation.
#[derive(Debug, Clone, Copy)]
pub struct Edit {
    pub min: WorldPos,
    pub max: WorldPos,
    /// Material placed (None when removing).
    pub placed: Option<MaterialId>,
}

/// Brush settings, edited from the UI and the keyboard.
#[derive(Debug, Clone)]
pub struct Brush {
    pub radius: f32,
    pub max_radius: f32,
    /// Index into [`Editor::placeable`].
    pub selected: usize,
}

#[derive(Debug)]
pub struct Editor {
    pub brush: Brush,
    reach: f64,
    repeat: Duration,
    /// Materials that can be placed (everything but air), in registry order.
    placeable: Vec<MaterialId>,
    /// Per material: may `Place` overwrite it? (air, liquids and gases).
    overwritable: Vec<bool>,
    target: Option<RayHit>,
    held: Option<BrushAction>,
    since_last_stroke: Duration,
}

impl Editor {
    pub fn new(config: &EditorConfig, registry: &MaterialRegistry) -> Self {
        let placeable: Vec<MaterialId> = registry
            .iter()
            .filter(|(id, _)| !id.is_air())
            .map(|(id, _)| id)
            .collect();
        let overwritable = registry
            .iter()
            .map(|(id, m)| id.is_air() || matches!(m.state, MatterState::Liquid | MatterState::Gas))
            .collect();
        let selected = registry
            .id("sand")
            .and_then(|sand| placeable.iter().position(|&id| id == sand))
            .unwrap_or(0);
        Self {
            brush: Brush {
                radius: config.brush_radius,
                max_radius: config.max_brush_radius,
                selected,
            },
            reach: config.reach,
            repeat: Duration::from_secs_f32(1.0 / config.repeat_hz),
            placeable,
            overwritable,
            target: None,
            held: None,
            since_last_stroke: Duration::ZERO,
        }
    }

    pub fn placeable(&self) -> &[MaterialId] {
        &self.placeable
    }

    pub fn selected_material(&self) -> Option<MaterialId> {
        self.placeable.get(self.brush.selected).copied()
    }

    /// Selects the `index`-th placeable material, if it exists.
    pub fn select(&mut self, index: usize) {
        if index < self.placeable.len() {
            self.brush.selected = index;
        }
    }

    /// Changes the radius by `steps` (mouse wheel notches).
    pub fn adjust_radius(&mut self, steps: f32) {
        let b = &mut self.brush;
        b.radius = (b.radius + steps).clamp(1.0, b.max_radius);
    }

    pub fn target(&self) -> Option<&RayHit> {
        self.target.as_ref()
    }

    /// Starts (or stops, with `None`) a held stroke. A new stroke applies at once.
    pub fn set_held(&mut self, action: Option<BrushAction>) {
        if action.is_some() && action != self.held {
            self.since_last_stroke = self.repeat;
        }
        self.held = action;
    }

    /// Updates the target under the crosshair and applies held strokes.
    /// Returns what was edited, if any voxel changed (for the simulation).
    pub fn update(&mut self, world: &mut World, camera: &Camera, dt: Duration) -> Option<Edit> {
        let _span = tracing::trace_span!("editor_update").entered();
        self.target = raycast(
            world,
            camera.position,
            camera.forward().as_dvec3(),
            self.reach,
        );
        let action = self.held?;
        self.since_last_stroke += dt;
        if self.since_last_stroke < self.repeat {
            return None;
        }
        self.since_last_stroke = Duration::ZERO;
        let hit = self.target?;
        let (center, changed, placed) = match action {
            BrushAction::Remove => (
                hit.voxel,
                fill_sphere(world, hit.voxel, self.brush.radius, MaterialId::AIR, |_| {
                    true
                }),
                None,
            ),
            BrushAction::Place => {
                let material = self.selected_material()?;
                let overwritable = &self.overwritable;
                let center = hit.adjacent();
                let changed = fill_sphere(world, center, self.brush.radius, material, |m| {
                    overwritable.get(m.index()).copied().unwrap_or(false)
                });
                (center, changed, Some(material))
            }
        };
        // Refresh the target so the preview follows the edited surface this frame.
        self.target = raycast(
            world,
            camera.position,
            camera.forward().as_dvec3(),
            self.reach,
        );
        let r = self.brush.radius.ceil() as i64;
        (changed > 0).then(|| Edit {
            min: WorldPos(center.0 - r),
            max: WorldPos(center.0 + r),
            placed,
        })
    }

    /// Sphere to highlight in the frame.
    pub fn preview(&self) -> Option<BrushPreview> {
        self.target.map(|hit| BrushPreview {
            center: hit.voxel.0.as_dvec3() + DVec3::splat(0.5),
            radius: self.brush.radius,
        })
    }
}
