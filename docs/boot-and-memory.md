# Boot and memory management

Build 1 starts with a UEFI application in `boot/`. It emits an early boot message
through the UEFI console and probes the standard UEFI serial protocol. The
`xtask` command stages the application as `EFI/BOOT/BOOTX64.EFI` on a FAT ESP.

The `kernel/` crate contains the early memory-management building blocks:

- `boot` exits UEFI boot services and copies the final UEFI memory descriptors
  into a bounded handoff array. Conventional and released boot-services memory
  become usable; loader-owned memory remains reserved.
- `frame` walks those regions in 4096-byte physical frames, reserving low memory
  for firmware data structures.
- `paging` allocates and initializes kernel-owned PML4, PDPT, PD, and PT pages,
  installs an identity map plus a high-half alias, and activates the root with
  `CR3`. It maps a writable high-half heap page from a physical frame.
- `heap` provides the small bump allocator used before a general allocator is
  available. The kernel writes and reads a test value through the mapped heap
  page and reports `PASS` or `FAIL` over COM1 during boot.
- `serial` logs messages over COM1. Besides `log`, it exposes `log_args`, a
  `core::fmt::Write` front end that formats structured reports such as the
  exception reports described in `interrupts.md`.

`cargo xtask run` uses `/usr/share/OVMF/OVMF_CODE_4M.fd`, copies
`/usr/share/OVMF/OVMF_VARS_4M.fd` to `target/OVMF_VARS_4M.fd` for each run, and
starts QEMU in serial-only (`-nographic`) mode. `cargo xtask run exception-test`
builds the same image with the `exception-test` feature and runs it.
