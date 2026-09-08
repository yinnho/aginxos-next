/* binder-init: stand-in for Android init's binderfs setup on a bare ramdisk.
 * mount binderfs, allocate binder/hwbinder/vndsbinder via binder-control,
 * then move them to /dev like init does, so vendor daemons (cnss-daemon's
 * libbase ProcessState) find /dev/binder.
 */
#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/ioctl.h>
#include <sys/stat.h>

#define BINDERFS_MAX_NAME 255
/* redbull 4.19 backport keeps major/minor out-params (dropped upstream later) */
struct binderfs_device {
	char name[BINDERFS_MAX_NAME + 1];
	unsigned int major;
	unsigned int minor;
};
#define BINDER_CTL_ADD _IOWR('b', 1, struct binderfs_device)

#define BFS_MNT "/dev/binderfs"

int main(void)
{
	const char *names[] = { "binder", "hwbinder", "vndsbinder" };
	mkdir(BFS_MNT, 0755);
	if (mount("binder", BFS_MNT, "binder", 0, "") < 0) {
		if (errno != EBUSY && errno != EEXIST) {
			perror("mount binderfs");
			return 1;
		}
	}
	int fd = open(BFS_MNT "/binder-control", O_RDONLY | O_CLOEXEC);
	if (fd < 0) { perror("open binder-control"); return 1; }
	for (unsigned i = 0; i < sizeof(names) / sizeof(names[0]); i++) {
		struct binderfs_device dev;
		memset(&dev, 0, sizeof(dev));
		strncpy(dev.name, names[i], sizeof(dev.name) - 1);
		if (ioctl(fd, BINDER_CTL_ADD, &dev) < 0) {
			if (errno != EEXIST)
				fprintf(stderr, "add %s: %s\n", names[i], strerror(errno));
		}
		/* this backport cannot rename out of binderfs (EXDEV); symlink instead */
		char src[300], dst[64];
		snprintf(src, sizeof(src), BFS_MNT "/%s", names[i]);
		snprintf(dst, sizeof(dst), "/dev/%s", names[i]);
		unlink(dst);
		if (symlink(src, dst) < 0)
			fprintf(stderr, "symlink %s: %s\n", dst, strerror(errno));
		else
			printf("%s -> %s\n", dst, src);
	}
	close(fd);
	return 0;
}
