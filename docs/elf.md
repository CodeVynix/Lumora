# ELF loader

Build 1 step 4, third slice replaces the hand-built payloads with a real ELF64
loader. `apps/hello` is a `no_std`, `no_main` static executable that prints a
line and exits through `int 0x80`; `kernel/src/elf.rs` validates it before a
single byte is mapped, and `kernel/src/process.rs` builds the address space
and runs it.

## The program

`apps/hello` is an ordinary Rust binary crate in the workspace with
`test = false` and `bench = false`, because its `#![no_main]` entry cannot be
run or benched on the host. It links with its own `link.ld` at a fixed base
of `0x80_0000_0000` under `-C relocation-model=static -C code-model=large`, so
the result is a static `ET_EXEC` image with two page-aligned `PT_LOAD`
segments and no interpreter. `xtask` builds it *before* the kernel, because
the kernel embeds it with `include_bytes!` and a stale image would be baked
into an otherwise correct build.

The program uses only two syscalls: `write(1, buf, 25)` and `exit(0)`.

## Validation

`elf::validate` returns a `BootImage` — the entry point plus at most
`MAX_LOAD_SEGMENTS` (8) loadable segments — or an `ElfError` whose `reason`
is what the kernel logs. Everything is bounds-checked against the length of
the buffer it was handed, because that buffer is attacker-controlled: every
read of a header field goes through `u16_at`/`u32_at`/`u64_at`, and every
`table + index * 56` style offset is proven to land inside the image before
use.

It checks the identification bytes, `ET_EXEC`, `EM_X86_64`, the two header
sizes, that the program header table fits in the file, that each segment's
`p_offset + p_filesz` fits, that `filesz <= memsz`, that the virtual range
cannot overflow, that the range lies inside the user window, that offsets and
addresses are page aligned, that no segment is both writable and executable,
that no two segments overlap, and that the entry point falls inside a
loadable segment. Zero-sized segments are skipped rather than rejected.

The loadable count is tallied in a first pass *before* any segment is
inspected, so a header claiming thousands of entries cannot force unbounded
work, and an over-long list is rejected as `too many segments` before its
contents are trusted.

## Address space

`process::spawn_elf` allocates the PML4 and a PDPT, copies the kernel's PML4
entries 0 and 256 so the kernel image, identity map, and interrupt stacks keep
working, and points PML4 entry 1 at the private PDPT. The user window is
`0x80_0000_0000` to `0x80_1FF0_0000`, which is 32 MB and therefore spans
sixteen page directories, so the page directories and page tables below the
PDPT are created on demand by `user_page_table` the first time an address in
their 2 MB window is mapped.

`walk_slots` is the only place that turns an address into PML4, PDPT, PD, and
PT slots. This matters more than it looks: an image linked at the base uses
slot 0 at every level, so a level using another level's index still maps its
first segment correctly. The stack page at `0x80_1FFF_F000` needs PDPT slot 0,
PD slot 255, and PT slot 511, and that is what exposed the mistake.

Segment pages are zeroed and then have `filesz` bytes copied from the file, so
the `.bss` tail beyond `filesz` reads as zero. Teardown walks PML4 entry 1 and
frees every leaf frame and every private table it finds, so the lazily created
directories are reclaimed too.

## Entry and exit

`process::enter_elf` loads the process root and drops to ring 3 at the entry
point with the stack top as RSP, and returns the exit code, the general
protection fault marker, or the page fault marker. The idle loop logs that as
`process 0 exited with code 0` or `process 0 killed: page fault`, then frees
the address space and records the frame count.

## Tests

`elf loader test` runs the real image and expects `hello from an ELF program`
followed by a clean exit, then feeds ten mutated images to `validate` and
requires each to be rejected for its own reason: bad magic, truncated file,
program header table past end of file, segment past end of file, `filesz`
exceeding `memsz`, segment outside the user range, address range overflow,
writable and executable segment, entry point outside every segment, and too
many segments. A case rejected for the wrong reason logs it and fails the
test.

The test also brackets the whole run with `free_frames` and requires them to
match, which is what proves teardown gives back every frame it took. It
already earned its keep: `FrameAllocator::allocate` was not decrementing its
count when it recycled a frame from the free list.

`ELF loader test PASS` needs all three: the program ran and exited 0, ten of
ten hostile images rejected with the expected reason, and no frame leaked.

`shortcut: the stack page sits at 0x80_1FFF_F000, just above the
USER_RANGE_END the validator enforces on segments, and the loader keeps a
single stack page rather than a guard page or a stack that grows; move the
stack inside the range and grow it when a real user program needs one.`

`shortcut: only `PT_LOAD` is honoured and there is no `PT_GNU_STACK` handling,
so the stack is fixed by the loader rather than requested by the image; parse
program stack requirements when a program needs to control its own stack.`
