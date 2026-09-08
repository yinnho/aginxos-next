/* snd-play — play raw S16_LE PCM on an ALSA playback device, no alsa-lib.
 *
 * M18 "speak" (说): the agent's mouth. Same HW_REFINE→HW_PARAMS idiom
 * as snd-cap, then WRITEI_FRAMES the file through. Paired defaults
 * (16000 Hz mono) so `snd-cap D 5 a.pcm && snd-play P a.pcm` round-trips.
 *
 * usage: snd-play <pcm-dev> <infile> [rate] [chans] [vol]
 *   vol 0..100 scales samples before writing (integer math, no mixer
 *   dependency — the q6 front-end may not expose one). Raw S16_LE in.
 * exit: 0 played, 1 usage, 2 open failed, 3 hw/sw params failed, 4 IO.
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
	if (argc < 3) {
		fprintf(stderr,
			"usage: snd-play <pcm-dev> <infile> [rate] [chans] [vol]\n");
		return 1;
	}
	const char *dev = argv[1];
	const char *in = argv[2];
	int rate = argc > 3 ? atoi(argv[3]) : 16000;
	int ch = argc > 4 ? atoi(argv[4]) : 1;
	int vol = argc > 5 ? atoi(argv[5]) : 100;
	if (rate <= 0 || ch <= 0 || vol < 0) { fprintf(stderr, "bad args\n"); return 1; }

	int fd = open(dev, O_WRONLY | O_NONBLOCK);
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
	/* Same lesson as snd-cap (2026-08-31): pinning period/buffer to
	 * rate/4 · rate*4 makes the q6 front-end reject HW_PARAMS with
	 * EINVAL — the cDSP only accepts its own default quantum. Pin
	 * format/rate/ch only, bound period/buffer loosely. */
	iv(&hp, P_PERIOD_SIZE)->min = 16;
	iv(&hp, P_PERIOD_SIZE)->max = rate / 2;
	iv(&hp, P_BUFFER_SIZE)->min = rate;
	iv(&hp, P_BUFFER_SIZE)->max = rate * 4;
	hp.rmask = (1u << P_ACCESS) | (1u << P_FORMAT) | (1u << P_CHANNELS) |
		   (1u << P_RATE) | (1u << P_PERIOD_SIZE) | (1u << P_BUFFER_SIZE);
	if (ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &hp) < 0) {
		perror("HW_PARAMS"); dump_caps(fd); return 3;
	}
	dbg_state(dev, "after hw_params");

	struct snd_pcm_sw_params sp;
	memset(&sp, 0, sizeof sp);
	sp.avail_min = rate / 4;
	sp.xfer_align = 1;		/* obsolete but old kernels validate it */
	sp.start_threshold = rate / 4;	/* first period kicks off playback */
	sp.stop_threshold = (uint64_t)rate * 4;
	if (ioctl(fd, SNDRV_PCM_IOCTL_SW_PARAMS, &sp) < 0) {
		perror("SW_PARAMS"); return 3;
	}
	if (ioctl(fd, SNDRV_PCM_IOCTL_PREPARE) < 0) {
		perror("PREPARE"); dbg_state(dev, "after prepare-fail"); return 3;
	}

	FILE *f = strcmp(in, "-") ? fopen(in, "rb") : stdin;
	if (!f) { perror(in); return 1; }

	const int frames = rate / 4;	/* one period per write */
	int16_t *buf = malloc((size_t)frames * ch * 2);
	long played = 0;
	for (;;) {
		size_t n = fread(buf, 2 * (size_t)ch, (size_t)frames, f);
		if (n == 0) break;
		if (vol != 100)
			for (size_t i = 0; i < n * (size_t)ch; i++) {
				int32_t s = buf[i] * vol / 100;
				if (s > 32767) s = 32767;
				if (s < -32768) s = -32768;
				buf[i] = (int16_t)s;
			}
		struct snd_xferi x = { 0, buf, (uint32_t)n };
		if (ioctl(fd, SNDRV_PCM_IOCTL_WRITEI_FRAMES, &x) < 0) {
			if (errno == EINTR) continue;
			perror("WRITEI"); fclose(f); return 4;
		}
		if (x.result <= 0) { fprintf(stderr, "xfer result %ld\n", x.result); break; }
		played += x.result;
	}
	fclose(f);
	/* let the tail drain before close (alsa drops on close otherwise) */
	ioctl(fd, SNDRV_PCM_IOCTL_DRAIN, 0);
	fprintf(stderr, "played %ld frames (%.1f s, %d Hz, %d ch) from %s\n",
		played, (double)played / rate, rate, ch, in);
	close(fd);
	return 0;
}
