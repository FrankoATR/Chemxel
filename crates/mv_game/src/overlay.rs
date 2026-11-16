//! Debug overlay (F3): performance, world and GPU statistics, and the brush tools.

use glam::{DVec3, IVec3};
use mv_core::consts::VOXELS_PER_BLOCK;
use mv_materials::MaterialRegistry;
use mv_render::VolumeStats;
use mv_sim::TickStats;
use mv_world::WorldStats;
use mv_world::raycast::RayHit;

use crate::editor::Editor;
use crate::game_world::SyncStats;

/// Exponential moving average for noisy per-frame timings.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ema(f64);

impl Ema {
    pub fn add(&mut self, sample: f64) {
        self.0 = if self.0 == 0.0 {
            sample
        } else {
            self.0 * 0.95 + sample * 0.05
        };
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

/// CPU timings in milliseconds, smoothed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Timings {
    pub frame: Ema,
    pub sim: Ema,
    pub editor: Ema,
    pub world: Ema,
    pub render: Ema,
}

/// Everything the overlay shows that is not owned by the editor.
#[derive(Debug)]
pub struct OverlayData<'a> {
    pub fps: f64,
    pub worst_frame_ms: f64,
    pub timings: &'a Timings,
    pub tick: u64,
    pub sim: &'a TickStats,
    pub sim_active: usize,
    pub sim_paused: bool,
    pub heat_view: bool,
    pub target_temperature: Option<f32>,
    pub camera: DVec3,
    pub sync: &'a SyncStats,
    pub world: &'a WorldStats,
    pub gpu: &'a VolumeStats,
    pub registry: &'a MaterialRegistry,
    pub cursor_grabbed: bool,
}

const MIB: f64 = 1024.0 * 1024.0;

/// Buttons pressed in the overlay this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayActions {
    pub drop_test: bool,
    pub lava_on_ice: bool,
    pub forest_fire: bool,
    pub toggle_pause: bool,
}

pub fn draw(ctx: &egui::Context, data: &OverlayData<'_>, editor: &mut Editor) -> OverlayActions {
    let mut actions = OverlayActions::default();
    egui::Window::new("Chemxel")
        .default_pos([12.0, 12.0])
        .default_width(300.0)
        .resizable(false)
        .show(ctx, |ui| {
            performance(ui, data);
            ui.separator();
            world(ui, data, editor.target());
            ui.separator();
            simulation(ui, data, &mut actions);
            ui.separator();
            brush(ui, data.registry, editor);
            ui.separator();
            help(ui, data.cursor_grabbed);
        });
    actions
}

fn simulation(ui: &mut egui::Ui, d: &OverlayData<'_>, actions: &mut OverlayActions) {
    let s = d.sim;
    egui::Grid::new("sim").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, label: &str, value: String| {
            ui.label(label);
            ui.monospace(value);
            ui.end_row();
        };
        row(
            ui,
            "Sim",
            format!(
                "{}{} active chunks",
                if d.sim_paused { "PAUSED, " } else { "" },
                d.sim_active
            ),
        );
        row(
            ui,
            "Moves/tick",
            format!("{} ({} across borders)", s.moves, s.border_moves),
        );
        row(ui, "Cells scanned", format!("{}", s.cells_scanned));
        row(
            ui,
            "Thermal",
            format!(
                "{} chunks, {} phase changes/tick",
                s.thermal_chunks, s.transitions
            ),
        );
        row(
            ui,
            "Chemistry",
            format!(
                "{} reactions, {} decays, {} flames/tick",
                s.reactions, s.decays, s.flames
            ),
        );
        row(
            ui,
            "Target temp.",
            d.target_temperature
                .map_or_else(|| "-".to_owned(), |t| format!("{t:.1} °C")),
        );
    });
    if d.heat_view {
        ui.small("Heat view (F5): blue < ambient · grey ambient · yellow 100 °C · orange 600 °C · white 1500 °C");
    }
    ui.horizontal(|ui| {
        if ui
            .button("Drop 100k sand + water")
            .on_hover_text("Above the voxel under the crosshair")
            .clicked()
        {
            actions.drop_test = true;
        }
        if ui
            .button("Lava on ice")
            .on_hover_text("On the voxel under the crosshair")
            .clicked()
        {
            actions.lava_on_ice = true;
        }
        if ui
            .button("Forest fire")
            .on_hover_text("A 6×6 forest around the voxel under the crosshair, lit at one corner")
            .clicked()
        {
            actions.forest_fire = true;
        }
        let label = if d.sim_paused {
            "Resume (P)"
        } else {
            "Pause (P)"
        };
        if ui.button(label).clicked() {
            actions.toggle_pause = true;
        }
    });
}

fn performance(ui: &mut egui::Ui, d: &OverlayData<'_>) {
    let t = d.timings;
    ui.heading(format!("{:.0} FPS", d.fps));
    ui.label(format!(
        "worst frame (last second): {:.1} ms",
        d.worst_frame_ms
    ));
    egui::Grid::new("perf").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, label: &str, value: String| {
            ui.label(label);
            ui.monospace(value);
            ui.end_row();
        };
        row(ui, "Frame", format!("{:6.2} ms", t.frame.get()));
        row(
            ui,
            "Sim (CPU)",
            format!("{:6.2} ms  tick {}", t.sim.get(), d.tick),
        );
        row(ui, "Editor (CPU)", format!("{:6.2} ms", t.editor.get()));
        row(ui, "World+upload", format!("{:6.2} ms", t.world.get()));
        row(ui, "Render (CPU)", format!("{:6.2} ms", t.render.get()));
    });
}

fn world(ui: &mut egui::Ui, d: &OverlayData<'_>, target: Option<&RayHit>) {
    let s = d.sync;
    let w = d.world;
    let g = d.gpu;
    let block = VOXELS_PER_BLOCK as f64;
    let chunk: IVec3 = crate::game_world::GameWorld::chunk_of(d.camera);
    egui::Grid::new("world").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, label: &str, value: String| {
            ui.label(label);
            ui.monospace(value);
            ui.end_row();
        };
        row(
            ui,
            "Position",
            format!(
                "{:.1}, {:.1}, {:.1} m",
                d.camera.x / block,
                d.camera.y / block,
                d.camera.z / block
            ),
        );
        row(
            ui,
            "Chunk",
            format!("{}, {}, {}", chunk.x, chunk.y, chunk.z),
        );
        row(
            ui,
            "Chunks loaded",
            format!("{} ({} uniform)", w.chunks, w.uniform_chunks),
        );
        row(
            ui,
            "Streaming",
            format!(
                "{} missing, {} generating",
                s.stream.missing, s.stream.inflight
            ),
        );
        row(
            ui,
            "Uploads",
            format!(
                "{} queued, {:.0} KiB/frame",
                s.pending_uploads,
                s.upload.bytes as f64 / 1024.0
            ),
        );
        row(
            ui,
            "CPU voxels",
            format!(
                "{:.0} MiB ({} dense bricks)",
                w.heap_bytes as f64 / MIB,
                w.dense_bricks
            ),
        );
        row(
            ui,
            "GPU bricks",
            format!(
                "{:.0}/{:.0} MiB ({:.0}%)",
                g.brick_bytes() as f64 / MIB,
                f64::from(g.brick_capacity) * mv_render::BRICK_BYTES as f64 / MIB,
                100.0 * f64::from(g.bricks_in_use) / f64::from(g.brick_capacity.max(1))
            ),
        );
        row(
            ui,
            "GPU chunks",
            format!(
                "{} resident, {}/{} slots",
                g.resident_chunks, g.chunk_slots_in_use, g.chunk_slot_capacity
            ),
        );
        let target = target.map_or_else(
            || "-".to_owned(),
            |hit| {
                let v = hit.voxel.0;
                format!(
                    "{} at {}, {}, {}",
                    d.registry.get(hit.material).name,
                    v.x,
                    v.y,
                    v.z
                )
            },
        );
        row(ui, "Target", target);
    });
}

fn brush(ui: &mut egui::Ui, registry: &MaterialRegistry, editor: &mut Editor) {
    ui.label("Brush");
    let max = editor.brush.max_radius;
    ui.add(egui::Slider::new(&mut editor.brush.radius, 1.0..=max).text("radius (voxels)"));
    let mut selected = None;
    ui.horizontal_wrapped(|ui| {
        for (i, &id) in editor.placeable().iter().enumerate() {
            let m = registry.get(id);
            let [r, g, b, _] = m.color;
            let label = if i < 9 {
                format!("{} {}", i + 1, m.name)
            } else {
                m.name.clone()
            };
            let text = egui::RichText::new(label).color(egui::Color32::from_rgb(r, g, b));
            if ui
                .selectable_label(editor.brush.selected == i, text)
                .clicked()
            {
                selected = Some(i);
            }
        }
    });
    if let Some(i) = selected {
        editor.select(i);
    }
}

fn help(ui: &mut egui::Ui, grabbed: bool) {
    let text = if grabbed {
        "LMB remove · RMB place · wheel radius · 1-9 material · P pause sim\nWASD/Space/Ctrl move · Shift fast · Esc release mouse · F3 overlay · F5 heat view"
    } else {
        "Click the world to capture the mouse · Esc quits · F3 overlay"
    };
    ui.small(text);
}

/// Crosshair at the screen centre (drawn while the mouse is captured).
pub fn crosshair(ctx: &egui::Context) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("crosshair"),
    ));
    let c = ctx.content_rect().center();
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_white_alpha(220));
    let shadow = egui::Stroke::new(4.0, egui::Color32::from_black_alpha(120));
    for s in [shadow, stroke] {
        painter.line_segment([c - egui::vec2(8.0, 0.0), c + egui::vec2(8.0, 0.0)], s);
        painter.line_segment([c - egui::vec2(0.0, 8.0), c + egui::vec2(0.0, 8.0)], s);
    }
}
