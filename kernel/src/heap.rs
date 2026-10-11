//! Bump heap used during early kernel initialization.

/// Virtual address of the kernel heap mapped by the boot code.
const HEAP_BASE: u64 = 0xffff_8000_4000_0000;

/// Size of one mapped page.
pub const PAGE_SIZE: u64 = 4096;

pub struct Heap {
    next: u64,
    remaining: u64,
}

impl Heap {
    pub const fn new() -> Self {
        Self {
            next: HEAP_BASE,
            remaining: PAGE_SIZE,
        }
    }

    /// Heap over `capacity` bytes starting at `start`.
    pub const fn with_range(start: u64, capacity: u64) -> Self {
        Self {
            next: start,
            remaining: capacity,
        }
    }

    /// Heap over `page_count` pages starting at `start`.
    pub const fn with_pages(start: u64, page_count: u64) -> Self {
        Self::with_range(start, page_count * PAGE_SIZE)
    }

    pub fn allocate(&mut self, size: u64, alignment: u64) -> Option<u64> {
        if size == 0 || !alignment.is_power_of_two() {
            return None;
        }

        let aligned = self.next.checked_add(alignment - 1)? & !(alignment - 1);
        let used = aligned.checked_add(size)? - self.next;
        if used > self.remaining {
            return None;
        }
        self.next += used;
        self.remaining -= used;
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

    #[test]
    fn allocations_stop_at_the_capacity() {
        let mut heap = Heap::with_range(0x1000, 0x100);
        assert!(heap.allocate(0x100, 1).is_some());
        assert!(heap.allocate(1, 1).is_none());
    }
}
