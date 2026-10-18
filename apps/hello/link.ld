/* Linker script for the hello user program: a static ELF64 executable
   with every section page-aligned, linked at a fixed user address.
   Rust/LLVM may emit .ltext/.lrodata/.ldata section names, so both
   variants are matched. */
ENTRY(_start)
SECTIONS
{
  . = 0x8000000000;
  .text : ALIGN(0x1000) { *(.text .text.* .ltext .ltext.*) }
  .rodata : ALIGN(0x1000) { *(.rodata .rodata.* .lrodata .lrodata.*) }
  .data : ALIGN(0x1000) { *(.data .data.* .ldata .ldata.*) }
  .bss (NOLOAD) : ALIGN(0x1000) { *(.bss .bss.* .lbss .lbss.* COMMON) }
  /DISCARD/ : { *(.comment*) *(.note*) *(.debug*) }
}
