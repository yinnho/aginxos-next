/* redfin 裸件第一级：链接在 qcon 惯例内核入口 0x80008000。
 * ABL 实际落点若是 0x80000000，start.S 自迁移吸收；对上则零拷贝。
 * 产出 objcopy -O binary 的平头裸二进制（stock 内核同款：0 偏移即指令）。
 */
ENTRY(_start)

SECTIONS
{
  . = 0x80008000;

  .text : {
    KEEP(*(.text.boot))
    *(.text*)
  }
  .rodata : { *(.rodata*) }
  .data   : { *(.data*) }
  .bss : {
    . = ALIGN(16);
    __bss_start = .;
    *(.bss*) *(COMMON)
    __bss_end = .;
  }
  . = ALIGN(16);
  _image_end = .;

  /DISCARD/ : { *(.comment) *(.note*) *(.ARM.attributes) *(.eh_frame*) *(.eh_frame_hdr) }
}
