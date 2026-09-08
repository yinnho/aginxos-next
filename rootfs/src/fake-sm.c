/* fake-sm.c — minimal binder context manager for AginxOS (task #46).
 *
 * pm-service (vendor.per_mgr) requires addService() on the vendor binder
 * device to succeed before it proceeds to its QMI registration. The stock
 * (vnd)servicemanager aborts without a kernel SELinux policy (no class
 * service_manager -> every addService denied), so this program registers
 * itself as THE context manager and answers every transaction with a
 * Status-ok parcel (int32 0). No SELinux, no names — just "ok".
 *
 * Usage: fake-sm /dev/vndbinder [/dev/binder ...]
 * Build: zig cc -target aarch64-linux-musl -static -O2 -o fake-sm fake-sm.c
 */
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <errno.h>

/* --- 4.19 binder uapi (LP64) --------------------------------------- */
struct bwr { uint64_t ws, wc, wb, rs, rc, rb; };
struct btd {
	union { uint32_t handle; uint64_t ptr; } target;
	uint64_t cookie;
	uint32_t code, flags;
	int32_t pid; uint32_t uid;
	uint64_t dsize, osize;
	union { struct { uint64_t buf, ofs; } p; uint8_t b[8]; } data;
};
struct bpc { uint64_t ptr, cookie; }; /* binder_ptr_cookie */

#define IOC_WB(d,n,s) ((d<<30)|(s<<16)|(0x62<<8)|n)
#define IOC_RC(n,s)   ((2<<30)|(s<<16)|(0x72<<8)|n)
#define IOC_CC(n,s)   ((1<<30)|(s<<16)|(0x63<<8)|n)

#define IOC_WRITE_READ   IOC_WB(3,1,48)   /* _IOWR('b',1,bwr) */
#define IOC_SET_CM       0x40046207       /* _IOW('b',7,s32)  */

#define BR_ERROR         0x80040072
#define BR_OK            0x7201
#define BR_TRANSACTION   0x80407202      /* _IOR('r',2,btd) — btd is 64 B: the
					  * data union is {u64 buf, u64 ofs} */
#define BR_REPLY         0x80407203
#define BR_DEAD_REPLY    0x7205
#define BR_T_COMPLETE    0x7206
#define BR_INCREFS       0x80107207
#define BR_ACQUIRE       0x80107208
#define BR_RELEASE       0x80107209
#define BR_DECREFS       0x8010720a
#define BR_NOOP          0x720c
#define BR_SPAWN_LOOPER  0x720d

#define BC_REPLY         0x40406301      /* _IOW('c',1,btd) = size 0x40 */
#define BC_FREE_BUFFER   0x40086303
#define BC_INCREFS_DONE  0x40106308
#define BC_ACQUIRE_DONE  0x40106309
#define BC_ENTER_LOOPER  0x630c

static int fd;
static uint64_t wbuf[64];

static int wpush_u32(uint32_t v, int off)
{
	memcpy((uint8_t *)wbuf + off, &v, 4); /* BYTE offset — wbuf is u64[] */
	return off + 4;
}
static int wpush_mem(const void *p, int len, int off)
{
	memcpy((uint8_t *)wbuf + off, p, len);
	return off + len;
}

int main(int argc, char **argv)
{
	if (argc < 2) { fprintf(stderr, "usage: %s /dev/vndbinder...\n", argv[0]); return 2; }

	for (int i = 1; i < argc; i++) {
		pid_t p = fork();
		if (p == 0) { /* child per device below */ }
		else if (p < 0) { perror("fork"); return 1; }
		else continue; /* parent spawns rest */
		fd = open(argv[i], O_RDWR);
		if (fd < 0) { fprintf(stderr, "%s: open %s: %s\n", argv[0], argv[i], strerror(errno)); _exit(1); }
		void *m = mmap(NULL, 128 * 1024, PROT_READ, MAP_PRIVATE, fd, 0);
		if (m == MAP_FAILED) { perror("mmap"); _exit(1); }
		int32_t zero = 0;
		if (ioctl(fd, IOC_SET_CM, &zero)) {
			fprintf(stderr, "%s: SET_CONTEXT_MGR on %s: %s\n", argv[0], argv[i], strerror(errno));
			_exit(1);
		}
		fprintf(stderr, "fake-sm: context manager on %s (pid %d)\n", argv[i], getpid());

		/* ENTER_LOOPER only — write-only BWR so it returns immediately.
		 * A read here would BLOCK inside this ioctl and swallow the
		 * first incoming transaction into a buffer we never parse. */
		int off = wpush_u32(BC_ENTER_LOOPER, 0);
		struct bwr rw = { .ws = off, .wb = (uint64_t)(uintptr_t)wbuf,
				  .rs = 0, .rb = 0 };
		ioctl(fd, IOC_WRITE_READ, &rw);
		uint64_t rbuf[512];
		for (;;) {
			int woff = 0;
			struct bwr r = { .ws = woff, .wb = (uint64_t)(uintptr_t)wbuf,
					 .rs = sizeof rbuf, .rb = (uint64_t)(uintptr_t)rbuf };
			if (ioctl(fd, IOC_WRITE_READ, &r)) { perror("bwr"); _exit(1); }
			uint8_t *rp = (uint8_t *)rbuf, *re = rp + r.rc;
			int pending_reply = 0; struct btd last_td;
			uint64_t free_bufs[8]; int nfree = 0;
			while (rp < re) {
				uint32_t cmd; memcpy(&cmd, rp, 4); rp += 4;
				if (cmd == BR_TRANSACTION) {
					memcpy(&last_td, rp, sizeof last_td);
					pending_reply = 1;
					rp += 64;
					const uint8_t *dp = (const uint8_t *)(uintptr_t)last_td.data.p.buf;
					fprintf(stderr, "fake-sm: TXN code=%08x pid=%d flags=%x dsize=%llu:",
						last_td.code, last_td.pid, last_td.flags,
						(unsigned long long)last_td.dsize);
					for (int k = 0; k < 16 && k < (int)last_td.dsize && dp; k++)
						fprintf(stderr, " %02x", dp[k]);
					fprintf(stderr, "\n");
				} else if (cmd == BR_INCREFS || cmd == BR_ACQUIRE) {
					struct bpc pc; memcpy(&pc, rp, 16); rp += 16;
					uint32_t done = (cmd == BR_INCREFS) ? BC_INCREFS_DONE : BC_ACQUIRE_DONE;
					woff = wpush_u32(done, woff);
					woff = wpush_mem(&pc, 16, woff);
				} else if (cmd == BR_REPLY) {
					struct btd td; memcpy(&td, rp, sizeof td); rp += 64;
					fprintf(stderr, "fake-sm: BR_REPLY code=%u\n", td.code);
				} else if (cmd == BR_RELEASE || cmd == BR_DECREFS) {
					rp += 16;
					fprintf(stderr, "fake-sm: ref cmd %08x\n", cmd);
				} else {
					fprintf(stderr, "fake-sm: BR cmd %08x\n", cmd);
				}
			}
			if (pending_reply && !(last_td.flags & 0x01)) { /* not oneway */
				/* Status::ok, then a NULL flat_binder_object (type 0).
				 * The null object is the difference between "ok" and
				 * "ok, and the @nullable IBinder you asked for does
				 * not exist": getService() callers (netmgrd waiting
				 * on Android's netd) parse the reply parcel with
				 * readStrongBinder() — a 4-byte reply leaves them a
				 * wild sp<> and they SIGSEGV on first use (observed
				 * 2026-08-28, netmgrd NetmgrNetdClientInit). With
				 * the null object they take the clean not-found path.
				 * Callers that expect no binder (addService, ping)
				 * ignore the trailing bytes. */
				static int32_t okreply[7]; /* Status::ok + 24 B object */
				struct btd rb; memset(&rb, 0, sizeof rb);
				rb.code = 0; rb.flags = 0x10; /* TF_ACCEPTS_FDS */
				rb.dsize = sizeof okreply; rb.osize = 0;
				rb.data.p.buf = (uint64_t)(uintptr_t)okreply;
				rb.data.p.ofs = 0;
				woff = wpush_u32(BC_REPLY, woff);
				woff = wpush_mem(&rb, sizeof rb, woff);
				if (last_td.data.p.buf && nfree < 8)
					free_bufs[nfree++] = last_td.data.p.buf;
			}
			for (int k = 0; k < nfree; k++) {
				woff = wpush_u32(BC_FREE_BUFFER, woff);
				woff = wpush_mem(&free_bufs[k], 8, woff);
			}
			if (woff) {
				struct bwr w = { .ws = woff, .wb = (uint64_t)(uintptr_t)wbuf, .rs = 0, .rb = 0 };
				if (ioctl(fd, IOC_WRITE_READ, &w)) perror("bwr-write");
			}
		}
	}
	/* only parent reaches here after forking all devices */
	for (;;) pause();
	return 0;
}
