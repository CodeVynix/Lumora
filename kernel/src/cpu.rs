//! Low-level x86_64 instructions that install descriptor tables and park the
//! CPU, shared by the interrupt subsystem.

use core::arch::asm;

/// Operand block for `lgdt` and `lidt` (Intel SDM Vol. 3, sections 2.4.1 and
/// 6.10).
#[repr(C, packed)]
pub struct DescriptorTablePointer {
    /// Largest valid byte offset inside the table.
    pub limit: u16,
    /// Address of the table's first byte.
    pub base: u64,
}

/// Loads `pointer` as the current global descriptor table.
///
/// # Safety
///
/// `pointer` must describe a GDT that stays mapped for as long as it is
/// loaded, and every selector used afterwards must resolve inside it.
pub unsafe fn load_gdt(pointer: &DescriptorTablePointer) {
    // SAFETY: the caller guarantees the table stays mapped and valid; `lgdt`
    // only reads the ten bytes at `pointer`.
    unsafe {
        asm!(
            "lgdt [{}]",
            in(reg) pointer,
            options(readonly, nostack, preserves_flags)
        );
    }
}

/// Loads `pointer` as the current interrupt descriptor table.
///
/// # Safety
///
/// `pointer` must describe an IDT that stays mapped for as long as it is
/// loaded, and every gate it exposes must point at a valid entry stub.
pub unsafe fn load_idt(pointer: &DescriptorTablePointer) {
    // SAFETY: the caller guarantees the table stays mapped and valid; `lidt`
    // only reads the ten bytes at `pointer`.
    unsafe {
        asm!(
            "lidt [{}]",
            in(reg) pointer,
            options(readonly, nostack, preserves_flags)
        );
    }
}

/// Marks `selector` as the active task state segment.
///
/// # Safety
///
/// `selector` must reference a present, available 64-bit TSS descriptor in
/// the currently loaded GDT, because `ltr` writes that descriptor's busy bit.
pub unsafe fn load_task_register(selector: u16) {
    // SAFETY: the documented precondition is checked by the caller; `ltr`
    // reads the selector register and writes one GDT byte.
    unsafe {
        asm!("ltr {0:x}", in(reg) selector, options(nostack, preserves_flags));
    }
}

/// Disables maskable interrupts so no IRQ can arrive while the tables that
/// replace the firmware's are only half built.
///
/// # Safety
///
/// Interrupts stay disabled until something enables them again; the caller
/// must not depend on periodic interrupts afterwards.
pub unsafe fn disable_interrupts() {
    // SAFETY: `cli` only clears the interrupt-enable flag, which is a defined
    // processor state change and touches no memory.
    unsafe {
        asm!("cli", options(nomem, nostack, preserves_flags));
    }
}

/// Returns true when maskable interrupts are currently enabled.
///
/// The context switch reads this so it can leave the interrupt flag exactly
/// as it found it once the other thread is running.
pub fn interrupts_enabled() -> bool {
    let flags: u64;
    // SAFETY: `pushfq` and `pop` only round-trip RFLAGS through `flags`; they
    // touch no memory and leave the flags unchanged.
    unsafe {
        asm!(
            "pushfq",
            "pop {flags}",
            flags = out(reg) flags,
            options(nomem, preserves_flags),
        );
    }
    // RFLAGS.IF is bit 9 (Intel SDM Vol. 1, section 3.4.3).
    flags & (1 << 9) != 0
}

/// Enables maskable interrupts once the IDT and the timer are ready.
///
/// # Safety
///
/// Every unmasked IRQ must already have a working IDT gate; otherwise the
/// first tick arrives with nowhere to go and triple-faults the machine.
pub unsafe fn enable_interrupts() {
    // SAFETY: the caller guarantees the IDT covers every unmasked vector;
    // `sti` only sets the interrupt-enable flag.
    unsafe {
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

/// Disables interrupts and halts the CPU until the machine is reset.
pub fn halt_forever() -> ! {
    // SAFETY: `cli` and `hlt` only change CPU state, and no memory is touched
    // because the loop never leaves this function.
    unsafe {
        loop {
            asm!("cli; hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
