# Boot and memory management

Build 1 starts with a UEFI application in `boot/`. It emits an early boot message
through the UEFI console and probes the standard UEFI serial protocol. The
`xtask` command stages the application as `EFI/BOOT/BOOTX64.EFI` on a FAT ESP.

The `kernel/` crate contains the early memory-management building blocks:

- `frame` walks UEFI-provided usable memory regions in 4096-byte physical frames.
- `paging` reserves canonical high-half virtual pages for future four-level page
  table installation.
- `heap` provides the small bump allocator used before a general allocator is
  available. Its unit test checks alignment and non-overlap.

The kernel entry consumes a memory map and reports the first mapped page, heap
allocation, and remaining frames. Actual page-table activation and kernel ELF
handoff remain the next implementation slice of the boot protocol.
