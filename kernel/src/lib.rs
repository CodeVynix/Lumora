#![no_std]

pub mod cpu;
pub mod elf;
pub mod frame;
pub mod gdt;
pub mod heap;
pub mod idt;
pub mod interrupts;
pub mod paging;
pub mod pci;
pub mod process;
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

/// Bytes of the hello user program built by xtask for the ELF loader test.
static HELLO_ELF: &[u8] = include_bytes!("../../target/x86_64-unknown-none/debug/hello");

/// Scratch space for building hostile ELF images, reused for every case.
static mut ELF_SCRATCH: [u8; 16384] = [0; 16384];

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

    // Record the kernel tables while they are loaded; every ring-3 exit
    // restores them.
    usermode::init(paging::current_root());

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
    idle_forever(&mut allocator);
}

/// Idles the boot thread: logs timer ticks, runs the thread tests once the
/// timer has proven itself, then the user-mode test, then the process test,
/// then keeps idling.
fn idle_forever(frames: &mut frame::FrameAllocator<'_>) -> ! {
    let mut logged_seconds = 0u64;
    let mut timer_pass_logged = false;
    let mut threads_spawned = false;
    let mut spawned_threads = 0usize;
    let mut preemption_logged = false;
    let mut user_test_done = false;
    let mut process_test_done = false;
    let mut elf_test_done = false;
    let mut pci_test_done = false;
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
            if user_test_done && !process_test_done {
                process_test_done = true;
                run_process_test(frames);
            }
            if process_test_done && !elf_test_done {
                elf_test_done = true;
                run_elf_test(frames);
            }
            if elf_test_done && !pci_test_done {
                pci_test_done = true;
                run_pci_test();
            }
        }
    }
}

/// Enumerates PCI and logs PASS only if a virtio-blk device is found.
fn run_pci_test() {
    serial::log("Lumora Kernel: PCI test starting");
    pci::enumerate();
    if pci::virtio_blk_location().is_some() {
        serial::log("Lumora Kernel: PCI test PASS");
    } else {
        serial::log("Lumora Kernel: PCI test FAIL");
    }
}

/// Runs the three ring-3 payloads and logs `user mode test PASS` only when the
/// first exits cleanly, the second dies by general protection fault, and the
/// third sees every hostile pointer rejected.
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
    let mut payload1_ok = false;
    if write_code == usermode::USER_EXIT_OK {
        payload1_ok = true;
        serial::log("Lumora Kernel: user payload 1 exited cleanly");
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
    let payload2_killed = privileged_code == usermode::USER_EXIT_GP;
    // SAFETY: same ring-3 preconditions as the first payload.
    let hostile_code = unsafe {
        usermode::enter_user(
            user_payload_hostile as *const () as usize as u64,
            user_stack_top(),
            usermode::user_resume_slot(),
        )
    };
    // SAFETY: the syscall exit path abandons its interrupt frame without
    // restoring flags, so interrupts stay off until re-enabled here.
    unsafe {
        cpu::enable_interrupts();
    }
    let payload3_ok = hostile_code == usermode::USER_EXIT_OK;
    if payload1_ok && payload2_killed && payload3_ok {
        serial::log("Lumora Kernel: user mode test PASS");
    } else {
        serial::log("Lumora Kernel: user mode test FAIL");
    }
}

/// Runs the per-process isolation test: two payloads share one user virtual
/// address in private address spaces, then a third probes kernel memory.
fn run_process_test(frames: &mut frame::FrameAllocator<'_>) {
    serial::log("Lumora Kernel: process test starting");
    let mut isolation_a_ok = false;
    let mut isolation_b_ok = false;
    if let Some(a) = process::spawn(payload_iso_a_bytes(), frames) {
        if process::enter(a, frames) == 0 {
            isolation_a_ok = true;
            serial::log("Lumora Kernel: isolation A ok");
        }
    }
    if let Some(b) = process::spawn(payload_iso_b_bytes(), frames) {
        if process::enter(b, frames) == 0 {
            isolation_b_ok = true;
            serial::log("Lumora Kernel: isolation B ok");
        }
    }
    let mut protected = false;
    if let Some(k) = process::spawn(payload_kernel_read_bytes(), frames) {
        if process::enter(k, frames) == usermode::USER_EXIT_PF {
            protected = true;
            serial::log("Lumora Kernel: kernel memory protected");
        }
    }
    if isolation_a_ok && isolation_b_ok && protected {
        serial::log("Lumora Kernel: process isolation test PASS");
    } else {
        serial::log("Lumora Kernel: process isolation test FAIL");
    }
}

/// Runs the ELF loader test: a real program from ELF bytes, ten hostile
/// images that must all be rejected, and a frame-leak check around all of it.
fn run_elf_test(frames: &mut frame::FrameAllocator<'_>) {
    serial::log("Lumora Kernel: ELF loader test starting");
    let before = frames.free_frames();
    let mut good_ok = false;
    match elf::validate(HELLO_ELF) {
        Ok(image) => {
            if let Some(id) = process::spawn_elf(&image, HELLO_ELF, frames) {
                let code = process::enter_elf(id, frames);
                log_process_outcome(id, code);
                good_ok = code == 0;
            }
        }
        Err(error) => {
            serial::log_args(format_args!(
                "Lumora Kernel: hostile ELF rejected: {}",
                error.reason()
            ));
        }
    }
    let mut hostile_ok = 0u32;
    // SAFETY: the scratch buffer is only touched here, one case at a time,
    // and every case below bounds its copy by both lengths first.
    let scratch = unsafe { &mut *core::ptr::addr_of_mut!(ELF_SCRATCH) };
    if hostile_case(scratch, HELLO_ELF, "bad magic", HELLO_ELF.len(), |bytes| {
        bytes[0] = 0x00;
    }) {
        hostile_ok += 1;
    }
    if hostile_case(scratch, HELLO_ELF, "truncated file", 32, |_| {}) {
        hostile_ok += 1;
    }
    if hostile_case(
        scratch,
        HELLO_ELF,
        "program header table past end of file",
        HELLO_ELF.len(),
        |bytes| {
            write_u64(bytes, 32, HELLO_ELF.len() as u64 + 0x100);
        },
    ) {
        hostile_ok += 1;
    }
    if hostile_case(
        scratch,
        HELLO_ELF,
        "segment past end of file",
        HELLO_ELF.len(),
        |bytes| {
            let table = read_u64(HELLO_ELF, 32) as usize;
            write_u64(bytes, table + 8, HELLO_ELF.len() as u64);
        },
    ) {
        hostile_ok += 1;
    }
    if hostile_case(
        scratch,
        HELLO_ELF,
        "filesz exceeds memsz",
        HELLO_ELF.len(),
        |bytes| {
            let table = read_u64(HELLO_ELF, 32) as usize;
            let memsz = read_u64(HELLO_ELF, table + 40);
            write_u64(bytes, table + 32, memsz + 1);
        },
    ) {
        hostile_ok += 1;
    }
    if hostile_case(
        scratch,
        HELLO_ELF,
        "segment outside user range",
        HELLO_ELF.len(),
        |bytes| {
            let table = read_u64(HELLO_ELF, 32) as usize;
            write_u64(bytes, table + 16, 0xffff_8000_4000_0000);
        },
    ) {
        hostile_ok += 1;
    }
    if hostile_case(
        scratch,
        HELLO_ELF,
        "address range overflow",
        HELLO_ELF.len(),
        |bytes| {
            let table = read_u64(HELLO_ELF, 32) as usize;
            write_u64(bytes, table + 16, u64::MAX - 0xFFF);
            write_u64(bytes, table + 40, 0x2000);
        },
    ) {
        hostile_ok += 1;
    }
    if hostile_case(
        scratch,
        HELLO_ELF,
        "writable and executable segment",
        HELLO_ELF.len(),
        |bytes| {
            let table = read_u64(HELLO_ELF, 32) as usize;
            write_u64(bytes, table + 4, 7);
        },
    ) {
        hostile_ok += 1;
    }
    if hostile_case(
        scratch,
        HELLO_ELF,
        "entry point outside any segment",
        HELLO_ELF.len(),
        |bytes| {
            write_u64(bytes, 24, 0x80_1000_0000);
        },
    ) {
        hostile_ok += 1;
    }
    if hostile_too_many(scratch) {
        hostile_ok += 1;
    }
    let after = frames.free_frames();
    serial::log_args(format_args!(
        "Lumora Kernel: frames before {} after {}",
        before, after
    ));
    if good_ok && hostile_ok == 10 && before == after {
        serial::log("Lumora Kernel: ELF loader test PASS");
    } else {
        serial::log("Lumora Kernel: ELF loader test FAIL");
    }
}

/// Builds an image with nine loadable headers by splicing six extra ones
/// after the hello program's own table, then checks it is rejected for
/// having too many segments. The count check fires before any segment is
/// inspected, so the filler headers only need the loadable type.
fn hostile_too_many(scratch: &mut [u8]) -> bool {
    let table = read_u64(HELLO_ELF, 32) as usize;
    let count = read_u16(HELLO_ELF, 56) as usize;
    // One more loadable header than the loader accepts.
    const EXTRA_HEADERS: usize = elf::MAX_LOAD_SEGMENTS - 2 + 1;
    const HEADER_SIZE: usize = 56;
    let total = HELLO_ELF.len();
    let table_end = table + count * HEADER_SIZE;
    let extra_end = table_end + EXTRA_HEADERS * HEADER_SIZE;
    // The program header table sits in the padding ahead of the first segment,
    // so the extra headers fit without moving any file data or real headers.
    if total > scratch.len() || table_end > total || extra_end > total {
        return false;
    }
    scratch[..total].copy_from_slice(HELLO_ELF);
    for extra in 0..EXTRA_HEADERS {
        let entry = table_end + extra * HEADER_SIZE;
        scratch[entry..entry + HEADER_SIZE].fill(0);
        write_u32(scratch, entry, 1);
    }
    write_u16(&mut scratch[..], 56, (count + EXTRA_HEADERS) as u16);
    match elf::validate(&scratch[..total]) {
        Err(error) if error.reason() == "too many segments" => {
            serial::log_args(format_args!(
                "Lumora Kernel: hostile ELF rejected: {}",
                error.reason()
            ));
            true
        }
        Err(error) => {
            serial::log_args(format_args!(
                "Lumora Kernel: hostile ELF rejected with wrong reason: {}",
                error.reason()
            ));
            false
        }
        Ok(_) => false,
    }
}

/// Logs how process `id` ended: a clean exit code or the fatal signal.
fn log_process_outcome(id: usize, code: u64) {
    match code {
        usermode::USER_EXIT_GP => {
            serial::log_args(format_args!(
                "Lumora Kernel: process {} killed: general protection fault",
                id
            ));
        }
        usermode::USER_EXIT_PF => {
            serial::log_args(format_args!(
                "Lumora Kernel: process {} killed: page fault",
                id
            ));
        }
        _ => {
            serial::log_args(format_args!(
                "Lumora Kernel: process {} exited with code {}",
                id, code
            ));
        }
    }
}

/// Reads a little-endian `u16` at `offset`; the caller bounds-checks first.
fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    let mut pair = [0u8; 2];
    pair.copy_from_slice(&bytes[offset..offset + 2]);
    u16::from_le_bytes(pair)
}

/// Reads a little-endian `u64` at `offset`; the caller bounds-checks first.
fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut oct = [0u8; 8];
    oct.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(oct)
}

/// Writes `value` little-endian at `offset`; the caller bounds-checks first.
fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// Writes `value` little-endian at `offset`; the caller bounds-checks first.
fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Writes `value` little-endian at `offset`; the caller bounds-checks first.
fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

/// Copies `len` bytes of `hello` into scratch, applies `patch`, and checks
/// validation fails with `expected`, logging the rejection.
fn hostile_case(
    scratch: &mut [u8],
    hello: &[u8],
    expected: &str,
    len: usize,
    patch: impl FnOnce(&mut [u8]),
) -> bool {
    let len = len.min(scratch.len()).min(hello.len());
    scratch[..len].copy_from_slice(&hello[..len]);
    patch(&mut scratch[..len]);
    match elf::validate(&scratch[..len]) {
        Err(error) if error.reason() == expected => {
            serial::log_args(format_args!(
                "Lumora Kernel: hostile ELF rejected: {}",
                error.reason()
            ));
            true
        }
        Err(error) => {
            serial::log_args(format_args!(
                "Lumora Kernel: hostile ELF rejected with wrong reason: {}",
                error.reason()
            ));
            false
        }
        Ok(_) => false,
    }
}

/// Bytes of the isolation payloads below, delimited by their start/end
/// labels. Every payload is position-independent: absolute addresses travel
/// in registers via `movabs`, and all jumps stay inside the payload.
macro_rules! payload_bytes {
    ($start:ident, $end:ident, $bytes:ident) => {
        unsafe extern "C" {
            static $start: u8;
            static $end: u8;
        }

        fn $bytes() -> &'static [u8] {
            // SAFETY: the labels delimit read-only image memory emitted by
            // the matching `global_asm!` block just above.
            unsafe {
                let start = &$start as *const u8;
                let end = &$end as *const u8;
                core::slice::from_raw_parts(start, end as usize - start as usize)
            }
        }
    };
}

core::arch::global_asm!(
    ".global payload_iso_a_start",
    "payload_iso_a_start:",
    "movabs rdi, {x}",
    "movabs rax, 0xAAAAAAAAAAAAAAAA",
    "mov [rdi], rax",
    "mov rax, [rdi]",
    "movabs rdx, 0xAAAAAAAAAAAAAAAA",
    "cmp rax, rdx",
    "jne 1f",
    "mov rax, {exit}",
    "mov rdi, 0",
    "int 0x80",
    "1:",
    "mov rax, {exit}",
    "mov rdi, 1",
    "int 0x80",
    ".global payload_iso_a_end",
    "payload_iso_a_end:",
    x = const process::USER_DATA_VA,
    exit = const usermode::SYS_EXIT,
);
payload_bytes!(payload_iso_a_start, payload_iso_a_end, payload_iso_a_bytes);

core::arch::global_asm!(
    ".global payload_iso_b_start",
    "payload_iso_b_start:",
    "movabs rdi, {x}",
    "mov rax, [rdi]",
    "test rax, rax",
    "jne 1f",
    "movabs rax, 0xBBBBBBBBBBBBBBBB",
    "mov [rdi], rax",
    "mov rax, [rdi]",
    "movabs rdx, 0xBBBBBBBBBBBBBBBB",
    "cmp rax, rdx",
    "jne 2f",
    "mov rax, {exit}",
    "mov rdi, 0",
    "int 0x80",
    "1:",
    "mov rax, {exit}",
    "mov rdi, 1",
    "int 0x80",
    "2:",
    "mov rax, {exit}",
    "mov rdi, 2",
    "int 0x80",
    ".global payload_iso_b_end",
    "payload_iso_b_end:",
    x = const process::USER_DATA_VA,
    exit = const usermode::SYS_EXIT,
);
payload_bytes!(payload_iso_b_start, payload_iso_b_end, payload_iso_b_bytes);

core::arch::global_asm!(
    ".global payload_kernel_read_start",
    "payload_kernel_read_start:",
    "movabs rdi, {k}",
    "mov rax, [rdi]",
    "mov rax, {exit}",
    "mov rdi, 1",
    "int 0x80",
    ".global payload_kernel_read_end",
    "payload_kernel_read_end:",
    k = const KERNEL_PROBE_ADDRESS,
    exit = const usermode::SYS_EXIT,
);
payload_bytes!(
    payload_kernel_read_start,
    payload_kernel_read_end,
    payload_kernel_read_bytes
);

/// Supervisor-only, mapped kernel address the third payload reads to prove
/// user mode cannot touch kernel memory.
const KERNEL_PROBE_ADDRESS: u64 = 0xffff_8000_4000_0000;

/// Exclusive end of the ring-3 stack handed to the user-mode payloads.
fn user_stack_top() -> u64 {
    core::ptr::addr_of!(USER_STACK) as *const u8 as u64 + USER_STACK_BYTES as u64
}

/// Ring-3 test payload: writes one message with a syscall, then exits.
fn user_payload_write() {
    let message = USER_HELLO;
    let _ = syscall_write(message.as_ptr() as u64, message.len() as u64);
    syscall_exit(usermode::USER_EXIT_OK);
}

/// Performs the write syscall from ring 3 and returns the kernel's result.
fn syscall_write(pointer: u64, length: u64) -> u64 {
    let result: u64;
    // SAFETY: `int 0x80` is the installed syscall gate, which is callable
    // from ring 3; the kernel validates the pointer and length before use.
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") usermode::SYS_WRITE_SERIAL,
            in("rdi") pointer,
            in("rsi") length,
            lateout("rax") result,
        );
    }
    result
}

/// Exits the ring-3 task with `code`; never returns.
fn syscall_exit(code: u64) -> ! {
    // SAFETY: `int 0x80` is the installed syscall gate, and the exit path
    // resumes the kernel instead of returning to the caller.
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") usermode::SYS_EXIT,
            in("rdi") code,
            options(noreturn),
        );
    }
}

/// Ring-3 test payload: feeds the write syscall three hostile arguments and
/// exits cleanly only if the kernel rejects every one of them.
fn user_payload_hostile() {
    let mut rejected = 0u32;
    // Supervisor-only kernel heap page with a valid-looking length.
    if syscall_write(0xffff_8000_4000_0000, 16) == u64::MAX {
        rejected += 1;
    }
    // Length that wraps the range past the end of the address space.
    if syscall_write(0x1000, u64::MAX) == u64::MAX {
        rejected += 1;
    }
    // Range straddling the end of the user-accessible low region.
    if syscall_write(0x3FFF_F000, 0x2000) == u64::MAX {
        rejected += 1;
    }
    if rejected == 3 {
        syscall_exit(usermode::USER_EXIT_OK);
    } else {
        syscall_exit(1);
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
