/* camss-shot — link qcom-camss RDI pipeline and grab one RAW frame.
 * usage: camss-shot [--view] [imx519|imx371|imx376] [outfile]
 *
 * --view: keep STREAMON, publish RGB565 preview (RGW1 /run/aginx-voice/eye.raw),
 *         read /run/aginx-cam/focus (0..2047) and snap via /run/aginx-cam/cmd.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/media.h>
#include <linux/v4l2-subdev.h>
#include <linux/videodev2.h>
#include <math.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int xioctl(int fd, unsigned long req, void *arg)
{
	int r;
	do {
		r = ioctl(fd, req, arg);
	} while (r < 0 && errno == EINTR);
	return r;
}

static int open_by_devnum(unsigned major, unsigned minor, int flags)
{
	char path[128], buf[32];
	for (int i = 0; i < 64; i++) {
		snprintf(path, sizeof(path), "/sys/dev/char/%u:%u", major, minor);
		/* fallback via video4linux */
		(void)path;
	}
	for (int i = 0; i < 64; i++) {
		int n;
		for (n = 0; n < 2; n++) {
			if (n == 0)
				snprintf(path, sizeof(path),
					 "/sys/class/video4linux/v4l-subdev%d/dev", i);
			else
				snprintf(path, sizeof(path),
					 "/sys/class/video4linux/video%d/dev", i);
			int f = open(path, O_RDONLY);
			if (f < 0)
				continue;
			int r = read(f, buf, sizeof(buf) - 1);
			close(f);
			if (r <= 0)
				continue;
			buf[r] = 0;
			unsigned ma = 0, mi = 0;
			if (sscanf(buf, "%u:%u", &ma, &mi) != 2)
				continue;
			if (ma != major || mi != minor)
				continue;
			if (n == 0)
				snprintf(path, sizeof(path), "/dev/v4l-subdev%d", i);
			else
				snprintf(path, sizeof(path), "/dev/video%d", i);
			int fd = open(path, flags);
			if (fd >= 0) {
				fprintf(stderr, "open %s (%u:%u)\n", path, major, minor);
				return fd;
			}
		}
	}
	return -1;
}

struct entity {
	struct media_entity_desc desc;
	int fd;
};

static int find_ent(struct entity *ents, int n, const char *want)
{
	for (int i = 0; i < n; i++)
		if (strcmp(ents[i].desc.name, want) == 0)
			return i;
	return -1;
}

static int setup_link(int mfd, uint32_t src_ent, uint16_t src_pad,
		      uint32_t snk_ent, uint16_t snk_pad, int enable)
{
	struct media_link_desc ld;
	memset(&ld, 0, sizeof(ld));
	ld.source.entity = src_ent;
	ld.source.index = src_pad;
	ld.source.flags = MEDIA_PAD_FL_SOURCE;
	ld.sink.entity = snk_ent;
	ld.sink.index = snk_pad;
	ld.sink.flags = MEDIA_PAD_FL_SINK;
	ld.flags = enable ? MEDIA_LNK_FL_ENABLED : 0;
	if (xioctl(mfd, MEDIA_IOC_SETUP_LINK, &ld) < 0) {
		fprintf(stderr, "SETUP_LINK %u:%u -> %u:%u en=%d: %s\n",
			src_ent, src_pad, snk_ent, snk_pad, enable,
			strerror(errno));
		return -1;
	}
	fprintf(stderr, "link %u:%u -> %u:%u %s\n", src_ent, src_pad, snk_ent,
		snk_pad, enable ? "ON" : "OFF");
	return 0;
}

static int set_fmt(int fd, unsigned pad, uint32_t code, uint32_t w, uint32_t h)
{
	struct v4l2_subdev_format fmt;
	memset(&fmt, 0, sizeof(fmt));
	fmt.which = V4L2_SUBDEV_FORMAT_ACTIVE;
	fmt.pad = pad;
	fmt.format.width = w;
	fmt.format.height = h;
	fmt.format.code = code;
	fmt.format.field = V4L2_FIELD_NONE;
	if (xioctl(fd, VIDIOC_SUBDEV_S_FMT, &fmt) < 0) {
		fprintf(stderr, "S_FMT pad%u %ux%u code=0x%x: %s\n", pad, w, h,
			code, strerror(errno));
		return -1;
	}
	fprintf(stderr, "S_FMT pad%u -> %ux%u code=0x%x\n", pad,
		fmt.format.width, fmt.format.height, fmt.format.code);
	return 0;
}

static int get_fmt(int fd, unsigned pad, struct v4l2_mbus_framefmt *out)
{
	struct v4l2_subdev_format fmt;
	memset(&fmt, 0, sizeof(fmt));
	fmt.which = V4L2_SUBDEV_FORMAT_ACTIVE;
	fmt.pad = pad;
	if (xioctl(fd, VIDIOC_SUBDEV_G_FMT, &fmt) < 0) {
		fprintf(stderr, "G_FMT pad%u: %s\n", pad, strerror(errno));
		return -1;
	}
	*out = fmt.format;
	fprintf(stderr, "G_FMT pad%u %ux%u code=0x%x field=%u\n", pad,
		out->width, out->height, out->code, out->field);
	return 0;
}

static uint32_t mbus_to_pix(uint32_t code)
{
	switch (code) {
	case MEDIA_BUS_FMT_SBGGR10_1X10:
		return V4L2_PIX_FMT_SBGGR10P;
	case MEDIA_BUS_FMT_SGBRG10_1X10:
		return V4L2_PIX_FMT_SGBRG10P;
	case MEDIA_BUS_FMT_SGRBG10_1X10:
		return V4L2_PIX_FMT_SGRBG10P;
	case MEDIA_BUS_FMT_SRGGB10_1X10:
		return V4L2_PIX_FMT_SRGGB10P;
	case MEDIA_BUS_FMT_SBGGR8_1X8:
		return V4L2_PIX_FMT_SBGGR8;
	default:
		return V4L2_PIX_FMT_SRGGB10P;
	}
}

static int g_ctrl(int fd, unsigned id, int *val)
{
	struct v4l2_control c;
	memset(&c, 0, sizeof(c));
	c.id = id;
	if (xioctl(fd, VIDIOC_G_CTRL, &c) < 0)
		return -1;
	*val = c.value;
	return 0;
}

static int s_ctrl(int fd, unsigned id, int val)
{
	struct v4l2_control c;
	memset(&c, 0, sizeof(c));
	c.id = id;
	c.value = val;
	if (xioctl(fd, VIDIOC_S_CTRL, &c) < 0) {
		fprintf(stderr, "S_CTRL 0x%x=%d: %s\n", id, val, strerror(errno));
		return -1;
	}
	fprintf(stderr, "S_CTRL 0x%x -> %d\n", id, c.value);
	return 0;
}

static int query_range(int fd, unsigned id, int *min, int *max, int *def)
{
	struct v4l2_queryctrl q;
	memset(&q, 0, sizeof(q));
	q.id = id;
	if (xioctl(fd, VIDIOC_QUERYCTRL, &q) < 0)
		return -1;
	if (min) *min = q.minimum;
	if (max) *max = q.maximum;
	if (def) *def = q.default_value;
	fprintf(stderr, "CTRL 0x%x %s min=%d max=%d def=%d\n",
		id, q.name, q.minimum, q.maximum, q.default_value);
	return 0;
}

static int frame_mean8(const uint8_t *p, size_t n)
{
	unsigned long long s = 0;
	size_t c = 0;
	for (size_t i = 0; i + 4 < n; i += 40) {
		s += (unsigned)p[i] + p[i + 1] + p[i + 2] + p[i + 3];
		c += 4;
	}
	return c ? (int)(s / c) : 0;
}

/* Center-50% mean |gx|+|gy| on RAW10 packed (byte i of each 5-byte group
 * = pixel i bits[9:2]). Same metric as redfin cam-shot frame_sharp
 * (2026-09-07): walking the 5th carry byte diluted a 22% focus swing to ~1%. */
static double frame_sharp(const uint8_t *map, int w, int h, int stride)
{
	if (!map || w < 8 || h < 8 || stride < (w / 4) * 5)
		return 0.0;
	int x0 = w * 3 / 8, x1 = w * 5 / 8, y0 = h * 3 / 8, y1 = h * 5 / 8;
	int cw = x1 - x0;
	uint8_t *row0 = malloc((size_t)cw * 2);
	if (!row0)
		return 0.0;
	uint8_t *row1 = row0 + cw;
	double acc = 0.0;
	unsigned long long n = 0;
	int cur = 0;
	for (int y = y0; y < y1; y++) {
		const uint8_t *src = map + (size_t)y * (size_t)stride;
		uint8_t *dst = cur ? row1 : row0;
		for (int x = x0; x < x1; x++)
			dst[x - x0] = src[(x >> 2) * 5 + (x & 3)];
		if (y > y0) {
			const uint8_t *p = cur ? row0 : row1;
			for (int x = 1; x < cw; x++) {
				int gx = (int)p[x] - (int)p[x - 1];
				int gy = (int)dst[x] - (int)p[x];
				acc += (double)(gx < 0 ? -gx : gx)
				     + (double)(gy < 0 ? -gy : gy);
				n++;
			}
		}
		cur ^= 1;
	}
	free(row0);
	return n ? acc / (double)n : 0.0;
}

#define AF_NCOARSE 8
#define AF_NFINE 5
/* 2 mmap buffers ⇒ 2 in-flight frames. SKIP=2 only drained those, so
 * measure(code i) was still code i-1/i-2: coarse 2047 scored 143 then
 * fine 2047 scored 89 and we kept 2047 (2026-09-17 cam-view.log). */
#define AF_SKIP 4

#define CAM_FOCUS_PATH "/run/aginx-cam/focus"
#define CAM_CMD_PATH "/run/aginx-cam/cmd"
#define CAM_EYE_RAW "/run/aginx-voice/eye.raw"
#define CAM_EYE_JPG "/run/aginx-voice/eye.jpg"

static int dq_one(int vfd, uint32_t btype, int mplane,
		  struct v4l2_buffer *buf, struct v4l2_plane *planes);
static int publish_preview(const uint8_t *raw, int w, int h, int stride, int cfa,
			   int rot);

static int requeue_all(int vfd, uint32_t btype, int mplane, unsigned nbuf)
{
	for (unsigned i = 0; i < nbuf; i++) {
		struct v4l2_buffer b;
		struct v4l2_plane p[VIDEO_MAX_PLANES];
		memset(&b, 0, sizeof(b));
		memset(p, 0, sizeof(p));
		b.type = btype;
		b.memory = V4L2_MEMORY_MMAP;
		b.index = i;
		if (mplane) {
			b.length = 1;
			b.m.planes = p;
		}
		if (xioctl(vfd, VIDIOC_QBUF, &b) < 0) {
			perror("QBUF requeue");
			return -1;
		}
	}
	return 0;
}

/* IMX519 CPHY: first STREAMON after boot often produces no frames;
 * a STREAMOFF/ON kick then DQBUF's (2026-09-17: first 20s timeout,
 * second shot seq=6). */
static int stream_kick(int vfd, uint32_t btype, int mplane, unsigned nbuf)
{
	fprintf(stderr, "stream kick\n");
	xioctl(vfd, VIDIOC_STREAMOFF, &btype);
	usleep(200000);
	if (requeue_all(vfd, btype, mplane, nbuf) < 0)
		return -1;
	if (xioctl(vfd, VIDIOC_STREAMON, &btype) < 0) {
		perror("STREAMON retry");
		return -1;
	}
	fprintf(stderr, "STREAMON retry ok\n");
	return 0;
}

static int drop_frames(int vfd, uint32_t btype, int mplane, int n,
		       struct v4l2_buffer *buf, struct v4l2_plane *planes)
{
	for (int i = 0; i < n; i++) {
		if (dq_one(vfd, btype, mplane, buf, planes) < 0)
			return -1;
		xioctl(vfd, VIDIOC_QBUF, buf);
	}
	return 0;
}

static int g_dq_ms;
static int g_view_cfa = -1, g_view_rot = 270;
static int g_view_w, g_view_h, g_view_stride;

static int measure_focus(int vfd, int act_fd, uint32_t btype, int mplane,
			 void **starts, int w, int h, int stride, int code,
			 struct v4l2_buffer *buf, struct v4l2_plane *planes,
			 double *sharp)
{
	int skip = g_dq_ms > 0 ? 2 : AF_SKIP;
	if (s_ctrl(act_fd, V4L2_CID_FOCUS_ABSOLUTE, code) < 0)
		return -1;
	if (drop_frames(vfd, btype, mplane, skip, buf, planes) < 0)
		return -1;
	if (dq_one(vfd, btype, mplane, buf, planes) < 0)
		return -1;
	*sharp = frame_sharp(starts[buf->index], w, h, stride);
	if (g_view_cfa >= 0)
		publish_preview(starts[buf->index], g_view_w, g_view_h,
				g_view_stride, g_view_cfa, g_view_rot);
	xioctl(vfd, VIDIOC_QBUF, buf);
	return 0;
}

/* 8-step coarse across the DAC then a 5-point window around the peak,
 * slid inside [fmin,fmax] so a rail peak does not re-measure the same
 * code (redfin 2026-09-10). Returns the landed code, or fdef on fail. */
static int contrast_af(int vfd, int act_fd, uint32_t btype, int mplane,
		       void **starts, int w, int h, int stride,
		       int fmin, int fmax, int fdef,
		       struct v4l2_buffer *buf, struct v4l2_plane *planes)
{
	int best = fdef;
	double bests = -1.0, infs = 0.0;
	int span = (fmax - fmin) / (AF_NCOARSE - 1);
	if (span < 1)
		span = 1;
	for (int i = 0; i < AF_NCOARSE; i++) {
		int code = fmin + (int)((long)(fmax - fmin) * i / (AF_NCOARSE - 1));
		double s = 0.0;
		if (measure_focus(vfd, act_fd, btype, mplane, starts, w, h,
				  stride, code, buf, planes, &s) < 0)
			goto fail;
		fprintf(stderr, "af: coarse %d code=%d sharp=%.2f%s\n",
			i, code, s, s > bests ? " <" : "");
		if (i == 0)
			infs = s;
		if (s > bests) {
			bests = s;
			best = code;
		}
	}
	int lo = best - span / 2;
	if (lo < fmin)
		lo = fmin;
	if (lo > fmax - span)
		lo = fmax - span;
	if (lo < fmin)
		lo = fmin;
	for (int i = 0; i < AF_NFINE; i++) {
		int code = lo + span * i / (AF_NFINE - 1);
		if (code > fmax)
			code = fmax;
		double s = 0.0;
		if (measure_focus(vfd, act_fd, btype, mplane, starts, w, h,
				  stride, code, buf, planes, &s) < 0)
			goto fail;
		fprintf(stderr, "af: fine   %d code=%d sharp=%.2f%s\n",
			i, code, s, s > bests ? " <" : "");
		if (s > bests) {
			bests = s;
			best = code;
		}
	}
	if (infs > 0.01 && bests < infs * 1.10) {
		best = fdef;
		fprintf(stderr, "af: peak +%.0f%% < 10%%, park inf %d\n",
			100.0 * (bests / infs - 1.0), fdef);
	}
	if (s_ctrl(act_fd, V4L2_CID_FOCUS_ABSOLUTE, best) < 0)
		goto fail;
	fprintf(stderr, "af: FOCUS code=%d sharp=%.2f (inf %.2f, %+.0f%%)\n",
		best, bests, infs,
		infs > 0.01 ? 100.0 * (bests / infs - 1.0) : 0.0);
	return best;
fail:
	fprintf(stderr, "af: scan aborted, parking %d\n", fdef);
	s_ctrl(act_fd, V4L2_CID_FOCUS_ABSOLUTE, fdef);
	return fdef;
}

/* Live AF: 8 coarse + 5 fine, SKIP=2 (one settle was in-flight and
 * graded noise as a peak — tap/open looked like no AF). Always land on
 * the peak; indoor is not infinity. */
static int live_af(int vfd, int act_fd, uint32_t btype, int mplane,
		   void **starts, int w, int h, int stride,
		   int fmin, int fmax, int fdef,
		   struct v4l2_buffer *buf, struct v4l2_plane *planes)
{
	int best = fdef;
	double bests = -1.0, infs = 0.0;
	int span = (fmax - fmin) / (AF_NCOARSE - 1);
	if (span < 1)
		span = 1;
	int ccodes[AF_NCOARSE];
	double csc[AF_NCOARSE];
	for (int i = 0; i < AF_NCOARSE; i++) {
		int code = fmin + (int)((long)(fmax - fmin) * i / (AF_NCOARSE - 1));
		double s = 0.0;
		if (measure_focus(vfd, act_fd, btype, mplane, starts, w, h,
				  stride, code, buf, planes, &s) < 0)
			return fdef;
		ccodes[i] = code;
		csc[i] = s;
		fprintf(stderr, "af: live coarse %d code=%d sharp=%.2f%s\n",
			i, code, s, s > bests ? " <" : "");
		if (i == 0)
			infs = s;
		if (s > bests) {
			bests = s;
			best = code;
		}
	}
	/* Rail peaks are often lagged or bokeh-edge. If 2047/0 wins by
	 * <20% vs the interior, use the interior max instead. */
	{
		double interior = 0.0;
		int icode = ccodes[AF_NCOARSE / 2];
		for (int i = 1; i < AF_NCOARSE - 1; i++) {
			if (csc[i] > interior) {
				interior = csc[i];
				icode = ccodes[i];
			}
		}
		if ((best == fmin || best == fmax) && infs > 0.01
		    && bests < interior * 1.20 && interior > infs * 0.9) {
			fprintf(stderr, "af: reject rail %d, interior %d\n",
				best, icode);
			best = icode;
			bests = interior;
		}
	}
	int lo = best - span;
	if (lo < fmin)
		lo = fmin;
	int hi = best + span;
	if (hi > fmax)
		hi = fmax;
	int fspan = hi - lo;
	if (fspan < 1)
		fspan = 1;
	bests = -1.0;
	for (int i = 0; i < 7; i++) {
		int code = lo + fspan * i / 6;
		double s = 0.0;
		if (measure_focus(vfd, act_fd, btype, mplane, starts, w, h,
				  stride, code, buf, planes, &s) < 0)
			break;
		fprintf(stderr, "af: live fine   %d code=%d sharp=%.2f%s\n",
			i, code, s, s > bests ? " <" : "");
		if (s > bests) {
			bests = s;
			best = code;
		}
	}
	/* Only steal back to inf when the coarse peak was not clearly
	 * better (lagged rail 2047). Indoor 877 vs inf 45 was a real peak
	 * that confirm-inf then overwrote (2026-09-17). */
	if (!(best != fmin && infs > 0.01 && bests >= infs * 1.12)) {
		double s0 = -1.0, sb = -1.0;
		if (measure_focus(vfd, act_fd, btype, mplane, starts, w, h,
				  stride, fmin, buf, planes, &s0) == 0)
			fprintf(stderr, "af: confirm inf %d sharp=%.2f\n", fmin, s0);
		if (best != fmin
		    && measure_focus(vfd, act_fd, btype, mplane, starts, w, h,
				     stride, best, buf, planes, &sb) == 0)
			fprintf(stderr, "af: confirm %d sharp=%.2f\n", best, sb);
		else
			sb = bests;
		if (s0 >= sb) {
			best = fmin;
			bests = s0;
		} else {
			bests = sb;
		}
	} else {
		fprintf(stderr, "af: keep peak %d (%.0f%% over inf)\n",
			best, 100.0 * (bests / infs - 1.0));
	}
	s_ctrl(act_fd, V4L2_CID_FOCUS_ABSOLUTE, best);
	if (drop_frames(vfd, btype, mplane, AF_SKIP, buf, planes) < 0)
		return best;
	char tmp[] = "/run/aginx-cam/focus.tmp";
	FILE *f = fopen(tmp, "w");
	if (f) {
		fprintf(f, "%d\n", best);
		fclose(f);
		rename(tmp, CAM_FOCUS_PATH);
	}
	fprintf(stderr, "af: live FOCUS code=%d sharp=%.2f\n", best, bests);
	return best;
}

static int dq_one(int vfd, uint32_t btype, int mplane,
		  struct v4l2_buffer *buf, struct v4l2_plane *planes)
{
	if (g_dq_ms > 0) {
		struct pollfd p = { .fd = vfd, .events = POLLIN };
		int r = poll(&p, 1, g_dq_ms);
		if (r == 0) {
			fprintf(stderr, "DQBUF timeout (%d ms)\n", g_dq_ms);
			return -1;
		}
		if (r < 0) {
			perror("poll DQBUF");
			return -1;
		}
	}
	memset(buf, 0, sizeof(*buf));
	memset(planes, 0, sizeof(struct v4l2_plane) * VIDEO_MAX_PLANES);
	buf->type = btype;
	buf->memory = V4L2_MEMORY_MMAP;
	if (mplane) {
		buf->length = 1;
		buf->m.planes = planes;
	}
	if (xioctl(vfd, VIDIOC_DQBUF, buf) < 0) {
		fprintf(stderr, "DQBUF: %s\n", strerror(errno));
		return -1;
	}
	return 0;
}

static volatile sig_atomic_t g_stop;

static void on_stop(int sig)
{
	(void)sig;
	g_stop = 1;
}

static uint8_t raw10_px(const uint8_t *raw, int stride, int x, int y)
{
	return raw[(size_t)y * (size_t)stride + (size_t)(x >> 2) * 5 + (size_t)(x & 3)];
}

static int atomic_write(const char *path, const void *data, size_t n)
{
	char tmp[128];
	snprintf(tmp, sizeof tmp, "%s.tmp", path);
	int fd = open(tmp, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	if (fd < 0)
		return -1;
	ssize_t w = write(fd, data, n);
	int rc = close(fd);
	if (w != (ssize_t)n || rc < 0) {
		unlink(tmp);
		return -1;
	}
	if (rename(tmp, path) < 0) {
		unlink(tmp);
		return -1;
	}
	return 0;
}

/* Gray-world + γ2.2, luma-preserving so lifting R/B does not wash the
 * frame white (user: 正常了，只是还有点偏白). Skip G<20 quads — same
 * floor gate as campix CP_WB_MIN_G. Mild contrast after gamma. */
static void preview_wb_gamma(uint8_t *rgb, int w, int h)
{
	size_t n = (size_t)w * (size_t)h;
	if (!n)
		return;
	unsigned long long sr = 0, sg = 0, sb = 0, cnt = 0;
	for (size_t i = 0; i < n; i++) {
		int g = rgb[i * 3 + 1];
		if (g < 20)
			continue;
		sr += rgb[i * 3];
		sg += (unsigned)g;
		sb += rgb[i * 3 + 2];
		cnt++;
	}
	double kr = 1.0, kg = 1.0, kb = 1.0;
	if (cnt > 0) {
		double mr = (double)sr / (double)cnt;
		double mg = (double)sg / (double)cnt;
		double mb = (double)sb / (double)cnt;
		if (mr > 1.0)
			kr = mg / mr;
		if (mb > 1.0)
			kb = mg / mb;
		if (kr > 4.0)
			kr = 4.0;
		if (kb > 4.0)
			kb = 4.0;
		double y0 = 0.299 * mr + 0.587 * mg + 0.114 * mb;
		double y1 = 0.299 * mr * kr + 0.587 * mg + 0.114 * mb * kb;
		if (y1 > 1.0) {
			double s = y0 / y1;
			kr *= s;
			kg *= s;
			kb *= s;
		}
	}
	static uint8_t lut[256];
	static int lut_ok;
	if (!lut_ok) {
		for (int v = 0; v < 256; v++) {
			double y = 255.0 * pow((double)v / 255.0, 1.0 / 2.2);
			int iv = (int)(y + 0.5);
			lut[v] = (uint8_t)(iv < 0 ? 0 : iv > 255 ? 255 : iv);
		}
		lut_ok = 1;
	}
	for (size_t i = 0; i < n; i++) {
		int r = (int)(rgb[i * 3] * kr);
		int g = (int)(rgb[i * 3 + 1] * kg);
		int b = (int)(rgb[i * 3 + 2] * kb);
		if (r > 255)
			r = 255;
		if (g > 255)
			g = 255;
		if (b > 255)
			b = 255;
		r = lut[r];
		g = lut[g];
		b = lut[b];
		r = (r - 128) * 6 / 5 + 128;
		g = (g - 128) * 6 / 5 + 128;
		b = (b - 128) * 6 / 5 + 128;
		if (r < 0)
			r = 0;
		if (r > 255)
			r = 255;
		if (g < 0)
			g = 0;
		if (g > 255)
			g = 255;
		if (b < 0)
			b = 0;
		if (b > 255)
			b = 255;
		rgb[i * 3] = (uint8_t)r;
		rgb[i * 3 + 1] = (uint8_t)g;
		rgb[i * 3 + 2] = (uint8_t)b;
	}
}

/* 1/4 Bayer 2×2 → WB/gamma → RGB565 rotate 90|270, RGW1. cfa: 0=RGGB 1=BGGR. */
static int publish_preview(const uint8_t *raw, int w, int h, int stride, int cfa,
			   int rot)
{
	const int step = (w >= 3200) ? 8 : 4;
	int dw = (w / step) & ~1, dh = (h / step) & ~1;
	if (dw < 2 || dh < 2)
		return -1;
	uint8_t *rgb = malloc((size_t)dw * (size_t)dh * 3);
	if (!rgb)
		return -1;
	for (int oy = 0; oy < dh; oy++) {
		int y = oy * step;
		if (y + 1 >= h)
			break;
		for (int ox = 0; ox < dw; ox++) {
			int x = ox * step;
			if (x + 1 >= w)
				break;
			int p00 = raw10_px(raw, stride, x, y);
			int p10 = raw10_px(raw, stride, x + 1, y);
			int p01 = raw10_px(raw, stride, x, y + 1);
			int p11 = raw10_px(raw, stride, x + 1, y + 1);
			int R, G, B;
			if (cfa == 0) { /* RGGB */
				R = p00;
				G = (p10 + p01) / 2;
				B = p11;
			} else { /* BGGR */
				B = p00;
				G = (p10 + p01) / 2;
				R = p11;
			}
			if (B < 16)
				B = 0;
			else
				B = (B - 16) * 255 / 239;
			if (G < 16)
				G = 0;
			else
				G = (G - 16) * 255 / 239;
			if (R < 16)
				R = 0;
			else
				R = (R - 16) * 255 / 239;
			uint8_t *p = rgb + ((size_t)oy * (size_t)dw + (size_t)ox) * 3;
			p[0] = (uint8_t)R;
			p[1] = (uint8_t)G;
			p[2] = (uint8_t)B;
		}
	}
	preview_wb_gamma(rgb, dw, dh);
	int ow = dh, oh = dw;
	size_t pixn = (size_t)ow * (size_t)oh;
	size_t bytes = 12 + pixn * 2;
	uint8_t *out = malloc(bytes);
	if (!out) {
		free(rgb);
		return -1;
	}
	uint32_t *hdr = (uint32_t *)out;
	hdr[0] = 0x31574752u;
	hdr[1] = (uint32_t)ow;
	hdr[2] = (uint32_t)oh;
	uint16_t *dst = (uint16_t *)(out + 12);
	for (int oy = 0; oy < dh; oy++) {
		for (int ox = 0; ox < dw; ox++) {
			const uint8_t *p = rgb + ((size_t)oy * (size_t)dw + (size_t)ox) * 3;
			int nx, ny;
			if (rot == 90) {
				nx = dh - 1 - oy;
				ny = ox;
			} else { /* 270 */
				nx = oy;
				ny = dw - 1 - ox;
			}
			dst[(size_t)ny * (size_t)ow + (size_t)nx] =
				(uint16_t)(((p[0] & 0xF8) << 8) | ((p[1] & 0xFC) << 3)
					   | (p[2] >> 3));
		}
	}
	free(rgb);
	int rc = atomic_write(CAM_EYE_RAW, out, bytes);
	free(out);
	return rc;
}

static int read_int_file(const char *path, int *out)
{
	FILE *f = fopen(path, "r");
	if (!f)
		return -1;
	int v, n = fscanf(f, "%d", &v);
	fclose(f);
	if (n != 1)
		return -1;
	*out = v;
	return 0;
}

/* 0=none 1=snap 2=af */
static int take_cmd(void)
{
	FILE *f = fopen(CAM_CMD_PATH, "r");
	if (!f)
		return 0;
	char b[32] = {0};
	if (!fgets(b, sizeof b, f)) {
		fclose(f);
		unlink(CAM_CMD_PATH);
		return 0;
	}
	fclose(f);
	unlink(CAM_CMD_PATH);
	if (!strncmp(b, "snap", 4))
		return 1;
	if (!strncmp(b, "af", 2))
		return 2;
	return 0;
}

static int copy_file(const char *src, const char *dst)
{
	int in = open(src, O_RDONLY);
	if (in < 0)
		return -1;
	char tmp[128];
	snprintf(tmp, sizeof tmp, "%s.tmp", dst);
	int out = open(tmp, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	if (out < 0) {
		close(in);
		return -1;
	}
	char buf[8192];
	ssize_t n;
	int ok = 1;
	while ((n = read(in, buf, sizeof buf)) > 0) {
		if (write(out, buf, (size_t)n) != n) {
			ok = 0;
			break;
		}
	}
	if (n < 0)
		ok = 0;
	close(in);
	if (close(out) < 0)
		ok = 0;
	if (!ok) {
		unlink(tmp);
		return -1;
	}
	if (rename(tmp, dst) < 0) {
		unlink(tmp);
		return -1;
	}
	return 0;
}

static int still_snap(const uint8_t *raw, size_t used, int w, int h, int stride,
		      const char *sensor)
{
	const char *rot = "0";
	const char *cfa = "bggr";
	if (!strcmp(sensor, "imx376"))
		rot = "270";
	else if (!strcmp(sensor, "imx519")) {
		rot = "90";
		cfa = "rggb";
	} else if (!strcmp(sensor, "imx371"))
		rot = "90";
	mkdir("/home/photos", 0755);
	char rawp[] = "/tmp/view-snap.raw";
	int fd = open(rawp, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	if (fd < 0)
		return -1;
	if (write(fd, raw, used) != (ssize_t)used) {
		close(fd);
		return -1;
	}
	close(fd);
	char archive[128];
	time_t now = time(NULL);
	struct tm tm;
	localtime_r(&now, &tm);
	strftime(archive, sizeof archive, "/home/photos/%Y%m%d-%H%M%S-", &tm);
	strncat(archive, sensor, sizeof archive - strlen(archive) - 5);
	strcat(archive, ".jpg");
	char wb[16], hb[16], sb[16];
	snprintf(wb, sizeof wb, "%d", w);
	snprintf(hb, sizeof hb, "%d", h);
	snprintf(sb, sizeof sb, "%d", stride);
	pid_t pid = fork();
	if (pid < 0)
		return -1;
	if (pid == 0) {
		execl("/usr/bin/raw2jpg", "raw2jpg", rawp, wb, hb, sb, "85",
		      "--color", "--cfa", cfa, "--rotate", rot, "--bl", "16",
		      "--wb", "--gamma", "2.2", "--out", archive, (char *)NULL);
		_exit(127);
	}
	int st = 0;
	waitpid(pid, &st, 0);
	unlink(rawp);
	if (!WIFEXITED(st) || WEXITSTATUS(st) != 0) {
		fprintf(stderr, "view: raw2jpg failed status=%d\n", st);
		return -1;
	}
	if (copy_file(archive, CAM_EYE_JPG) < 0)
		return -1;
	fprintf(stderr, "view: snap %s\n", archive);
	return 0;
}

static int view_loop(int vfd, int act_fd, int sensor_fd, uint32_t btype, int mplane,
		     void **starts, int w, int h, int stride, int fmin, int fmax,
		     int gmax, int emax, int dmax,
		     struct v4l2_buffer *buf, struct v4l2_plane *planes,
		     const char *sensor, unsigned nbuf)
{
	g_stop = 0;
	signal(SIGTERM, on_stop);
	signal(SIGINT, on_stop);
	signal(SIGPIPE, SIG_IGN);
	alarm(0);
	mkdir("/run/aginx-cam", 0755);
	mkdir("/run/aginx-voice", 0755);
	unlink(CAM_CMD_PATH);
	unlink(CAM_EYE_JPG);
	int last_f = 0;
	int did_af = 0;
	int prev = 0;
	int kicks = 0;
	fprintf(stderr, "view: live %dx%d stride=%d sensor=%s\n", w, h, stride,
		sensor);
	while (!g_stop) {
		if (dq_one(vfd, btype, mplane, buf, planes) < 0) {
			if (kicks < 3 && stream_kick(vfd, btype, mplane, nbuf) == 0) {
				kicks++;
				continue;
			}
			break;
		}
		kicks = 0;
		size_t used = mplane ? planes[0].bytesused : buf->bytesused;
		const uint8_t *raw = starts[buf->index];
		int fv = 0;
		if (act_fd >= 0 && read_int_file(CAM_FOCUS_PATH, &fv) == 0) {
			if (fv < fmin)
				fv = fmin;
			if (fv > fmax)
				fv = fmax;
			if (fv != last_f) {
				s_ctrl(act_fd, V4L2_CID_FOCUS_ABSOLUTE, fv);
				last_f = fv;
			}
		}
		int cmd = take_cmd();
		int mean8 = frame_mean8(raw, used);
		int want_af = act_fd >= 0 && fmax > fmin && cmd == 2;
		if (cmd == 1) {
			/* Still: unity digital gain, mid analog, max CIT.
			 * Then SKIP=4 contrast AF (view AF locked macro 1949
			 * on lagged rail + bokeh edges). */
			xioctl(vfd, VIDIOC_QBUF, buf);
			if (sensor_fd >= 0 && strcmp(sensor, "imx519") == 0) {
				int ag = gmax > 3 ? gmax / 4 : gmax;
				if (ag > 0)
					s_ctrl(sensor_fd, V4L2_CID_ANALOGUE_GAIN, ag);
				if (emax > 4)
					s_ctrl(sensor_fd, V4L2_CID_EXPOSURE, emax);
				s_ctrl(sensor_fd, V4L2_CID_DIGITAL_GAIN, 256);
				if (drop_frames(vfd, btype, mplane, 3, buf, planes) < 0)
					break;
			}
			if (act_fd >= 0 && fmax > fmin) {
				int save_ms = g_dq_ms;
				g_dq_ms = 0;
				g_view_cfa = -1;
				last_f = live_af(vfd, act_fd, btype, mplane, starts,
						 w, h, stride, fmin, fmax, last_f,
						 buf, planes);
				g_dq_ms = save_ms;
				if (drop_frames(vfd, btype, mplane, 3, buf, planes) < 0)
					break;
			}
			if (dq_one(vfd, btype, mplane, buf, planes) < 0)
				break;
			used = mplane ? planes[0].bytesused : buf->bytesused;
			raw = starts[buf->index];
			if (still_snap(raw, used, w, h, stride, sensor) < 0)
				fprintf(stderr, "view: snap failed\n");
			if (sensor_fd >= 0 && strcmp(sensor, "imx519") == 0) {
				if (gmax > 1)
					s_ctrl(sensor_fd, V4L2_CID_ANALOGUE_GAIN, gmax / 2);
				s_ctrl(sensor_fd, V4L2_CID_EXPOSURE,
				       emax < 800 ? emax : 800);
				s_ctrl(sensor_fd, V4L2_CID_DIGITAL_GAIN, 1024);
			}
		} else if (want_af) {
			g_view_cfa = strcmp(sensor, "imx519") == 0 ? 0 : 1;
			g_view_rot = strcmp(sensor, "imx519") == 0 ? 90 : 270;
			g_view_w = w;
			g_view_h = h;
			g_view_stride = stride;
			xioctl(vfd, VIDIOC_QBUF, buf);
			last_f = live_af(vfd, act_fd, btype, mplane, starts, w, h,
					 stride, fmin, fmax, last_f, buf, planes);
			g_view_cfa = -1;
			did_af = 1;
			continue;
		} else {
			publish_preview(raw, w, h, stride,
					strcmp(sensor, "imx519") == 0 ? 0 : 1,
					strcmp(sensor, "imx519") == 0 ? 90 : 270);
			prev++;
		}
		xioctl(vfd, VIDIOC_QBUF, buf);
	}
	fprintf(stderr, "view: stop\n");
	return 0;
}

static int capture(int vfd, int sensor_fd, int act_fd, const char *outpath,
		   uint32_t w, uint32_t h, uint32_t mbus, int view,
		   const char *sensor)
{
	struct v4l2_capability cap;
	memset(&cap, 0, sizeof(cap));
	if (xioctl(vfd, VIDIOC_QUERYCAP, &cap) < 0) {
		perror("QUERYCAP");
		return -1;
	}
	fprintf(stderr, "video driver=%s card=%s caps=0x%x dcaps=0x%x\n",
		cap.driver, cap.card, cap.capabilities, cap.device_caps);

	int mplane = !!(cap.device_caps & V4L2_CAP_VIDEO_CAPTURE_MPLANE);
	uint32_t btype = mplane ? V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
				: V4L2_BUF_TYPE_VIDEO_CAPTURE;
	uint32_t pixfmt = mbus_to_pix(mbus);
	fprintf(stderr, "mplane=%d type=%u pix=%c%c%c%c %ux%u\n", mplane, btype,
		pixfmt & 0xff, (pixfmt >> 8) & 0xff, (pixfmt >> 16) & 0xff,
		(pixfmt >> 24) & 0xff, w, h);

	struct v4l2_fmtdesc fdsc;
	for (uint32_t i = 0; i < 16; i++) {
		memset(&fdsc, 0, sizeof(fdsc));
		fdsc.index = i;
		fdsc.type = btype;
		if (xioctl(vfd, VIDIOC_ENUM_FMT, &fdsc) < 0)
			break;
		fprintf(stderr, "fmt %u %c%c%c%c %s\n", i,
			fdsc.pixelformat & 0xff, (fdsc.pixelformat >> 8) & 0xff,
			(fdsc.pixelformat >> 16) & 0xff,
			(fdsc.pixelformat >> 24) & 0xff, fdsc.description);
	}

	struct v4l2_format fmt;
	memset(&fmt, 0, sizeof(fmt));
	fmt.type = btype;
	if (mplane) {
		fmt.fmt.pix_mp.width = w;
		fmt.fmt.pix_mp.height = h;
		fmt.fmt.pix_mp.pixelformat = pixfmt;
		fmt.fmt.pix_mp.field = V4L2_FIELD_NONE;
		fmt.fmt.pix_mp.num_planes = 1;
	} else {
		fmt.fmt.pix.width = w;
		fmt.fmt.pix.height = h;
		fmt.fmt.pix.pixelformat = pixfmt;
		fmt.fmt.pix.field = V4L2_FIELD_NONE;
	}
	if (xioctl(vfd, VIDIOC_S_FMT, &fmt) < 0) {
		perror("S_FMT video");
		return -1;
	}
	int stride = 0;
	if (mplane)
		fprintf(stderr, "S_FMT video %ux%u fourcc=%c%c%c%c planes=%u size=%u\n",
			fmt.fmt.pix_mp.width, fmt.fmt.pix_mp.height,
			fmt.fmt.pix_mp.pixelformat & 0xff,
			(fmt.fmt.pix_mp.pixelformat >> 8) & 0xff,
			(fmt.fmt.pix_mp.pixelformat >> 16) & 0xff,
			(fmt.fmt.pix_mp.pixelformat >> 24) & 0xff,
			fmt.fmt.pix_mp.num_planes,
			fmt.fmt.pix_mp.plane_fmt[0].sizeimage);
	else
		fprintf(stderr, "S_FMT video %ux%u fourcc=%c%c%c%c size=%u\n",
			fmt.fmt.pix.width, fmt.fmt.pix.height,
			fmt.fmt.pix.pixelformat & 0xff,
			(fmt.fmt.pix.pixelformat >> 8) & 0xff,
			(fmt.fmt.pix.pixelformat >> 16) & 0xff,
			(fmt.fmt.pix.pixelformat >> 24) & 0xff,
			fmt.fmt.pix.sizeimage);
	stride = mplane ? (int)fmt.fmt.pix_mp.plane_fmt[0].bytesperline
			: (int)fmt.fmt.pix.bytesperline;

	struct v4l2_requestbuffers req;
	memset(&req, 0, sizeof(req));
	req.count = 2;
	req.type = btype;
	req.memory = V4L2_MEMORY_MMAP;
	if (xioctl(vfd, VIDIOC_REQBUFS, &req) < 0) {
		perror("REQBUFS");
		return -1;
	}
	fprintf(stderr, "REQBUFS count=%u\n", req.count);

	struct {
		void *start;
		size_t len;
	} maps[4];
	memset(maps, 0, sizeof(maps));
	unsigned nbuf = req.count < 4 ? req.count : 4;
	for (unsigned i = 0; i < nbuf; i++) {
		struct v4l2_buffer buf;
		struct v4l2_plane planes[VIDEO_MAX_PLANES];
		memset(&buf, 0, sizeof(buf));
		memset(planes, 0, sizeof(planes));
		buf.type = btype;
		buf.memory = V4L2_MEMORY_MMAP;
		buf.index = i;
		if (mplane) {
			buf.length = 1;
			buf.m.planes = planes;
		}
		if (xioctl(vfd, VIDIOC_QUERYBUF, &buf) < 0) {
			perror("QUERYBUF");
			return -1;
		}
		size_t len = mplane ? planes[0].length : buf.length;
		off_t off = mplane ? planes[0].m.mem_offset : buf.m.offset;
		maps[i].len = len;
		maps[i].start = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_SHARED,
				     vfd, off);
		if (maps[i].start == MAP_FAILED) {
			perror("mmap");
			return -1;
		}
		if (xioctl(vfd, VIDIOC_QBUF, &buf) < 0) {
			perror("QBUF");
			return -1;
		}
	}

	int gmin = 0, gmax = 0, gdef = 0, emin = 0, emax = 0, edef = 0;
	int dmin = 0, dmax = 0, ddef = 0;
	int fmin = 0, fmax = 0, fdef = 0;
	query_range(sensor_fd, V4L2_CID_ANALOGUE_GAIN, &gmin, &gmax, &gdef);
	query_range(sensor_fd, V4L2_CID_EXPOSURE, &emin, &emax, &edef);
	query_range(sensor_fd, V4L2_CID_DIGITAL_GAIN, &dmin, &dmax, &ddef);
	/* handler_setup applies these at s_stream. Default analogue gain is 0
	 * (IMX376) — indoor stills need analog+digital+long exposure. */
	if (gmax > 0)
		s_ctrl(sensor_fd, V4L2_CID_ANALOGUE_GAIN, gmax);
	if (emax > 4) {
		int exp = emax;
		if (view && strcmp(sensor, "imx519") != 0)
			exp = emax < 1600 ? emax : 1600;
		s_ctrl(sensor_fd, V4L2_CID_EXPOSURE, exp);
	}
	if (dmax > 1024)
		s_ctrl(sensor_fd, V4L2_CID_DIGITAL_GAIN,
		       dmax < 2048 ? dmax : 2048);
	/* IMX519 view: analog/2 + dgain 1024 + CIT 800. Max analog+8×
	 * digital was snow; user can focus close now. */
	if (view && strcmp(sensor, "imx519") == 0) {
		if (gmax > 1)
			s_ctrl(sensor_fd, V4L2_CID_ANALOGUE_GAIN, gmax / 2);
		if (emax > 4)
			s_ctrl(sensor_fd, V4L2_CID_EXPOSURE,
			       emax < 800 ? emax : 800);
		s_ctrl(sensor_fd, V4L2_CID_DIGITAL_GAIN, 1024);
	}
	if (act_fd >= 0) {
		query_range(act_fd, V4L2_CID_FOCUS_ABSOLUTE, &fmin, &fmax, &fdef);
		s_ctrl(act_fd, V4L2_CID_FOCUS_ABSOLUTE, fdef);
	}

	if (xioctl(vfd, VIDIOC_STREAMON, &btype) < 0) {
		fprintf(stderr, "STREAMON: %s\n", strerror(errno));
		return -1;
	}
	fprintf(stderr, "STREAMON ok\n");
	if (view)
		g_dq_ms = 8000;
	else
		alarm(20);

	struct v4l2_buffer buf;
	struct v4l2_plane planes[VIDEO_MAX_PLANES];
	if (view) {
		int stride = 0;
		if (mplane)
			stride = (int)fmt.fmt.pix_mp.plane_fmt[0].bytesperline;
		else
			stride = (int)fmt.fmt.pix.bytesperline;
		void *starts[4];
		for (unsigned i = 0; i < 4; i++)
			starts[i] = maps[i].start;
		/* First CPHY frame can take seconds; do not sit in a
		 * 7-frame skip before publishing. Auto-AF after the
		 * viewfinder is live (prev>=2). */
		view_loop(vfd, act_fd, sensor_fd, btype, mplane, starts, (int)w, (int)h,
			  stride, fmin, fmax, gmax, emax, dmax, &buf, planes, sensor, nbuf);
		xioctl(vfd, VIDIOC_STREAMOFF, &btype);
		for (unsigned i = 0; i < nbuf; i++)
			if (maps[i].start && maps[i].start != MAP_FAILED)
				munmap(maps[i].start, maps[i].len);
		return 0;
	}
	/* drop startup frames — first buffers are often dark/garbage */
	for (int i = 0; i < 3; i++) {
		if (dq_one(vfd, btype, mplane, &buf, planes) < 0) {
			xioctl(vfd, VIDIOC_STREAMOFF, &btype);
			return -1;
		}
		xioctl(vfd, VIDIOC_QBUF, &buf);
	}

	int gain = 0, expv = 0, dgain = 0;
	g_ctrl(sensor_fd, V4L2_CID_ANALOGUE_GAIN, &gain);
	g_ctrl(sensor_fd, V4L2_CID_EXPOSURE, &expv);
	g_ctrl(sensor_fd, V4L2_CID_DIGITAL_GAIN, &dgain);
	fprintf(stderr, "after stream gain=%d exp=%d dgain=%d\n", gain, expv, dgain);

	for (int i = 0; i < 3; i++) {
		if (dq_one(vfd, btype, mplane, &buf, planes) < 0) {
			xioctl(vfd, VIDIOC_STREAMOFF, &btype);
			return -1;
		}
		xioctl(vfd, VIDIOC_QBUF, &buf);
	}
	if (dq_one(vfd, btype, mplane, &buf, planes) < 0) {
		xioctl(vfd, VIDIOC_STREAMOFF, &btype);
		return -1;
	}
	size_t used = mplane ? planes[0].bytesused : buf.bytesused;
	int mean = frame_mean8(maps[buf.index].start, used);
	fprintf(stderr, "DQBUF seq=%u used=%zu mean8=%d\n", buf.sequence, used, mean);

	const int target = view ? 64 : 80;
	if (mean > 4 && mean < 50) {
		int dg = 0;
		g_ctrl(sensor_fd, V4L2_CID_DIGITAL_GAIN, &dg);
		long nd = (long)(dg > 256 ? dg : 1024) * target / mean;
		if (dmax > 0 && nd > dmax) nd = dmax;
		int dcap = view ? 2048 : 3072;
		if (nd > dcap) nd = dcap;
		if (nd < 1024) nd = 1024;
		s_ctrl(sensor_fd, V4L2_CID_DIGITAL_GAIN, (int)nd);
		xioctl(vfd, VIDIOC_QBUF, &buf);
		for (int i = 0; i < 4; i++) {
			if (dq_one(vfd, btype, mplane, &buf, planes) < 0)
				break;
			xioctl(vfd, VIDIOC_QBUF, &buf);
		}
		if (dq_one(vfd, btype, mplane, &buf, planes) == 0) {
			used = mplane ? planes[0].bytesused : buf.bytesused;
			mean = frame_mean8(maps[buf.index].start, used);
			fprintf(stderr, "AE2 seq=%u mean8=%d\n", buf.sequence, mean);
		}
	} else if (mean > (view ? 110 : 160) && ddef > 0) {
		s_ctrl(sensor_fd, V4L2_CID_DIGITAL_GAIN, ddef);
		s_ctrl(sensor_fd, V4L2_CID_ANALOGUE_GAIN, gmax / 4);
	}

	/* return the AE frame to the queue; contrast AF / view need a live stream */
	xioctl(vfd, VIDIOC_QBUF, &buf);
	if (stride <= 0 && h)
		stride = (int)(used / h);
	if (view) {
		void *starts[4];
		for (unsigned i = 0; i < 4; i++)
			starts[i] = maps[i].start;
		view_loop(vfd, act_fd, sensor_fd, btype, mplane, starts, (int)w, (int)h,
			  stride, fmin, fmax, gmax, emax, dmax, &buf, planes, sensor, nbuf);
		xioctl(vfd, VIDIOC_STREAMOFF, &btype);
		for (unsigned i = 0; i < nbuf; i++)
			if (maps[i].start && maps[i].start != MAP_FAILED)
				munmap(maps[i].start, maps[i].len);
		return 0;
	}
	if (act_fd >= 0 && fmax > fmin && mean >= 40) {
		void *starts[4];
		for (unsigned i = 0; i < 4; i++)
			starts[i] = maps[i].start;
		contrast_af(vfd, act_fd, btype, mplane, starts, (int)w, (int)h,
			    stride, fmin, fmax, fdef, &buf, planes);
		if (drop_frames(vfd, btype, mplane, 2, &buf, planes) < 0) {
			xioctl(vfd, VIDIOC_STREAMOFF, &btype);
			return -1;
		}
	} else if (act_fd >= 0 && mean < 40) {
		fprintf(stderr, "af: skip (mean8=%d, too dark)\n", mean);
	}
	if (dq_one(vfd, btype, mplane, &buf, planes) < 0) {
		xioctl(vfd, VIDIOC_STREAMOFF, &btype);
		return -1;
	}
	used = mplane ? planes[0].bytesused : buf.bytesused;
	mean = frame_mean8(maps[buf.index].start, used);

	int out = open(outpath, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	if (out < 0) {
		perror(outpath);
		xioctl(vfd, VIDIOC_STREAMOFF, &btype);
		return -1;
	}
	if (write(out, maps[buf.index].start, used) != (ssize_t)used)
		perror("write");
	close(out);
	fprintf(stderr, "wrote %s (%zu bytes) mean8=%d\n", outpath, used, mean);

	xioctl(vfd, VIDIOC_STREAMOFF, &btype);
	for (unsigned i = 0; i < nbuf; i++)
		if (maps[i].start && maps[i].start != MAP_FAILED)
			munmap(maps[i].start, maps[i].len);
	return 0;
}

int main(int argc, char **argv)
{
	int view = 0;
	const char *sensor = NULL;
	const char *outpath = NULL;
	for (int i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--view"))
			view = 1;
		else if (!sensor)
			sensor = argv[i];
		else if (!outpath)
			outpath = argv[i];
	}
	if (!sensor)
		sensor = view ? "imx376" : "imx371";
	if (!outpath)
		outpath = "/tmp/frame.raw";
	const char *csiphy_name;
	const char *sensor_name;
	if (strcmp(sensor, "imx519") == 0) {
		sensor_name = "imx519 16-001a";
		csiphy_name = "msm_csiphy0";
	} else if (strcmp(sensor, "imx376") == 0) {
		sensor_name = "imx376 17-0010";
		csiphy_name = "msm_csiphy1";
	} else {
		sensor_name = "imx371 16-0010";
		csiphy_name = "msm_csiphy2";
	}

	int mfd = open("/dev/media0", O_RDWR);
	if (mfd < 0) {
		perror("media0");
		return 1;
	}

	struct entity ents[64];
	int nent = 0;
	uint32_t id = 0;
	for (; nent < 64;) {
		memset(&ents[nent].desc, 0, sizeof(ents[nent].desc));
		ents[nent].desc.id = id | MEDIA_ENT_ID_FLAG_NEXT;
		ents[nent].fd = -1;
		if (xioctl(mfd, MEDIA_IOC_ENUM_ENTITIES, &ents[nent].desc) < 0)
			break;
		id = ents[nent].desc.id;
		if (ents[nent].desc.dev.major)
			ents[nent].fd = open_by_devnum(ents[nent].desc.dev.major,
						       ents[nent].desc.dev.minor,
						       O_RDWR);
		nent++;
	}

	int isen = find_ent(ents, nent, sensor_name);
	int iphy = find_ent(ents, nent, csiphy_name);
	int icsid = find_ent(ents, nent, "msm_csid0");
	int irdi = find_ent(ents, nent, "msm_vfe0_rdi0");
	int ivid = find_ent(ents, nent, "msm_vfe0_video0");
	if (isen < 0 || iphy < 0 || icsid < 0 || irdi < 0 || ivid < 0) {
		fprintf(stderr, "missing entity sen=%d phy=%d csid=%d rdi=%d vid=%d\n",
			isen, iphy, icsid, irdi, ivid);
		return 1;
	}
	if (ents[isen].fd < 0 || ents[iphy].fd < 0 || ents[icsid].fd < 0 ||
	    ents[irdi].fd < 0 || ents[ivid].fd < 0) {
		fprintf(stderr, "missing fd sen=%d phy=%d csid=%d rdi=%d vid=%d\n",
			ents[isen].fd, ents[iphy].fd, ents[icsid].fd,
			ents[irdi].fd, ents[ivid].fd);
		return 1;
	}

	/* Drop other CSIPHY->CSID0 links so the pipe is not busy. */
	{
		const char *phys[] = { "msm_csiphy0", "msm_csiphy1", "msm_csiphy2",
				       "msm_csiphy3", NULL };
		for (int i = 0; phys[i]; i++) {
			int p = find_ent(ents, nent, phys[i]);
			if (p >= 0)
				setup_link(mfd, ents[p].desc.id, 1,
					   ents[icsid].desc.id, 0, 0);
		}
	}
	/* csiphy SOURCE -> csid SINK; csid SOURCE pad1 -> rdi SINK */
	if (setup_link(mfd, ents[iphy].desc.id, 1, ents[icsid].desc.id, 0, 1) < 0)
		return 1;
	if (setup_link(mfd, ents[icsid].desc.id, 1, ents[irdi].desc.id, 0, 1) < 0)
		return 1;

	struct v4l2_mbus_framefmt mf;
	if (get_fmt(ents[isen].fd, 0, &mf) < 0)
		return 1;
	if (mf.width == 0 || mf.height == 0) {
		/* try a common still size */
		mf.width = 1920;
		mf.height = 1080;
		mf.code = MEDIA_BUS_FMT_SRGGB10_1X10;
		mf.field = V4L2_FIELD_NONE;
		if (set_fmt(ents[isen].fd, 0, mf.code, mf.width, mf.height) < 0)
			return 1;
		get_fmt(ents[isen].fd, 0, &mf);
	}
	/* OP6 mainline receipt: 3840x2160 CPHY produced a frame; native
	 * 4656x3496 STREAMON has never DQBUF'd on this 6.11. */
	if (strcmp(sensor, "imx519") == 0) {
		mf.width = 3840;
		mf.height = 2160;
		mf.field = V4L2_FIELD_NONE;
		if (set_fmt(ents[isen].fd, 0, mf.code, mf.width, mf.height) < 0)
			fprintf(stderr, "imx519 3840x2160 S_FMT failed, keeping G_FMT\n");
		else
			get_fmt(ents[isen].fd, 0, &mf);
	}

	if (set_fmt(ents[isen].fd, 0, mf.code, mf.width, mf.height) < 0)
		return 1;
	if (set_fmt(ents[iphy].fd, 0, mf.code, mf.width, mf.height) < 0)
		return 1;
	if (set_fmt(ents[iphy].fd, 1, mf.code, mf.width, mf.height) < 0)
		return 1;
	if (set_fmt(ents[icsid].fd, 0, mf.code, mf.width, mf.height) < 0)
		return 1;
	if (set_fmt(ents[icsid].fd, 1, mf.code, mf.width, mf.height) < 0)
		return 1;
	if (set_fmt(ents[irdi].fd, 0, mf.code, mf.width, mf.height) < 0)
		return 1;
	if (set_fmt(ents[irdi].fd, 1, mf.code, mf.width, mf.height) < 0)
		return 1;

	int iact = -1;
	if (strcmp(sensor, "imx376") == 0)
		iact = find_ent(ents, nent, "lc898217xc 17-0074");
	else if (strcmp(sensor, "imx519") == 0)
		iact = find_ent(ents, nent, "lc898217xc 16-0072");
	int act_fd = (iact >= 0) ? ents[iact].fd : -1;

	int rc = capture(ents[ivid].fd, ents[isen].fd, act_fd, outpath,
			 mf.width, mf.height, mf.code, view, sensor);
	close(mfd);
	return rc ? 1 : 0;
}
