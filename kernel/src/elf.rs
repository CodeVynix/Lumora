//! Minimal ELF64 loader: validate first, map afterwards.
//!
//! `validate` checks a byte slice against the ELF specification (ELF header
//! layout and program header layout below follow the System V ABI chapter on
//! object files) without touching page tables or allocating anything, so a
//! hostile file can only cost CPU time. Only validated images reach the page
//! mapper in [`crate::process`].

/// Maximum loadable segments mapped from one image.
pub const MAX_LOAD_SEGMENTS: usize = 8;

/// First byte of the ELF magic (`0x7F`, `E`, `L`, `F`).
const MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
/// e_ident index of the file class; 2 means 64-bit objects.
const EI_CLASS: usize = 4;
/// e_ident index of the data encoding; 1 means little-endian.
const EI_DATA: usize = 5;
/// e_ident index of the ABI version; 1 means the current version.
const EI_VERSION: usize = 6;
/// File type for executables.
const ET_EXEC: u16 = 2;
/// Machine type for x86-64.
const EM_X86_64: u16 = 62;
/// Size of the ELF64 header in bytes.
const HEADER_SIZE: usize = 64;
/// Size of one ELF64 program header in bytes.
const PROGRAM_HEADER_SIZE: usize = 56;
/// Program header type for loadable segments.
const PT_LOAD: u32 = 1;
/// Segment flag bit: execute.
const FLAG_EXECUTE: u32 = 1;
/// Segment flag bit: write.
pub(crate) const FLAG_WRITE: u32 = 2;

/// One validated loadable segment: file bytes to copy and memory to map.
#[derive(Clone, Copy)]
pub struct Segment {
    /// Virtual address the segment is mapped at; page-aligned by validation.
    pub vaddr: u64,
    /// File offset the segment bytes start at; page-aligned by validation.
    pub offset: u64,
    /// Bytes present in the file.
    pub filesz: u64,
    /// Bytes mapped in memory; the tail past `filesz` is zeroed BSS.
    pub memsz: u64,
    /// Raw ELF segment flags (`PF_R`/`PF_W`/`PF_X`).
    pub flags: u32,
}

impl Segment {
    const EMPTY: Self = Self {
        vaddr: 0,
        offset: 0,
        filesz: 0,
        memsz: 0,
        flags: 0,
    };
}

/// A validated executable: entry point plus its loadable segments.
#[derive(Clone, Copy)]
pub struct BootImage {
    /// Virtual address execution starts at, inside a loadable segment.
    pub entry: u64,
    /// Validated segments, `segments[..segment_count]` significant.
    pub segments: [Segment; MAX_LOAD_SEGMENTS],
    /// Number of significant entries in `segments`.
    pub segment_count: usize,
}

/// Why an image was rejected; [`ElfError::reason`] renders each for the log.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ElfError {
    /// First four bytes are not the ELF magic.
    BadMagic,
    /// Fewer than 64 bytes, so not even the header fits.
    Truncated,
    /// Not a 64-bit object.
    BadClass,
    /// Not little-endian.
    BadData,
    /// Neither header carries version 1.
    BadVersion,
    /// Not an executable.
    BadType,
    /// Not an x86-64 object.
    BadMachine,
    /// Header or program-header size fields are wrong.
    BadHeaderSizes,
    /// The program-header table runs past the end of the file.
    TablePastEnd,
    /// More loadable segments than the mapper supports.
    TooManySegments,
    /// A segment's file bytes run past the end of the file.
    SegmentPastEnd,
    /// A segment claims more file bytes than memory bytes.
    FileszExceedsMemsz,
    /// A segment address range overflows the address space.
    RangeOverflow,
    /// A segment lies outside the user address range.
    OutsideUserRange,
    /// A segment address or offset is not page-aligned.
    Misaligned,
    /// Two segments map the same page.
    OverlappingSegments,
    /// A segment is both writable and executable.
    WritableAndExecutable,
    /// The entry point is inside no loadable segment.
    EntryOutsideSegments,
}

impl ElfError {
    /// Short stable reason logged for every rejection.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::BadMagic => "bad magic",
            Self::Truncated => "truncated file",
            Self::BadClass => "not 64-bit",
            Self::BadData => "not little-endian",
            Self::BadVersion => "bad version",
            Self::BadType => "not executable",
            Self::BadMachine => "not x86-64",
            Self::BadHeaderSizes => "bad header sizes",
            Self::TablePastEnd => "program header table past end of file",
            Self::TooManySegments => "too many segments",
            Self::SegmentPastEnd => "segment past end of file",
            Self::FileszExceedsMemsz => "filesz exceeds memsz",
            Self::RangeOverflow => "address range overflow",
            Self::OutsideUserRange => "segment outside user range",
            Self::Misaligned => "segment not page-aligned",
            Self::OverlappingSegments => "overlapping segments",
            Self::WritableAndExecutable => "writable and executable segment",
            Self::EntryOutsideSegments => "entry point outside any segment",
        }
    }
}

/// First user virtual address a segment may occupy: start of the private
/// page-table window every process shares the layout of.
pub(crate) const USER_RANGE_START: u64 = 0x80_0000_0000;
/// One past the last user virtual address a segment may occupy. The final
/// page below this boundary stays reserved for the process stack.
pub(crate) const USER_RANGE_END: u64 = 0x80_1FF0_0000;
/// Top of the user stack mapped for every ELF process, one fresh page below
/// the end of the shared page-table window.
pub const ELF_STACK_TOP: u64 = 0x80_2000_0000;

/// Validates `bytes` as a static x86-64 executable without mapping anything.
///
/// Every check below runs before any page is touched, in an order that bounds
/// the work first: header, table bounds, segment count, then one segment at
/// a time, then cross-segment overlap, then the entry point.
pub fn validate(bytes: &[u8]) -> Result<BootImage, ElfError> {
    if bytes.len() < HEADER_SIZE {
        return Err(ElfError::Truncated);
    }
    if bytes[0..4] != MAGIC {
        return Err(ElfError::BadMagic);
    }
    if bytes[EI_CLASS] != 2 {
        return Err(ElfError::BadClass);
    }
    if bytes[EI_DATA] != 1 {
        return Err(ElfError::BadData);
    }
    if bytes[EI_VERSION] != 1 {
        return Err(ElfError::BadVersion);
    }
    if u16_at(bytes, 16) != ET_EXEC {
        return Err(ElfError::BadType);
    }
    if u16_at(bytes, 18) != EM_X86_64 {
        return Err(ElfError::BadMachine);
    }
    if u32_at(bytes, 20) != 1 {
        return Err(ElfError::BadVersion);
    }
    if u16_at(bytes, 52) != HEADER_SIZE as u16 || u16_at(bytes, 54) != PROGRAM_HEADER_SIZE as u16 {
        return Err(ElfError::BadHeaderSizes);
    }
    let entry = u64_at(bytes, 24);
    let table = u64_at(bytes, 32) as usize;
    let count = u16_at(bytes, 56) as usize;
    let table_size = count.saturating_mul(PROGRAM_HEADER_SIZE);
    let table_end = table.saturating_add(table_size);
    if table_end > bytes.len() {
        return Err(ElfError::TablePastEnd);
    }
    let mut image = BootImage {
        entry,
        segments: [Segment::EMPTY; MAX_LOAD_SEGMENTS],
        segment_count: 0,
    };
    // Count loadable segments before inspecting any, so a hostile header
    // count can never force unbounded work below.
    let mut loadable = 0usize;
    for index in 0..count {
        let header = table + index * PROGRAM_HEADER_SIZE;
        if u32_at(bytes, header) == PT_LOAD {
            loadable += 1;
        }
    }
    if loadable > MAX_LOAD_SEGMENTS {
        return Err(ElfError::TooManySegments);
    }
    for index in 0..count {
        let header = table + index * PROGRAM_HEADER_SIZE;
        if u32_at(bytes, header) != PT_LOAD {
            continue;
        }
        let flags = u32_at(bytes, header + 4);
        let offset = u64_at(bytes, header + 8);
        let vaddr = u64_at(bytes, header + 16);
        let filesz = u64_at(bytes, header + 32);
        let memsz = u64_at(bytes, header + 40);
        if offset
            .checked_add(filesz)
            .filter(|end| *end <= bytes.len() as u64)
            .is_none()
        {
            return Err(ElfError::SegmentPastEnd);
        }
        if filesz > memsz {
            return Err(ElfError::FileszExceedsMemsz);
        }
        if memsz == 0 {
            continue;
        }
        let memory_end = vaddr.checked_add(memsz).ok_or(ElfError::RangeOverflow)?;
        if vaddr < USER_RANGE_START || memory_end > USER_RANGE_END {
            return Err(ElfError::OutsideUserRange);
        }
        if !vaddr.is_multiple_of(0x1000) || !offset.is_multiple_of(0x1000) {
            return Err(ElfError::Misaligned);
        }
        if flags & (FLAG_WRITE | FLAG_EXECUTE) == (FLAG_WRITE | FLAG_EXECUTE) {
            return Err(ElfError::WritableAndExecutable);
        }
        image.segments[image.segment_count] = Segment {
            vaddr,
            offset,
            filesz,
            memsz,
            flags,
        };
        image.segment_count += 1;
    }
    for left in 0..image.segment_count {
        for right in (left + 1)..image.segment_count {
            let first = image.segments[left];
            let second = image.segments[right];
            let first_end = first.vaddr + first.memsz;
            let second_end = second.vaddr + second.memsz;
            if first.vaddr < second_end && second.vaddr < first_end {
                return Err(ElfError::OverlappingSegments);
            }
        }
    }
    if !image
        .segments
        .iter()
        .take(image.segment_count)
        .any(|segment| entry >= segment.vaddr && entry - segment.vaddr < segment.memsz)
    {
        return Err(ElfError::EntryOutsideSegments);
    }
    Ok(image)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    let mut pair = [0u8; 2];
    pair.copy_from_slice(&bytes[offset..offset + 2]);
    u16::from_le_bytes(pair)
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut quad = [0u8; 4];
    quad.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(quad)
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut oct = [0u8; 8];
    oct.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(oct)
}

#[cfg(test)]
mod tests {
    use super::*;

    static HELLO: &[u8] = include_bytes!("../../target/x86_64-unknown-none/debug/hello");

    #[test]
    fn good_hello_validates() {
        let image = validate(HELLO).expect("hello must validate");
        assert!(image.segment_count >= 1);
    }

    #[test]
    fn entry_outside_full_length() {
        let mut image = [0u8; 16384];
        assert!(HELLO.len() <= image.len());
        image[..HELLO.len()].copy_from_slice(HELLO);
        image[24..32].copy_from_slice(&0x80_1000_0000u64.to_le_bytes());
        assert_eq!(
            validate(&image[..HELLO.len()]).map(|_| ()),
            Err(ElfError::EntryOutsideSegments)
        );
    }
}
