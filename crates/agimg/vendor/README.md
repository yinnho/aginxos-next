# vendored libjpeg-turbo 2.1.5.1 (subset)

Source: https://github.com/libjpeg-turbo/libjpeg-turbo `2.1.5.1` release
tarball (IJG/BSD-style license, same terms as upstream — headers in each
file). Committed subset = the files build.rs compiles, no more:

- 51 core `.c` from CMakeLists `JPEG_SOURCES` (+ arithmetic coding trio)
- template `.c` files that are `#include`d, not compiled (jccolext.c,
  jdcolext.c, jdcol565.c, jdmrgext.c, jdmrg565.c, jstdhuff.c)
- `simd/jsimd.h`, `simd/jsimddct.h`, root headers, `jsimd_none.c`
- aarch64 full NEON-intrinsics set (no `.S`): `simd/arm/*.c` ×16 +
  `simd/arm/aarch64/{jsimd.c,jchuff-neon.c,jccolext-neon.c}`
- `agimg_shim.c` — our FFI wrapper (the only file not from upstream)

## generated headers

`jconfig.h`, `jconfigint.h`, `jversion.h` are what cmake generated for an
`-DCMAKE_BUILD_TYPE=Release -DWITH_SIMD=1` macOS arm64 configure of this
exact tree (`WITH_SIMD` flipped on by hand to match the NEON build; the
others are byte-for-byte cmake output). `simd/arm/neon-compat.h` is the
`.in` template with the optional multi-vector intrinsics left undefined —
portable fallback. To regenerate after a version bump:

    cmake -B build -DCMAKE_BUILD_TYPE=Release -DWITH_SIMD=1 \
          -DCMAKE_POLICY_VERSION_MINIMUM=3.5
    # copy build/{jconfig.h,jconfigint.h,jversion.h} here, diff the rest
    # against the new tarball's CMakeLists source lists, fix build.rs

Upstream 16 MB full tree and release tarballs stay in /tmp — only this
subset ships in the repo (~830 KB).
