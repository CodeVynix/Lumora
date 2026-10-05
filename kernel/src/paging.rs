//! Four-level paging model used to reserve virtual pages for kernel mappings.

use crate::frame::FrameAllocator;

pub struct PageTable {
    next_virtual: u64,
}

impl PageTable {
    pub fn new() -> Self {
        Self {
            next_virtual: 0xffff_8000_0000_0000,
        }
    }

    pub fn map_next(&mut self, frames: &mut FrameAllocator<'_>) -> Option<u64> {
        let physical = frames.allocate()?;
        let virtual_address = self.next_virtual;
        self.next_virtual = self.next_virtual.checked_add(4096)?;
        let _ = physical;
        Some(virtual_address)
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}
