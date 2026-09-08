/* LD_PRELOAD file-access tracer for bionic binaries.
 * Logs open/openat/opendir/fopen/access paths to fd 2 with a [T] prefix
 * so they survive interleaving with the target's own stderr noise.
 * Build: aarch64-linux-android24-clang -shared -fPIC -o trace_open.so */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdarg.h>
#include <stddef.h>
#include <fcntl.h>
#include <string.h>
#include <stdio.h>
#include <unistd.h>
#include <dirent.h>
#include <sys/stat.h>

static void log2(const char *tag, const char *p, int r)
{
    char buf[512];
    int n = snprintf(buf, sizeof buf, "[T] %s %s -> %d\n", tag, p ? p : "(null)", r);
    if (n > 0) write(2, buf, (size_t)n);
}

int open(const char *p, int flags, ...)
{
    static int (*real)(const char *, int, ...);
    if (!real) real = dlsym(RTLD_NEXT, "open");
    mode_t m = 0;
    if (flags & O_CREAT) {
        va_list ap; va_start(ap, flags); m = va_arg(ap, int); va_end(ap);
        int r = real(p, flags, m); log2("open", p, r); return r;
    }
    int r = real(p, flags); log2("open", p, r); return r;
}

int openat(int dirfd, const char *p, int flags, ...)
{
    static int (*real)(int, const char *, int, ...);
    if (!real) real = dlsym(RTLD_NEXT, "openat");
    mode_t m = 0;
    if (flags & O_CREAT) {
        va_list ap; va_start(ap, flags); m = va_arg(ap, int); va_end(ap);
        int r = real(dirfd, p, flags, m); log2("openat", p, r); return r;
    }
    int r = real(dirfd, p, flags); log2("openat", p, r); return r;
}

DIR *opendir(const char *p)
{
    static DIR *(*real)(const char *);
    if (!real) real = dlsym(RTLD_NEXT, "opendir");
    DIR *r = real(p); log2("opendir", p, r ? 0 : -1); return r;
}

FILE *fopen(const char *p, const char *mode)
{
    static FILE *(*real)(const char *, const char *);
    if (!real) real = dlsym(RTLD_NEXT, "fopen");
    FILE *r = real(p, mode); log2("fopen", p, r ? 0 : -1); return r;
}

int access(const char *p, int mode)
{
    static int (*real)(const char *, int);
    if (!real) real = dlsym(RTLD_NEXT, "access");
    int r = real(p, mode); log2("access", p, r); return r;
}

int stat(const char *p, struct stat *st)
{
    static int (*real)(const char *, struct stat *);
    if (!real) real = dlsym(RTLD_NEXT, "stat");
    int r = real(p, st); log2("stat", p, r); return r;
}

/* pd-mapper logs its jsn parse results to logcat; surface them on stderr. */
int __android_log_print(int prio, const char *tag, const char *fmt, ...)
{
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    log2(tag && *tag ? tag : "log", buf, prio);
    return 0;
}

/* libjson's file parser - reveal which jsn it is asked to parse and whether
 * it succeeded (NULL return = parse failure). */
void *json_object_from_file(const char *fn)
{
    static void *(*real)(const char *);
    if (!real) real = dlsym(RTLD_NEXT, "json_object_from_file");
    void *r = real(fn);
    log2("json", fn, r ? 0 : -1);
    return r;
}

/* libbase's LOG(FATAL) goes through __android_log_write, not _print. */
int __android_log_write(int prio, const char *tag, const char *text)
{
    log2(tag && *tag ? tag : "log", text ? text : "(null)", prio);
    return 0;
}

int __android_log_buf_print(int bufid, int prio, const char *tag, const char *fmt, ...)
{
    char b[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(b, sizeof b, fmt, ap);
    va_end(ap);
    log2(tag && *tag ? tag : "log", b, prio);
    return 0;
}
