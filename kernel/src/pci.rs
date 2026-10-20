//! PCI configuration space access and enumeration.
//!
//! The kernel talks to PCI devices through configuration mechanism #1: write
//! the address to I/O port 0xCF8 and read or write the data through 0xCFC.
//! Only enumeration and device detection live here; no driver touches a
//! device yet.

use crate::serial;

/// I/O port that selects the PCI configuration address.
const CONFIG_ADDRESS: u16 = 0xCF8;

/// I/O port that carries the selected configuration data.
const CONFIG_DATA: u16 = 0xCFC;

/// A device's location in the PCI hierarchy.
#[derive(Clone, Copy)]
pub struct Location {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

/// Where the virtio-blk device was found, if enumeration saw one.
static mut VIRTIO_BLK: Option<Location> = None;

/// Returns the virtio-blk location found by the last enumeration, if any.
pub fn virtio_blk_location() -> Option<Location> {
    // SAFETY: enumeration runs once, before any other code reads this, and
    // the kernel is single-threaded at that point.
    unsafe { VIRTIO_BLK }
}

/// Reads a 32-bit value from PCI configuration space.
///
/// `offset` is masked to a dword boundary, matching the mechanism #1 rule
/// that the low two bits of the address are always zero.
pub fn read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = 0x8000_0000u32
        | (bus as u32) << 16
        | (device as u32) << 11
        | (function as u32) << 8
        | (offset as u32 & 0xFC);
    // SAFETY: 0xCF8 and 0xCFC are the documented configuration mechanism #1
    // ports; writing the address register then reading the data register is
    // the sequence defined by the PCI specification.
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") CONFIG_ADDRESS,
            in("eax") address,
            options(nomem, nostack, preserves_flags),
        );
        let value: u32;
        core::arch::asm!(
            "in eax, dx",
            in("dx") CONFIG_DATA,
            out("eax") value,
            options(nomem, nostack, preserves_flags),
        );
        value
    }
}

/// Enumerates every PCI device on buses 0 through 255 and logs one line per
/// function found. Sets [`VIRTIO_BLK`] when a virtio block device appears.
pub fn enumerate() {
    for bus in 0u8..=255 {
        for device in 0u8..32 {
            let first = read_u32(bus, device, 0, 0x00);
            let vendor = first & 0xFFFF;
            if vendor == 0xFFFF {
                continue;
            }
            // Header type lives at config offset 0x0E (byte 2 of the dword
            // at 0x0C). The multifunction bit is bit 7.
            let header_dword = read_u32(bus, device, 0, 0x0C);
            let header_type = ((header_dword >> 16) & 0xFF) as u8;
            let multifunction = header_type & 0x80 != 0;
            let functions = if multifunction { 8u8 } else { 1u8 };

            for function in 0u8..functions {
                let id = if function == 0 {
                    first
                } else {
                    read_u32(bus, device, function, 0x00)
                };
                let vendor = id & 0xFFFF;
                if vendor == 0xFFFF {
                    continue;
                }
                let device_id = (id >> 16) & 0xFFFF;
                let class_dword = read_u32(bus, device, function, 0x08);
                // Offset 0x08 dword: byte 0 revision, byte 1 prog IF, byte 2
                // subclass, byte 3 class code.
                let subclass = (class_dword >> 16) & 0xFF;
                let class_code = (class_dword >> 24) & 0xFF;

                serial::log_args(format_args!(
                    "Lumora Kernel: PCI {:02x}:{:02x}.{} vendor=0x{:04x} device=0x{:04x} class=0x{:02x} subclass=0x{:02x}",
                    bus, device, function, vendor, device_id, class_code, subclass
                ));

                if vendor == 0x1AF4 && (device_id == 0x1001 || device_id == 0x1042) {
                    // SAFETY: enumeration is single-threaded and runs once.
                    unsafe {
                        VIRTIO_BLK = Some(Location {
                            bus,
                            device,
                            function,
                        });
                    }
                    serial::log_args(format_args!(
                        "Lumora Kernel: virtio-blk found at {:02x}:{:02x}.{}",
                        bus, device, function
                    ));
                }
            }
        }
    }
}

// TODO: PCI config writes (0xCF8/0xCFC) are not yet needed; add when a
// driver needs to enable bus mastering or set up BARs. The layout of the
// type-0 header (BARs at 0x10..0x28, subsystem IDs at 0x2C/0x2E) is
// standard PCI but is only read once a driver exists.
