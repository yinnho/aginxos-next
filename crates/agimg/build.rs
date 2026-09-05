// Build vendored libjpeg-turbo 2.1.5.1 (static, decode-capable) into the
// crate. Source lists mirror upstream CMakeLists.txt exactly — core
// JPEG_SOURCES (arithmetic coding on) plus, on aarch64, the full NEON
// intrinsics set (NEON_INTRINSICS=1: no .S assembly, pure C intrinsics, so
// zig cc handles the musl cross like any other C). Non-aarch64 hosts get
// jsimd_none.c — the scalar fallback — so `cargo test` works anywhere.
//
// jconfig.h / jconfigint.h / jversion.h are the files cmake generated for a
// macOS arm64 configure of this exact tree (WITH_SIMD flipped on by hand to
// match the NEON build); neon-compat.h is the .in template with the optional
// multi-vector intrinsics left undefined (portable fallback). Regen note in
// vendor/README.md.

use std::path::PathBuf;

fn main() {
    let vend = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("vendor");

    let core = [
        "jcapimin.c", "jcapistd.c", "jccoefct.c", "jccolor.c", "jcdctmgr.c", "jchuff.c",
        "jcicc.c", "jcinit.c", "jcmainct.c", "jcmarker.c", "jcmaster.c", "jcomapi.c",
        "jcparam.c", "jcphuff.c", "jcprepct.c", "jcsample.c", "jctrans.c", "jdapimin.c",
        "jdapistd.c", "jdatadst.c", "jdatasrc.c", "jdcoefct.c", "jdcolor.c", "jddctmgr.c",
        "jdhuff.c", "jdicc.c", "jdinput.c", "jdmainct.c", "jdmarker.c", "jdmaster.c",
        "jdmerge.c", "jdphuff.c", "jdpostct.c", "jdsample.c", "jdtrans.c", "jerror.c",
        "jfdctflt.c", "jfdctfst.c", "jfdctint.c", "jidctflt.c", "jidctfst.c", "jidctint.c",
        "jidctred.c", "jquant1.c", "jquant2.c", "jutils.c", "jmemmgr.c", "jmemnobs.c",
        "jaricom.c", "jcarith.c", "jdarith.c",
    ];

    // simd/CMakeLists.txt, CPU_TYPE=arm64 + NEON_INTRINSICS=1 branch
    let neon = [
        "jcgray-neon.c", "jcphuff-neon.c", "jcsample-neon.c", "jdmerge-neon.c",
        "jdsample-neon.c", "jfdctfst-neon.c", "jidctred-neon.c", "jquanti-neon.c",
        "jccolor-neon.c", "jidctint-neon.c", "jidctfst-neon.c", "jdcolor-neon.c",
        "jfdctint-neon.c",
    ];

    let mut b = cc::Build::new();
    b.include(&vend)
        .include(vend.join("simd"))
        .warnings(false);
    b.file(vend.join("agimg_shim.c"));
    for f in core {
        b.file(vend.join(f));
    }
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        b.include(vend.join("simd/arm")).define("NEON_INTRINSICS", None);
        for f in neon {
            b.file(vend.join("simd/arm").join(f));
        }
        // aarch64/jccolext-neon.c stays on disk but is NOT compiled — it is
        // #included by jccolor-neon.c (CMake compiles only these two).
        for f in ["jsimd.c", "jchuff-neon.c"] {
            b.file(vend.join("simd/arm/aarch64").join(f));
        }
    } else {
        b.file(vend.join("jsimd_none.c"));
    }
    b.compile("agimg_jpeg");
    println!("cargo:rerun-if-changed=vendor/");
}
