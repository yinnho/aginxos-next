/* reboot2 [poweroff|bootloader|<reason>] — direct reboot(2) frontend.
 * No args: plain restart. "poweroff": RB_POWER_OFF — the PMIC cuts power
 * (M15 shutdown path). Anything else: RESTART2 with that reason string,
 * e.g. `reboot2 bootloader` == what `adb reboot bootloader` does on
 * Android; Qualcomm ABL reads the restart reason and enters fastboot. */
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <linux/reboot.h>

int main(int argc, char **argv)
{
	sync();
	long r;
	if (argc >= 2 && !strcmp(argv[1], "poweroff")) {
		r = syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
			    LINUX_REBOOT_CMD_POWER_OFF);
	} else if (argc >= 2) {
		r = syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
			    LINUX_REBOOT_CMD_RESTART2, argv[1]);
	} else {
		r = syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
			    LINUX_REBOOT_CMD_RESTART);
	}
	if (r)
		perror("reboot");
	return r != 0;
}
