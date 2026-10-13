/* redfin 裸件第一级：直链装载真身。
 * ABL 按 arm64 协议装在 0x80008000（text_offset=0x80000），我们 64B
 * arm64 头把代码推到 +0x40 ⇒ 代码真身起跑于 0x80008040，入口即
 * code_0 支过来的 _start。零迁移（六代自迁移件 L10–L15 全冻死的教训）。
 * 产出 objcopy -O binary 的平头裸二进制（0 偏移即 _start）。
 */
ENTRY(_start)

SECTIONS
{
  . = 0x80008040;

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
  /* B2 自诊断向量网：2KB 对齐（VBAR 硬要求），必须在 _image_end 内
     （迁移拷贝范围跟着 _image_end 走） */
  . = ALIGN(0x800);
  .vectors : { KEEP(*(.vectors)) }
  . = ALIGN(16);
  _image_end = .;

  /DISCARD/ : { *(.comment) *(.note*) *(.ARM.attributes) *(.eh_frame*) *(.eh_frame_hdr) }
}
