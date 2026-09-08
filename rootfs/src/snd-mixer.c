// snd-mixer — controlC0 element list/get/set for the M18 DPCM routes.
//   snd-mixer [-controlC0]          list: numid 'name' type access count values
//   snd-mixer [-dev] NAME           read one element by name
//   snd-mixer [-dev] NAME V...      write longs (switch 0/1, enum index, dB*100)
// Nr/size discovered on device (2026-08-31): this vendor kernel trims the
// trailing reserved of snd_ctl_elem_info (272 B) and keeps the old
// indirect-bit layout of snd_ctl_elem_value (1224 B); the canonical uapi
// numbers are ELEM_INFO 0x11 / READ 0x12 / WRITE 0x13.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <sys/ioctl.h>

struct snd_ctl_elem_id {
  unsigned int numid, iface, device, subdevice;
  unsigned char name[44];
  unsigned int index;
};

struct ctl_info272 {          /* kernel's snd_ctl_elem_info, 272 B */
  struct snd_ctl_elem_id id;
  int type;                   /* 1 bool 2 int 3 enum 4 int64 */
  unsigned int access;
  unsigned int count;
  int owner;
  union {
    struct { long min, max, step; } integer;
    struct { unsigned int items, item; char name[64]; } enumerated;
    unsigned char reserved[128];
  } value;
  union { unsigned long long min64, max64; unsigned char r1[64]; } value64;
};

struct ctl_value1224 {        /* kernel's snd_ctl_elem_value, 1224 B */
  struct snd_ctl_elem_id id;
  unsigned int indirect : 1;
  union { long value[128]; unsigned char data[512]; } v;
  unsigned char reserved[128];
};

#define CTL_INFO  _IOWR('U', 0x11, struct ctl_info272)
#define CTL_READ  _IOWR('U', 0x12, struct ctl_value1224)
#define CTL_WRITE _IOWR('U', 0x13, struct ctl_value1224)

static int fd;

static void show(struct ctl_info272 *ei, struct ctl_value1224 *ev)
{
  int i;
  printf("%u '%s' t%d c%u", ei->id.numid, ei->id.name, ei->type, ei->count);
  for (i = 0; i < (int)ei->count && i < 16; i++)
    printf("%s%ld", i ? "," : " ", ev->v.value[i]);
  printf("\n");
}

int main(int argc, char **argv)
{
  const char *name = NULL, *path = "/dev/snd/controlC0";
  struct ctl_info272 ei;
  struct ctl_value1224 ev;
  unsigned n, i, found = 0, misses = 0;
  int first_arg = 1, want_write = 0;

  if (argc > 1 && argv[1][0] == '-') { path = argv[1] + 1; first_arg = 2; }
  if (argc > first_arg) name = argv[first_arg];
  if (argc > first_arg + 1) want_write = 1;

  fd = open(path, O_RDWR);
  if (fd < 0) { perror(path); return 1; }

  for (n = 1; misses < 16; n++) {
    memset(&ei, 0, sizeof(ei));
    ei.id.numid = n;
    if (ioctl(fd, CTL_INFO, &ei) < 0) { misses++; continue; }
    misses = 0;
    if (!name) {
      memset(&ev, 0, sizeof(ev)); ev.id.numid = n;
      if (ioctl(fd, CTL_READ, &ev) == 0) show(&ei, &ev);
      continue;
    }
    if (strcmp((char *)ei.id.name, name) == 0) {
      found = 1;
      if (want_write) {
        memset(&ev, 0, sizeof(ev)); ev.id.numid = n;
        for (i = 0; i + first_arg + 1 < (unsigned)argc && i < 128; i++)
          ev.v.value[i] = strtol(argv[i + first_arg + 1], NULL, 0);
        if (ioctl(fd, CTL_WRITE, &ev) < 0) { perror("write"); return 1; }
      }
      memset(&ev, 0, sizeof(ev)); ev.id.numid = n;
      if (ioctl(fd, CTL_READ, &ev) == 0) {
        show(&ei, &ev);
        /* enum query (M43): kernel fills value.enumerated.name for each
           requested item — one info ioctl per index. Only for named
           queries; full-list mode would drown in items. */
        if (ei.type == 3 && ei.value.enumerated.items <= 256) {
          struct ctl_info272 qi;
          for (i = 0; i < ei.value.enumerated.items; i++) {
            memset(&qi, 0, sizeof(qi));
            qi.id.numid = n;
            qi.value.enumerated.item = i;
            if (ioctl(fd, CTL_INFO, &qi) == 0)
              printf("  %u: %s\n", i, qi.value.enumerated.name);
          }
        }
      }
    }
  }
  if (name && !found) { fprintf(stderr, "no element '%s'\n", name); return 1; }
  return 0;
}
