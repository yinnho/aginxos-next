// wdt — M20c watchdog probe/feeder/starve for /dev/watchdog (msm_watchdog,
// watchdog_v2 module).
//
// Why this exists: dmesg says "wdog absent resource not present" for
// 17c10000.qcom,wdt — the APPS-side bark/pet IRQ resources are absent in
// this DT, so the kernel does not self-pet and the M14 bad-kernel hang
// sat dark forever. Whether OPENING /dev/watchdog arms a real PMIC
// watchdog that hard-resets an unpetted SoC is the open question; on
// this driver the answer is not documented anywhere we can read, so we
// measure it. Starve below is a one-shot live-fire experiment.
//
// Modes:
//   wdt probe            open, immediately SETTIMEOUT 128, dump SUPPORT/
//                        STATUS/BOOTSTATUS/TIMEOUT, pet for ~5 s, close.
//                        Safe-ish: if the driver disarms on close
//                        (no MAGICCLOSE needed), we leave nothing armed.
//   wdt arm <secs>       open, SETTIMEOUT secs, pet every secs/3 forever.
//                        The feeder rcS/agsvc would run in production.
//   wdt starve <secs>    open, SETTIMEOUT secs, print a countdown, and
//                        deliberately never pet. If real hardware is
//                        behind this, the box hard-resets around the
//                        timeout; if we print "still alive", the wdt is
//                        a no-op and userspace self-rescue is dead on
//                        this kernel.
//
// NB: if the driver is built with nowayout (common), close() does NOT
// disarm — after probe/starve the box may still be armed at whatever
// timeout was last set. Probe sets 128 s so a forgotten armed state
// resets a hung box rather than a busy one.
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <linux/watchdog.h>

static int open_wdt(void)
{
    int fd = open("/dev/watchdog", O_WRONLY | O_CLOEXEC);
    if (fd < 0) {
        perror("wdt: open /dev/watchdog");
        exit(1);
    }
    return fd;
}

static void set_timeout(int fd, int secs)
{
    if (ioctl(fd, WDIOC_SETTIMEOUT, &secs) < 0) {
        perror("wdt: WDIOC_SETTIMEOUT");
        fprintf(stderr, "wdt: continuing with driver default\n");
    } else {
        printf("wdt: timeout set to %ds\n", secs);
    }
}

static void pet(int fd)
{
    int dummy = 0;
    if (ioctl(fd, WDIOC_KEEPALIVE, &dummy) < 0) {
        perror("wdt: keepalive");
    }
}

int main(int argc, char **argv)
{
    if (argc < 2) {
        fprintf(stderr, "usage: wdt probe | arm <secs> | starve <secs>\n");
        return 2;
    }
    int fd = open_wdt();
    struct watchdog_info info;
    if (ioctl(fd, WDIOC_GETSUPPORT, &info) == 0) {
        printf("wdt: identity='%s' fw=%u options=0x%x%s%s%s\n",
               info.identity, info.firmware_version, info.options,
               info.options & WDIOF_SETTIMEOUT ? " set-timeout" : "",
               info.options & WDIOF_MAGICCLOSE ? " magic-close" : "",
               info.options & WDIOF_KEEPALIVEPING ? " keepalive" : "");
    } else {
        perror("wdt: GETSUPPORT");
    }

    if (!strcmp(argv[1], "probe")) {
        set_timeout(fd, 128);
        int t = 0, b = 0, s = 0;
        ioctl(fd, WDIOC_GETTIMEOUT, &t);
        ioctl(fd, WDIOC_GETBOOTSTATUS, &b);
        ioctl(fd, WDIOC_GETSTATUS, &s);
        printf("wdt: timeout=%d bootstatus=0x%x status=0x%x\n", t, b, s);
        for (int i = 0; i < 5; i++) { pet(fd); sleep(1); }
        printf("wdt: petted 5x, closing\n");
        close(fd);
        return 0;
    }
    if (argc < 3) { fprintf(stderr, "wdt: need <secs>\n"); return 2; }
    int secs = atoi(argv[2]);
    if (secs < 5) { fprintf(stderr, "wdt: refusing timeout < 5s\n"); return 2; }

    if (!strcmp(argv[1], "arm")) {
        set_timeout(fd, secs);
        printf("wdt: arming — petting every %ds, pid %d\n", secs / 3, getpid());
        for (;;) { pet(fd); sleep(secs / 3); }
    }
    if (!strcmp(argv[1], "starve")) {
        set_timeout(fd, secs);
        int t = 0; ioctl(fd, WDIOC_GETTIMEOUT, &t);
        printf("wdt: STARVE — timeout=%ds, no pets from now\n", t);
        fflush(stdout);
        for (int i = t + 30; i > 0; i--) {
            printf("wdt: %d\n", i);
            fflush(stdout);
            sleep(1);
        }
        printf("wdt: still alive after timeout+30s — no real hardware behind this\n");
        close(fd);
        return 1;
    }
    fprintf(stderr, "wdt: unknown mode %s\n", argv[1]);
    return 2;
}
