//! CPU exception entry stubs and the Rust dispatcher behind them.
//!
//! Stable Rust does not provide the `x86-interrupt` calling convention, so
//! every gate gets a hand-written naked stub. Each stub pushes a vector number
//! (and a fake error code where the processor supplies none), saves the
//! general registers into a [`TrapFrame`], and calls [`exception_dispatch`].

use crate::cpu;
use crate::paging;
use crate::serial;
use crate::timer;
use crate::usermode;

/// Divide error, raised by `div` or `idiv` with a zero divisor.
pub const VECTOR_DIVIDE_ERROR: u64 = 0;
/// Breakpoint, raised by `int3`.
pub const VECTOR_BREAKPOINT: u64 = 3;
/// Double fault, raised when a second fault arrives before the first was handled.
pub const VECTOR_DOUBLE_FAULT: u64 = 8;
/// General protection fault.
pub const VECTOR_GENERAL_PROTECTION: u64 = 13;
/// Page fault.
pub const VECTOR_PAGE_FAULT: u64 = 14;
/// System timer, raised by IRQ0 once the PICs are remapped.
pub const VECTOR_TIMER: u64 = 32;
/// Software-interrupt syscall vector, callable from ring 3.
pub const VECTOR_SYSCALL: u64 = 0x80;

/// Divide error pushes no error code, so the stub synthesises a zero one.
#[unsafe(naked)]
pub extern "C" fn divide_error_stub() {
    core::arch::naked_asm!(
        "push 0",
        "push {vector}",
        "jmp {common}",
        vector = const VECTOR_DIVIDE_ERROR,
        common = sym common_entry,
    );
}

/// Breakpoint pushes no error code, so the stub synthesises a zero one.
#[unsafe(naked)]
pub extern "C" fn breakpoint_stub() {
    core::arch::naked_asm!(
        "push 0",
        "push {vector}",
        "jmp {common}",
        vector = const VECTOR_BREAKPOINT,
        common = sym common_entry,
    );
}

/// The processor pushes the double fault's error code itself.
#[unsafe(naked)]
pub extern "C" fn double_fault_stub() {
    core::arch::naked_asm!(
        "push {vector}",
        "jmp {common}",
        vector = const VECTOR_DOUBLE_FAULT,
        common = sym common_entry,
    );
}

/// The processor pushes the general protection fault's error code itself.
#[unsafe(naked)]
pub extern "C" fn general_protection_stub() {
    core::arch::naked_asm!(
        "push {vector}",
        "jmp {common}",
        vector = const VECTOR_GENERAL_PROTECTION,
        common = sym common_entry,
    );
}

/// The processor pushes the page fault's error code itself.
#[unsafe(naked)]
pub extern "C" fn page_fault_stub() {
    core::arch::naked_asm!(
        "push {vector}",
        "jmp {common}",
        vector = const VECTOR_PAGE_FAULT,
        common = sym common_entry,
    );
}

/// IRQs push no error code, so the stub synthesises a zero one.
#[unsafe(naked)]
pub extern "C" fn timer_stub() {
    core::arch::naked_asm!(
        "push 0",
        "push {vector}",
        "jmp {common}",
        vector = const VECTOR_TIMER,
        common = sym common_entry,
    );
}

/// Software interrupts push no error code, so the stub synthesises a zero one.
#[unsafe(naked)]
pub extern "C" fn syscall_stub() {
    core::arch::naked_asm!(
        "push 0",
        "push {vector}",
        "jmp {common}",
        vector = const VECTOR_SYSCALL,
        common = sym common_entry,
    );
}

/// Shared tail of every stub: saves the registers, aligns the stack the way
/// the UEFI target's Microsoft x64 ABI expects, and hands the frame to
/// [`exception_dispatch`]. The first argument travels in RCX there, not RDI.
#[unsafe(naked)]
extern "C" fn common_entry() {
    core::arch::naked_asm!(
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rbp, rsp",
        "mov rcx, rsp",
        "and rsp, -16",
        "sub rsp, 32",
        "call {dispatch}",
        "mov rsp, rbp",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "add rsp, 16",
        "iretq",
        dispatch = sym exception_dispatch,
    );
}

/// Register state pushed by the stubs plus the frame the processor pushes on
/// entry (Intel SDM Vol. 3, section 6.13). Field order mirrors the stubs.
#[repr(C)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    /// Vector number the stub pushed for this exception.
    pub vector: u64,
    /// Error code the processor pushed, or the zero the stub synthesised.
    pub error_code: u64,
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub rflags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

/// Reports the exception in `frame`. A breakpoint returns so the interrupted
/// code resumes, the timer only records a tick, and a syscall runs its
/// handler; a general protection fault from user mode kills that task and
/// resumes the kernel instead of halting. Every other vector is fatal.
///
/// # Safety
///
/// `frame` must point at a [`TrapFrame`] written by the stubs in this module.
/// They build it from eight-byte pushes on the active stack, so it is always
/// eight-byte aligned and stays alive until the dispatcher returns.
pub unsafe extern "C" fn exception_dispatch(frame: *mut TrapFrame) {
    // SAFETY: the stub passes the address of the frame it just built, and the
    // documented preconditions guarantee it is a live, aligned `TrapFrame`.
    let frame = unsafe { &mut *frame };
    match frame.vector {
        VECTOR_BREAKPOINT => breakpoint_handler(frame),
        VECTOR_TIMER => timer::tick(),
        VECTOR_SYSCALL => syscall_handler(frame),
        VECTOR_GENERAL_PROTECTION if frame.code_segment & 3 == 3 => {
            serial::log_args(format_args!(
                "Lumora Kernel: fatal exception General Protection Fault vector={} error_code=0x{:016x} instruction_pointer=0x{:016x} in user mode; killing task",
                frame.vector, frame.error_code, frame.instruction_pointer
            ));
            usermode::exit_to_kernel(usermode::USER_EXIT_GP, usermode::user_resume_slot());
        }
        _ => fatal_handler(frame),
    }
}

/// Logs the breakpoint and returns so `iretq` resumes the interrupted code.
fn breakpoint_handler(frame: &TrapFrame) {
    serial::log_args(format_args!(
        "Lumora Kernel: Breakpoint vector={} instruction_pointer=0x{:016x} handled, resuming execution",
        frame.vector, frame.instruction_pointer
    ));
}

/// Runs one syscall from user mode: number in RAX, pointer in RDI, length in
/// RSI. Returns the result in RAX: bytes written, the exit code path never
/// returning, or `u64::MAX` for a bad call.
fn syscall_handler(frame: &mut TrapFrame) {
    match frame.rax {
        usermode::SYS_WRITE_SERIAL => {
            let pointer = frame.rdi;
            let length = frame.rsi;
            if !paging::is_user_range(pointer, length) {
                frame.rax = u64::MAX;
                return;
            }
            // SAFETY: the range check above proved every byte is present and
            // user-accessible, and single-threaded user tasks cannot remap
            // memory under the handler.
            let bytes =
                unsafe { core::slice::from_raw_parts(pointer as *const u8, length as usize) };
            match core::str::from_utf8(bytes) {
                Ok(text) => {
                    serial::log(text);
                    frame.rax = length;
                }
                Err(_) => {
                    frame.rax = u64::MAX;
                }
            }
        }
        usermode::SYS_EXIT => {
            let code = frame.rdi;
            usermode::exit_to_kernel(code, usermode::user_resume_slot());
        }
        _ => {
            frame.rax = u64::MAX;
        }
    }
}

/// Logs the fatal exception and halts the CPU until the machine is reset.
fn fatal_handler(frame: &TrapFrame) -> ! {
    serial::log_args(format_args!(
        "Lumora Kernel: fatal exception {} vector={} error_code=0x{:016x} instruction_pointer=0x{:016x}",
        exception_name(frame.vector),
        frame.vector,
        frame.error_code,
        frame.instruction_pointer
    ));
    serial::log("Lumora Kernel: halting");
    cpu::halt_forever()
}

/// Human-readable name for the vectors this kernel installs gates for.
const fn exception_name(vector: u64) -> &'static str {
    match vector {
        VECTOR_DIVIDE_ERROR => "Divide Error",
        VECTOR_BREAKPOINT => "Breakpoint",
        VECTOR_DOUBLE_FAULT => "Double Fault",
        VECTOR_GENERAL_PROTECTION => "General Protection Fault",
        VECTOR_PAGE_FAULT => "Page Fault",
        VECTOR_TIMER => "System Timer",
        VECTOR_SYSCALL => "System Call",
        _ => "Unknown Exception",
    }
}

/// Executes a deliberate `int3` so the boot log proves the breakpoint gate
/// returns to the caller.
pub fn test_breakpoint() {
    // SAFETY: the IDT gate for the breakpoint vector is loaded before this
    // runs, and `int3` only pushes an exception frame on the active stack.
    unsafe {
        core::arch::asm!("int3");
    }
}

/// Executes a divide whose divisor is zero so the boot log proves the divide
/// error gate reports the fault instead of letting the machine reset.
pub fn test_divide_error() {
    // SAFETY: `div` by a zero divisor raises a divide error before writing
    // any result, and the loaded gate handles that exception; the inputs are
    // plain registers, so no memory is touched on either path.
    unsafe {
        core::arch::asm!(
            "xor edx, edx",
            "div {divisor}",
            divisor = in(reg) 0u64,
            inout("rax") 1u64 => _,
            out("rdx") _,
        );
    }
}
