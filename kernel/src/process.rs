//! Per-process address spaces for the isolation test.
//!
//! Each process gets its own top-level page table that shares the kernel's
//! low identity mapping and high-half heap mapping, so interrupts, syscalls,
//! and the kernel stack keep working, plus fresh physical frames for its own
//! user code, data, and stack. User pages are mapped user-accessible; kernel
//! pages never are. Processes run one at a time in this slice: entering one
//! loads its tables, and exiting or killing it switches back and frees its
//! frames.

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
