//! Deterministic randomness: a hash of (seed, tick, world position).
//!
//! No RNG state is carried between voxels, so results do not depend on processing
//! order or thread count, and any tick is reproducible from the world state.

use glam::I64Vec3;

/// 32 well-mixed bits for `pos` at `tick`.
#[inline]
pub fn hash(seed: u64, tick: u64, pos: I64Vec3) -> u32 {
    let mut h = seed ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (pos.x as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= (pos.y as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= (pos.z as u64).wrapping_mul(0x85EB_CA77_C2B2_AE63);
    // splitmix64 finaliser
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (h ^ (h >> 31)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_spread() {
        let p = I64Vec3::new(-5, 7, 1 << 40);
        assert_eq!(hash(1, 2, p), hash(1, 2, p));
        assert_ne!(hash(1, 2, p), hash(1, 3, p));
        assert_ne!(hash(1, 2, p), hash(2, 2, p));
        // Low two bits (used to pick among 4 directions) are roughly uniform.
        let mut counts = [0u32; 4];
        for i in 0..4000 {
            counts[(hash(9, 0, I64Vec3::new(i, 0, 0)) & 3) as usize] += 1;
        }
        assert!(
            counts.iter().all(|&c| (850..1150).contains(&c)),
            "{counts:?}"
        );
    }
}
