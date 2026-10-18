//! Per-process address spaces for the isolation test.
//!
//! Each process gets its own top-level page table that shares the kernel's
//! low identity mapping and high-half heap mapping, so interrupts, syscalls,
//! and the kernel stack keep working, plus fresh physical frames for its own
//! user code, data, and stack. User pages are mapped user-accessible; kernel
//! pages never are. Processes run one at a time in this slice: entering one
//! loads its tables, and exiting or killing it switches back and frees its
//! frames.

use crate::elf;
use crate::frame::FrameAllocator;
use crate::paging;
use crate::usermode;

/// Maximum live processes; the test needs three.
const MAX_PROCESSES: usize = 4;

/// User virtual addresses, identical in every process: code page, data page,
/// and stack page under one fresh page-table hierarchy.
pub const USER_CODE_VA: u64 = 0x80_0000_0000;
/// Data page both isolation payloads map at the same address.
pub const USER_DATA_VA: u64 = 0x80_0000_1000;
const USER_STACK_PAGE: u64 = 0x80_0000_2000;
/// Top of the user stack handed to a process on entry.
pub const USER_STACK_TOP: u64 = USER_STACK_PAGE + 0x1000;

/// Frames owned by one process: PML4, PDPT, PD, page table, code, data, and
/// stack, in that order.
const FRAMES_PER_PROCESS: usize = 7;

/// Lifecycle of a process, and also of a table slot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessState {
    /// Slot holds a live process that may be entered.
    Alive,
    /// Slot held a process whose frames were freed on exit.
    Dead,
    /// Slot never held a process.
    Unused,
}

/// One process and everything needed to run and release it.
#[derive(Clone, Copy)]
struct Process {
    id: usize,
    /// Physical address of the process's own PML4 frame.
    root: u64,
    entry: u64,
    user_stack_top: u64,
    frames: [u64; FRAMES_PER_PROCESS],
    state: ProcessState,
}

impl Process {
    /// Placeholder for slots that never held a process.
    const UNUSED: Self = Self {
        id: usize::MAX,
        root: 0,
        entry: 0,
        user_stack_top: 0,
        frames: [0; FRAMES_PER_PROCESS],
        state: ProcessState::Unused,
    };
}

/// The process table.
static mut PROCESSES: [Process; MAX_PROCESSES] = [Process::UNUSED; MAX_PROCESSES];

/// Builds an address space for `code` and returns the process id.
///
/// Copies `code` (which must fit in one page) into a fresh user-executable
/// frame, maps private zeroed data and stack pages at the fixed user
/// addresses, and shares the kernel's low and high mappings. Only the running
/// thread may call this; it runs before the process exists.
pub fn spawn(code: &[u8], frames: &mut FrameAllocator) -> Option<usize> {
    let index = next_free_slot()?;
    if code.len() > 4096 {
        return None;
    }
    let mut owned = [0u64; FRAMES_PER_PROCESS];
    let mut count = 0usize;
    while count < FRAMES_PER_PROCESS {
        match frames.allocate() {
            Some(frame) => {
                owned[count] = frame;
                count += 1;
            }
            None => {
                for frame in owned.iter().take(count) {
                    frames.free(*frame);
                }
                return None;
            }
        }
    }
    let [pml4, pdpt, pd, pt, code_frame, data_frame, stack_frame] = owned;

    // SAFETY: all seven frames came from the allocator in this call, none is
    // referenced anywhere yet, and the identity map keeps them writable. The
    // new PML4 stays unloaded until `enter`, so nothing can observe the
    // half-built tables.
    unsafe {
        paging::zero_table(pml4);
        paging::zero_table(pdpt);
        paging::zero_table(pd);
        paging::zero_table(pt);
        paging::zero_table(data_frame);
        paging::zero_table(stack_frame);
        core::ptr::write_bytes(code_frame as *mut u8, 0, 4096);
        core::ptr::copy_nonoverlapping(code.as_ptr(), code_frame as *mut u8, code.len());
        let kernel_root = paging::current_root();
        paging::write_entry(pml4, 0, paging::read_entry(kernel_root, 0));
        paging::write_entry(pml4, 256, paging::read_entry(kernel_root, 256));
        paging::write_entry(
            pml4,
            1,
            pdpt | paging::PRESENT | paging::WRITABLE | paging::USER,
        );
        paging::write_entry(
            pdpt,
            0,
            pd | paging::PRESENT | paging::WRITABLE | paging::USER,
        );
        paging::write_entry(
            pd,
            0,
            pt | paging::PRESENT | paging::WRITABLE | paging::USER,
        );
        let code_flags = paging::PRESENT | paging::WRITABLE | paging::USER;
        paging::write_entry(pt, 0, code_frame | code_flags);
        paging::write_entry(pt, 1, data_frame | code_flags);
        paging::write_entry(pt, 2, stack_frame | code_flags);
        core::ptr::write(
            slot(index),
            Process {
                id: index,
                root: pml4,
                entry: USER_CODE_VA,
                user_stack_top: USER_STACK_TOP,
                frames: owned,
                state: ProcessState::Alive,
            },
        );
    }
    // Process ids are their slot indices; read the stored one back so callers
    // name the process exactly as the table holds it.
    let id = unsafe { core::ptr::read(slot(index)).id };
    Some(id)
}

/// Builds an address space for a validated image and returns the process id.
///
/// Maps every segment at its validated address with permissions from its
/// flags, maps a fresh zeroed stack page, and shares the kernel's low and
/// high mappings. Only the running thread may call this; it runs before the
/// process exists.
pub fn spawn_elf(
    image: &elf::BootImage,
    file: &[u8],
    frames: &mut FrameAllocator,
) -> Option<usize> {
    let index = next_free_slot()?;
    let pml4 = frames.allocate()?;
    let pdpt = match frames.allocate() {
        Some(frame) => frame,
        None => {
            frames.free(pml4);
            return None;
        }
    };

    // Both tables came from the allocator in this call, none is referenced
    // anywhere yet, and the identity map keeps them writable. The new PML4
    // stays unloaded until `enter_elf`, so nothing can observe the
    // half-built tables. The page directories and page tables below it are
    // created on demand by `user_page_table`.
    paging::zero_table(pml4);
    paging::zero_table(pdpt);
    let kernel_root = paging::current_root();
    paging::write_entry(pml4, 0, paging::read_entry(kernel_root, 0));
    paging::write_entry(pml4, 256, paging::read_entry(kernel_root, 256));
    paging::write_entry(
        pml4,
        1,
        pdpt | paging::PRESENT | paging::WRITABLE | paging::USER,
    );
    for segment in image.segments.iter().take(image.segment_count) {
        if !map_segment(pdpt, segment, file, frames) {
            // SAFETY: the half-built tables are still unloaded; tearing them
            // down releases whatever the loop above managed to map.
            unsafe {
                teardown_tables(pml4, frames);
            }
            return None;
        }
    }
    // SAFETY: same half-built, still-unloaded tables as above.
    if !map_stack_page(pdpt, frames) {
        // SAFETY: the half-built tables are still unloaded; tearing them
        // down releases whatever the loop above managed to map.
        unsafe {
            teardown_tables(pml4, frames);
        }
        return None;
    }
    // SAFETY: the address space is complete: every validated segment page and
    // the stack page are mapped user-accessible, and the kernel mappings are
    // shared, so interrupts keep working once it loads.
    unsafe {
        core::ptr::write(
            slot(index),
            Process {
                id: index,
                root: pml4,
                entry: image.entry,
                user_stack_top: elf::ELF_STACK_TOP,
                frames: [0; FRAMES_PER_PROCESS],
                state: ProcessState::Alive,
            },
        );
    }
    // Process ids are their slot indices; read the stored one back so callers
    // name the process exactly as the table holds it.
    let id = unsafe { core::ptr::read(slot(index)).id };
    Some(id)
}

/// Runs the process: switches to its tables, enters ring 3, and on exit or
/// kill switches back, frees its frames, and returns the exit code.
///
/// The slot must hold a live process; each test process is entered once, so a
/// second entry attempt reports an error instead of reusing freed frames.
pub fn enter(id: usize, frames: &mut FrameAllocator) -> u64 {
    if id >= MAX_PROCESSES {
        return u64::MAX;
    }
    let process = unsafe { core::ptr::read(slot(id)) };
    if process.state != ProcessState::Alive {
        return u64::MAX;
    }
    let interrupts_were_enabled = crate::cpu::interrupts_enabled();
    // SAFETY: the slot holds a live process whose tables map its entry point
    // and stack user-accessible while sharing the kernel mappings, so
    // interrupts keep working; the exit and fault paths restore the kernel
    // tables before this function touches anything again.
    let code = unsafe {
        usermode::enter_process(
            process.entry,
            process.user_stack_top,
            usermode::user_resume_slot(),
            process.root,
        )
    };
    // The exit and fault paths abandon their interrupt frames with interrupts
    // disabled, so restore whatever the caller had before releasing anything.
    if interrupts_were_enabled {
        // SAFETY: the kernel tables and stack are restored already, and the
        // IDT covers every IRQ the kernel leaves unmasked.
        unsafe {
            crate::cpu::enable_interrupts();
        }
    }
    // SAFETY: back on the kernel tables with the kernel stack restored, so
    // the slot write and the frame releases below only touch live memory;
    // marking the slot dead first guarantees no second entry.
    unsafe {
        let mut dead = core::ptr::read(slot(id));
        dead.state = ProcessState::Dead;
        core::ptr::write(slot(id), dead);
        for frame in process.frames {
            frames.free(frame);
        }
    }
    code
}

/// Top of the user stack mapped for every ELF process: one fresh page below
/// the end of the shared page-table window, so segments can never collide
/// with it.
pub const ELF_STACK_TOP: u64 = 0x80_2000_0000;
/// Base of that stack page.
const ELF_STACK_PAGE: u64 = ELF_STACK_TOP - 0x1000;

/// Runs a process built by [`spawn_elf`]: switches to its tables, enters ring
/// 3, and on exit or kill switches back, tears the address space down, and
/// returns the exit code.
///
/// The slot must hold a live process; each test process is entered once, so a
/// second entry attempt reports an error instead of reusing freed frames.
pub fn enter_elf(id: usize, frames: &mut FrameAllocator) -> u64 {
    if id >= MAX_PROCESSES {
        return u64::MAX;
    }
    let process = unsafe { core::ptr::read(slot(id)) };
    if process.state != ProcessState::Alive {
        return u64::MAX;
    }
    let interrupts_were_enabled = crate::cpu::interrupts_enabled();
    // SAFETY: the slot holds a live process whose tables map its entry point
    // and stack user-accessible while sharing the kernel mappings, so
    // interrupts keep working; the exit and fault paths restore the kernel
    // tables before this function touches anything again.
    let code = unsafe {
        usermode::enter_process(
            process.entry,
            process.user_stack_top,
            usermode::user_resume_slot(),
            process.root,
        )
    };
    // The exit and fault paths abandon their interrupt frames with interrupts
    // disabled, so restore whatever the caller had before releasing anything.
    if interrupts_were_enabled {
        // SAFETY: the kernel tables and stack are restored already, and the
        // IDT covers every IRQ the kernel leaves unmasked.
        unsafe {
            crate::cpu::enable_interrupts();
        }
    }
    // SAFETY: back on the kernel tables with the kernel stack restored, so
    // marking the slot dead first guarantees no second entry, and the walk
    // below only frees frames this address space owns.
    unsafe {
        let mut dead = core::ptr::read(slot(id));
        dead.state = ProcessState::Dead;
        core::ptr::write(slot(id), dead);
        teardown_tables(process.root, frames);
    }
    code
}

/// Maps one validated segment into the process page table, one fresh zeroed
/// frame per page, copying the file bytes into place. Returns false when
/// frames run out, leaving a half-built table the caller tears down.
/// Slot indices for `address` at the PML4, PDPT, PD, and PT levels.
///
/// The user window spans many page directories, so every level has to use its
/// own slot; reusing one level's index for the level below silently misplaces
/// addresses that happen to land on zero.
fn walk_slots(address: u64) -> (usize, usize, usize, usize) {
    (
        ((address >> 39) & 0x1FF) as usize,
        ((address >> 30) & 0x1FF) as usize,
        ((address >> 21) & 0x1FF) as usize,
        ((address >> 12) & 0x1FF) as usize,
    )
}

/// Returns the page table that maps `address`, creating the page directory and
/// page table it needs the first time an address in their 2 MB window is used.
/// The user window spans many page directories, so each address is placed with
/// its own directory index instead of being flattened into one table.
fn user_page_table(pdpt: u64, address: u64, frames: &mut FrameAllocator) -> Option<u64> {
    let (_, pdpt_index, pd_index, _) = walk_slots(address);
    let pde = paging::read_entry(pdpt, pdpt_index);
    let pd = if pde & paging::PRESENT != 0 && pde & paging::HUGE == 0 {
        pde & !0xFFF
    } else {
        let frame = frames.allocate()?;
        paging::zero_table(frame);
        paging::write_entry(
            pdpt,
            pdpt_index,
            frame | paging::PRESENT | paging::WRITABLE | paging::USER,
        );
        frame
    };
    let pte = paging::read_entry(pd, pd_index);
    if pte & paging::PRESENT != 0 && pte & paging::HUGE == 0 {
        return Some(pte & !0xFFF);
    }
    let frame = frames.allocate()?;
    paging::zero_table(frame);
    paging::write_entry(
        pd,
        pd_index,
        frame | paging::PRESENT | paging::WRITABLE | paging::USER,
    );
    Some(frame)
}

/// Copies a validated segment into the user address space, one page at a time.
fn map_segment(
    pdpt: u64,
    segment: &elf::Segment,
    file: &[u8],
    frames: &mut FrameAllocator,
) -> bool {
    let pages = segment.memsz.div_ceil(4096);
    let mut mapped = 0u64;
    while mapped < pages {
        let address = segment.vaddr + mapped * 4096;
        let Some(pt) = user_page_table(pdpt, address, frames) else {
            return false;
        };
        let frame = match frames.allocate() {
            Some(frame) => frame,
            None => return false,
        };
        // SAFETY: `frame` is fresh from the allocator and writable through
        // the identity map; the page table belongs to still-unloaded tables.
        // The validated range keeps every computed address inside the file
        // and inside the user window.
        unsafe {
            paging::zero_table(frame);
            let file_offset = segment.offset + mapped * 4096;
            let chunk = segment.filesz.saturating_sub(mapped * 4096).min(4096) as usize;
            if chunk > 0 {
                let src = file.as_ptr().add(file_offset as usize);
                core::ptr::copy_nonoverlapping(src, frame as *mut u8, chunk);
            }
            let mut flags = paging::PRESENT | paging::USER;
            if segment.flags & elf::FLAG_WRITE != 0 {
                flags |= paging::WRITABLE;
            }
            paging::write_entry(pt, walk_slots(address).3, frame | flags);
        }
        mapped += 1;
    }
    true
}

/// Maps one fresh zeroed stack page at the top of the user window. Returns
/// false when frames run out.
fn map_stack_page(pdpt: u64, frames: &mut FrameAllocator) -> bool {
    let Some(pt) = user_page_table(pdpt, ELF_STACK_PAGE, frames) else {
        return false;
    };
    match frames.allocate() {
        Some(frame) => {
            // Fresh frame, writable through the identity map; the page table
            // belongs to still-unloaded tables built by the caller.
            paging::zero_table(frame);
            paging::write_entry(
                pt,
                walk_slots(ELF_STACK_PAGE).3,
                frame | paging::PRESENT | paging::WRITABLE | paging::USER,
            );
            true
        }
        None => false,
    }
}

/// Releases a process address space: every page mapped under PML4 entry 1,
/// the private tables, and the PML4 itself. Shared kernel entries are never
/// touched.
///
/// # Safety
///
/// `root` must be a PML4 built by [`spawn_elf`] (complete, or half-built on
/// the failure path) that is not currently loaded.
unsafe fn teardown_tables(root: u64, frames: &mut FrameAllocator) {
    let pml4e = paging::read_entry(root, 1);
    if pml4e & paging::PRESENT != 0 {
        let pdpt = pml4e & !0xFFF;
        for pdpt_index in 0..512usize {
            let pdpte = paging::read_entry(pdpt, pdpt_index);
            if pdpte & paging::PRESENT == 0 || pdpte & paging::HUGE != 0 {
                continue;
            }
            let pd = pdpte & !0xFFF;
            for pd_index in 0..512usize {
                let pde = paging::read_entry(pd, pd_index);
                if pde & paging::PRESENT == 0 || pde & paging::HUGE != 0 {
                    continue;
                }
                let pt = pde & !0xFFF;
                for pt_index in 0..512usize {
                    let pte = paging::read_entry(pt, pt_index);
                    if pte & paging::PRESENT != 0 {
                        frames.free(pte & !0xFFF);
                    }
                }
                frames.free(pt);
            }
            frames.free(pd);
        }
        frames.free(pdpt);
    }
    frames.free(root);
}

/// Raw pointer to slot `index` in the process table.
///
/// # Safety
///
/// `index` must be inside the table. The pointer is only safe to use while
/// nothing else reads or writes that slot, which holds for the running thread
/// whenever it holds the call.
unsafe fn slot(index: usize) -> *mut Process {
    unsafe {
        core::ptr::addr_of_mut!(PROCESSES)
            .cast::<Process>()
            .add(index)
    }
}

/// Finds the first slot that never held a live process.
fn next_free_slot() -> Option<usize> {
    (0..MAX_PROCESSES).find(|index| unsafe {
        matches!(
            core::ptr::read(slot(*index)).state,
            ProcessState::Unused | ProcessState::Dead
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real image walks to PML4[1] -> PDPT[0] -> PD[0] -> PT[0], so an index
    /// mistake at any level stays hidden until an address needs a higher PD or
    /// PDPT slot. These cases pin the slot each level must use.
    #[test]
    fn user_window_walk_uses_the_right_slot_per_level() {
        assert_eq!(walk_slots(0x80_0000_0000), (1, 0, 0, 0));
        assert_eq!(walk_slots(0x80_0000_1000), (1, 0, 0, 1));
        // The stack page is in the last 2 MB of the window, so it needs a
        // different page directory and the last slot of its table.
        assert_eq!(walk_slots(ELF_STACK_PAGE), (1, 0, 255, 511));
        assert_eq!(walk_slots(ELF_STACK_TOP - 0x18), (1, 0, 255, 511));
    }
}
