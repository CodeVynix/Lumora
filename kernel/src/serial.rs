//! Serial logging abstraction used by early kernel code.

use core::fmt;

/// Formats `arguments` over COM1 and terminates the line.
pub fn log_args(arguments: fmt::Arguments<'_>) {
    let _ = fmt::write(&mut SerialWriter, arguments);
    write_byte(b'\r');
    write_byte(b'\n');
}

/// Logs `message` over COM1 on a line of its own.
pub fn log(message: &str) {
    for byte in message.bytes() {
        write_byte(byte);
    }
    write_byte(b'\r');
    write_byte(b'\n');
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
