/* E5: no liblzma on musl/zig — uncompressed .jsn configs never call this. */
int lzma_decomp(const char *file) { (void)file; return -1; }
