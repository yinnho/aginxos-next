// android-shims.c — M42d 静态终链的 Android 系统库垫片。
// 这些符号在动态世界来自 libandroid.so/liblog.so/libdl.so——NDK 不发静态版。
// 我们跑在 musl rootfs 上：AAsset/APK 资产路径与 dlopen provider 加载是死码
// （模型全走文件路径，provider=cpu 内建）；logcat 系列重定向 stderr 保住
// sherpa 的错误诊断（melo 静音那次就是靠 stderr 日志定位的）。
#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <sys/types.h>

// ---- liblog → stderr ----
int __android_log_print(int prio, const char *tag, const char *fmt, ...) {
    (void)prio;
    va_list ap;
    va_start(ap, fmt);
    fprintf(stderr, "[sherpa:%s] ", tag);
    int r = vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    va_end(ap);
    return r;
}

int __android_log_vprint(int prio, const char *tag, const char *fmt, va_list ap) {
    (void)prio;
    fprintf(stderr, "[sherpa:%s] ", tag);
    int r = vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    return r;
}

int __android_log_write(int prio, const char *tag, const char *text) {
    (void)prio;
    return fprintf(stderr, "[sherpa:%s] %s\n", tag, text);
}

// ---- libandroid AAsset：APK 资产加载，CLI 永不触发 ----
struct AAssetManager;
struct AAsset;
struct AAsset *AAssetManager_open(struct AAssetManager *m, const char *f, int mode) {
    (void)m; (void)f; (void)mode;
    fprintf(stderr, "aginx-shim: AAssetManager_open hit (APK path) — abort\n");
    abort();
}
void AAsset_close(struct AAsset *a) { (void)a; abort(); }
const void *AAsset_getBuffer(struct AAsset *a) { (void)a; abort(); }
off_t AAsset_getLength(struct AAsset *a) { (void)a; abort(); }

// ---- libdl：cpu provider 内建；动态 provider 注册失败可优雅降级 ----
void *dlopen(const char *f, int flags) { (void)f; (void)flags; return NULL; }
char *dlerror(void) { return (char *)"static link: dlopen unavailable"; }
void *dlsym(void *h, const char *n) { (void)h; (void)n; return NULL; }
int dlclose(void *h) { (void)h; return 0; }
int dladdr(const void *addr, void *info) { (void)addr; (void)info; return 0; }
