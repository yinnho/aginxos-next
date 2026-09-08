/* i2c-reg — read/write 32-bit registers of an I2C device, no libraries.
 *
 * M18: the rt5514p codec (bus 0, addr 0x57) exposes every register at
 * "reg | 0x18000000" (RT5514_DSP_MAPPING), regmap-i2c big-endian. The
 * kernel has no debugfs here, so this is how we see mute/clock/power
 * bits live — and, when a bring-up bug leaves a mute stuck on, push a
 * register write the driver never sends.
 *
 * usage:
 *   i2c-reg <dev> <addr> <reg>...          read regs, print hex
 *   i2c-reg -w <dev> <addr> <reg> <val>... write pairs
 *   dev e.g. /dev/i2c-0, addr 0x57, reg 0x18002190 (already mapped) or
 *   0x2190 (we OR in 0x18000000 for you). rmw: -w ... reg val|mask? keep
 *   it simple: -w writes the whole 32-bit value; read first, modify, write.
 * exit: 0 ok, 1 usage, 2 open/ioctl failed, 3 xfer failed.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdint.h>
#include <sys/ioctl.h>

/* minimal linux/i2c-dev.h uapi (zig cc musl sysroot lacks it) */
typedef uint16_t i2c_dev_addr_t;
struct i2c_msg {
	uint16_t addr;
	uint16_t flags;
	uint16_t len;
	uint8_t *buf;
};
struct i2c_rdwr_ioctl_data {
	struct i2c_msg *msgs;
	uint32_t nmsgs;
};
#define I2C_M_RD 0x0001
#define I2C_RDWR 0x0707

static uint32_t map_reg(uint32_t r)
{
	/* bare rt5514 page-2 regs (0x2000..0x2fff) get the DSP mapping base */
	if ((r & 0xffff0000u) == 0 && r >= 0x2000)
		return r | 0x18000000u;
	return r;
}

static int xfer_read(int fd, uint8_t addr, uint32_t reg, uint32_t *val)
{
	struct i2c_msg msgs[2] = {
		{ .addr = addr, .flags = 0,     .len = 4, .buf = (uint8_t *)&reg },
		{ .addr = addr, .flags = I2C_M_RD, .len = 4, .buf = (uint8_t *)val },
	};
	struct i2c_rdwr_ioctl_data d = { .msgs = msgs, .nmsgs = 2 };
	uint8_t b[4] = { reg >> 24, reg >> 16, reg >> 8, reg };
	msgs[0].buf = b;
	uint8_t v[4] = { 0 };
	msgs[1].buf = v;
	if (ioctl(fd, I2C_RDWR, &d) < 0)
		return -1;
	*val = (uint32_t)v[0] << 24 | (uint32_t)v[1] << 16 |
	       (uint32_t)v[2] << 8 | v[3];
	return 0;
}

static int xfer_write(int fd, uint8_t addr, uint32_t reg, uint32_t val)
{
	uint8_t b[8] = { reg >> 24, reg >> 16, reg >> 8, reg,
			 val >> 24, val >> 16, val >> 8, val };
	struct i2c_msg m = { .addr = addr, .flags = 0, .len = 8, .buf = b };
	struct i2c_rdwr_ioctl_data d = { .msgs = &m, .nmsgs = 1 };
	return ioctl(fd, I2C_RDWR, &d) < 0 ? -1 : 0;
}

int main(int argc, char **argv)
{
	int write_mode = 0, i = 1;
	if (argc > 1 && !strcmp(argv[1], "-w")) { write_mode = 1; i = 2; }
	if (argc < i + 2) {
		fprintf(stderr, "usage: i2c-reg [-w] <i2c-dev> <addr> <reg> [val...]\n");
		return 1;
	}
	const char *dev = argv[i++];
	uint8_t addr = (uint8_t)strtoul(argv[i++], NULL, 0);

	int fd = open(dev, O_RDWR);
	if (fd < 0) { perror(dev); return 2; }

	int rc = 0;
	if (write_mode) {
		if (argc < i + 2) { fprintf(stderr, "-w needs reg val pairs\n"); return 1; }
		for (; i + 1 < argc + 1 && i + 1 <= argc; i += 2) {
			uint32_t reg = map_reg(strtoul(argv[i], NULL, 0));
			uint32_t val = strtoul(argv[i + 1], NULL, 0);
			if (xfer_write(fd, addr, reg, val) < 0) {
				fprintf(stderr, "write 0x%08x: %s\n", reg, strerror(errno));
				rc = 3;
			} else
				printf("0x%08x <= 0x%08x\n", reg, val);
		}
	} else {
		for (; i < argc; i++) {
			uint32_t reg = map_reg(strtoul(argv[i], NULL, 0));
			uint32_t val = 0;
			if (xfer_read(fd, addr, reg, &val) < 0) {
				printf("0x%08x: ERROR %s\n", reg, strerror(errno));
				rc = 3;
			} else
				printf("0x%08x = 0x%08x\n", reg, val);
		}
	}
	close(fd);
	return rc;
}
