//! Global descriptor table and task state segment used to deliver exceptions.

use crate::cpu::{self, DescriptorTablePointer};
use core::arch::asm;

/// Kernel code selector installed by [`init`].
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
/// Kernel data selector installed by [`init`].
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
/// Task state segment selector installed by [`init`].
pub const TSS_SELECTOR: u16 = 0x18;
/// Ring-3 data selector installed by [`init`]; the low two bits are the
/// requested privilege level the CPU checks on `iretq` into user mode.
pub const USER_DATA_SELECTOR: u16 = 0x2B;
/// Ring-3 code selector installed by [`init`].
pub const USER_CODE_SELECTOR: u16 = 0x33;

/// Slots in the table: null, kernel code, kernel data, the two halves of the
/// TSS, then user data and user code.
const GDT_SLOTS: usize = 7;

/// Length of a task state segment in bytes (Intel SDM Vol. 3, section 7.7).
const TSS_BYTES: usize = 0x68;

/// Size of the stack the CPU switches to when a double fault arrives.
const DOUBLE_FAULT_STACK_BYTES: usize = 16 * 1024;

/// Size of the kernel stack the CPU loads from the TSS when an interrupt,
/// exception, or syscall arrives from ring 3.
const RING0_STACK_BYTES: usize = 16 * 1024;

/// Ring 0, 64-bit code segment: present, flat, execute and read.
const CODE_DESCRIPTOR: u64 = 0x00AF_9A00_0000_FFFF;
/// Ring 0 data segment covering the low 4 GiB.
const DATA_DESCRIPTOR: u64 = 0x00CF_9200_0000_FFFF;
/// Access byte of a present, available 64-bit TSS (Intel SDM Vol. 3, 7.2.3).
const TSS_ACCESS: u64 = 0x89;
/// Ring 3, 64-bit code segment: present, flat, execute and read.
const USER_CODE_DESCRIPTOR: u64 = 0x00AF_FA00_0000_FFFF;
/// Ring 3 data segment covering the low 4 GiB.
const USER_DATA_DESCRIPTOR: u64 = 0x00CF_F200_0000_FFFF;

/// Task state segment. The packed layout reproduces the processor's offsets,
/// so `interrupt_stack_table` really starts at `0x24` where IST1 lives.
#[repr(C, packed)]
struct TaskStateSegment {
    reserved0: u32,
    privilege_stack_table: [u64; 3],
    reserved1: u64,
    interrupt_stack_table: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

/// Alignment wrapper so the table the processor reads is naturally aligned.
#[repr(C, align(16))]
struct AlignedTss(TaskStateSegment);

/// The task state segment loaded by [`init`].
static mut TSS: AlignedTss = AlignedTss(TaskStateSegment {
    reserved0: 0,
    privilege_stack_table: [0; 3],
    reserved1: 0,
    interrupt_stack_table: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: TSS_BYTES as u16,
});

/// The segment table loaded by [`init`]; `ltr` writes its busy bit in place.
static mut GDT: [u64; GDT_SLOTS] = [0; GDT_SLOTS];

/// Stack reserved for double faults; IST1 of the TSS points at its top.
static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_BYTES] = [0; DOUBLE_FAULT_STACK_BYTES];

/// Alignment wrapper so the ring-0 stack top stays 16-byte aligned.
#[repr(C, align(16))]
struct AlignedRing0Stack([u8; RING0_STACK_BYTES]);

/// Kernel stack the CPU loads from TSS RSP0 on entry from ring 3.
static mut RING0_STACK: AlignedRing0Stack = AlignedRing0Stack([0; RING0_STACK_BYTES]);

/// Loads the kernel GDT and task state segment and disables interrupts.
///
/// Call this before [`crate::idt::init`]: the IDT gates reference
/// [`KERNEL_CODE_SELECTOR`], and no maskable interrupt may arrive while the
/// firmware's tables are still the active ones.
pub fn init() {
    // SAFETY: `cli` only clears the interrupt-enable flag, and the exception
    // tests that follow are synchronous, so they are unaffected by it.
    unsafe { cpu::disable_interrupts() };

    let mut interrupt_stack_table = [0u64; 7];
    interrupt_stack_table[0] = double_fault_stack_top();
    let tss = TaskStateSegment {
        reserved0: 0,
        privilege_stack_table: [ring0_stack_top(), 0, 0],
        reserved1: 0,
        interrupt_stack_table,
        reserved2: 0,
        reserved3: 0,
        iomap_base: TSS_BYTES as u16,
    };
    let descriptor = tss_descriptor(core::ptr::addr_of!(TSS) as usize as u64);
    let gdt = [
        0u64,
        CODE_DESCRIPTOR,
        DATA_DESCRIPTOR,
        descriptor[0],
        descriptor[1],
        USER_DATA_DESCRIPTOR,
        USER_CODE_DESCRIPTOR,
    ];

    // SAFETY: `TSS` and `GDT` are statics written only here, during
    // single-threaded boot, before `lgdt` or `ltr` reads them.
    unsafe {
        core::ptr::write(core::ptr::addr_of_mut!(TSS), AlignedTss(tss));
        core::ptr::copy_nonoverlapping(
            gdt.as_ptr(),
            core::ptr::addr_of_mut!(GDT).cast::<u64>(),
            gdt.len(),
        );
    }

    let pointer = DescriptorTablePointer {
        limit: (GDT_SLOTS * core::mem::size_of::<u64>() - 1) as u16,
        base: core::ptr::addr_of!(GDT) as usize as u64,
    };
    // SAFETY: `pointer` describes the table written above, and `GDT` is a
    // static that stays mapped for the lifetime of the kernel.
    unsafe { cpu::load_gdt(&pointer) };

    reload_segment_selectors();

    // SAFETY: the TSS descriptor now inside the active GDT points at the
    // fully initialized static TSS, which is exactly what `ltr` requires.
    unsafe { cpu::load_task_register(TSS_SELECTOR) };
}

/// Retargets CS at the kernel code descriptor and DS, ES, and SS at the flat
/// data descriptor now that the new table is active.
fn reload_segment_selectors() {
    // SAFETY: the new GDT is loaded and both descriptors are present, flat,
    // and writable, so the far return only changes which code selector CS
    // caches; the pushed stack words are consumed by `retfq` itself.
    unsafe {
        asm!(
            "push {selector}",
            "lea {target}, [rip + 9f]",
            "push {target}",
            "retfq",
            "9:",
            selector = in(reg) u64::from(KERNEL_CODE_SELECTOR),
            target = lateout(reg) _,
        );
    }
    // SAFETY: the data descriptor loaded above covers the whole low 4 GiB, so
    // DS, ES, and SS keep describing the memory and the boot stack in use.
    unsafe {
        asm!(
            "mov ax, cx",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            in("rcx") u64::from(KERNEL_DATA_SELECTOR),
            out("rax") _,
        );
    }
}

/// Exclusive end of the double-fault stack, which is what the TSS stores.
fn double_fault_stack_top() -> u64 {
    core::ptr::addr_of!(DOUBLE_FAULT_STACK) as usize as u64 + DOUBLE_FAULT_STACK_BYTES as u64
}

/// Exclusive end of the ring-0 stack the TSS hands to ring-3 entries.
fn ring0_stack_top() -> u64 {
    core::ptr::addr_of!(RING0_STACK) as *const u8 as u64 + RING0_STACK_BYTES as u64
}

/// Builds the 16-byte TSS descriptor for a segment starting at `base`
/// (Intel SDM Vol. 3, figure 8-11).
fn tss_descriptor(base: u64) -> [u64; 2] {
    let limit = (TSS_BYTES - 1) as u64;
    let low = (limit & 0xFFFF)
        | ((base & 0x0000_FFFF) << 16)
        | ((base & 0x00FF_0000) << 16)
        | (TSS_ACCESS << 40)
        | ((base & 0xFF00_0000) << 32);
    [low, base >> 32]
}
