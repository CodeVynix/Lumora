//! System timer: legacy PIC remap, PIT channel 0, and the tick counter.
//!
//! The two 8259 PICs are remapped so hardware IRQs arrive on vectors 32-47
//! instead of colliding with CPU exceptions, and every IRQ except the timer
//! (IRQ0) is masked. PIT channel 0 runs as a rate generator at 100 Hz, and
//! each IRQ0 increments an atomic tick counter. A local APIC timer will
//! replace this hardware later; the counter API stays the same.

use core::sync::atomic::{AtomicU64, Ordering};

/// PIT input clock in hertz (standard PC value).
const PIT_INPUT_HZ: u32 = 1_193_182;
/// Timer interrupts per second.
const TIMER_HZ: u32 = 100;
/// Ticks per logged second.
pub const TICKS_PER_SECOND: u64 = TIMER_HZ as u64;
/// PIT divisor for 100 Hz (1193182 / 100, truncated).
const PIT_DIVISOR: u16 = (PIT_INPUT_HZ / TIMER_HZ) as u16;

/// 8259 master PIC command port.
const PIC1_COMMAND: u16 = 0x20;
/// 8259 master PIC data port.
const PIC1_DATA: u16 = 0x21;
/// 8259 slave PIC command port.
const PIC2_COMMAND: u16 = 0xA0;
/// 8259 slave PIC data port.
const PIC2_DATA: u16 = 0xA1;
/// 8254 PIT channel 0 data port.
const PIT_CHANNEL0: u16 = 0x40;
/// 8254 PIT command port.
const PIT_COMMAND: u16 = 0x43;

/// Ticks elapsed since the timer was programmed.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Remaps the PICs, masks everything but IRQ0, and starts the 100 Hz timer.
///
/// Call this after the IDT is loaded and before enabling interrupts, so the
/// first tick already has a gate to travel through.
pub fn init() {
    remap_pic();
    program_pit();
}

/// Records one timer tick and acknowledges IRQ0.
///
/// This runs inside the timer interrupt handler, so it only touches an atomic
/// counter and the PIC command port: it never prints and never takes a lock.
pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    send_eoi();
}

/// Ticks elapsed since [`init`] programmed the timer.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Remaps IRQs 0-15 to vectors 32-47 and masks every IRQ except IRQ0.
///
/// The four initialization command words are the standard 8259 sequence:
/// start init and expect ICW4, set the vector offsets, wire the
/// master/slave cascade, then select 8086 mode.
fn remap_pic() {
    outb(PIC1_COMMAND, 0x11);
    outb(PIC2_COMMAND, 0x11);
    outb(PIC1_DATA, 0x20);
    outb(PIC2_DATA, 0x28);
    outb(PIC1_DATA, 0x04);
    outb(PIC2_DATA, 0x02);
    outb(PIC1_DATA, 0x01);
    outb(PIC2_DATA, 0x01);
    outb(PIC1_DATA, 0xFE);
    outb(PIC2_DATA, 0xFF);
}

/// Programs PIT channel 0 as a rate generator (mode 2) for 100 Hz.
///
/// Command byte 0x34 means channel 0, lobyte/hibyte access, mode 2, binary.
fn program_pit() {
    outb(PIT_COMMAND, 0x34);
    outb(PIT_CHANNEL0, (PIT_DIVISOR & 0xFF) as u8);
    outb(PIT_CHANNEL0, (PIT_DIVISOR >> 8) as u8);
}

/// Sends end-of-interrupt for IRQ0, which lives on the master PIC alone.
fn send_eoi() {
    outb(PIC1_COMMAND, 0x20);
}

/// Writes `value` to I/O port `port`.
fn outb(port: u16, value: u8) {
    // SAFETY: `out` to the PIC and PIT ports named above is the documented
    // way to program them, and this kernel is the only driver touching them.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}
