//! Inventory: an 8-slot hotbar, a GTA-style selection wheel (hold `Tab`) and a
//! creative catalogue screen (`Enter`) with category tabs and drag and drop.
//!
//! * The **hotbar** holds what the brush places; `1`–`8` select a slot.
//! * The **wheel** shows the 8 slots in a ring while `Tab` is held. The direction
//!   of the mouse movement since it opened (from wherever the view was) highlights
//!   a slot; releasing `Tab` (or clicking) picks it.
//! * The **catalogue** lists every material by category (tabs on the left, 8×8
//!   pages) above the hotbar row. Drag from the catalogue onto a slot to put it
//!   there, between slots to swap them, out of the hotbar to clear a slot.
//!   Clicking an item puts it in the selected slot; right-clicking a slot clears it.

use std::sync::Arc;

use glam::Vec2;
use mv_materials::{MaterialId, MaterialRegistry};

/// Hotbar slots (and wheel sectors).
pub const SLOTS: usize = 8;
/// Catalogue page: 8 × 8 items.
const PAGE: usize = 64;
const ALL: &str = "All";

/// What the default hotbar holds, by name (missing materials are skipped).
const DEFAULT_HOTBAR: [&str; SLOTS] = [
    "stone", "dirt", "sand", "water", "lava", "wood", "fire", "acid",
];

#[derive(Debug, Clone)]
pub struct Inventory {
    pub hotbar: [Option<MaterialId>; SLOTS],
    pub selected: usize,
    /// (name, items), "All" first, then categories in order of first appearance.
    categories: Vec<(String, Vec<MaterialId>)>,
}

impl Inventory {
    pub fn new(registry: &MaterialRegistry) -> Self {
        let mut inv = Self {
            hotbar: [None; SLOTS],
            selected: 0,
            categories: Vec::new(),
        };
        inv.set_materials(registry);
        for (slot, name) in inv.hotbar.iter_mut().zip(DEFAULT_HOTBAR) {
            *slot = registry.id(name);
        }
        inv
    }

    /// Rebuilds the catalogue (after a hot reload). Hotbar items that no longer
    /// exist are cleared.
    pub fn set_materials(&mut self, registry: &MaterialRegistry) {
        let mut categories: Vec<(String, Vec<MaterialId>)> = vec![(ALL.to_owned(), Vec::new())];
        for (id, m) in registry.iter() {
            if id.is_air() {
                continue;
            }
            categories[0].1.push(id);
            match categories.iter_mut().find(|(c, _)| *c == m.category) {
                Some((_, items)) => items.push(id),
                None => categories.push((m.category.clone(), vec![id])),
            }
        }
        self.categories = categories;
        for slot in &mut self.hotbar {
            if slot.is_some_and(|m| m.index() >= registry.len()) {
                *slot = None;
            }
        }
    }

    pub fn categories(&self) -> &[(String, Vec<MaterialId>)] {
        &self.categories
    }

    /// What the selected slot holds.
    pub fn current(&self) -> Option<MaterialId> {
        self.hotbar[self.selected]
    }

    pub fn select(&mut self, slot: usize) {
        if slot < SLOTS {
            self.selected = slot;
        }
    }

    pub fn assign(&mut self, slot: usize, item: MaterialId) {
        if slot < SLOTS {
            self.hotbar[slot] = Some(item);
        }
    }

    /// Moves slot `from` to `to`, swapping their contents.
    pub fn swap(&mut self, from: usize, to: usize) {
        if from < SLOTS && to < SLOTS {
            self.hotbar.swap(from, to);
        }
    }

    pub fn clear(&mut self, slot: usize) {
        if slot < SLOTS {
            self.hotbar[slot] = None;
        }
    }

    /// Hotbar as material names, for the save file.
    pub fn to_names(&self, registry: &MaterialRegistry) -> Vec<Option<String>> {
        self.hotbar
            .iter()
            .map(|s| s.map(|m| registry.get(m).name.clone()))
            .collect()
    }

    /// Restores a saved hotbar (unknown names become empty slots).
    pub fn load_names(
        &mut self,
        names: &[Option<String>],
        selected: usize,
        registry: &MaterialRegistry,
    ) {
        for (slot, name) in self.hotbar.iter_mut().zip(names) {
            *slot = name.as_deref().and_then(|n| registry.id(n));
        }
        self.select(selected.min(SLOTS - 1));
    }
}

/// State of the selection wheel while `Tab` is held.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wheel {
    pub open: bool,
    /// Accumulated mouse motion since opening, clamped to [`Wheel::REACH`].
    aim: Vec2,
}

impl Wheel {
    /// Pixels of mouse motion before a direction counts.
    const DEAD_ZONE: f32 = 24.0;
    /// The aim vector is clamped to this length, so a new direction takes over
    /// after a short movement.
    const REACH: f32 = 90.0;

    pub fn open(&mut self) {
        *self = Self {
            open: true,
            aim: Vec2::ZERO,
        };
    }

    /// Adds raw mouse motion (screen pixels, y down).
    pub fn add_motion(&mut self, delta: Vec2) {
        self.aim = (self.aim + delta).clamp_length_max(Self::REACH);
    }

    /// The sector the mouse points at: 0 at the top, then clockwise.
    pub fn highlighted(&self) -> Option<usize> {
        sector(self.aim, Self::DEAD_ZONE)
    }

    /// Closes the wheel; returns the slot it pointed at, if any.
    pub fn close(&mut self) -> Option<usize> {
        let pick = self.highlighted();
        *self = Self::default();
        pick
    }

    /// Closes without picking.
    pub fn cancel(&mut self) {
        *self = Self::default();
    }
}

/// Sector of direction `v` (screen space, y down) among [`SLOTS`]: 0 points up,
/// indices grow clockwise. `None` inside the dead zone.
pub fn sector(v: Vec2, dead_zone: f32) -> Option<usize> {
    if v.length() < dead_zone {
        return None;
    }
    let angle = v.x.atan2(-v.y).rem_euclid(std::f32::consts::TAU);
    let step = std::f32::consts::TAU / SLOTS as f32;
    Some(((angle / step).round() as usize) % SLOTS)
}

/// UI state of the catalogue screen.
#[derive(Debug, Clone, Copy, Default)]
pub struct Screen {
    pub open: bool,
    tab: usize,
    page: usize,
}

impl Screen {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }
}

/// Something being dragged in the catalogue screen.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    Catalogue(MaterialId),
    Slot(usize),
}

fn color(registry: &MaterialRegistry, m: MaterialId) -> egui::Color32 {
    let [r, g, b, _] = registry.get(m).color;
    egui::Color32::from_rgb(r, g, b)
}

/// Paints one slot (empty or with an item) in `rect`.
fn paint_slot(
    painter: &egui::Painter,
    rect: egui::Rect,
    item: Option<MaterialId>,
    registry: &MaterialRegistry,
    highlight: bool,
    label: Option<&str>,
) {
    let bg = if highlight {
        egui::Color32::from_rgba_unmultiplied(255, 230, 140, 90)
    } else {
        egui::Color32::from_black_alpha(150)
    };
    let stroke = if highlight {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 220, 120))
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(60))
    };
    painter.rect(rect, 6.0, bg, stroke, egui::StrokeKind::Inside);
    if let Some(m) = item {
        let swatch = rect.shrink(rect.width() * 0.2);
        painter.rect_filled(
            swatch.translate(egui::vec2(0.0, -rect.height() * 0.06)),
            4.0,
            color(registry, m),
        );
        let name = &registry.get(m).name;
        let font = egui::FontId::proportional((rect.width() * 0.2).clamp(9.0, 13.0));
        painter.text(
            rect.center_bottom() - egui::vec2(0.0, 2.0),
            egui::Align2::CENTER_BOTTOM,
            name,
            font,
            egui::Color32::WHITE,
        );
    }
    if let Some(label) = label {
        painter.text(
            rect.left_top() + egui::vec2(4.0, 2.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::monospace(11.0),
            egui::Color32::from_white_alpha(200),
        );
    }
}

/// The hotbar at the bottom of the screen.
pub fn hud(ctx: &egui::Context, inv: &Inventory, registry: &MaterialRegistry) {
    let size = 46.0;
    let gap = 4.0;
    let width = SLOTS as f32 * (size + gap) - gap;
    let screen = ctx.content_rect();
    let origin = egui::pos2(
        screen.center().x - width * 0.5,
        screen.bottom() - size - 14.0,
    );
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("hotbar"),
    ));
    for i in 0..SLOTS {
        let rect = egui::Rect::from_min_size(
            origin + egui::vec2(i as f32 * (size + gap), 0.0),
            egui::vec2(size, size),
        );
        paint_slot(
            &painter,
            rect,
            inv.hotbar[i],
            registry,
            i == inv.selected,
            Some(&(i + 1).to_string()),
        );
    }
}

/// The selection wheel (drawn while `Tab` is held).
pub fn wheel(ctx: &egui::Context, inv: &Inventory, wheel: &Wheel, registry: &MaterialRegistry) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("wheel"),
    ));
    let screen = ctx.content_rect();
    let center = screen.center();
    let (inner, outer) = (70.0, 190.0);
    painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(90));
    let step = std::f32::consts::TAU / SLOTS as f32;
    let highlighted = wheel.highlighted();
    let point = |angle: f32, r: f32| center + egui::vec2(angle.sin(), -angle.cos()) * r;
    for i in 0..SLOTS {
        let mid = i as f32 * step;
        let (a0, a1) = (mid - step * 0.5 + 0.03, mid + step * 0.5 - 0.03);
        // Annular sector as a polygon: outer arc forward, inner arc back.
        let n = 12;
        let mut pts = Vec::with_capacity(2 * (n + 1));
        for k in 0..=n {
            pts.push(point(a0 + (a1 - a0) * k as f32 / n as f32, outer));
        }
        for k in (0..=n).rev() {
            pts.push(point(a0 + (a1 - a0) * k as f32 / n as f32, inner));
        }
        let hot = highlighted == Some(i);
        let fill = if hot {
            egui::Color32::from_rgba_unmultiplied(255, 220, 120, 150)
        } else if i == inv.selected {
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 60)
        } else {
            egui::Color32::from_black_alpha(160)
        };
        // Sectors are not convex as a whole: draw them as quads per arc step.
        for k in 0..n {
            let quad = vec![
                pts[k],
                pts[k + 1],
                pts[2 * n + 1 - (k + 1)],
                pts[2 * n + 1 - k],
            ];
            painter.add(egui::Shape::convex_polygon(quad, fill, egui::Stroke::NONE));
        }
        let mut outline = pts.clone();
        outline.push(pts[0]);
        painter.add(egui::Shape::line(
            outline,
            egui::Stroke::new(
                if hot { 2.5 } else { 1.0 },
                egui::Color32::from_white_alpha(if hot { 230 } else { 70 }),
            ),
        ));
        let c = point(mid, (inner + outer) * 0.5);
        let item_rect = egui::Rect::from_center_size(c, egui::vec2(58.0, 58.0));
        if let Some(m) = inv.hotbar[i] {
            painter.rect_filled(
                item_rect.shrink(12.0).translate(egui::vec2(0.0, -6.0)),
                5.0,
                color(registry, m),
            );
            painter.text(
                c + egui::vec2(0.0, 24.0),
                egui::Align2::CENTER_CENTER,
                &registry.get(m).name,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
        } else {
            painter.text(
                c,
                egui::Align2::CENTER_CENTER,
                "empty",
                egui::FontId::proportional(13.0),
                egui::Color32::from_white_alpha(120),
            );
        }
        painter.text(
            point(mid, outer - 14.0),
            egui::Align2::CENTER_CENTER,
            (i + 1).to_string(),
            egui::FontId::monospace(12.0),
            egui::Color32::from_white_alpha(180),
        );
    }
    let pick = highlighted.unwrap_or(inv.selected);
    let name = inv.hotbar[pick].map_or("empty", |m| registry.get(m).name.as_str());
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(18.0),
        egui::Color32::WHITE,
    );
    painter.text(
        center + egui::vec2(0.0, outer + 22.0),
        egui::Align2::CENTER_CENTER,
        "move the mouse towards a slot · release Tab or click to pick · 1–8",
        egui::FontId::proportional(13.0),
        egui::Color32::from_white_alpha(200),
    );
}

/// Allocates a slot widget; returns its response (click and drag aware).
fn slot_widget(
    ui: &mut egui::Ui,
    size: f32,
    item: Option<MaterialId>,
    registry: &MaterialRegistry,
    highlight: bool,
    label: Option<&str>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let hovered = response.hovered();
    paint_slot(
        ui.painter(),
        rect,
        item,
        registry,
        highlight || hovered,
        label,
    );
    match item {
        Some(m) => response.on_hover_text(&registry.get(m).name),
        None => response,
    }
}

/// The catalogue screen (`Enter`). Returns false when the user closed it.
pub fn screen(
    ctx: &egui::Context,
    inv: &mut Inventory,
    state: &mut Screen,
    registry: &MaterialRegistry,
) -> bool {
    let mut open = true;
    let size = 46.0;
    egui::Window::new("Inventory")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                // Category tabs.
                ui.vertical(|ui| {
                    ui.set_width(120.0);
                    for (i, (name, items)) in inv.categories().iter().enumerate() {
                        let label = format!("{name} ({})", items.len());
                        if ui.add_sized([116.0, 28.0], egui::Button::selectable(state.tab == i, label)).clicked() {
                            state.tab = i;
                            state.page = 0;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    let tab = state.tab.min(inv.categories().len().saturating_sub(1));
                    let items: Vec<MaterialId> = inv.categories().get(tab).map(|(_, v)| v.clone()).unwrap_or_default();
                    let pages = items.len().div_ceil(PAGE).max(1);
                    state.page = state.page.min(pages - 1);
                    let page = &items[(state.page * PAGE).min(items.len())..((state.page + 1) * PAGE).min(items.len())];
                    // 8 × 8 catalogue grid; dropping a hotbar slot here clears it.
                    let grid = egui::Frame::new().show(ui, |ui| {
                        egui::Grid::new("catalogue").spacing([4.0, 4.0]).show(ui, |ui| {
                            for row in 0..8 {
                                for col in 0..8 {
                                    let item = page.get(row * 8 + col).copied();
                                    match item {
                                        Some(m) => {
                                            let id = egui::Id::unique(("cat", m.0));
                                            let r = ui.dnd_drag_source(id, Drag::Catalogue(m), |ui| {
                                                slot_widget(ui, size, Some(m), registry, false, None)
                                            });
                                            if r.inner.clicked() {
                                                let s = inv.selected;
                                                inv.assign(s, m);
                                            }
                                        }
                                        None => {
                                            slot_widget(ui, size, None, registry, false, None);
                                        }
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    });
                    if let Some(drag) = grid.response.dnd_release_payload::<Drag>()
                        && let Drag::Slot(s) = *drag
                    {
                        inv.clear(s);
                    }
                    if pages > 1 {
                        ui.horizontal(|ui| {
                            if ui.button("◀").clicked() && state.page > 0 {
                                state.page -= 1;
                            }
                            ui.label(format!("page {}/{pages}", state.page + 1));
                            if ui.button("▶").clicked() && state.page + 1 < pages {
                                state.page += 1;
                            }
                        });
                    }
                    ui.add_space(6.0);
                    ui.separator();
                    ui.label("Hotbar (Tab wheel · 1–8)");
                    hotbar_row(ui, inv, registry, size);
                });
            });
            ui.add_space(4.0);
            ui.small("Drag an item onto a slot · drag between slots to swap · drag a slot out to clear it · click an item to put it in the selected slot · right-click a slot to clear it · Enter/Esc closes");
        });
    open
}

/// The 8 hotbar slots inside the catalogue screen, as drag sources and targets.
fn hotbar_row(ui: &mut egui::Ui, inv: &mut Inventory, registry: &MaterialRegistry, size: f32) {
    let mut action: Option<(usize, Arc<Drag>)> = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for i in 0..SLOTS {
            let item = inv.hotbar[i];
            let label = (i + 1).to_string();
            let selected = i == inv.selected;
            let response = if item.is_some() {
                let r = ui.dnd_drag_source(egui::Id::unique(("slot", i)), Drag::Slot(i), |ui| {
                    slot_widget(ui, size, item, registry, selected, Some(&label))
                });
                if r.inner.clicked() {
                    inv.select(i);
                }
                if r.inner.secondary_clicked() {
                    inv.clear(i);
                }
                r.response.union(r.inner)
            } else {
                let r = slot_widget(ui, size, None, registry, selected, Some(&label));
                if r.clicked() {
                    inv.select(i);
                }
                r
            };
            if let Some(drag) = response.dnd_release_payload::<Drag>() {
                action = Some((i, drag));
            }
        }
    });
    if let Some((target, drag)) = action {
        match *drag {
            Drag::Catalogue(m) => inv.assign(target, m),
            Drag::Slot(from) => inv.swap(from, target),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> MaterialRegistry {
        MaterialRegistry::from_sources([(
            "m.ron",
            r#"[
                Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
                Material(name: "stone", category: "Terrain", state: Solid, density: 2600, color: (1, 1, 1, 255)),
                Material(name: "sand",  category: "Terrain", state: Powder, density: 1600, color: (2, 2, 2, 255)),
                Material(name: "water", state: Liquid, density: 1000, color: (3, 3, 3, 255)),
                Material(name: "fire",  state: Energy, density: 0.3, color: (4, 4, 4, 255)),
            ]"#,
        )])
        .unwrap()
    }

    #[test]
    fn categories_and_default_hotbar() {
        let reg = registry();
        let inv = Inventory::new(&reg);
        let names: Vec<&str> = inv.categories().iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(names, ["All", "Terrain", "Liquids", "Energy"]);
        assert_eq!(inv.categories()[0].1.len(), 4, "everything but air");
        assert_eq!(inv.hotbar[0], reg.id("stone"));
        assert_eq!(inv.hotbar[1], None, "no dirt in this registry");
        assert_eq!(inv.hotbar[3], reg.id("water"));
        assert_eq!(inv.current(), reg.id("stone"));
    }

    #[test]
    fn slots_can_be_assigned_swapped_cleared_and_saved() {
        let reg = registry();
        let mut inv = Inventory::new(&reg);
        let sand = reg.id("sand").unwrap();
        inv.assign(1, sand);
        inv.swap(1, 7);
        assert_eq!(inv.hotbar[7], Some(sand));
        assert_eq!(
            inv.hotbar[1], None,
            "swapped with slot 8 (acid is not in this registry)"
        );
        inv.clear(0);
        inv.select(7);
        assert_eq!(inv.current(), Some(sand));
        let saved = inv.to_names(&reg);
        let mut other = Inventory::new(&reg);
        other.load_names(&saved, 7, &reg);
        assert_eq!(other.hotbar, inv.hotbar);
        assert_eq!(other.selected, 7);
    }

    #[test]
    fn wheel_sectors_follow_the_mouse_direction() {
        assert_eq!(sector(Vec2::new(0.0, -50.0), 20.0), Some(0), "up");
        assert_eq!(sector(Vec2::new(50.0, 0.0), 20.0), Some(2), "right");
        assert_eq!(sector(Vec2::new(0.0, 50.0), 20.0), Some(4), "down");
        assert_eq!(sector(Vec2::new(-50.0, 0.0), 20.0), Some(6), "left");
        assert_eq!(sector(Vec2::new(35.0, -35.0), 20.0), Some(1), "up-right");
        assert_eq!(sector(Vec2::new(5.0, 5.0), 20.0), None, "dead zone");

        let mut w = Wheel::default();
        w.open();
        w.add_motion(Vec2::new(0.0, 200.0)); // far down
        assert_eq!(w.highlighted(), Some(4));
        // A short movement the other way takes over (the aim is clamped).
        w.add_motion(Vec2::new(-200.0, -90.0));
        assert_eq!(w.highlighted(), Some(6));
        assert_eq!(w.close(), Some(6));
        assert!(!w.open);
        w.open();
        assert_eq!(w.close(), None, "no movement: keep the current slot");
    }
}
