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

    /// Marks the first-gigabyte identity mapping user-accessible so ring-3
    /// code can run from the low image pages.
    ///
    /// shortcut: the whole low gigabyte becomes user-accessible instead of
    /// only the user payload pages; upgrade to per-process page tables when
    /// the process model needs real isolation.
    pub fn make_identity_user_accessible(&mut self) {
        // SAFETY: the table addresses below come from this page table's own
        // entries, so they are live frames, and the identity map keeps them
        // writable; reloading CR3 flushes stale supervisor-only translations.
        unsafe {
            let root = self.root;
            let pml4e = read_entry(root, 0);
            let pdpt = pml4e & !0xFFF;
            write_entry(root, 0, pml4e | USER);
            let pdpte = read_entry(pdpt, 0);
            write_entry(pdpt, 0, pdpte | USER);
            let pd = pdpte & !0xFFF;
            for index in 0..512usize {
                write_entry(pd, index, read_entry(pd, index) | USER);
            }
            core::arch::asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags));
        }
    }
}

pub(crate) const PRESENT: u64 = 1;
pub(crate) const WRITABLE: u64 = 1 << 1;
pub(crate) const USER: u64 = 1 << 2;
pub(crate) const HUGE: u64 = 1 << 7;

/// Root of the currently loaded page tables, with flags masked off.
pub fn current_root() -> u64 {
    read_cr3() & !0xFFF
}

/// Returns true when every byte in `[start, start + len)` is present and
/// user-accessible in the currently loaded page tables.
pub fn is_user_range(start: u64, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return false,
    };
    if !is_canonical(start) || !is_canonical(end - 1) {
        return false;
    }
    let mut page = start & !0xFFF;
    loop {
        if !is_user_page(page) {
            return false;
        }
        match page.checked_add(0x1000) {
            Some(next) if next < end => page = next,
            _ => break,
        }
    }
    true
}

/// Returns true when the page containing `address` is present and
/// user-accessible in the currently loaded page tables.
fn is_user_page(address: u64) -> bool {
    // Every table address below is read back out of a live entry of the
    // current page tables, and page-table memory stays mapped.
    let root = read_cr3() & !0xFFF;
    let pml4e = read_entry(root, ((address >> 39) & 0x1FF) as usize);
    if pml4e & (PRESENT | USER) != (PRESENT | USER) {
        return false;
    }
    let pdpt = pml4e & !0xFFF;
    let pdpte = read_entry(pdpt, ((address >> 30) & 0x1FF) as usize);
    if pdpte & (PRESENT | USER) != (PRESENT | USER) {
        return false;
    }
    if pdpte & HUGE != 0 {
        return true;
    }
    let pd = pdpte & !0xFFF;
    let pde = read_entry(pd, ((address >> 21) & 0x1FF) as usize);
    if pde & (PRESENT | USER) != (PRESENT | USER) {
        return false;
    }
    if pde & HUGE != 0 {
        return true;
    }
    let pt = pde & !0xFFF;
    let pte = read_entry(pt, ((address >> 12) & 0x1FF) as usize);
    pte & (PRESENT | USER) == (PRESENT | USER)
}

/// Returns true for canonical 64-bit addresses, whose top 16 bits are all
/// clear or all set (Intel SDM Vol. 3, section 3.3.7.1).
fn is_canonical(address: u64) -> bool {
    address >> 47 == 0 || address >> 47 == 0x1FFFF
}

pub(crate) fn zero_table(physical: u64) {
    // SAFETY: the frame allocator returns aligned, usable physical frames and
    // the boot identity map makes them writable before and after CR3 changes.
    unsafe {
        core::ptr::write_bytes(physical as *mut u64, 0, 512);
    }
}

pub(crate) fn read_entry(table: u64, index: usize) -> u64 {
    // SAFETY: `table` is a page-table frame from live tables and `index` is
    // bounded to the 512 entries in that frame.
    unsafe { core::ptr::read((table as *const u64).add(index)) }
}

pub(crate) fn write_entry(table: u64, index: usize, value: u64) {
    // SAFETY: `table` is a page-table frame owned by live tables under
    // construction and `index` is bounded to the 512 entries in that frame.
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

fn read_cr3() -> u64 {
    let root: u64;
    // SAFETY: reading CR3 only reports the active root and changes nothing.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) root, options(nomem, nostack, preserves_flags));
    }
    root
}
