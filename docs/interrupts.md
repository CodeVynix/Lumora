# CPU exceptions

Build 1 installs the tables the processor needs so a fault produces a readable
serial report instead of a silent reboot. Four kernel modules share the work:

- `kernel/src/cpu.rs` wraps `lgdt`, `lidt`, `ltr`, and `cli`, and parks the CPU
  with `cli; hlt` once a fatal handler is done.
- `kernel/src/gdt.rs` owns the global descriptor table and the task state
  segment. The table holds a null entry, a ring 0 64-bit code descriptor, a
  flat ring 0 data descriptor, and a 16-byte TSS descriptor. The TSS's IST1
  slot points at a private 16 KiB double-fault stack inside the kernel image,
  so a fault on an exhausted or broken stack still has somewhere to run. After
  `lgdt` the module retargets CS through a far return and reloads DS, ES, and
  SS, then loads the task register.
- `kernel/src/idt.rs` installs five gates: divide error (0), breakpoint (3),
  double fault (8, routed to IST1), general protection fault (13), and page
  fault (14). Every other vector stays not present.
- `kernel/src/interrupts.rs` holds the entry stubs, the `TrapFrame` layout, and
  the Rust dispatcher. Stable Rust has no `x86-interrupt` calling convention,
  so each gate gets a naked stub that pushes the vector number (and a zero
  error code where the processor supplies none), saves every general register,
  aligns the stack to the System V ABI, and calls the dispatcher.

A fatal handler logs the exception name, vector, error code, and instruction
pointer over COM1 through `serial::log_args`, then halts the CPU. The
breakpoint handler logs a message and returns, so `iretq` resumes the code
that executed `int3`.

## Tests

`cargo xtask run` loads the tables and executes a deliberate `int3`. The log
shows the breakpoint report followed by `breakpoint test PASS`, which is only
printed after the handler has returned.

`cargo xtask run exception-test` additionally executes a divide whose divisor
is zero. The divide-error handler prints its report and halts, so the log ends
with `Lumora Kernel: halting`. Without the handler the machine would triple
fault and reboot with no explanation. `cargo xtask build exception-test`
builds the same image without running it.

The feature lives on the `lumora-kernel` crate and is forwarded by the
`lumora-boot` crate, so `--features lumora-boot/exception-test` reaches the
kernel code.

## Known pre-existing fault after return

After the tests, `kernel_entry` still returns to the UEFI firmware as in
earlier milestones. The firmware then page-faults (instruction pointer
`0xa5003`, fault address above the first gigabyte) because the step 2 page
tables only identity-map the first gigabyte. The clean baseline faults the
same way, but the firmware's own handler recovers silently; with the kernel
IDT loaded, the page-fault gate reports it and halts instead. Mapping the
firmware's high memory, or not returning to firmware at all, is later work
and not part of this step.
