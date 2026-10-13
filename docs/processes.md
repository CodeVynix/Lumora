# Processes

Build 1 step 4, second slice gives every test payload its own address space.
There is still no ELF parser and no process API: `kernel/src/process.rs`
builds the tables by hand, copies an embedded payload into a fresh code page,
and runs exactly one process at a time.

## Process table

A fixed array of four slots keeps `id`, the frame address of the process's own
top-level page table (its PML4), the entry point, the user stack top, the
seven frames the process owns, and its state. Process ids are their slot
indices. The seven frames are PML4, PDPT, PD, the user page table, and the
code, data, and stack pages.

## Address space

Each process gets a fresh, zeroed PML4 with two kinds of entries:

- PML4 entry 0 and entry 256 are copied verbatim from the kernel's tables, so
  the low identity map (the payload code before it is copied, interrupt
  stacks, the kernel image) and the high-half heap keep working unchanged.
- PML4 entry 1 points at a private PDPT/PD/page-table chain built for this
  process, and every entry in that chain is marked user-accessible.

The user pages sit at fixed addresses shared by every process: code at
`0x80_0000_0000`, data at `0x80_0000_1000`, and the stack at
`0x80_0000_2000`. The code, data, and stack pages are separate fresh frames
per process, mapped user-accessible and writable; they are zeroed before the
payload bytes are copied in. Kernel pages never carry the user bit, so a
attempt to touch them from ring 3 faults. Because PML4 entry 1 is private,
two processes mapping their data page at the same user address alias
different physical frames — which is what the isolation test measures.

`shortcut: the payload page is writable as well as executable, and the low
identity map stays user-accessible so the earlier single-space user-mode test
still works; upgrade to per-process low mappings and read-only code pages when
a real loader exists.`

## Entry, exit, and teardown

`process::enter` saves the interrupt flag the way `yield_now` does, switches
CR3 to the process root inside the entry stub, drops to ring 3 with `iretq`,
and on return restores the flag before it touches anything else. That
restore matters: the syscall exit and the fault paths abandon their interrupt
frames with interrupts disabled, so without it the idle loop's `hlt` would
sleep forever.

`usermode::exit_to_kernel` switches CR3 back to the kernel root (recorded at
boot while the kernel tables were loaded) before restoring the saved kernel
stack, so the pops and the following frame releases always run on kernel
mappings. After `enter` returns, the slot is marked dead first and its seven
frames are handed to the allocator, so a second entry of the same id reports
an error instead of reusing freed frames.

## Frame reuse

`FrameAllocator` gained a free: returned frames go on a small LIFO list that
`allocate` drains first, so process frames are handed back out to later
processes. The list is bounded and drops what it cannot track; frames handed
to `free` must be unmapped everywhere and must not be freed twice.

## Tests

The idle loop runs two payloads that share one data address: the first writes
`0xAAAA...` and checks it, and the second must read zero (not the first
payload's value), write `0xBBBB...`, and check that. Then a third payload
reads a supervisor-only kernel heap address, faults, and is reported and
killed. `process isolation test PASS` requires all three to hold.
