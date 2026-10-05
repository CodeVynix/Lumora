//! Bump heap used during early kernel initialization.

pub struct Heap {
    next: u64,
}

impl Heap {
    pub const fn new() -> Self {
        Self {
            next: 0xffff_9000_0000_0000,
        }
    }

    pub fn allocate(&mut self, size: u64, alignment: u64) -> Option<u64> {
        if size == 0 || !alignment.is_power_of_two() {
            return None;
        }

        let aligned = self.next.checked_add(alignment - 1)? & !(alignment - 1);
        self.next = aligned.checked_add(size)?;
        Some(aligned)
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Heap;

    #[test]
    fn allocations_are_aligned_and_non_overlapping() {
        let mut heap = Heap::new();
        let first = heap.allocate(64, 8).expect("first allocation");
        let second = heap.allocate(32, 16).expect("second allocation");
        assert_eq!(first % 8, 0);
        assert_eq!(second % 16, 0);
        assert!(second >= first + 64);
    }
}
