//! Physical frame allocator over the memory map supplied by the bootloader.

use crate::MemoryRegion;

const FRAME_SIZE: u64 = 4096;
pub const FRAME_SIZE_BYTES: u64 = FRAME_SIZE;
const LOW_MEMORY_LIMIT: u64 = 0x0010_0000;

/// Freed frames the allocator remembers for reuse.
const FREE_LIST_CAPACITY: usize = 64;

pub struct FrameAllocator<'a> {
    regions: &'a [MemoryRegion],
    region: usize,
    next: u64,
    end: u64,
    remaining: usize,
    freed: [u64; FREE_LIST_CAPACITY],
    freed_count: usize,
}

impl<'a> FrameAllocator<'a> {
    pub fn new(regions: &'a [MemoryRegion]) -> Self {
        let mut allocator = Self {
            regions,
            region: 0,
            next: 0,
            end: 0,
            remaining: 0,
            freed: [0; FREE_LIST_CAPACITY],
            freed_count: 0,
        };
        allocator.select_next_region();
        allocator
    }

    pub fn allocate(&mut self) -> Option<u64> {
        if self.freed_count > 0 {
            self.freed_count -= 1;
            // A recycled frame leaves the pool just like a fresh one, so the
            // count stays exact across allocate/free round trips.
            self.remaining = self.remaining.saturating_sub(1);
            return Some(self.freed[self.freed_count]);
        }
        loop {
            if self.next < self.end {
                let frame = self.next;
                self.next = self.next.checked_add(FRAME_SIZE)?;
                self.remaining = self.remaining.saturating_sub(1);
                return Some(frame);
            }
            self.region = self.region.saturating_add(1);
            if self.region >= self.regions.len() {
                return None;
            }
            self.select_next_region();
        }
    }

    /// Returns `frame` for reuse by later allocations.
    ///
    /// The frame must have come from this allocator, must be unmapped
    /// everywhere before freeing it, and must not be freed twice. Frames
    /// beyond the free-list capacity are dropped instead of tracked.
    pub fn free(&mut self, frame: u64) {
        debug_assert!(frame != 0 && frame.is_multiple_of(FRAME_SIZE));
        if self.freed_count < FREE_LIST_CAPACITY {
            self.freed[self.freed_count] = frame;
            self.freed_count += 1;
            self.remaining = self.remaining.saturating_add(1);
        }
    }

    pub fn free_frames(&self) -> usize {
        self.remaining
    }

    fn select_next_region(&mut self) {
        while self.region < self.regions.len() {
            let region = self.regions[self.region];
            if region.usable {
                let start = region.start.saturating_add(FRAME_SIZE - 1) & !(FRAME_SIZE - 1);
                let start = start.max(LOW_MEMORY_LIMIT);
                self.next = start;
                self.end = region.start.saturating_add(region.length) & !(FRAME_SIZE - 1);
                self.remaining = self
                    .end
                    .saturating_sub(self.next)
                    .checked_div(FRAME_SIZE)
                    .unwrap_or(0) as usize;
                if self.next < self.end {
                    return;
                }
            }
            self.region = self.region.saturating_add(1);
        }
        self.next = 0;
        self.end = 0;
        self.remaining = 0;
    }
}
