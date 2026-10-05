#![no_std]

pub mod frame;
pub mod heap;
pub mod paging;
pub mod serial;

/// Kernel entry called by the bootloader after the UEFI handoff.
pub fn kernel_entry(memory_map: &[MemoryRegion]) -> BootReport {
    let mut allocator = frame::FrameAllocator::new(memory_map);
    let mut tables = paging::PageTable::new();
    let page = tables.map_next(&mut allocator);
    let mut heap = heap::Heap::new();
    let allocation = heap.allocate(64, 8);
    serial::log("Lumora Kernel: memory management online");
    BootReport {
        page,
        allocation,
        free_frames: allocator.free_frames(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: u64,
    pub length: u64,
    pub usable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootReport {
    pub page: Option<u64>,
    pub allocation: Option<u64>,
    pub free_frames: usize,
}
