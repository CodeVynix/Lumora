//! Interrupt descriptor table holding the CPU exception gates.

use crate::cpu::{self, DescriptorTablePointer};
use crate::gdt::KERNEL_CODE_SELECTOR;
use crate::interrupts::{
    breakpoint_stub, divide_error_stub, double_fault_stub, general_protection_stub,
    page_fault_stub, timer_stub, VECTOR_BREAKPOINT, VECTOR_DIVIDE_ERROR, VECTOR_DOUBLE_FAULT,
    VECTOR_GENERAL_PROTECTION, VECTOR_PAGE_FAULT, VECTOR_TIMER,
};

/// Vectors the x86_64 architecture reserves in the table.
const GATE_COUNT: usize = 256;

/// Access byte of a present interrupt gate at ring 0 (Intel SDM Vol. 3, 6.11).
const INTERRUPT_GATE: u64 = 0x8E;

/// Interrupt stack table slot reserved for double faults (Intel SDM Vol. 3, 7.7).
const DOUBLE_FAULT_IST: u64 = 1;

/// The exceptions and timer interrupt Build 1 routes: vector, entry stub, and
/// IST slot.
const GATES: [(u64, extern "C" fn(), u64); 6] = [
    (VECTOR_DIVIDE_ERROR, divide_error_stub, 0),
    (VECTOR_BREAKPOINT, breakpoint_stub, 0),
    (VECTOR_DOUBLE_FAULT, double_fault_stub, DOUBLE_FAULT_IST),
    (VECTOR_GENERAL_PROTECTION, general_protection_stub, 0),
    (VECTOR_PAGE_FAULT, page_fault_stub, 0),
    (VECTOR_TIMER, timer_stub, 0),
];

/// The table loaded by [`init`]; untouched vectors stay not present.
static mut IDT: [[u64; 2]; GATE_COUNT] = [[0; 2]; GATE_COUNT];

/// Installs the exception and timer gates and loads the table.
///
/// Call this after [`crate::gdt::init`], because the gates name that table's
/// kernel code selector.
pub fn init() {
    for (vector, handler, interrupt_stack_table) in GATES {
        let gate = gate_descriptor(handler as usize as u64, interrupt_stack_table);
        // SAFETY: `IDT` is written only here, during single-threaded boot,
        // and nothing reads it until `load_idt` below.
        unsafe {
            (*core::ptr::addr_of_mut!(IDT))[vector as usize] = gate;
        }
    }

    let pointer = DescriptorTablePointer {
        limit: (GATE_COUNT * core::mem::size_of::<[u64; 2]>() - 1) as u16,
        base: core::ptr::addr_of!(IDT) as usize as u64,
    };
    // SAFETY: `pointer` describes the table written above, and `IDT` is a
    // static that stays mapped for the lifetime of the kernel.
    unsafe { cpu::load_idt(&pointer) };
}

/// Builds a 16-byte interrupt gate for `offset` (Intel SDM Vol. 3, figure 6-8).
fn gate_descriptor(offset: u64, interrupt_stack_table: u64) -> [u64; 2] {
    let low = (offset & 0xFFFF)
        | (u64::from(KERNEL_CODE_SELECTOR) << 16)
        | ((interrupt_stack_table & 0x7) << 32)
        | (INTERRUPT_GATE << 40)
        | ((offset & 0xFFFF_0000) << 32);
    [low, offset >> 32]
}
