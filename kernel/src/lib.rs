#![no_std]

pub mod cpu;
pub mod frame;
pub mod gdt;
pub mod heap;
pub mod idt;
pub mod interrupts;
pub mod paging;
pub mod serial;

/// Kernel entry called by the bootloader after the UEFI handoff.
pub fn kernel_entry(memory_map: &[MemoryRegion]) -> BootReport {
    serial::log("Lumora Kernel: entry");
    let mut allocator = frame::FrameAllocator::new(memory_map);
    serial::log("Lumora Kernel: frame allocator ready");
    let mut tables = match paging::PageTable::new(&mut allocator) {
        Some(tables) => tables,
        None => {
            serial::log("Lumora Kernel: page-table allocation FAIL");
            return BootReport::failed();
        }
    };
    let page = tables.map_heap_page(&mut allocator);
    let mut heap = heap::Heap::new();
    let allocation = page.and_then(|_| heap.allocate(64, 8));
    let heap_test_passed = allocation.is_some_and(|address| {
        // SAFETY: the page was mapped writable by `PageTable::map_heap_page`,
        // and the bump allocator returns an address within that page.
        unsafe {
            let pointer = address as *mut u64;
            pointer.write(0x4c554d4f_52415f31);
            pointer.read() == 0x4c554d4f_52415f31
        }
    });
    serial::log("Lumora Kernel: memory map handoff received");
    serial::log("Lumora Kernel: kernel page tables active");
    serial::log(if heap_test_passed {
        "Lumora Kernel: heap test PASS"
    } else {
        "Lumora Kernel: heap test FAIL"
    });

    gdt::init();
    idt::init();
    serial::log("Lumora Kernel: GDT, TSS, and IDT loaded");

    serial::log("Lumora Kernel: breakpoint test starting");
    interrupts::test_breakpoint();
    serial::log("Lumora Kernel: breakpoint test PASS - execution continued");

    #[cfg(feature = "exception-test")]
    {
        serial::log("Lumora Kernel: divide-by-zero test starting");
        interrupts::test_divide_error();
        serial::log("Lumora Kernel: divide-by-zero test FAIL - divide error returned");
    }

    serial::log("Lumora Kernel: boot complete, idling");
    cpu::halt_forever()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: u64,
    pub length: u64,
    pub usable: bool,
}

impl MemoryRegion {
    pub const EMPTY: Self = Self {
        start: 0,
        length: 0,
        usable: false,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootReport {
    pub page: Option<u64>,
    pub allocation: Option<u64>,
    pub free_frames: usize,
    pub heap_test_passed: bool,
}

impl BootReport {
    const fn failed() -> Self {
        Self {
            page: None,
            allocation: None,
            free_frames: 0,
            heap_test_passed: false,
        }
    }
}
