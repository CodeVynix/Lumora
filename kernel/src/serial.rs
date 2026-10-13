//! Serial logging abstraction used by early kernel code.

use core::fmt;

use crate::cpu;

/// Formats `arguments` over COM1 and terminates the line.
///
/// Interrupts stay disabled for the whole call, so a timer tick can never
/// preempt a thread in the middle of a line; the previous interrupt state is
/// restored before returning.
pub fn log_args(arguments: fmt::Arguments<'_>) {
    let interrupts_were_enabled = cpu::interrupts_enabled();
    // SAFETY: interrupts stay off only for the port writes below, and nothing
    // else in the kernel logs from inside an interrupt handler.
    unsafe {
        cpu::disable_interrupts();
    }
    let _ = fmt::write(&mut SerialWriter, arguments);
    write_byte(b'\r');
    write_byte(b'\n');
    if interrupts_were_enabled {
        // SAFETY: the line is complete, and callers that log with interrupts
        // on expect ticks to keep arriving afterwards.
        unsafe {
            cpu::enable_interrupts();
        }
    }
}

/// Logs `message` over COM1 on a line of its own.
///
/// Interrupts stay disabled for the whole call, so a timer tick can never
/// preempt a thread in the middle of a line; the previous interrupt state is
/// restored before returning.
pub fn log(message: &str) {
    let interrupts_were_enabled = cpu::interrupts_enabled();
    // SAFETY: interrupts stay off only for the port writes below, and nothing
    // else in the kernel logs from inside an interrupt handler.
    unsafe {
        cpu::disable_interrupts();
    }
    for byte in message.bytes() {
        write_byte(byte);
    }
    write_byte(b'\r');
    write_byte(b'\n');
    if interrupts_were_enabled {
        // SAFETY: the line is complete, and callers that log with interrupts
        // on expect ticks to keep arriving afterwards.
        unsafe {
            cpu::enable_interrupts();
        }
    }
}

/// `core::fmt::Write` front end so kernel code can log structured reports.
struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, message: &str) -> fmt::Result {
        for byte in message.bytes() {
            write_byte(byte);
        }
        Ok(())
    }
}

fn write_byte(byte: u8) {
    // SAFETY: COM1 is the conventional PC serial port and QEMU exposes it for
    // the kernel's early boot log.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x3f8u16,
            in("al") byte,
            options(nomem, nostack, preserves_flags),
        );
    }
}
