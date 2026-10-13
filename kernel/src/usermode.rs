//! Ring-3 entry and exit for the embedded user-mode test payloads.
//!
//! `enter_user` saves the kernel context and drops to ring 3 with `iretq`;
//! `exit_to_kernel` abandons the interrupt stack and resumes that context.
//! Only one user task runs at a time in this slice: there is a single resume
//! slot, and the syscall and fault paths below are its only writers.

/// Writes a message to the serial port. Arguments: message pointer, length.
pub const SYS_WRITE_SERIAL: u64 = 1;
/// Exits the task. Argument: exit code.
pub const SYS_EXIT: u64 = 2;

/// Exit code for a task that called the exit syscall.
pub const USER_EXIT_OK: u64 = 0;
/// Exit code for a task killed by a user-mode general protection fault.
pub const USER_EXIT_GP: u64 = 13;
/// Exit code for a task killed by a user-mode page fault.
pub const USER_EXIT_PF: u64 = 14;

/// Kernel stack pointer saved by [`enter_user`] and restored when a user task
/// exits or faults.
static mut USER_RESUME_RSP: u64 = 0;

/// Root of the kernel page tables, restored on every exit from ring 3.
static mut KERNEL_ROOT: u64 = 0;

/// Records the kernel page-table root that [`exit_to_kernel`] restores.
///
/// Call once during boot while the kernel tables are loaded.
pub fn init(kernel_root: u64) {
    // SAFETY: called once during single-threaded boot before any user task
    // exists; afterwards the slot is only read.
    unsafe {
        *core::ptr::addr_of_mut!(KERNEL_ROOT) = kernel_root;
    }
}

/// Address of the resume slot, handed to [`enter_user`].
pub fn user_resume_slot() -> *mut u64 {
    core::ptr::addr_of_mut!(USER_RESUME_RSP)
}

/// Enters ring 3 at `entry_rip` with `user_rsp`, returning the task's exit
/// code once it exits or is killed.
///
/// # Safety
///
/// The user code and data descriptors must be loaded, TSS RSP0 must point at
/// a mapped kernel stack, `entry_rip` and `user_rsp` must address
/// user-accessible memory, `resume_slot` must be [`user_resume_slot`], and no
/// other user task may be running: the slot holds a single context.
#[unsafe(naked)]
pub unsafe extern "C" fn enter_user(entry_rip: u64, user_rsp: u64, resume_slot: *mut u64) -> u64 {
    core::arch::naked_asm!(
        "push rbx",
        "push rbp",
        "push rdi",
        "push rsi",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // The UEFI target uses the Microsoft x64 ABI: entry address in RCX,
        // user stack in RDX, resume slot in R8.
        "mov [r8], rsp",
        "push {user_data}",
        "push rdx",
        "push 0x202",
        "push {user_code}",
        "push rcx",
        "iretq",
        user_data = const crate::gdt::USER_DATA_SELECTOR,
        user_code = const crate::gdt::USER_CODE_SELECTOR,
    );
}

/// Enters ring 3 in a process address space: switches to `root`, then behaves
/// exactly like [`enter_user`].
///
/// # Safety
///
/// Same preconditions as [`enter_user`], plus `root` must be a live PML4 that
/// shares the kernel's low and high mappings and maps `entry_rip` and
/// `user_rsp` user-accessible.
#[unsafe(naked)]
pub unsafe extern "C" fn enter_process(
    entry_rip: u64,
    user_rsp: u64,
    resume_slot: *mut u64,
    root: u64,
) -> u64 {
    core::arch::naked_asm!(
        "push rbx",
        "push rbp",
        "push rdi",
        "push rsi",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Microsoft x64 ABI: entry in RCX, stack in RDX, slot in R8, root
        // in R9. The pushes below stay valid across the CR3 switch because
        // the kernel stack is mapped in both address spaces.
        "mov [r8], rsp",
        "mov cr3, r9",
        "push {user_data}",
        "push rdx",
        "push 0x202",
        "push {user_code}",
        "push rcx",
        "iretq",
        user_data = const crate::gdt::USER_DATA_SELECTOR,
        user_code = const crate::gdt::USER_CODE_SELECTOR,
    );
}

/// Abandons the current interrupt stack and resumes the kernel context saved
/// by [`enter_user`], returning `code` to its caller.
pub fn exit_to_kernel(code: u64, resume_slot: *mut u64) -> ! {
    let kernel_root = unsafe { *core::ptr::addr_of!(KERNEL_ROOT) };
    debug_assert!(kernel_root != 0);
    // SAFETY: `enter_user` stored a live kernel stack pointer in the resume
    // slot before dropping to ring 3, and only one user task runs at a time,
    // so the slot still holds it. Loading the recorded kernel root first
    // restores the kernel mappings before the pops touch the kernel stack;
    // the pops restore exactly the registers `enter_user` saved, and `ret`
    // returns to its caller with `code`. The abandoned interrupt frame is
    // never touched again.
    unsafe {
        core::arch::asm!(
            "mov cr3, r9",
            "mov rsp, [r8]",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rsi",
            "pop rdi",
            "pop rbp",
            "pop rbx",
            "mov rax, rcx",
            "ret",
            in("r9") kernel_root,
            in("r8") resume_slot,
            in("rcx") code,
            options(noreturn),
        );
    }
}
