# User mode

Build 1 step 4, first slice runs one embedded static payload in ring 3. There
is no libc, no process API, and no ELF parser here: the payloads are ordinary
kernel functions entered with `iretq`, and a single resume slot unwinds them
back to the kernel. A per-process page-table design replaces all of this
later.

## Segments and stacks

`kernel/src/gdt.rs` adds ring-3 code and data descriptors next to the kernel
ones. They are the same flat 64-bit shapes with the descriptor privilege
level raised from 0 to 3, and their selectors carry RPL 3 (`0x2B` for data,
`0x33` for code), which is what `iretq` demands for the jump down. The TSS
gains a real privilege stack: RSP0 points at a private 16 KiB kernel stack,
so any interrupt, exception, or syscall from ring 3 lands on kernel memory
instead of the user stack.

The low first-gigabyte identity map is marked user-accessible, because the
payloads live in the kernel image down there. That is deliberately coarse:
`shortcut: the whole low gigabyte is user-accessible, upgrade to per-process
page tables when the process model needs real isolation.`

## Syscalls

The payload uses `int 0x80`, not the `syscall` instruction. A software
interrupt reuses the existing IDT gates and entry stubs with no model-specific
registers to program; the only extra piece is a gate whose descriptor
privilege level is 3 so ring 3 may invoke it. The stub synthesises the missing
error code exactly like the breakpoint and timer stubs, and the dispatcher
routes vector `0x80` to the syscall handler.

Two calls exist. Write takes a pointer and a length and logs the bytes;
anything else, including an unknown number, returns `u64::MAX`. Exit never
returns: it abandons the interrupt stack and resumes the kernel context saved
before entry.

## Validation

The write handler walks the current page tables for the whole
`[pointer, pointer + length)` range and requires every page present and
user-accessible before touching a byte. Zero-length writes are vacuously
safe; a wrapping range or a supervisor-only page fails the call instead.

## Faults and exit

`enter_user` saves the kernel registers, stores the stack pointer in the
resume slot, builds an `iretq` frame, and drops to ring 3. `exit_to_kernel`
loads that slot, pops the saved registers, and returns the exit code to
`enter_user`'s caller. A general protection fault from user mode takes the
same way out after logging the fault and the instruction pointer, so the
privileged-instruction payload is reported and killed while the kernel keeps
running. Kernel-mode faults still halt as before.

The idle loop runs a writing payload (exit code 0 expected) and then a
privileged-instruction payload (general-protection kill expected) and logs
`user mode test PASS` only when both behaved.
