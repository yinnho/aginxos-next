/* redfin 裸件 Rust 链接基 = ABL 落点 0xA7E80000 + 壳 0x40（boot-pi.S
 * 头注 Ghidra 收据；B1–B4 链错基址六代冻死的反面教材）。
 * .text.boot（global_asm 入口）必须居首——objcopy 平铺后即镜像 +0x40。 */
ENTRY(_start)
SECTIONS {
  . = 0xA7E80040;
  .text : {
    KEEP(*(.text.boot))
    *(.text .text.*)
  }
  /* S0-3 异常向量表：16 槽×128B，VBAR 须 2KB 对齐 */
  . = ALIGN(0x800);
  .vectors : { KEEP(*(.vectors)) }
  /* s59：数据域抬 0x82000——wlanmdsp 真身 ~3.8MB 使平铺超 0x80000，
   * 而打包垫在 flat+0x80000 写双入口字节（两种 ABL 入口约定），必须
   * 让它落在 0x80000..0x82000 的 objcopy 零填洞里（text+vectors
   * ~0x34000 ≪ 0x80000，洞恒在；blob/rodata 一个字节不被碰）。 */
  . = 0xA7F02000;
  .rodata ALIGN(8) : { *(.rodata .rodata.*) }
  .data ALIGN(8) : { *(.data .data.*) }
  . = ALIGN(16);
  __bss_start = .;
  .bss (NOLOAD) : { *(.bss .bss.* COMMON) }
  . = ALIGN(16);
  __bss_end = .;
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) }
}
