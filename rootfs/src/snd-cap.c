/* snd-cap — capture raw PCM from an ALSA capture device, no alsa-lib.
 *
 * M18 "listen" (听): the agent's microphone path. Opens a PCM capture
 * device, negotiates S16_LE / rate / channels via HW_REFINE→HW_PARAMS
 * (a mini alsa-lib: refine everything first, then pin the params we
 * care about), and READI_FRAMES-loops for N seconds into a file.
 *
 * usage: snd-cap <pcm-dev> <secs> <outfile> [rate] [chans]
 *   pcm-dev e.g. /dev/snd/pcmC0D0c; rate default 16000 (speech/ASR
 *   friendly), chans default 1. Raw interleaved S16_LE output.
 * exit: 0 wrote the file, 1 usage, 2 open failed, 3 hw/sw params failed,
 *   4 capture IO error.
 *
 * The hw_params layout must match uapi sound/asound.h exactly — that is
 * the whole contract; the kernel reads masks/intervals in place.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <stdint.h>

#include "snd-pcm-uapi.h"

int main(int argc, char **argv)
{
	if (argc < 4) {
		fprintf(stderr,
			"usage: snd-cap <pcm-dev> <secs> <outfile> [rate] [chans]\n");
		return 1;
	}
	const char *dev = argv[1];
	int secs = atoi(argv[2]);
	const char *out = argv[3];
	int rate = argc > 4 ? atoi(argv[4]) : 16000;
	int ch = argc > 5 ? atoi(argv[5]) : 1;
	if (secs <= 0 || rate <= 0 || ch <= 0) { fprintf(stderr, "bad args\n"); return 1; }

	/* O_NONBLOCK first: opening a busy capture device blocks forever
	 * without it; we then return the fd to blocking mode for the
	 * readi loop (like snd_pcm_readi semantics). */
	int fd = open(dev, O_RDONLY | O_NONBLOCK);
	if (fd < 0) { perror(dev); return 2; }
	int fl = fcntl(fd, F_GETFL);
	if (fl >= 0) fcntl(fd, F_SETFL, fl & ~O_NONBLOCK);

	struct snd_pcm_hw_params hp;
	memset(&hp, 0, sizeof hp);
	for (int i = 0; i < 3; i++) memset(&hp.masks[i], 0xff, sizeof hp.masks[i]);
	for (int i = 0; i < 12; i++)
		hp.intervals[i].min = 0, hp.intervals[i].max = UINT32_MAX;
	hp.rmask = ~0u;
	if (ioctl(fd, SNDRV_PCM_IOCTL_HW_REFINE, &hp) < 0) {
		perror("HW_REFINE"); return 3;
	}
	mask_one(&hp.masks[P_ACCESS], SNDRV_PCM_ACCESS_RW_INTERLEAVED);
	mask_one(&hp.masks[P_FORMAT], SNDRV_PCM_FORMAT_S16_LE);
	iv(&hp, P_CHANNELS)->min = iv(&hp, P_CHANNELS)->max = ch;
	iv(&hp, P_RATE)->min = iv(&hp, P_RATE)->max = rate;
	/* 2026-08-31: pinning period/buffer (rate/4, rate*4) made q6asm RUN
	 * return ADSP_EFAILED on the MultiMedia FE — the cDSP only accepts
	 * its own default quantum. Pin format/rate/ch only; for geometry take
	 * the bounds HW_REFINE just reported (clamped to sane ranges) so
	 * restrictive FEs (hostless "Primary TDM TX 0" allows only period
	 * ≤1024, buffer ≤4096) also accept HW_PARAMS. */
	iv(&hp, P_PERIOD_SIZE)->min = iv(&hp, P_PERIOD_SIZE)->min > 16 ?
		iv(&hp, P_PERIOD_SIZE)->min : 16;
	iv(&hp, P_PERIOD_SIZE)->max = iv(&hp, P_PERIOD_SIZE)->max < (uint32_t)rate / 2 ?
		iv(&hp, P_PERIOD_SIZE)->max : (uint32_t)rate / 2;
	iv(&hp, P_BUFFER_SIZE)->min = iv(&hp, P_BUFFER_SIZE)->min > 128 ?
		iv(&hp, P_BUFFER_SIZE)->min : 128;
	iv(&hp, P_BUFFER_SIZE)->max = iv(&hp, P_BUFFER_SIZE)->max < (uint32_t)rate * 4 ?
		iv(&hp, P_BUFFER_SIZE)->max : (uint32_t)rate * 4;
	hp.rmask = (1u << P_ACCESS) | (1u << P_FORMAT) | (1u << P_CHANNELS) |
		   (1u << P_RATE) | (1u << P_PERIOD_SIZE) | (1u << P_BUFFER_SIZE);
	if (ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &hp) < 0) {
		perror("HW_PARAMS"); dump_caps(fd); return 3;
	}
	/* kernel writes the chosen geometry back into hp — read period_size
	 * from it (2026-08-31: reading a fixed 1024 while the q6 FE picks a
	 * 2000-frame period drains only half the stream: one READI wakes per
	 * period, so consumption runs at 1024/period and the buffer overruns
	 * into an XRUN a few seconds in). */
	int period = iv(&hp, P_PERIOD_SIZE)->min;
	if (period < 16 || period > rate)
		period = 1024;
	fprintf(stderr, "hw_params ok (period %d)\n", period);
	dbg_state(dev, "after hw_params");

	struct snd_pcm_sw_params sp;
	memset(&sp, 0, sizeof sp);
	sp.avail_min = 1;
	sp.xfer_align = 1;		/* obsolete but old kernels validate it */
	sp.start_threshold = 1;		/* start on first frame */
	/* capture must never self-stop on overrun: a stop_threshold of
	 * rate*4 made the kernel XRUN the stream once unread frames hit it
	 * (EPIPE + aDSP session flush). ~INT64_MAX = ALSA "boundary". */
	sp.stop_threshold = (uint64_t)1 << 62;
	if (ioctl(fd, SNDRV_PCM_IOCTL_SW_PARAMS, &sp) < 0) {
		perror("SW_PARAMS"); return 3;
	}
	dbg_state(dev, "after sw_params");
	if (ioctl(fd, SNDRV_PCM_IOCTL_PREPARE) < 0) {
		perror("PREPARE"); dbg_state(dev, "after prepare-fail"); return 3;
	}
	dbg_state(dev, "after prepare");

	FILE *f = strcmp(out, "-") ? fopen(out, "wb") : stdout;
	if (!f) { perror(out); return 1; }

	const int frames = period;	/* one period per READI — keep pace */
	int16_t *buf = malloc((size_t)frames * ch * 2);
	long total = (long)rate * secs, got = 0;
	while (got < total) {
		struct snd_xferi x = { 0, buf, (uint32_t)frames };
		if (ioctl(fd, SNDRV_PCM_IOCTL_READI_FRAMES, &x) < 0) {
			if (errno == EINTR) continue;
			perror("READI"); fclose(f); return 4;
		}
		if (x.result <= 0) { fprintf(stderr, "xfer result %ld\n", x.result); break; }
		fwrite(buf, 2 * (size_t)ch, (size_t)x.result, f);
		got += x.result;
	}
	if (f != stdout) fclose(f);
	fprintf(stderr, "captured %ld frames (%.1f s, %d Hz, %d ch, S16_LE) -> %s\n",
		got, (double)got / rate, rate, ch, out);
	close(fd);
	return 0;
}
