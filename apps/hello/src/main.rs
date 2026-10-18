//! Hello program for the Lumora ELF loader test.
//!
//! A freestanding static executable: it writes one message with the write
//! syscall and exits with code 0. It deliberately uses no libc, no thread
//! support, and no other syscalls.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

/// Writes bytes to the serial port. Arguments: pointer, length.
const SYS_WRITE_SERIAL: u64 = 1;
/// Exits the task. Argument: exit code.
const SYS_EXIT: u64 = 2;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let message = b"hello from an ELF program";
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") SYS_WRITE_SERIAL,
            in("rdi") message.as_ptr() as u64,
            in("rsi") message.len() as u64,
            lateout("rax") _,
        );
        core::arch::asm!(
            "int 0x80",
            in("rax") SYS_EXIT,
            in("rdi") 0u64,
            options(noreturn),
        );
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
