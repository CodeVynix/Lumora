//! Four-level paging model used to reserve virtual pages for kernel mappings.

use crate::frame::FrameAllocator;

pub struct PageTable {
    root: u64,
    next_virtual: u64,
}

impl PageTable {
    pub fn new(frames: &mut FrameAllocator<'_>) -> Option<Self> {
        let root = frames.allocate()?;
        let identity_pdpt = frames.allocate()?;
        let identity_pd = frames.allocate()?;
        zero_table(root);
        zero_table(identity_pdpt);
        zero_table(identity_pd);

        write_entry(root, 0, identity_pdpt | PRESENT | WRITABLE);
        write_entry(identity_pdpt, 0, identity_pd | PRESENT | WRITABLE);
        for index in 0..512u64 {
            write_entry(
                identity_pd,
                index as usize,
                (index * (2 * 1024 * 1024)) | PRESENT | WRITABLE | HUGE,
            );
        }

        // Mirror the identity map in the kernel's high-half address space.
        write_entry(root, 256, identity_pdpt | PRESENT | WRITABLE);
        load_root(root);
        Some(Self {
            root,
            next_virtual: 0xffff_8000_4000_0000,
        })
    }

    /// Maps one page for the kernel heap.
    pub fn map_heap_page(&mut self, frames: &mut FrameAllocator<'_>) -> Option<u64> {
        self.map_heap_range(frames, 1)
    }

    /// Maps `page_count` consecutive pages for the kernel heap and returns the
    /// first virtual address. `page_count` must fit in a single page table.
    pub fn map_heap_range(
        &mut self,
        frames: &mut FrameAllocator<'_>,
        page_count: u64,
    ) -> Option<u64> {
        if page_count == 0 || page_count > 512 {
            return None;
        }
        let pdpt = frames.allocate()?;
        let pd = frames.allocate()?;
        let pt = frames.allocate()?;
        zero_table(pdpt);
        zero_table(pd);
        zero_table(pt);
        for index in 0..page_count {
            let physical = frames.allocate()?;
            write_entry(pt, index as usize, physical | PRESENT | WRITABLE);
        }
        write_entry(self.root, 256, pdpt | PRESENT | WRITABLE);
        write_entry(pdpt, 1, pd | PRESENT | WRITABLE);
        write_entry(pd, 0, pt | PRESENT | WRITABLE);
        let virtual_address = self.next_virtual;
        Some(virtual_address)
    }
}

const PRESENT: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const HUGE: u64 = 1 << 7;

fn zero_table(physical: u64) {
    // SAFETY: the frame allocator returns aligned, usable physical frames and
    // the boot identity map makes them writable before and after CR3 changes.
    unsafe {
        core::ptr::write_bytes(physical as *mut u64, 0, 512);
    }
}

fn write_entry(table: u64, index: usize, value: u64) {
    // SAFETY: `table` is a page-table frame owned by this PageTable and
    // `index` is bounded to the 512 entries in that frame.
    unsafe {
        core::ptr::write((table as *mut u64).add(index), value);
    }
}

fn load_root(root: u64) {
    // SAFETY: `root` points to the fully initialized, identity-mapped PML4
    // created above, so loading it preserves execution and stack mappings.
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags));
    }
}
