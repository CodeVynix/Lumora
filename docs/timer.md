# System timer

Build 1 step 3, part B drives the kernel tick from the legacy PC timer
hardware. A local APIC timer will replace the PIT later; the tick counter API
stays the same.

- `kernel/src/timer.rs` remaps the two 8259 PICs so IRQs 0-15 arrive on
  vectors 32-47 instead of colliding with CPU exceptions, then masks every
  IRQ except IRQ0 (the timer). It programs PIT channel 0 as a rate generator
  (mode 2) with divisor 11931 against the standard 1193182 Hz input clock,
  which yields ~100 Hz.
- `kernel/src/idt.rs` installs a gate for vector 32 using the existing stub
  pattern: IRQs push no error code, so the stub synthesises a zero one.
- The handler (`timer::tick`) increments an atomic tick counter and sends
  end-of-interrupt to the master PIC. It never prints and never takes a lock,
  so it is safe to run on every tick.
- Every tenth tick ends the running thread's 100 ms time slice: the handler
  sends EOI first, then asks the round-robin queue for the next thread, and a
  global atomic counter records the switch. Slices that expire with nothing
  else runnable are not counted.
- `kernel_entry` enables interrupts with `sti` after the IDT and the timer
  are ready, then idles on `hlt`. Each time the tick count crosses a multiple
  of 100 it logs `timer tick N (S s)`; after 500 ticks (5 s) it logs
  `timer test PASS` once and keeps idling.

With the `exception-test` feature the divide-by-zero check still runs before
the timer is enabled, so that boot ends in the fatal divide-error handler as
before.
