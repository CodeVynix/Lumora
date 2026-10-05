#![no_std]
#![no_main]

use lumora_kernel::{kernel_entry, MemoryRegion};
use uefi::boot::{self, MemoryType};
use uefi::mem::memory_map::MemoryMap;
use uefi::prelude::*;

#[entry]
fn efi_main() -> Status {
    uefi::println!("Lumora bootloader: collecting UEFI memory map");
    // SAFETY: all UEFI console and allocator references are dropped before
    // leaving boot services; the returned map is the final handoff map.
    let map = unsafe { boot::exit_boot_services(MemoryType::LOADER_DATA) };
    let mut memory_map = [MemoryRegion::EMPTY; 128];
    let mut count = 0;
    for descriptor in map.entries() {
        if count == memory_map.len() {
            break;
        }
        memory_map[count] = MemoryRegion {
            start: descriptor.phys_start,
            length: descriptor.page_count.saturating_mul(4096),
            usable: matches!(
                descriptor.ty,
                MemoryType::CONVENTIONAL
                    | MemoryType::BOOT_SERVICES_CODE
                    | MemoryType::BOOT_SERVICES_DATA
            ),
        };
        count += 1;
    }
    lumora_kernel::serial::log("Lumora bootloader: UEFI services exited");
    let memory_map = &memory_map[..count];
    let report = kernel_entry(memory_map);
    if report.heap_test_passed {
        return Status::SUCCESS;
    }
    Status::ABORTED
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
