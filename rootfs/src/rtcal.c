// rtcal: pm8xxx RTC tool (M23b). Two jobs:
//   1. wake-alarm arm/read — the suspend probes' wake path (`set <epoch>`
//      arms the alarm; legacy name kept from the /tmp zig one-off).
//   2. `sync` — push ntpd-corrected system time into the RTC. The PMIC RTC
//      comes up on its own -53y scale (since_epoch ~1.76e6 = 1970-01-21,
//      matching the stray 1970 mtimes); one sync makes early-boot wall time
//      true on the next HCTOSYS pass and keeps alarm math on the real scale.
// sync disarms any pending alarm first: a stale alarm epoch on the old scale
// would land in the past and fire immediately after the time jump.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <sys/ioctl.h>
#include <linux/rtc.h>

static void epoch_to_tm(long long e, struct rtc_time *t)
{
	time_t tt = (time_t)e;
	struct tm tm;
	gmtime_r(&tt, &tm);
	memset(t, 0, sizeof(*t));
	t->tm_sec = tm.tm_sec; t->tm_min = tm.tm_min; t->tm_hour = tm.tm_hour;
	t->tm_mday = tm.tm_mday; t->tm_mon = tm.tm_mon; t->tm_year = tm.tm_year;
	t->tm_wday = tm.tm_wday; t->tm_yday = tm.tm_yday;
}

static long long tm_to_epoch(const struct rtc_time *t)
{
	struct tm tm;
	memset(&tm, 0, sizeof(tm));
	tm.tm_sec = t->tm_sec; tm.tm_min = t->tm_min; tm.tm_hour = t->tm_hour;
	tm.tm_mday = t->tm_mday; tm.tm_mon = t->tm_mon; tm.tm_year = t->tm_year;
	tm.tm_isdst = -1;
	return (long long)timegm(&tm);
}

static int arm(int fd, long long epoch)
{
	struct rtc_wkalrm a;
	memset(&a, 0, sizeof(a));
	epoch_to_tm(epoch, &a.time);
	a.enabled = 1;
	if (ioctl(fd, RTC_WKALM_SET, &a) < 0) {
		perror("WKALM_SET");
		return 1;
	}
	printf("armed for %lld\n", epoch);
	return 0;
}

int main(int argc, char **argv)
{
	int fd = open("/dev/rtc0", O_RDWR);

	if (fd < 0) {
		perror("/dev/rtc0");
		return 1;
	}
	if (argc == 1) {
		struct rtc_time t;
		struct rtc_wkalrm a;

		if (ioctl(fd, RTC_RD_TIME, &t) < 0) {
			perror("RD_TIME");
			return 1;
		}
		printf("since_epoch=%lld\n", tm_to_epoch(&t));
		if (ioctl(fd, RTC_WKALM_RD, &a) == 0)
			printf("alarm enabled=%d epoch=%lld\n",
			       a.enabled, tm_to_epoch(&a.time));
		return 0;
	}
	if (!strcmp(argv[1], "set") && argc == 3)	/* legacy: ARM */
		return arm(fd, atoll(argv[2]));
	if (!strcmp(argv[1], "arm") && argc == 3) {
		long long base = 0;

		if (argv[2][0] == '+') {
			struct rtc_time t;

			if (ioctl(fd, RTC_RD_TIME, &t) < 0) {
				perror("RD_TIME");
				return 1;
			}
			base = tm_to_epoch(&t);
		}
		return arm(fd, base + atoll(argv[2]));
	}
	if (!strcmp(argv[1], "sync") && argc == 2) {
		struct timespec now;
		struct rtc_time old, t;
		struct rtc_wkalrm a;

		clock_gettime(CLOCK_REALTIME, &now);
		if (ioctl(fd, RTC_RD_TIME, &old) < 0) {
			perror("RD_TIME");
			return 1;
		}
		/* disarm first — see header comment */
		memset(&a, 0, sizeof(a));
		ioctl(fd, RTC_WKALM_SET, &a);
		epoch_to_tm(now.tv_sec, &t);
		if (ioctl(fd, RTC_SET_TIME, &t) < 0) {
			perror("SET_TIME");
			return 1;
		}
		printf("rtc %lld -> %lld (system %lld)\n",
		       tm_to_epoch(&old), tm_to_epoch(&t),
		       (long long)now.tv_sec);
		return 0;
	}
	fprintf(stderr,
		"usage: rtcal [set <epoch> | arm <+delta|epoch> | sync]\n");
	return 2;
}
