# PCI enumeration

Build 1 step 5, first slice adds PCI configuration-space access and a bus
scan that detects a virtio block device. There is no driver yet and no
filesystem; the disk image xtask attaches is only there so the device
appears.

## Configuration mechanism #1

`kernel/src/pci.rs` reads PCI config space through I/O ports 0xCF8
(CONFIG_ADDRESS) and 0xCFC (CONFIG_DATA). Writing a dword to 0xCF8 selects
a (bus, device, function, dword offset); reading 0xCFC returns that dword.
The address word has bit 31 set as the enable bit, bits 23-16 for the bus,
bits 15-11 for the device, bits 10-8 for the function, and bits 7-2 for the
register offset (masked to a dword boundary). Every read goes through
`read_u32`, which is the only place that touches those ports.

## Enumeration

`pci::enumerate` walks buses 0 through 255, devices 0 through 31, and
functions 0 through 7. A device whose function 0 reports vendor 0xFFFF is
absent and the whole slot is skipped. Functions 1 through 7 are only
scanned when function 0's header type has the multifunction bit (bit 7 of
the byte at offset 0x0E) set, and each function is skipped individually if
its vendor is 0xFFFF. For each present function one line is logged with
bus:device.function, vendor ID, device ID, class code, and subclass. The
class/subclass come from the dword at offset 0x08 (byte 3 and byte 2
respectively); the header type and multifunction bit come from offset 0x0E.

## virtio-blk detection

A function with vendor 0x1AF4 and device ID 0x1001 (transitional) or 0x1042
(modern) is a virtio block device. Its location is stored in a static
`Option<Location>` and `Lumora Kernel: virtio-blk found at B:D.F` is logged.
`run_pci_test` logs `Lumora Kernel: PCI test PASS` only when one was found,
and runs after the process and ELF tests.

## What is not here yet

No BAR parsing, no bus mastering enable, no driver. The type-0 header
layout (BARs at 0x10..0x28, subsystem IDs at 0x2C/0x2E) is standard PCI
but is only read once a driver exists.

TODO: config writes are not implemented; add them when a driver needs to
enable bus mastering or size BARs. The exact offset of the command register
(0x04) and its bus-master bit are from the PCI specification, but are left
unused until then.
