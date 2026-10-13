# Threads

Build 1 step 3, part C adds kernel threads with a cooperative context switch,
and part D makes the scheduler preemptive: every tenth timer tick ends the
running thread's time slice (see `timer.md`). Threads still yield voluntarily
with `yield_now()`, and the timer preempts threads that never yield.
`kernel/src/thread.rs` owns everything below.

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
into `thread_trampoline`, which enables interrupts so the new thread runs
preemptible like every other thread, allocates the shadow space the ABI
requires, calls the thread's entry point, and then marks the thread finished
and keeps yielding. Nothing ever falls back into the saved frame.

## Yield

`yield_now` finds the next runnable slot after the current one, wrapping
around the table but never returning the current slot itself: switching to a
thread whose stack pointer was never saved would load garbage. It disables
interrupts, switches, restores the interrupt flag to whatever the caller had,
and reports whether a switch happened. A yield with no other runnable thread
is a no-op.

The same function serves voluntary yields from thread code and preemption
from the timer handler. Inside the handler interrupts are already off, so the
restore is correctly skipped there; when the preempted thread resumes later,
its own `yield_now` frame restores the flag it had.

## Preemption

Every tenth tick ends the running thread's 100 ms slice: the handler sends
EOI first so the PIC keeps delivering ticks, then asks the round robin for
the next thread. A global atomic counter records each timer-triggered switch;
it only counts switches that actually happen, never bare slice expiries with
nothing else runnable.

Two rules keep preemption sound. First, every thread runs with interrupts
enabled: the trampoline executes `sti` before a new thread's entry point, and
`yield_now` restores the caller's flag after every switch. Second, serial
logging disables interrupts for the duration of each log call, so a tick can
never preempt a thread in the middle of a line; the timer handler itself never
logs.

The boot thread's loop yields once the timer test has passed; once both
spawned threads are finished it logs the preemption count and the preemption
test result, then keeps idling and logging ticks as before.
