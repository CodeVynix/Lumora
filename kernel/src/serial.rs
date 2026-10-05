//! Serial logging abstraction used by early kernel code.

pub fn log(message: &str) {
    for byte in message.bytes() {
        // SAFETY: COM1 is the conventional PC serial port and QEMU exposes it
        // for the kernel's early boot log.
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0x3f8u16,
                in("al") byte,
                options(nomem, nostack, preserves_flags),
            );
        }
    }
    log_byte(b'\r');
    log_byte(b'\n');
}

fn log_byte(byte: u8) {
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
