//! Temperature, diffusion and phase-change tests, with the shipped materials.

use mv_core::config::SimConfig;
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry};
use mv_world::edit::fill_box;
use mv_world::{Chunk, World};

use super::*;

const MATERIALS: &str = include_str!("../../../assets/materials/basic.ron");

struct Fixture {
    world: World,
    sim: Simulation,
    registry: MaterialRegistry,
}

fn w(x: i64, y: i64, z: i64) -> WorldPos {
    WorldPos::new(x, y, z)
}

/// Air chunks `x, z in -1..=1`, `y in -1..=2`, with a stone floor at `y = 0`.
fn fixture() -> Fixture {
    let registry = MaterialRegistry::from_sources([("basic.ron", MATERIALS)]).unwrap();
    let mut world = World::new();
    for z in -1..=1 {
        for y in -1..=2 {
            for x in -1..=1 {
                world.insert(ChunkPos::new(x, y, z), Chunk::uniform(MaterialId::AIR));
            }
        }
    }
    let stone = registry.id("stone").unwrap();
    fill_box(&mut world, w(-64, 0, -64), w(127, 0, 127), stone, |_| true);
    Fixture {
        sim: Simulation::new(
            &registry,
            &ReactionSet::empty(&registry),
            &SimConfig::default(),
        ),
        world,
        registry,
    }
}

impl Fixture {
    fn id(&self, name: &str) -> MaterialId {
        self.registry.id(name).unwrap()
    }

    /// Places material like the editor does (wakes the sim, sets base temperature).
    fn place(&mut self, min: WorldPos, max: WorldPos, name: &str) {
        let m = self.id(name);
        fill_box(&mut self.world, min, max, m, |_| true);
        self.sim.edited(&self.world, min, max, Some(m));
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.sim.step(&mut self.world);
        }
    }

    fn count(&self, name: &str, min: WorldPos, max: WorldPos) -> usize {
        let m = self.id(name);
        let mut n = 0;
        for z in min.0.z..=max.0.z {
            for y in min.0.y..=max.0.y {
                for x in min.0.x..=max.0.x {
                    n += usize::from(self.world.voxel(w(x, y, z)) == Some(m));
                }
            }
        }
        n
    }

    /// Highest y with a voxel of `name` inside the box, if any.
    fn top(&self, name: &str, min: WorldPos, max: WorldPos) -> Option<i64> {
        let m = self.id(name);
        (min.0.y..=max.0.y).rev().find(|&y| {
            (min.0.z..=max.0.z)
                .any(|z| (min.0.x..=max.0.x).any(|x| self.world.voxel(w(x, y, z)) == Some(m)))
        })
    }
}

#[test]
fn placed_materials_get_their_base_temperature() {
    let mut f = fixture();
    f.place(w(8, 4, 8), w(15, 11, 15), "lava");
    f.place(w(40, 4, 40), w(47, 11, 47), "ice");
    assert!((f.sim.temperature_at(w(10, 6, 10)) - 1300.0).abs() < 1.0);
    assert!((f.sim.temperature_at(w(42, 6, 42)) + 10.0).abs() < 1.0);
    assert_eq!(f.sim.temperature_at(w(-40, 30, -40)), 20.0);
}

#[test]
fn stable_materials_need_no_field() {
    let mut f = fixture();
    f.place(w(8, 1, 8), w(15, 4, 15), "water");
    assert_eq!(f.sim.thermal_chunks(), 0, "water is stable at 20 °C");
}

#[test]
fn heat_flows_from_hot_to_cold() {
    let mut f = fixture();
    f.place(w(8, 1, 8), w(15, 8, 15), "lava");
    let near = w(20, 4, 12);
    let far = w(40, 4, 12);
    f.run(60);
    let (t_near, t_far) = (f.sim.temperature_at(near), f.sim.temperature_at(far));
    assert!(
        t_near > t_far,
        "near {t_near} must be hotter than far {t_far}"
    );
    assert!(t_near > 21.0);
    assert!(
        f.sim.temperature_at(w(12, 4, 12)) < 1300.0,
        "the lava cools"
    );
}

#[test]
fn energy_is_conserved_in_a_closed_system() {
    let mut f = fixture();
    // Lava next to ice and water: diffusion, advection and phase changes all happen.
    f.place(w(4, 1, 4), w(11, 8, 11), "lava");
    f.place(w(12, 1, 4), w(19, 8, 11), "ice");
    f.place(w(4, 9, 4), w(11, 12, 11), "water");
    let injected = 512.0 * 3100.0 * 1280.0; // lava sensible heat above ambient
    let before = f.sim.total_energy(&f.world);
    let imbalance_before = f.sim.energy_imbalance();
    f.run(300);
    let after = f.sim.total_energy(&f.world);
    let imbalance = f.sim.energy_imbalance() - imbalance_before;
    let error = (after - before - imbalance).abs();
    assert!(
        error < 0.01 * injected,
        "energy drift {error:.3e} (injected {injected:.3e}, accounted imbalance {imbalance:.3e})"
    );
    assert!(f.sim.stats().thermal_chunks > 0);
}

#[test]
fn a_lava_voxel_freezes_into_stone() {
    let mut f = fixture();
    // Bury one lava voxel in stone: it loses heat to the rock and solidifies.
    f.place(w(20, 1, 20), w(28, 9, 28), "stone");
    f.place(w(24, 5, 24), w(24, 5, 24), "lava");
    f.run(600);
    assert_eq!(f.count("lava", w(20, 1, 20), w(28, 9, 28)), 0);
}

#[test]
fn steam_at_ambient_condenses() {
    let mut f = fixture();
    // A closed stone box so the steam stays put while it condenses.
    f.place(w(-1, 1, -1), w(5, 7, 5), "stone");
    fill_box(
        &mut f.world,
        w(0, 1, 0),
        w(4, 6, 4),
        MaterialId::AIR,
        |_| true,
    );
    f.place(w(0, 4, 0), w(4, 6, 4), "steam");
    assert!(f.sim.thermal_chunks() > 0, "steam is unstable at 20 °C");
    f.run(200);
    assert_eq!(f.count("steam", w(0, 1, 0), w(4, 6, 4)), 0);
    assert!(f.count("water", w(0, 1, 0), w(4, 6, 4)) > 0);
}

/// PROMPT.md milestone 4 acceptance: lava on ice makes water, then steam; the
/// steam rises and condenses far from the source.
#[test]
fn lava_on_ice_melts_boils_and_condenses_far_away() {
    let mut f = fixture();
    // An ice slab with a lava block on top, in the open.
    f.place(w(-8, 1, -8), w(7, 6, 7), "ice");
    f.place(w(-4, 7, -4), w(3, 14, 3), "lava");
    let area = (w(-60, 1, -60), w(120, 190, 120));
    let ice_before = f.count("ice", area.0, area.1);

    let mut saw_water = false;
    let mut saw_steam = false;
    let mut condensed_high = None;
    for tick in 0..900 {
        f.run(1);
        if tick % 30 != 29 {
            continue;
        }
        let (water, steam) = (
            f.count("water", area.0, area.1),
            f.count("steam", area.0, area.1),
        );
        saw_water |= water > 0;
        saw_steam |= steam > 0;
        eprintln!(
            "tick {tick}: ice {} water {water} steam {steam} lava {} stone-ish top steam {:?} lava T {:.0}",
            f.count("ice", area.0, area.1),
            f.count("lava", area.0, area.1),
            f.top("steam", area.0, area.1),
            f.sim.temperature_at(w(0, 10, 0))
        );
        // Water well above the source can only come from condensed steam.
        if condensed_high.is_none()
            && let Some(y) = f.top("water", w(-60, 40, -60), w(120, 190, 120))
        {
            condensed_high = Some((tick, y));
        }
    }
    assert!(f.count("ice", area.0, area.1) < ice_before, "ice melts");
    assert!(saw_water, "melting produces water");
    assert!(saw_steam, "the water boils into steam");
    let (tick, y) = condensed_high.expect("steam condenses into water far above the lava");
    assert!(y >= 40, "condensed at y = {y} (tick {tick})");
}
