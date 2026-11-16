//! Fixed-capacity slot allocator for GPU pools.

/// Hands out indices in `0..capacity`, reusing freed ones (LIFO).
#[derive(Debug, Clone)]
pub struct SlotAllocator {
    capacity: u32,
    next_unused: u32,
    free: Vec<u32>,
}

impl SlotAllocator {
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            next_unused: 0,
            free: Vec::new(),
        }
    }

    /// Returns a free slot, or `None` if the pool is full.
    pub fn alloc(&mut self) -> Option<u32> {
        if let Some(slot) = self.free.pop() {
            return Some(slot);
        }
        if self.next_unused < self.capacity {
            self.next_unused += 1;
            return Some(self.next_unused - 1);
        }
        None
    }

    /// Returns `slot` to the pool. Freeing a slot twice is a logic error.
    pub fn free(&mut self, slot: u32) {
        debug_assert!(slot < self.next_unused, "freeing a slot never allocated");
        debug_assert!(!self.free.contains(&slot), "double free of slot {slot}");
        self.free.push(slot);
    }

    pub fn in_use(&self) -> u32 {
        self.next_unused - self.free.len() as u32
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_until_full_and_reuses() {
        let mut a = SlotAllocator::new(3);
        assert_eq!(
            (a.alloc(), a.alloc(), a.alloc()),
            (Some(0), Some(1), Some(2))
        );
        assert_eq!(a.alloc(), None);
        a.free(1);
        assert_eq!(a.in_use(), 2);
        assert_eq!(a.alloc(), Some(1));
        assert_eq!(a.alloc(), None);
    }
}
