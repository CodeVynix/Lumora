//! Cooperative kernel threads: one context switch and a round-robin queue.
//!
//! Threads never preempt each other. A thread runs until it calls
//! [`yield_now`], which switches to the next runnable thread; the timer
//! interrupt only counts ticks and never switches. The boot context is thread
//! 0 and never finishes, so it always keeps the round robin alive.

use crate::cpu;
use crate::heap;

/// Maximum number of threads, including thread 0 (the boot context).
const MAX_THREADS: usize = 8;

/// Stack size carved out of the kernel heap for every spawned thread.
pub const THREAD_STACK_BYTES: u64 = 16 * 1024;

/// Bytes a fresh thread's first switch consumes from its stack: the eight
/// callee-saved registers the switch pops plus the slot its `ret` jumps
/// through into the trampoline.
const FRAME_BYTES: u64 = 8 * 8 + 8;

/// Lifecycle of a thread, and also of a table slot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ThreadState {
    /// Slot holds a thread that may be switched to.
    Runnable,
    /// Slot holds a thread whose entry point returned.
    Finished,
    /// Slot never held a thread.
    Unused,
}

/// One thread and everything the context switch needs to resume it.
#[derive(Clone, Copy)]
struct Thread {
    id: usize,
    entry: Option<fn()>,
    /// Stack pointer saved by the context switch. For a new thread it points
    /// at the frame [`spawn`] builds for the first switch.
    stack_pointer: u64,
    state: ThreadState,
}

impl Thread {
    /// Placeholder for slots that never held a thread.
    const UNUSED: Self = Self {
        id: usize::MAX,
        entry: None,
        stack_pointer: 0,
        state: ThreadState::Unused,
    };
}

/// The thread table. Slot 0 is the boot context, created by [`init`].
static mut THREADS: [Thread; MAX_THREADS] = [Thread::UNUSED; MAX_THREADS];

/// Slot of the thread that is running right now.
static mut CURRENT: usize = 0;

/// Kernel heap that [`spawn`] allocates thread stacks from.
static mut HEAP: *mut heap::Heap = core::ptr::null_mut();

/// Creates thread 0: the boot context that calls this function.
///
/// Call this once, before any thread is spawned and while the CPU is still
/// single threaded, so nothing else can observe a half-built table.
pub fn init() {
    // SAFETY: `kernel_entry` is the only caller and runs before interrupts
    // are enabled, so the table and the current slot are ours alone.
    unsafe {
        core::ptr::write(slot(0), Thread::thread_0());
        core::ptr::write(core::ptr::addr_of_mut!(CURRENT), 0);
    }
}

/// Registers the kernel heap that [`spawn`` allocates thread stacks from.
///
/// The heap must outlive every thread, so this runs before any spawn and the
/// heap must stay owned by the caller that outlives the scheduler.
pub fn register_heap(heap: &mut heap::Heap) {
    // SAFETY: the heap is owned by `kernel_entry`, which also runs the idle
    // loop, so it outlives every thread; the boot context is the only caller
    // and the only thread that exists at that point.
    unsafe {
        *core::ptr::addr_of_mut!(HEAP) = heap as *mut heap::Heap;
    }
}

/// Spawns a thread that runs `entry` and returns its slot index.
///
/// The thread gets a 16 KiB stack from the kernel heap. Its first context
/// switch lands in a trampoline that calls `entry`; when `entry` returns the
/// trampoline marks the thread finished and keeps yielding, so control never
/// falls back into the saved frame.
///
/// Only the running thread may call this: with no preemption it is the only
/// thread that can be active at that moment.
pub fn spawn(entry: fn()) -> Option<usize> {
    let index = next_free_slot()?;
    let heap = unsafe { *core::ptr::addr_of!(HEAP) };
    if heap.is_null() {
        return None;
    }
    let stack = unsafe { (*heap).allocate(THREAD_STACK_BYTES, 16) }?;
    // The heap hands out 16-byte aligned memory, so the top of the stack is
    // 16-byte aligned too. Starting the frame 72 bytes below that leaves the
    // entry stack pointer 8 bytes off a 16-byte boundary, which is exactly
    // what the trampoline's first `call` needs under the ABI.
    let frame = stack + THREAD_STACK_BYTES - FRAME_BYTES;

    // SAFETY: the frame plus the 72 bytes above it all sit inside the freshly
    // allocated, writable 16 KiB stack.
    unsafe {
        core::ptr::write_bytes(frame as *mut u64, 0, (FRAME_BYTES / 8) as usize);
        (frame as *mut u64)
            .add(8)
            .write(thread_trampoline as *const () as usize as u64);
        core::ptr::write(
            slot(index),
            Thread {
                id: index,
                entry: Some(entry),
                stack_pointer: frame,
                state: ThreadState::Runnable,
            },
        );
    }
    // Thread ids are their slot indices; read the stored one back so callers
    // name the thread exactly as the table holds it.
    let id = unsafe { core::ptr::read(slot(index)).id };
    Some(id)
}

/// Switches to the next runnable thread, round robin.
///
/// Interrupts are disabled across the switch so a tick cannot land in the
/// middle of it, then restored to whatever the caller had.
pub fn yield_now() {
    let interrupts_were_enabled = cpu::interrupts_enabled();
    // SAFETY: interrupts stay off only until the switch below finishes, and
    // the timer handler only counts ticks, so nothing is lost meanwhile.
    unsafe {
        cpu::disable_interrupts();
    }
    let current = unsafe { *core::ptr::addr_of!(CURRENT) };
    if let Some(next) = next_runnable(current) {
        // SAFETY: both slots hold threads: `current` is the running thread and
        // `next` was found by scanning the table. The switch stores this
        // thread's stack pointer in its slot and then loads the other one's.
        unsafe {
            let saved = stack_pointer_slot(current);
            core::ptr::write(core::ptr::addr_of_mut!(CURRENT), next);
            context_switch(saved, core::ptr::read(stack_pointer_slot(next)));
        }
    }
    if interrupts_were_enabled {
        // SAFETY: the switch is complete, and the IDT covers every IRQ the
        // kernel leaves unmasked.
        unsafe {
            cpu::enable_interrupts();
        }
    }
}

/// Counts spawned threads whose entry point already returned.
pub fn finished_threads() -> usize {
    (1..MAX_THREADS)
        .filter(|index| unsafe { core::ptr::read(slot(*index)).state } == ThreadState::Finished)
        .count()
}

/// Finds the first slot that never held a thread, starting at 1.
fn next_free_slot() -> Option<usize> {
    (1..MAX_THREADS)
        .find(|index| unsafe { core::ptr::read(slot(*index)).state } == ThreadState::Unused)
}

/// Finds the next runnable slot after `from`, wrapping around the table.
fn next_runnable(from: usize) -> Option<usize> {
    (1..=MAX_THREADS).find_map(|step| {
        let index = (from + step) % MAX_THREADS;
        (unsafe { core::ptr::read(slot(index)).state } == ThreadState::Runnable).then_some(index)
    })
}

/// Entry point the first context switch into a new thread lands in.
///
/// It calls the thread's entry point and, once that returns, marks the thread
/// finished and keeps yielding, so control never falls back into the saved
/// frame. The `2: jmp 2b` tail is unreachable: the last call never returns.
#[unsafe(naked)]
extern "C" fn thread_trampoline() {
    core::arch::naked_asm!(
        "sub rsp, 32",
        "call {run}",
        "add rsp, 32",
        "call {finish}",
        "2:",
        "jmp 2b",
        run = sym run_current_entry,
        finish = sym finish_current_entry,
    );
}

/// Calls the entry point of the current thread.
///
/// # Safety
///
/// Only the trampoline calls this, as the first thing a new thread runs, so
/// the current slot always holds that thread's entry point.
unsafe extern "C" fn run_current_entry() {
    // SAFETY: the trampoline runs on the current thread's stack, so `CURRENT`
    // is its slot, and that slot holds its entry point.
    unsafe {
        let current = *core::ptr::addr_of!(CURRENT);
        if let Some(entry) = core::ptr::read(slot(current)).entry {
            entry();
        }
    }
}

/// Marks the current thread finished and yields forever.
///
/// # Safety
///
/// Only the trampoline calls this, after the current thread's entry point
/// returned.
unsafe extern "C" fn finish_current_entry() -> ! {
    // SAFETY: the trampoline runs on the current thread's stack, so `CURRENT`
    // is its slot, and no other thread touches the table while this one runs
    // because threads only switch inside `yield_now`, with interrupts off.
    unsafe {
        let current = *core::ptr::addr_of!(CURRENT);
        let mut thread = core::ptr::read(slot(current));
        thread.state = ThreadState::Finished;
        core::ptr::write(slot(current), thread);
    }
    loop {
        yield_now();
    }
}

/// Saves this thread's callee-saved registers and resumes another thread.
///
/// The registers are pushed on the running thread's stack and its stack
/// pointer is written through `saved`, then `next_stack_pointer` is loaded.
/// On a fresh thread that pointer addresses the frame [`spawn`] built: the
/// pops below restore eight zeroed registers and the final `ret` jumps to
/// [`thread_trampoline`] with the stack 16-byte aligned.
///
/// # Safety
///
/// `saved` must point at the running thread's stack-pointer slot and
/// `next_stack_pointer` must be a stack pointer stored by an earlier call
/// on a thread that is still alive.
#[unsafe(naked)]
unsafe extern "C" fn context_switch(saved: *mut u64, next_stack_pointer: u64) {
    core::arch::naked_asm!(
        "push rbx",
        "push rbp",
        "push rdi",
        "push rsi",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // The UEFI target uses the Microsoft x64 ABI, so the first argument
        // is in RCX and the second in RDX (Intel SDM is ABI-agnostic here).
        "mov [rcx], rsp",
        "mov rsp, rdx",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rsi",
        "pop rdi",
        "pop rbp",
        "pop rbx",
        "ret",
    );
}

impl Thread {
    /// The boot context: no entry point, stack pointer filled in by the first
    /// switch away from it.
    const fn thread_0() -> Self {
        Self {
            id: 0,
            entry: None,
            stack_pointer: 0,
            state: ThreadState::Runnable,
        }
    }
}

/// Raw pointer to slot `index` in the thread table.
///
/// # Safety
///
/// `index` must be inside the table. The pointer is only safe to use while
/// nothing else reads or writes that slot, which holds for the running thread
/// whenever it holds the call.
unsafe fn slot(index: usize) -> *mut Thread {
    core::ptr::addr_of_mut!(THREADS).cast::<Thread>().add(index)
}

/// Address of the saved stack pointer stored in slot `index`.
///
/// # Safety
///
/// `index` must be inside the table and hold a live thread, because the
/// context switch writes the current thread's stack pointer through it.
unsafe fn stack_pointer_slot(index: usize) -> *mut u64 {
    core::ptr::addr_of_mut!((*slot(index)).stack_pointer)
}
