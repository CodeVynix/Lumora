#![no_std]

pub mod cpu;
pub mod frame;
pub mod gdt;
pub mod heap;
pub mod idt;
pub mod interrupts;
pub mod paging;
pub mod serial;
pub mod thread;
pub mod timer;

/// Pages the kernel heap spans: the boot allocations, plus one 16 KiB thread
/// stack for each of the two test threads.
const HEAP_PAGES: u64 = 9;

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
    let page = tables.map_heap_range(&mut allocator, HEAP_PAGES);
    let mut heap = match page {
        Some(base) => heap::Heap::with_pages(base, HEAP_PAGES),
        None => {
            serial::log("Lumora Kernel: heap mapping FAIL");
            return BootReport::failed();
        }
    };
    let allocation = heap.allocate(64, 8);
    let heap_test_passed = allocation.is_some_and(|address| {
        // SAFETY: the page was mapped writable by `PageTable::map_heap_range`,
        // and the bump allocator returns an address within that mapping.
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

    thread::init();
    thread::register_heap(&mut heap);

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

    timer::init();
    // SAFETY: the IDT is loaded (including the vector 32 timer gate) and the
    // timer is programmed with only IRQ0 unmasked, so every interrupt that
    // can arrive has a handler.
    unsafe {
        cpu::enable_interrupts();
    }

    serial::log("Lumora Kernel: boot complete, idling");
    idle_forever();
}

/// Idles the boot thread: logs timer ticks, runs the thread test once the
/// timer has proven itself, then keeps idling and logging ticks.
fn idle_forever() -> ! {
    let mut logged_seconds = 0u64;
    let mut timer_pass_logged = false;
    let mut threads_spawned = false;
    let mut spawned_threads = 0usize;
    let mut thread_pass_logged = false;
    loop {
        // SAFETY: interrupts are enabled before the kernel reaches this loop,
        // so `hlt` only waits for the next timer tick; it touches no memory.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
        let ticks = timer::ticks();
        let seconds = ticks / timer::TICKS_PER_SECOND;
        while logged_seconds < seconds {
            logged_seconds += 1;
            serial::log_args(format_args!(
                "Lumora Kernel: timer tick {} ({} s)",
                logged_seconds * timer::TICKS_PER_SECOND,
                logged_seconds
            ));
        }
        if !timer_pass_logged && ticks >= 5 * timer::TICKS_PER_SECOND {
            timer_pass_logged = true;
            serial::log("Lumora Kernel: timer test PASS");
        }
        if timer_pass_logged && !threads_spawned {
            threads_spawned = true;
            if let (Some(thread_a_id), Some(thread_b_id)) =
                (thread::spawn(thread_a), thread::spawn(thread_b))
            {
                spawned_threads = 2;
                serial::log_args(format_args!(
                    "Lumora Kernel: spawned threads A and B as ids {} and {}",
                    thread_a_id, thread_b_id
                ));
            } else {
                serial::log("Lumora Kernel: thread spawn FAIL");
            }
        }
        if spawned_threads > 0 {
            thread::yield_now();
            if !thread_pass_logged && thread::finished_threads() == spawned_threads {
                thread_pass_logged = true;
                serial::log("Lumora Kernel: thread test PASS");
            }
        }
    }
}

/// Body of test thread A: ten steps, yielding after each one.
fn thread_a() {
    for step in 1..=10 {
        serial::log_args(format_args!("thread A: step {}", step));
        thread::yield_now();
    }
}

/// Body of test thread B: ten steps, yielding after each one.
fn thread_b() {
    for step in 1..=10 {
        serial::log_args(format_args!("thread B: step {}", step));
        thread::yield_now();
    }
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
