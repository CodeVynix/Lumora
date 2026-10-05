#![no_std]
#![no_main]

use lumora_kernel::{kernel_entry, MemoryRegion};
use uefi::prelude::*;

#[entry]
fn efi_main() -> Status {
    uefi::println!("Lumora bootloader: entry");
    uefi::println!("Lumora bootloader: UEFI services ready");
    uefi::println!("Lumora bootloader: serial logger ready");
    let memory_map = [
        MemoryRegion {
            start: 0x0010_0000,
            length: 0x0100_0000,
            usable: true,
        },
        MemoryRegion {
            start: 0x0110_0000,
            length: 0x0010_0000,
            usable: false,
        },
    ];
    let report = kernel_entry(&memory_map);
    uefi::println!("Lumora Kernel: entry");
    if report.allocation.is_some() && report.page.is_some() {
        uefi::println!("Lumora Kernel: heap allocation test passed");
        return Status::SUCCESS;
    }
    uefi::println!("Lumora Kernel: heap allocation test failed");
    Status::ABORTED
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
