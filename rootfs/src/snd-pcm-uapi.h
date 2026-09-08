/* snd-pcm-uapi.h — the uapi sound/asound.h subset our PCM tools use,
 * inlined so snd-cap/snd-play stay freestanding. Copied verbatim from
 * bionic's auto-generated uapi header (NDK sysroot sound/asound.h),
 * which every Android userland compiles against — same layout the
 * redfin kernel's ioctl numbers encode. The _Static_asserts freeze
 * the ABI: if a field ever shifts, the build breaks instead of the
 * device session.
 */
#ifndef SND_PCM_UAPI_H
#define SND_PCM_UAPI_H

#include <sys/ioctl.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>

struct snd_mask { uint32_t bits[8]; };		/* 256-bit bitmap */
struct snd_interval { uint32_t min, max; uint32_t openmin_int; };
	/* uapi: min, max, then one bitfield word (openmin/openmax/
	 * integer/empty). We only ever need it zeroed = no flags set. */
struct snd_pcm_hw_params {
	uint32_t flags;
	struct snd_mask masks[3];		/* ids 0..2: ACCESS FORMAT SUBFORMAT */
	struct snd_mask mres[5];		/* kernel-private */
	struct snd_interval intervals[12];	/* ids 8..19: SAMPLE_BITS..TICK_TIME */
	struct snd_interval ires[9];		/* kernel-private */
	uint32_t rmask, cmask, info, msbits, rate_num, rate_den;
	uint64_t fifo_size;
	uint8_t reserved[64];
};
struct snd_pcm_sw_params {
	int32_t tstamp_mode;
	uint32_t period_step;
	uint32_t sleep_min;			/* long-obsolete, must be 0 */
	uint64_t avail_min;
	uint64_t xfer_align;			/* obsolete, must be 1 */
	uint64_t start_threshold;
	uint64_t stop_threshold;
	uint64_t silence_threshold;
	uint64_t silence_size;
	uint64_t boundary;			/* kernel writes this back */
	uint32_t proto;				/* ≥6.12 only; 4.19: reserved */
	uint32_t tstamp_type;
	uint8_t reserved[56];
};
struct snd_xferi {
	int64_t result;				/* snd_pcm_sframes_t = long */
	void *buf;
	uint64_t frames;
};

_Static_assert(sizeof(struct snd_interval) == 12, "interval ABI");
_Static_assert(sizeof(struct snd_pcm_hw_params) == 608, "hw_params ABI");
_Static_assert(sizeof(struct snd_pcm_sw_params) == 136, "sw_params ABI");
_Static_assert(sizeof(struct snd_xferi) == 24, "xferi ABI");

#define SNDRV_PCM_IOCTL_HW_REFINE	_IOWR('A', 0x10, struct snd_pcm_hw_params)
#define SNDRV_PCM_IOCTL_HW_PARAMS	_IOWR('A', 0x11, struct snd_pcm_hw_params)
#define SNDRV_PCM_IOCTL_SW_PARAMS	_IOWR('A', 0x13, struct snd_pcm_sw_params)
/* 2026-08-31: these three were previously wrong — PREPARE carried 0x22
 * (HWSYNC's slot: kernel uapi has HWSYNC=_IO('A',0x22), PREPARE=_IO('A',
 * 0x40)) so every "PREPARE" silently did HWSYNC, which returns -EBADFD
 * for a SETUP-state stream. READI/WRITEI also had compat-range nrs
 * (0x32/0x33) and a wrong READI direction. Numbers now match uapi
 * sound/asound.h exactly; verified against the device kernel's ioctl
 * dispatch jump table (snd_pcm_common_ioctl). */
#define SNDRV_PCM_IOCTL_PREPARE		_IO('A', 0x40)
#define SNDRV_PCM_IOCTL_DRAIN		_IO('A', 0x44)
#define SNDRV_PCM_IOCTL_READI_FRAMES	_IOR('A', 0x51, struct snd_xferi)
#define SNDRV_PCM_IOCTL_WRITEI_FRAMES	_IOW('A', 0x50, struct snd_xferi)

#if defined(__linux__)
/* canonical ioctl numbers — also freezes the dir/size encoding, which
 * the struct-size asserts above cannot see. (Skipped on BSD/macOS hosts:
 * their _IOW encodes direction with the opposite polarity.) */
_Static_assert(SNDRV_PCM_IOCTL_HW_REFINE == 0xc2604110, "ioctl ABI");
_Static_assert(SNDRV_PCM_IOCTL_HW_PARAMS == 0xc2604111, "ioctl ABI");
_Static_assert(SNDRV_PCM_IOCTL_SW_PARAMS == 0xc0884113, "ioctl ABI");
_Static_assert(SNDRV_PCM_IOCTL_PREPARE == 0x4140, "ioctl ABI");
_Static_assert(SNDRV_PCM_IOCTL_DRAIN == 0x4144, "ioctl ABI");
_Static_assert(SNDRV_PCM_IOCTL_READI_FRAMES == 0x80184151, "ioctl ABI");
_Static_assert(SNDRV_PCM_IOCTL_WRITEI_FRAMES == 0x40184150, "ioctl ABI");
#endif

/* hw param ids (uapi), split mask vs interval */
#define P_ACCESS	0
#define P_FORMAT	1
#define P_CHANNELS	10
#define P_RATE		11
#define P_PERIOD_SIZE	13
#define P_BUFFER_SIZE	17

#define SNDRV_PCM_ACCESS_RW_INTERLEAVED	3
#define SNDRV_PCM_FORMAT_S16_LE		2

static inline struct snd_interval *iv(struct snd_pcm_hw_params *p, int id)
{
	return &p->intervals[id - 8];	/* interval ids run 8..19 */
}

static inline void mask_one(struct snd_mask *m, unsigned bit)
{
	memset(m, 0, sizeof *m);
	m->bits[bit / 32] |= 1u << (bit % 32);
}

/* dbg_state — cat /proc/asound/card0/.../sub0/status after each ioctl step so a
 * state flip between HW_PARAMS/SW_PARAMS/PREPARE (async DSP event, SSR,
 * hw_free unwind) shows up as data instead of a bare EBADFD. dev name
 * e.g. "/dev/snd/pcmC0D0c" -> proc dir "pcm0c" (card0, device 0, capture;
 * proc uses pcm<dev><c|p>, not the devfs name). */
static inline void dbg_state(const char *dev, const char *tag)
{
	char path[128], buf[256];
	const char *d = dev + strlen("/dev/snd/pcmC0D");	/* -> "0c" */
	int card = 0;
	snprintf(path, sizeof path, "/proc/asound/card%d/pcm%c%c/sub0/status",
		 card, d[0], d[1]);
	FILE *f = fopen(path, "r");
	if (!f) return;
	int n = fread(buf, 1, sizeof buf - 1, f);
	fclose(f);
	if (n <= 0) return;
	buf[n] = 0;
	while (n > 0 && (buf[n-1] == '\n' || buf[n-1] == ' ')) buf[--n] = 0;
	fprintf(stderr, "[%s] %s\n", tag, buf);
}

/* dump_caps — on HW_PARAMS failure, re-refine a fresh params struct and
 * print what the device actually supports, so a failed pin (e.g. no
 * S16_LE, or only 48 kHz) reads as data instead of a bare EINVAL. */
static inline void dump_caps(int fd)
{
	static const struct { unsigned id; const char *name; } fmts[] = {
		{0, "S8"}, {1, "U8"}, {2, "S16_LE"}, {3, "S16_BE"},
		{6, "S24_LE"}, {10, "S32_LE"}, {11, "S24_3LE"},
	};
	struct snd_pcm_hw_params p;
	memset(&p, 0, sizeof p);
	for (int i = 0; i < 3; i++) memset(&p.masks[i], 0xff, sizeof p.masks[i]);
	for (int i = 0; i < 12; i++)
		p.intervals[i].min = 0, p.intervals[i].max = UINT32_MAX;
	p.rmask = ~0u;
	if (ioctl(fd, SNDRV_PCM_IOCTL_HW_REFINE, &p) < 0) {
		fprintf(stderr, "refine-for-dump: %s\n", strerror(errno));
		return;
	}
	fprintf(stderr, "device supports:");
	for (size_t i = 0; i < sizeof fmts / sizeof fmts[0]; i++)
		if (p.masks[P_FORMAT].bits[fmts[i].id / 32] & (1u << (fmts[i].id % 32)))
			fprintf(stderr, " %s", fmts[i].name);
	fprintf(stderr, " | rate %u..%u | ch %u..%u | period %u..%u | buf %u..%u\n",
		iv(&p, P_RATE)->min, iv(&p, P_RATE)->max,
		iv(&p, P_CHANNELS)->min, iv(&p, P_CHANNELS)->max,
		iv(&p, P_PERIOD_SIZE)->min, iv(&p, P_PERIOD_SIZE)->max,
		iv(&p, P_BUFFER_SIZE)->min, iv(&p, P_BUFFER_SIZE)->max);
}

#endif
