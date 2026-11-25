//! Reactions, lifetimes and combustion (milestone 5).

use mv_core::config::SimConfig;
use mv_core::{ChunkPos, WorldPos};
use mv_materials::{MaterialId, MaterialRegistry, ReactionSet};
use mv_world::edit::fill_box;
use mv_world::{Chunk, World};

use super::*;

const MATERIALS: &str = include_str!("../../../assets/materials/basic.ron");
const REACTIONS: &str = include_str!("../../../assets/reactions/basic.ron");

struct Fixture {
    world: World,
    sim: Simulation,
    registry: MaterialRegistry,
}

fn w(x: i64, y: i64, z: i64) -> WorldPos {
    WorldPos::new(x, y, z)
}

/// Air chunks `x, z in -1..=1`, `y in -1..=2`, with a stone floor at `y = 0`.
fn fixture_with(materials: &str, reactions: &str) -> Fixture {
    let registry = MaterialRegistry::from_sources([("m.ron", materials)]).unwrap();
    let reactions = ReactionSet::from_sources([("r.ron", reactions)], &registry).unwrap();
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
    let mut sim = Simulation::new(&registry, &reactions, &SimConfig::default());
    // Not about structural integrity (tests_integrity.rs): nothing detaches.
    sim.set_anchor(Some(Box::new(|_| true)));
    Fixture {
        sim,
        world,
        registry,
    }
}

fn fixture() -> Fixture {
    fixture_with(MATERIALS, REACTIONS)
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

    fn at(&self, p: WorldPos) -> &str {
        &self.registry.get(self.world.voxel(p).unwrap()).name
    }
}

const ALL: (WorldPos, WorldPos) = (
    WorldPos(glam::I64Vec3::new(-64, 0, -64)),
    WorldPos(glam::I64Vec3::new(127, 191, 127)),
);

// --- Exact rules, with synthetic materials -------------------------------------

const SYNTH: &str = r#"[
    Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
    Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
    Material(name: "a",     state: Liquid, density: 1000, color: (2, 2, 2, 255)),
    Material(name: "b",     state: Solid,  density: 1000, color: (3, 3, 3, 255)),
    Material(name: "c",     state: Solid,  density: 1000, color: (4, 4, 4, 255)),
    Material(name: "puff",  state: Gas,    density: 0.5,  color: (5, 5, 5, 255),
             lifetime: (1, 1), decays_into: "c"),
    Material(name: "hot",   state: Solid,  density: 1000, color: (6, 6, 6, 255),
             base_temperature: 500, conductivity: 1),
]"#;

#[test]
fn a_certain_reaction_happens_on_the_next_tick() {
    let mut f = fixture_with(
        SYNTH,
        r#"[Reaction(a: "a", b: "b", result_a: "c", result_b: "c", probability: 1)]"#,
    );
    f.place(w(10, 1, 10), w(10, 1, 10), "b");
    f.place(w(11, 1, 10), w(11, 1, 10), "a");
    f.run(1);
    assert_eq!(f.at(w(10, 1, 10)), "c");
    assert_eq!(f.at(w(11, 1, 10)), "c");
    assert_eq!(f.sim.stats().reactions, 1);
}

#[test]
fn reactions_are_directional() {
    // Only `b + a` is defined, so `b` is the initiator and `result_a` applies to it,
    // even though the voxel of material `a` is the one that moves.
    let mut f = fixture_with(
        SYNTH,
        r#"[Reaction(a: "b", b: "a", result_a: "c", probability: 1)]"#,
    );
    f.place(w(9, 1, 9), w(11, 2, 11), "stone");
    f.place(w(10, 1, 10), w(10, 1, 10), "a");
    f.place(w(11, 1, 10), w(11, 1, 10), "b");
    f.run(1);
    assert_eq!(f.at(w(11, 1, 10)), "c");
    assert_eq!(f.at(w(10, 1, 10)), "a");
}

#[test]
fn reactions_respect_their_temperature_range() {
    let rules = r#"[Reaction(a: "a", b: "b", result_b: "c", probability: 1, min_temp: 200)]"#;
    // At ambient: no reaction.
    let mut f = fixture_with(SYNTH, rules);
    f.place(w(10, 1, 10), w(10, 1, 10), "b");
    f.place(w(11, 1, 10), w(11, 1, 10), "a");
    f.run(5);
    assert_eq!(f.at(w(10, 1, 10)), "b");
    // In a hot cell: it reacts.
    let mut f = fixture_with(SYNTH, rules);
    f.place(w(8, 1, 8), w(11, 4, 11), "hot");
    f.place(w(9, 1, 9), w(9, 1, 9), "b");
    f.place(w(10, 1, 9), w(10, 1, 9), "a");
    f.run(1);
    assert_eq!(f.at(w(9, 1, 9)), "c");
}

#[test]
fn reactions_cross_chunk_borders() {
    let mut f = fixture_with(
        SYNTH,
        r#"[Reaction(a: "a", b: "b", result_a: "c", result_b: "c", probability: 1)]"#,
    );
    // x = 63 | 64 is a chunk border; `a` sits in a 1-voxel slot so it cannot move.
    f.place(w(64, 1, 10), w(64, 1, 10), "b");
    f.place(w(63, 1, 9), w(63, 1, 11), "stone");
    f.place(w(62, 1, 10), w(62, 1, 10), "stone");
    f.place(w(63, 2, 10), w(63, 2, 10), "stone");
    f.place(w(63, 1, 10), w(63, 1, 10), "a");
    f.run(1);
    assert_eq!(f.at(w(64, 1, 10)), "c");
    assert_eq!(f.at(w(63, 1, 10)), "c");
}

#[test]
fn a_voxel_decays_at_the_end_of_its_lifetime() {
    let mut f = fixture_with(SYNTH, "[]");
    f.place(w(10, 1, 10), w(12, 3, 12), "puff");
    f.run(1);
    assert_eq!(f.count("puff", ALL.0, ALL.1), 0);
    assert_eq!(f.count("c", ALL.0, ALL.1), 27);
}

#[test]
fn resting_reactants_keep_their_chunk_awake() {
    // A low-probability reaction between voxels that never move must still happen
    // after the chunk would normally have gone to sleep.
    let mut f = fixture_with(
        SYNTH,
        r#"[Reaction(a: "a", b: "b", result_a: "puff", probability: 0.02)]"#,
    );
    // `a` is walled in by inert `c`, with a single `b` neighbour.
    f.place(w(9, 1, 9), w(11, 2, 11), "c");
    f.place(w(11, 1, 10), w(11, 1, 10), "b");
    f.place(w(10, 1, 10), w(10, 1, 10), "a");
    f.run(600);
    assert_ne!(f.at(w(10, 1, 10)), "a");
}

// --- Shipped materials and reactions ------------------------------------------

#[test]
fn water_and_lava_make_obsidian_at_the_interface() {
    let mut f = fixture();
    // A lava pool against a water pool, separated by nothing.
    f.place(w(0, 1, 0), w(7, 4, 7), "lava");
    f.place(w(8, 1, 0), w(15, 4, 7), "water");
    f.run(60);
    let obsidian = f.count("obsidian", ALL.0, ALL.1);
    assert!(obsidian > 0, "quenched lava becomes obsidian");
    assert!(f.sim.stats().thermal_chunks > 0);
    // Obsidian forms where they met: the side facing the water, or the top if water
    // flowed over. Lava swaps with steam bubbles and water flows into those pockets,
    // so a few voxels inside get quenched too, but most of it is at the interface.
    let interior = f.count("obsidian", w(1, 1, 1), w(5, 2, 6));
    assert!(
        interior * 4 <= obsidian,
        "{interior} of {obsidian} obsidian voxels inside the pool"
    );
    assert!(
        f.count("water", ALL.0, ALL.1) < 256,
        "some water boiled off"
    );
}

#[test]
fn acid_dissolves_stone_but_not_glass() {
    let mut f = fixture();
    f.place(w(0, 1, 0), w(15, 4, 15), "stone");
    f.place(w(20, 1, 0), w(35, 4, 15), "glass");
    f.place(w(4, 5, 4), w(11, 8, 11), "acid");
    f.place(w(24, 5, 4), w(31, 8, 11), "acid");
    let stone = f.count("stone", w(0, 1, 0), w(15, 4, 15));
    f.run(400);
    assert!(
        f.count("stone", w(0, 1, 0), w(15, 4, 15)) < stone,
        "acid eats stone"
    );
    assert_eq!(f.count("glass", w(20, 1, 0), w(35, 4, 15)), 1024);
    assert!(f.count("acid", ALL.0, ALL.1) < 512, "acid is used up");
}

#[test]
fn smoke_rises_and_fades_away() {
    let mut f = fixture();
    f.place(w(0, 1, 0), w(7, 8, 7), "smoke");
    f.run(20);
    assert!(
        f.count("smoke", w(0, 20, 0), w(7, 191, 7)) > 0,
        "smoke rises"
    );
    f.run(1500);
    assert_eq!(f.count("smoke", ALL.0, ALL.1), 0, "smoke fades");
}

#[test]
fn hot_wood_burns_into_ash_and_emits_flames() {
    let mut f = fixture();
    f.place(w(0, 1, 0), w(7, 8, 7), "wood");
    // Heat the block well above its ignition point.
    let pos = ChunkPos::new(0, 0, 0);
    f.sim.ensure_field(&f.world, pos);
    for c in 0..2 {
        for b in 0..2 {
            for a in 0..2 {
                let cell = cell_of(glam::IVec3::new(a * 4, 4 + b * 4, c * 4));
                f.sim.thermal.get_mut(&pos).unwrap().temp[cell] = 600.0;
            }
        }
    }
    let mut flames = 0;
    for _ in 0..200 {
        f.run(1);
        flames += f.sim.stats().flames;
    }
    assert!(flames > 0, "burning wood emits flames");
    let ash = f.count("ash", ALL.0, ALL.1);
    let burnt = 512 - f.count("wood", w(0, 1, 0), w(7, 8, 7));
    assert!(ash > 0, "wood burns into ash");
    // `burn_residue`: most of the wood burns away, only a little ash is left.
    assert!(
        burnt > 100 && ash * 4 < burnt,
        "{ash} ash from {burnt} burnt wood"
    );
}

/// Builds a tree: a 2×2 trunk of `height` and a leaf canopy around its top.
fn tree(f: &mut Fixture, x: i64, z: i64, height: i64) {
    f.place(
        w(x - 3, height - 2, z - 3),
        w(x + 4, height + 2, z + 4),
        "leaves",
    );
    f.place(w(x, 1, z), w(x + 1, height, z + 1), "wood");
}

/// PROMPT.md milestone 5 acceptance: a forest burns credibly. A row of trees with
/// touching canopies is lit at one end; the fire climbs, spreads from tree to tree,
/// leaves ash and smoke, and dies out.
#[test]
fn a_forest_fire_spreads_from_tree_to_tree() {
    let mut f = fixture();
    let trees: Vec<i64> = (0..5).map(|i| -40 + i * 8).collect();
    for &x in &trees {
        tree(&mut f, x, 0, 14);
    }
    let fuel = |f: &Fixture, x: i64| {
        let (lo, hi) = (w(x - 3, 1, -3), w(x + 4, 16, 4));
        f.count("wood", lo, hi) + f.count("leaves", lo, hi)
    };
    let initial: Vec<usize> = trees.iter().map(|&x| fuel(&f, x)).collect();
    // Light the base of the first trunk.
    f.place(w(-43, 1, -1), w(-41, 4, 2), "fire");

    let mut saw_smoke = false;
    let mut max_flames = 0;
    let mut max_temp = f32::MIN;
    for tick in 0..1500 {
        f.run(1);
        max_flames = max_flames.max(f.sim.stats().flames);
        max_temp = f
            .sim
            .thermal_fields()
            .fold(max_temp, |m, (_, t)| t.iter().fold(m, |m, &v| m.max(v)));
        if tick % 100 == 99 {
            let smoke = f.count("smoke", ALL.0, ALL.1);
            saw_smoke |= smoke > 0;
            let left: Vec<usize> = trees.iter().map(|&x| fuel(&f, x)).collect();
            eprintln!(
                "tick {tick}: fuel per tree {left:?} fire {} smoke {smoke} ash {} thermal {} T0 {:.0}",
                f.count("fire", ALL.0, ALL.1),
                f.count("ash", ALL.0, ALL.1),
                f.sim.thermal_chunks(),
                f.sim.temperature_at(w(-40, 8, 0)),
            );
        }
    }
    let left: Vec<usize> = trees.iter().map(|&x| fuel(&f, x)).collect();
    for (i, (&l, &init)) in left.iter().zip(&initial).enumerate() {
        assert!(
            l < init / 2,
            "tree {i} must have mostly burnt: {l} of {init} fuel voxels left"
        );
    }
    assert!(max_flames > 0, "flames were emitted");
    assert!(
        max_temp < 1000.0,
        "a wood fire stays below ~flame temperature: {max_temp}"
    );
    assert!(saw_smoke, "the fire smokes");
    assert!(f.count("ash", ALL.0, ALL.1) > 0, "trunks leave ash");
}

#[test]
fn energy_is_accounted_with_reactions_and_fire() {
    let mut f = fixture();
    f.place(w(0, 1, 0), w(7, 4, 7), "lava");
    f.place(w(8, 1, 0), w(15, 4, 7), "water");
    tree(&mut f, 30, 30, 12);
    f.place(w(28, 1, 29), w(29, 3, 32), "fire");
    let before = f.sim.total_energy(&f.world);
    let imbalance_before = f.sim.energy_imbalance();
    f.run(300);
    let after = f.sim.total_energy(&f.world);
    let imbalance = f.sim.energy_imbalance() - imbalance_before;
    let error = (after - before - imbalance).abs();
    let scale = 256.0 * 3100.0 * 1280.0;
    assert!(
        error < 0.01 * scale,
        "energy drift {error:.3e} (scale {scale:.3e}, accounted {imbalance:.3e})"
    );
}

// --- Resting liquids, evaporation and clouds (ADR 012) ----------------------------

/// Positions of every voxel of `name` in the box, to check that nothing moves.
fn snapshot(f: &Fixture, name: &str, min: WorldPos, max: WorldPos) -> Vec<WorldPos> {
    let m = f.id(name);
    let mut out = Vec::new();
    for z in min.0.z..=max.0.z {
        for y in min.0.y..=max.0.y {
            for x in min.0.x..=max.0.x {
                if f.world.voxel(w(x, y, z)) == Some(m) {
                    out.push(w(x, y, z));
                }
            }
        }
    }
    out
}

/// A 12×12 stone basin (inside x, z in 0..=11, y in 1..=10).
fn basin(f: &mut Fixture) {
    f.place(w(-1, 1, -1), w(12, 10, 12), "stone");
    let (lo, hi) = (w(0, 1, 0), w(11, 10, 11));
    fill_box(&mut f.world, lo, hi, MaterialId::AIR, |_| true);
    f.sim.wake_box(&f.world, lo, hi);
}

#[test]
fn an_incomplete_top_layer_evaporates_and_the_pool_settles() {
    let mut f = fixture();
    basin(&mut f);
    // Two full layers (288 voxels) plus 40 voxels of a third one.
    f.place(w(0, 1, 0), w(11, 2, 11), "water");
    f.place(w(0, 3, 0), w(4, 3, 7), "water");
    let mut vapour = 0;
    for _ in 0..2000 {
        f.run(1);
        vapour += f.sim.stats().evaporations;
    }
    let inside = (w(0, 1, 0), w(11, 10, 11));
    assert_eq!(
        f.count("water", w(0, 1, 0), w(11, 2, 11)),
        288,
        "full layers stay"
    );
    assert_eq!(
        f.count("water", w(0, 3, 0), w(11, 10, 11)),
        0,
        "the incomplete layer is gone"
    );
    assert!(vapour >= 40, "it evaporated into vapour ({vapour})");
    // And then the pool is still.
    let before = snapshot(&f, "water", inside.0, inside.1);
    f.run(60);
    assert_eq!(
        snapshot(&f, "water", inside.0, inside.1),
        before,
        "the water rests"
    );
}

#[test]
fn a_full_pool_keeps_its_water() {
    let mut f = fixture();
    basin(&mut f);
    f.place(w(0, 1, 0), w(11, 2, 11), "water");
    f.run(600);
    assert_eq!(f.count("water", w(0, 1, 0), w(11, 10, 11)), 288);
    assert_eq!(f.sim.stats().evaporations, 0);
}

const WEATHER: &str = r#"[
    Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
    Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
    Material(name: "goo",   state: Liquid, density: 1000, color: (2, 2, 2, 255), dispersion: 3,
             evaporates_into: "crust", evaporate_rate: 1),
    Material(name: "crust", state: Solid,  density: 2000, color: (3, 3, 3, 255)),
    Material(name: "vapor", state: Gas,    density: 0.5,  color: (4, 4, 4, 255),
             ceiling: 30, ceiling_into: "cloud"),
    Material(name: "cloud", state: Gas,    density: 0.4,  color: (5, 5, 5, 255), drifts: false),
    Material(name: "mist",  state: Gas,    density: 0.5,  color: (6, 6, 6, 255),
             lifetime: (1, 1), ceiling: 100, ceiling_into: "haze"),
    Material(name: "haze",  state: Gas,    density: 0.4,  color: (7, 7, 7, 255), drifts: false,
             lifetime: (600, 600)),
]"#;

#[test]
fn a_loose_drop_turns_into_its_evaporation_product() {
    let mut f = fixture_with(WEATHER, "[]");
    f.place(w(10, 1, 10), w(10, 1, 10), "goo");
    f.run(3);
    assert_eq!(f.count("goo", ALL.0, ALL.1), 0);
    assert_eq!(f.count("crust", ALL.0, ALL.1), 1);
}

#[test]
fn vapour_turns_into_a_still_cloud_at_its_ceiling() {
    let mut f = fixture_with(WEATHER, "[]");
    f.place(w(10, 5, 10), w(12, 7, 12), "vapor");
    f.run(80);
    assert_eq!(f.count("vapor", ALL.0, ALL.1), 0);
    let clouds = snapshot(&f, "cloud", ALL.0, ALL.1);
    assert_eq!(clouds.len(), 27);
    // The first vapour turns into cloud at the ceiling; the rest joins it from below.
    assert!(
        clouds.iter().all(|p| (27..=30).contains(&p.0.y)),
        "clouds form at the ceiling height"
    );
    f.run(60);
    assert_eq!(
        snapshot(&f, "cloud", ALL.0, ALL.1),
        clouds,
        "clouds stay still"
    );
}

#[test]
fn dying_vapour_condenses_into_the_cloud_layer_above_which_fades_slowly() {
    let mut f = fixture_with(WEATHER, "[]");
    // Short-lived "mist" whose clouds ("haze") form at y = 100.
    f.place(w(10, 5, 10), w(12, 5, 12), "mist");
    f.run(3);
    assert_eq!(f.count("mist", ALL.0, ALL.1), 0);
    let haze = snapshot(&f, "haze", ALL.0, ALL.1);
    assert_eq!(haze.len(), 9, "one cloud voxel per vapour voxel");
    assert!(
        haze.iter().all(|p| p.0.y == 100),
        "at the ceiling height above"
    );
    // Clouds do not keep the simulation awake...
    f.run(10);
    assert_eq!(f.sim.active_chunks(), 0, "clouds are still");
    // ...and fade in the slow pass (mean life 600 ticks).
    f.run(3000);
    assert!(f.count("haze", ALL.0, ALL.1) < 9, "clouds fade");
}

#[test]
fn condensing_vapour_thickens_the_cloud_downwards() {
    let mut f = fixture_with(WEATHER, "[]");
    f.place(w(10, 5, 10), w(10, 5, 10), "mist");
    f.run(2);
    f.place(w(10, 5, 10), w(10, 5, 10), "mist");
    f.run(2);
    assert_eq!(f.at(w(10, 100, 10)), "haze");
    assert_eq!(f.at(w(10, 99, 10)), "haze");
}

#[test]
fn a_per_material_transition_rate_slows_its_phase_changes() {
    // Two solids created hot, above their melting point, one at a hundredth of the
    // default rate. Each fills its own 4³ thermal cell, so neither cools the other.
    let materials = r#"[
        Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
        Material(name: "stone", state: Solid,  density: 2600, color: (1, 1, 1, 255)),
        Material(name: "goo",   state: Liquid, density: 1000, color: (2, 2, 2, 255)),
        Material(name: "fast",  state: Solid,  density: 1000, color: (3, 3, 3, 255),
                 base_temperature: 400, melt_at: 100, melts_into: "goo"),
        Material(name: "slow",  state: Solid,  density: 1000, color: (4, 4, 4, 255),
                 base_temperature: 400, melt_at: 100, melts_into: "goo", transition_rate: 0.001),
    ]"#;
    let mut f = fixture_with(materials, "[]");
    f.place(w(8, 4, 8), w(11, 7, 11), "fast");
    f.place(w(20, 4, 8), w(23, 7, 11), "slow");
    f.run(30);
    assert!(
        f.count("fast", ALL.0, ALL.1) < 10,
        "the default rate melts it quickly"
    );
    assert!(
        f.count("slow", ALL.0, ALL.1) > 50,
        "the slow rate keeps most of it solid"
    );
}
