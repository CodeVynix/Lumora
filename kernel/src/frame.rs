//! Physical frame allocator over the memory map supplied by the bootloader.

use crate::MemoryRegion;

const FRAME_SIZE: u64 = 4096;

pub struct FrameAllocator<'a> {
    regions: &'a [MemoryRegion],
    region: usize,
    next: u64,
    end: u64,
    remaining: usize,
}

impl<'a> FrameAllocator<'a> {
    pub fn new(regions: &'a [MemoryRegion]) -> Self {
        let mut allocator = Self {
            regions,
            region: 0,
            next: 0,
            end: 0,
            remaining: 0,
        };
        allocator.select_next_region();
        allocator
    }

    pub fn allocate(&mut self) -> Option<u64> {
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

    pub fn free_frames(&self) -> usize {
        self.remaining
    }

    fn select_next_region(&mut self) {
        while self.region < self.regions.len() {
            let region = self.regions[self.region];
            if region.usable {
                let start = (region.start + FRAME_SIZE - 1) & !(FRAME_SIZE - 1);
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
