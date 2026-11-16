//! Fixed 512-bit set with one bit per render brick of a chunk.

use mv_core::consts::BRICKS_PER_CHUNK;

const WORDS: usize = BRICKS_PER_CHUNK / 64;

/// One bit per 8³ brick of a chunk, indexed like [`mv_core::LocalPos::brick_index`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BrickMask([u64; WORDS]);

impl BrickMask {
    pub const EMPTY: Self = Self([0; WORDS]);
    pub const FULL: Self = Self([u64::MAX; WORDS]);

    #[inline]
    pub fn set(&mut self, brick: usize) {
        self.0[brick / 64] |= 1 << (brick % 64);
    }

    #[inline]
    pub fn clear(&mut self, brick: usize) {
        self.0[brick / 64] &= !(1 << (brick % 64));
    }

    #[inline]
    pub fn get(&self, brick: usize) -> bool {
        self.0[brick / 64] & (1 << (brick % 64)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }

    pub fn count(&self) -> u32 {
        self.0.iter().map(|w| w.count_ones()).sum()
    }

    /// Iterates the indices of set bits in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(wi, &word)| {
            let mut w = word;
            std::iter::from_fn(move || {
                if w == 0 {
                    return None;
                }
                let bit = w.trailing_zeros() as usize;
                w &= w - 1;
                Some(wi * 64 + bit)
            })
        })
    }

    /// Returns the current mask and clears it.
    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_clear() {
        let mut m = BrickMask::EMPTY;
        for b in [0, 63, 64, 300, 511] {
            assert!(!m.get(b));
            m.set(b);
            assert!(m.get(b));
        }
        assert_eq!(m.count(), 5);
        m.clear(64);
        assert!(!m.get(64));
        assert_eq!(m.iter().collect::<Vec<_>>(), vec![0, 63, 300, 511]);
    }

    #[test]
    fn full_and_take() {
        let mut m = BrickMask::FULL;
        assert_eq!(m.count(), 512);
        assert_eq!(m.iter().count(), 512);
        let taken = m.take();
        assert!(m.is_empty());
        assert_eq!(taken, BrickMask::FULL);
    }
}
