# Threads

Build 1 step 3, part C adds cooperative kernel threads. Threads never preempt
each other: the timer interrupt only counts ticks (see `timer.md`) and never
switches, so a thread runs until it calls `yield_now()`. `kernel/src/thread.rs`
owns everything below.

## Thread table

The table is a fixed array of eight slots, so no allocation happens for the
scheduler itself. Slot 0 is the boot context, created by `thread::init()`; it
runs on the boot stack and never finishes, which keeps the round robin alive.
Each slot holds the thread's id, its entry point, its saved stack pointer, and
its state (runnable, finished, or unused). Thread ids are their slot indices.

Thread stacks are the only heap users: `spawn` takes 16 KiB from the kernel
heap for each thread. The boot code therefore maps nine heap pages — one for
the early allocations and eight for the two test threads.

## Stack layout

A fresh thread's stack comes from the heap 16-byte aligned, so its top is
aligned too. `spawn` builds a frame 72 bytes below that top:

- 64 bytes for the eight callee-saved registers the switch pops
- 8 bytes for the return address the switch's `ret` jumps through

so the saved stack pointer starts 8 bytes off a 16-byte boundary. That is
deliberate: when the switch's `ret` moves the stack pointer to the top of the
frame it leaves the new thread exactly 16-byte aligned, which is what the
ABI wants at a `call`. The thread's working stack grows down from the bottom
of that frame, inside the same 16 KiB.

## The switch

`context_switch` is a naked function, because stable Rust has no
`x86-interrupt` calling convention. It pushes `rbx`, `rbp`, `rdi`, `rsi`, and
`r12`-`r15` on the running thread's stack, writes that stack pointer through
its first argument, loads the second thread's stack pointer, pops the same
registers, and returns. The UEFI target uses the Microsoft x64 ABI, so the
first argument travels in `rcx` and the second in `rdx`; that ABI also makes
`rdi` and `rsi` callee-saved, which is why they are pushed even though the
System V list would stop at `rbx`, `rbp`, and `r12`-`r15`.

The first switch into a spawned thread pops eight zeroed registers and returns
into `thread_trampoline`, which allocates the shadow space the ABI requires,
calls the thread's entry point, and then marks the thread finished and keeps
yielding. Nothing ever falls back into the saved frame.

## Yield

`yield_now` finds the next runnable slot after the current one, wrapping
around the table, disables interrupts, switches, and restores the interrupt
flag to whatever the interrupted thread had. `next_runnable` never selects the
current thread, so a yield with no other runnable thread is a no-op.

The boot thread's loop yields once the timer test has passed; it logs
`thread test PASS` once both spawned threads are finished and then keeps
idling and logging ticks as before.
