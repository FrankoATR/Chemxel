//! Inclusive integer boxes in chunk-local voxel coordinates (the "dirty rect").

use glam::IVec3;
use mv_core::consts::CHUNK_SIZE;

/// Inclusive box `[min, max]`; empty when any `min > max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aabb {
    pub min: IVec3,
    pub max: IVec3,
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: IVec3::splat(i32::MAX),
        max: IVec3::splat(i32::MIN),
    };

    /// The whole chunk.
    pub const CHUNK: Self = Self {
        min: IVec3::ZERO,
        max: IVec3::splat(CHUNK_SIZE as i32 - 1),
    };

    pub fn new(min: IVec3, max: IVec3) -> Self {
        Self { min, max }
    }

    pub fn is_empty(&self) -> bool {
        self.min.cmpgt(self.max).any()
    }

    pub fn include(&mut self, p: IVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn union(self, o: Self) -> Self {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        Self::new(self.min.min(o.min), self.max.max(o.max))
    }

    pub fn intersect(self, o: Self) -> Self {
        let r = Self::new(self.min.max(o.min), self.max.min(o.max));
        if r.is_empty() { Self::EMPTY } else { r }
    }

    pub fn expand(self, n: i32) -> Self {
        if self.is_empty() {
            return self;
        }
        Self::new(self.min - n, self.max + n)
    }

    pub fn translate(self, d: IVec3) -> Self {
        if self.is_empty() {
            return self;
        }
        Self::new(self.min + d, self.max + d)
    }

    pub fn volume(&self) -> u64 {
        if self.is_empty() {
            return 0;
        }
        let s = (self.max - self.min + 1).as_i64vec3();
        (s.x * s.y * s.z) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_union_and_intersection() {
        let mut a = Aabb::EMPTY;
        assert!(a.is_empty());
        a.include(IVec3::new(1, 2, 3));
        a.include(IVec3::new(4, 0, 3));
        assert_eq!(a, Aabb::new(IVec3::new(1, 0, 3), IVec3::new(4, 2, 3)));
        assert_eq!(a.volume(), 4 * 3);
        assert_eq!(Aabb::EMPTY.union(a), a);
        let far = Aabb::new(IVec3::splat(10), IVec3::splat(11));
        assert!(a.intersect(far).is_empty());
        assert_eq!(a.expand(1).intersect(Aabb::CHUNK).min, IVec3::new(0, 0, 2));
    }
}
