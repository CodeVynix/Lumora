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
pub mod usermode;

/// Pages the kernel heap spans: the boot allocations, plus one 16 KiB thread
/// stack for each of the two test threads.
const HEAP_PAGES: u64 = 9;

/// Steps each preemption-test thread logs before it finishes.
const PREEMPTION_TEST_STEPS: u32 = 5;

/// Ticks a test thread waits between two of its log lines.
const PREEMPTION_TEST_PERIOD: u64 = 20;

/// Preemption count that proves the timer switched threads.
const PREEMPTION_TEST_MINIMUM: u64 = 10;

/// Bytes in the ring-3 stack used by the user-mode test payloads.
const USER_STACK_BYTES: usize = 16 * 1024;

/// Message the first user-mode payload asks the kernel to write to serial.
static USER_HELLO: &[u8] = b"hello from ring 3";

/// Alignment wrapper so the user stack top stays 16-byte aligned.
#[repr(C, align(16))]
struct AlignedUserStack([u8; USER_STACK_BYTES]);

/// Stack the kernel hands to the ring-3 test payloads on entry.
static mut USER_STACK: AlignedUserStack = AlignedUserStack([0; USER_STACK_BYTES]);

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

    // Ring 3 can only run from user-accessible pages; the low identity map
    // covers the image the payloads live in.
    tables.make_identity_user_accessible();

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

/// Idles the boot thread: logs timer ticks, runs the thread tests once the
/// timer has proven itself, then the user-mode test, then keeps idling.
fn idle_forever() -> ! {
    let mut logged_seconds = 0u64;
    let mut timer_pass_logged = false;
    let mut threads_spawned = false;
    let mut spawned_threads = 0usize;
    let mut preemption_logged = false;
    let mut user_test_done = false;
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
            if !preemption_logged && thread::finished_threads() == spawned_threads {
                preemption_logged = true;
                let preemptions = timer::preemptions();
                serial::log_args(format_args!(
                    "Lumora Kernel: preemption count {}",
                    preemptions
                ));
                if preemptions >= PREEMPTION_TEST_MINIMUM {
                    serial::log("Lumora Kernel: preemption test PASS");
                }
            }
            if preemption_logged && !user_test_done {
                user_test_done = true;
                run_user_mode_test();
            }
        }
    }
}

/// Runs the two ring-3 payloads and logs `user mode test PASS` only when the
/// first exits cleanly and the second dies by general protection fault.
fn run_user_mode_test() {
    serial::log("Lumora Kernel: user mode test starting");
    // SAFETY: the user segments are loaded, TSS RSP0 points at the ring-0
    // stack, and the payload plus the user stack sit in user-accessible
    // pages; no other user task can run while this one does.
    let write_code = unsafe {
        usermode::enter_user(
            user_payload_write as *const () as usize as u64,
            user_stack_top(),
            usermode::user_resume_slot(),
        )
    };
    // SAFETY: the syscall exit path abandons its interrupt frame without
    // restoring flags, so interrupts stay off until re-enabled here.
    unsafe {
        cpu::enable_interrupts();
    }
    // SAFETY: same ring-3 preconditions as the first payload.
    let privileged_code = unsafe {
        usermode::enter_user(
            user_payload_privileged as *const () as usize as u64,
            user_stack_top(),
            usermode::user_resume_slot(),
        )
    };
    // SAFETY: the fault path abandons its interrupt frame the same way the
    // syscall exit does.
    unsafe {
        cpu::enable_interrupts();
    }
    if write_code == usermode::USER_EXIT_OK && privileged_code == usermode::USER_EXIT_GP {
        serial::log("Lumora Kernel: user mode test PASS");
    } else {
        serial::log("Lumora Kernel: user mode test FAIL");
    }
}

/// Exclusive end of the ring-3 stack handed to the user-mode payloads.
fn user_stack_top() -> u64 {
    core::ptr::addr_of!(USER_STACK) as *const u8 as u64 + USER_STACK_BYTES as u64
}

/// Ring-3 test payload: writes one message with a syscall, then exits.
fn user_payload_write() {
    let message = USER_HELLO;
    // SAFETY: `int 0x80` is the installed syscall gate, which is callable
    // from ring 3; the pointer and length name the static message above.
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") usermode::SYS_WRITE_SERIAL,
            in("rdi") message.as_ptr() as u64,
            in("rsi") message.len() as u64,
            lateout("rax") _,
        );
        core::arch::asm!(
            "int 0x80",
            in("rax") usermode::SYS_EXIT,
            in("rdi") usermode::USER_EXIT_OK,
            options(noreturn),
        );
    }
}

/// Ring-3 test payload: executes a privileged instruction so the kernel
/// reports the fault and kills the task instead of halting.
fn user_payload_privileged() {
    // SAFETY: `hlt` is privileged at CPL 3 with IOPL 0, so it raises exactly
    // the general protection fault this test expects; it never completes.
    unsafe {
        core::arch::asm!("hlt", options(noreturn));
    }
}

/// Body of test thread A: busy-loops on the tick counter and logs every
/// twenty ticks. It never yields; only the timer preempts it.
fn thread_a() {
    let mut last_logged = timer::ticks();
    let mut steps = 0u32;
    while steps < PREEMPTION_TEST_STEPS {
        let now = timer::ticks();
        if now - last_logged >= PREEMPTION_TEST_PERIOD {
            last_logged = now;
            steps += 1;
            serial::log_args(format_args!("thread A: tick {}", now));
        }
    }
}

/// Body of test thread B: busy-loops on the tick counter and logs every
/// twenty ticks. It never yields; only the timer preempts it.
fn thread_b() {
    let mut last_logged = timer::ticks();
    let mut steps = 0u32;
    while steps < PREEMPTION_TEST_STEPS {
        let now = timer::ticks();
        if now - last_logged >= PREEMPTION_TEST_PERIOD {
            last_logged = now;
            steps += 1;
            serial::log_args(format_args!("thread B: tick {}", now));
        }
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
