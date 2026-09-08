/*
 * AginxOS rdinit on Pixel 5 (redfin).
 * Flow: mount → optional display modules → solid-color DRM frames → first_stage.
 *
 * SPLASH without modules usually cannot paint (msm_drm is a module).
 * Prefer loading stock /lib/modules/modules.load up through msm_drm.ko.
 */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/sysmacros.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef SYS_finit_module
#define SYS_finit_module 313 /* aarch64 */
#endif
#ifndef SYS_delete_module
#define SYS_delete_module 106 /* aarch64 */
#endif
#ifndef SYS_reboot
#define SYS_reboot 142 /* aarch64 */
#endif

static void kmsg(const char *s);

/* Track modules we loaded so we can unload before first_stage handoff. */
#define MAX_LOADED 256
static char g_loaded[MAX_LOADED][64];
static int g_nloaded;

static void kmsg(const char *s) {
  int fd = open("/dev/kmsg", O_WRONLY | O_CLOEXEC);
  if (fd < 0) {
    /* devtmpfs may not be mounted in our env (observed 2026-08-26: every
     * trampoline log line was lost because ensure_fs's devtmpfs mount
     * failed silently). /dev/kmsg is char 1:11 — create a static node as a
     * logging fallback; this masks no driver, unlike fake DRM nodes. */
    mkdir("/dev", 0755);
    if (mknod("/dev/kmsg", S_IFCHR | 0600, makedev(1, 11)) == 0 || errno == EEXIST)
      fd = open("/dev/kmsg", O_WRONLY | O_CLOEXEC);
  }
  if (fd < 0)
    return;
  write(fd, s, strlen(s));
  close(fd);
}

static int exists(const char *p) { return access(p, F_OK) == 0; }

/* Write a string to a (configfs) file; -1 on failure. */
static int wf(const char *path, const char *val) {
  int fd = open(path, O_WRONLY | O_CLOEXEC);
  if (fd < 0)
    return -1;
  size_t n = strlen(val);
  ssize_t w = write(fd, val, n);
  int e = errno;
  close(fd);
  return (w == (ssize_t)n) ? 0 : -(e ? e : 1);
}

static void try_mount(const char *src, const char *tgt, const char *fstype,
                      const char *data) {
  mkdir(tgt, 0755);
  mount(src, tgt, fstype, 0, data);
}

static void ensure_fs(void) {
  try_mount("proc", "/proc", "proc", "");
  try_mount("sysfs", "/sys", "sysfs", "");
  mkdir("/dev", 0755);
  if (mount("devtmpfs", "/dev", "devtmpfs", 0, "mode=0755") == 0) {
    kmsg("aginxos-trampoline: devtmpfs mounted\n");
    return;
  }
  /* v28 root cause of the adbd SIGABRT: bionic's getentropy() needs
   * /dev/urandom and fatal()s on ENOENT ("getentropy failed: No such file
   * or directory"). This kernel likely has no devtmpfs (stock Android
   * mounts tmpfs on /dev and lets ueventd mknod); fall back to creating
   * the major-1 char essentials ourselves — those drivers are always
   * registered, only the nodes are missing. */
  {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: devtmpfs mount errno=%d\n",
             errno);
    kmsg(b);
  }
  static const struct { const char *name; int minor; } nodes[] = {
      {"null", 3}, {"zero", 5}, {"full", 7},
      {"random", 8}, {"urandom", 9}, {"kmsg", 11},
  };
  for (unsigned i = 0; i < sizeof nodes / sizeof nodes[0]; i++) {
    char p[24];
    snprintf(p, sizeof p, "/dev/%s", nodes[i].name);
    if (!exists(p))
      mknod(p, S_IFCHR | 0666, makedev(1, nodes[i].minor));
  }
}

/* devpts + /dev/ptmx: without them adbd cannot allocate a pty, and `adb
 * reboot` (and any interactive `adb shell`) fails device-side with
 * "failed to create pty master: No such file or directory" (2026-08-27).
 * devtmpfs provides no ptmx of its own; devpts creates the node inside
 * the mounted instance, /dev/ptmx just points at it. */
static void ensure_devpts(void) {
  if (exists("/dev/ptmx"))
    return;
  mkdir("/dev/pts", 0755);
  if (mount("devpts", "/dev/pts", "devpts", 0, "mode=0620") != 0) {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: devpts mount errno=%d\n", errno);
    kmsg(b);
    return;
  }
  if (symlink("/dev/pts/ptmx", "/dev/ptmx") != 0) {
    /* /dev is tmpfs here (devtmpfs won't mount on this kernel — ENODEV);
     * tmpfs permits mknod as root, so fall back to the node itself */
    mknod("/dev/ptmx", S_IFCHR | 0666, makedev(5, 2));
  }
  kmsg("aginxos-trampoline: devpts mounted, /dev/ptmx ready\n");
}

/* Copy /aginxos/props/ contents (packed property area files pulled from a real
 * Android boot of this unit) into /dev/__properties__/ so bionic resolves
 * properties in the rdinit env. Called before adbd is forked — bionic
 * maps the area at first property access inside adbd. Filenames are the
 * SELinux context of each area (e.g. "u:object_r:default_prop:s0"); flat
 * directory, contexts file included. 0444: readers map read-only. */
static void stage_property_area(void) {
  if (!exists("/aginxos/props/property_info")) {
    kmsg("aginxos-trampoline: no /aginxos/props/property_info, skipping property area\n");
    return;
  }
  mkdir("/dev/__properties__", 0755);
  DIR *d = opendir("/aginxos/props");
  if (!d) {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: opendir /aginxos/props errno=%d\n",
             errno);
    kmsg(b);
    return;
  }
  int n = 0;
  struct dirent *de;
  while ((de = readdir(d)) != NULL) {
    if (de->d_name[0] == '.')
      continue;
    char src[80], dst[80];
    snprintf(src, sizeof src, "/aginxos/props/%s", de->d_name);
    snprintf(dst, sizeof dst, "/dev/__properties__/%s", de->d_name);
    struct stat st;
    if (stat(src, &st) != 0 || !S_ISREG(st.st_mode))
      continue;
    int in = open(src, O_RDONLY | O_CLOEXEC);
    int out = open(dst, O_WRONLY | O_CREAT | O_TRUNC, 0444);
    if (in >= 0 && out >= 0) {
      char buf[4096];
      ssize_t r;
      while ((r = read(in, buf, sizeof buf)) > 0)
        if (write(out, buf, r) < 0)
          break;
      n++;
    }
    if (in >= 0)
      close(in);
    if (out >= 0)
      close(out);
  }
  closedir(d);
  char b[96];
  snprintf(b, sizeof b, "aginxos-trampoline: staged %d property area files\n", n);
  kmsg(b);
}

/* "msm_drm.ko" or "/lib/modules/msm_drm.ko" → "msm_drm" */
static void mod_basename(const char *line, char *out, size_t outsz) {
  const char *base = strrchr(line, '/');
  base = base ? base + 1 : line;
  snprintf(out, outsz, "%s", base);
  size_t n = strlen(out);
  if (n > 3 && strcmp(out + n - 3, ".ko") == 0)
    out[n - 3] = 0;
  /* Kernel module names use '_' even when the .ko filename uses '-'
   * (phy-msm-ssusb-qmp.ko -> phy_msm_ssusb_qmp); delete_module() takes the
   * kernel name, so the tracked names must be normalized. */
  for (char *p = out; *p; p++)
    if (*p == '-')
      *p = '_';
}

static int load_one(const char *path, const char *name_for_track) {
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0)
    return -1;
  long rc = syscall(SYS_finit_module, fd, "", 0);
  int err = errno;
  close(fd);
  if (rc == 0 || err == EEXIST) {
    if (name_for_track && g_nloaded < MAX_LOADED) {
      snprintf(g_loaded[g_nloaded], sizeof g_loaded[0], "%s", name_for_track);
      g_nloaded++;
    }
    return 0;
  }
  return -err;
}

static void unload_loaded_modules(void) {
  kmsg("aginxos-trampoline: unloading modules before handoff\n");
  int ok = 0, fail = 0;
  for (int i = g_nloaded - 1; i >= 0; i--) {
    /* O_NONBLOCK|O_TRUNC → force unload if busy */
    long rc = syscall(SYS_delete_module, g_loaded[i], 0x800 | 0x200);
    if (rc == 0)
      ok++;
    else {
      fail++;
      if (fail <= 8) {
        char b[96];
        snprintf(b, sizeof b, "aginxos-trampoline: rmmod %s errno=%d\n",
                 g_loaded[i], errno);
        kmsg(b);
      }
    }
  }
  char b[80];
  snprintf(b, sizeof b, "aginxos-trampoline: rmmod ok=%d fail=%d\n", ok, fail);
  kmsg(b);
  g_nloaded = 0;
}

/* Write "add" to uevent files under dir (one level + common drm paths). */
static void coldplug_uevent(const char *path) {
  char ue[256];
  snprintf(ue, sizeof ue, "%s/uevent", path);
  int fd = open(ue, O_WRONLY | O_CLOEXEC);
  if (fd < 0)
    return;
  write(fd, "add\n", 4);
  close(fd);
}

static void coldplug_drm(void) {
  coldplug_uevent("/sys/class/drm");
  DIR *d = opendir("/sys/class/drm");
  if (d) {
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
      if (e->d_name[0] == '.')
        continue;
      char p[256];
      snprintf(p, sizeof p, "/sys/class/drm/%s", e->d_name);
      coldplug_uevent(p);
    }
    closedir(d);
  }
  /* common platform nodes */
  coldplug_uevent("/sys/devices/platform");
  kmsg("aginxos-trampoline: coldplug drm\n");
}

/* Load a list file: either modules.allow or modules.load style names. */
static void load_list_file(const char *list_path, int stop_after_msm_drm) {
  FILE *f = fopen(list_path, "r");
  if (!f) {
    char b[128];
    snprintf(b, sizeof b, "aginxos-trampoline: no list %s\n", list_path);
    kmsg(b);
    return;
  }
  char line[256];
  int ok = 0, fail = 0, skip = 0;
  while (fgets(line, sizeof line, f)) {
    size_t n = strlen(line);
    while (n && (line[n - 1] == '\n' || line[n - 1] == '\r'))
      line[--n] = 0;
    if (!n || line[0] == '#')
      continue;

    char path[320];
    if (line[0] == '/')
      snprintf(path, sizeof path, "%s", line);
    else
      snprintf(path, sizeof path, "/lib/modules/%s", line);

    if (!exists(path)) {
      skip++;
      continue;
    }
    char modname[64];
    mod_basename(line, modname, sizeof modname);
    int r = load_one(path, modname);
    if (r == 0)
      ok++;
    else {
      fail++;
      if (fail <= 12) {
        char buf[160];
        snprintf(buf, sizeof buf, "aginxos-trampoline: mod fail %s err=%d\n",
                 line, -r);
        kmsg(buf);
      }
    }

    if (stop_after_msm_drm &&
        (strcmp(line, "msm_drm.ko") == 0 || strstr(line, "/msm_drm.ko"))) {
      kmsg("aginxos-trampoline: stop list at msm_drm.ko\n");
      break;
    }
  }
  fclose(f);
  char buf[120];
  snprintf(buf, sizeof buf,
           "aginxos-trampoline: modules ok=%d fail=%d skip=%d from %s\n", ok,
           fail, skip, list_path);
  kmsg(buf);
}

static void dump_dir(const char *path) {
  DIR *d = opendir(path);
  if (!d) {
    char b[128];
    snprintf(b, sizeof b, "aginxos-trampoline: no dir %s\n", path);
    kmsg(b);
    return;
  }
  char line[200];
  size_t used = 0;
  line[0] = 0;
  struct dirent *e;
  while ((e = readdir(d)) != NULL) {
    if (e->d_name[0] == '.')
      continue;
    size_t ln = strlen(e->d_name);
    if (used + ln + 2 >= sizeof line)
      break;
    if (used) {
      line[used++] = ',';
      line[used] = 0;
    }
    memcpy(line + used, e->d_name, ln + 1);
    used += ln;
  }
  closedir(d);
  char b[240];
  snprintf(b, sizeof b, "aginxos-trampoline: %s: %s\n", path,
           used ? line : "(empty)");
  kmsg(b);
}

/* --- DRM --- */
#define DRM_IOCTL_BASE 'd'
#define DRM_IOWR(nr, type)                                                     \
  _IOC(_IOC_READ | _IOC_WRITE, DRM_IOCTL_BASE, (nr), sizeof(type))

struct drm_mode_card_res {
  uint64_t fb_id_ptr;
  uint64_t crtc_id_ptr;
  uint64_t connector_id_ptr;
  uint64_t encoder_id_ptr;
  uint32_t count_fbs;
  uint32_t count_crtcs;
  uint32_t count_connectors;
  uint32_t count_encoders;
  uint32_t min_width, max_width, min_height, max_height;
};

struct drm_mode_modeinfo {
  uint32_t clock;
  uint16_t hdisplay, hsync_start, hsync_end, htotal, hskew;
  uint16_t vdisplay, vsync_start, vsync_end, vtotal, vscan;
  uint32_t vrefresh, flags, type;
  char name[32];
};

struct drm_mode_crtc {
  uint64_t set_connectors_ptr;
  uint32_t count_connectors;
  uint32_t crtc_id;
  uint32_t fb_id;
  uint32_t x, y;
  uint32_t gamma_size;
  uint32_t mode_valid;
  struct drm_mode_modeinfo mode;
};

struct drm_mode_get_encoder {
  uint32_t encoder_id;
  uint32_t encoder_type;
  uint32_t crtc_id;
  uint32_t possible_crtcs;
  uint32_t possible_clones;
};

struct drm_mode_get_connector {
  uint64_t encoders_ptr;
  uint64_t modes_ptr;
  uint64_t props_ptr;
  uint64_t prop_values_ptr;
  uint32_t count_modes;
  uint32_t count_props;
  uint32_t count_encoders;
  uint32_t encoder_id;
  uint32_t connector_id;
  uint32_t connector_type;
  uint32_t connector_type_id;
  uint32_t connection;
  uint32_t mm_width, mm_height;
  uint32_t subpixel;
  uint32_t pad;
};

struct drm_mode_create_dumb {
  uint32_t height, width, bpp, flags;
  uint32_t handle, pitch;
  uint64_t size;
};

struct drm_mode_map_dumb {
  uint32_t handle, pad;
  uint64_t offset;
};

struct drm_mode_fb_cmd {
  uint32_t fb_id, width, height, pitch, bpp, depth, handle;
};

struct drm_mode_fb_cmd2 {
  uint32_t fb_id;
  uint32_t width;
  uint32_t height;
  uint32_t pixel_format;
  uint32_t flags;
  uint32_t handles[4];
  uint32_t pitches[4];
  uint32_t offsets[4];
  uint64_t modifier[4];
};

#define DRM_IOCTL_MODE_GETRESOURCES DRM_IOWR(0xA0, struct drm_mode_card_res)
#define DRM_IOCTL_MODE_GETCRTC DRM_IOWR(0xA1, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_SETCRTC DRM_IOWR(0xA2, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_GETENCODER DRM_IOWR(0xA6, struct drm_mode_get_encoder)
#define DRM_IOCTL_MODE_GETCONNECTOR                                            \
  DRM_IOWR(0xA7, struct drm_mode_get_connector)
#define DRM_IOCTL_MODE_ADDFB DRM_IOWR(0xAE, struct drm_mode_fb_cmd)
#define DRM_IOCTL_MODE_ADDFB2 DRM_IOWR(0xB8, struct drm_mode_fb_cmd2)
#define DRM_IOCTL_MODE_CREATE_DUMB DRM_IOWR(0xB2, struct drm_mode_create_dumb)
#define DRM_IOCTL_MODE_MAP_DUMB DRM_IOWR(0xB3, struct drm_mode_map_dumb)
#define DRM_IOCTL_SET_MASTER _IO(DRM_IOCTL_BASE, 0x1e)

#define DRM_FORMAT_XRGB8888 0x34325258u /* 'XR24' */
#define DRM_FORMAT_ARGB8888 0x34325241u /* 'AR24' */
#define DRM_FORMAT_XBGR8888 0x34324258u /* 'XB24' */
#define DRM_MODE_CONNECTED 1

static int wait_path(const char *path, int tries_ms) {
  for (int t = 0; t < tries_ms; t += 50) {
    if (exists(path))
      return 0;
    usleep(50000);
  }
  return exists(path) ? 0 : -1;
}

static int open_drm(void) {
  /* Do NOT mknod fake nodes — that masks a missing driver. */
  mkdir("/dev/dri", 0755);
  wait_path("/dev/dri/card0", 5000);
  const char *cands[] = {"/dev/dri/card0", "/dev/dri/card1", NULL};
  for (int i = 0; cands[i]; i++) {
    if (!exists(cands[i]))
      continue;
    int fd = open(cands[i], O_RDWR | O_CLOEXEC);
    if (fd >= 0) {
      char b[80];
      snprintf(b, sizeof b, "aginxos-trampoline: opened %s\n", cands[i]);
      kmsg(b);
      ioctl(fd, DRM_IOCTL_SET_MASTER);
      return fd;
    }
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: open %s errno=%d\n", cands[i],
             errno);
    kmsg(b);
  }
  kmsg("aginxos-trampoline: open card* failed\n");
  return -1;
}

static int pick_mode(int fd, uint32_t *crtc_id_out, uint32_t *conn_id_out,
                     struct drm_mode_modeinfo *mode_out) {
  struct drm_mode_card_res res;
  memset(&res, 0, sizeof res);
  if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res)) {
    char b[80];
    snprintf(b, sizeof b, "aginxos-trampoline: GETRESOURCES errno=%d\n",
             errno);
    kmsg(b);
    return -1;
  }

  uint32_t crtcs[16], conns[16], encs[16];
  if (res.count_crtcs > 16)
    res.count_crtcs = 16;
  if (res.count_connectors > 16)
    res.count_connectors = 16;
  if (res.count_encoders > 16)
    res.count_encoders = 16;
  res.crtc_id_ptr = (uint64_t)(uintptr_t)crtcs;
  res.connector_id_ptr = (uint64_t)(uintptr_t)conns;
  res.encoder_id_ptr = (uint64_t)(uintptr_t)encs;
  res.fb_id_ptr = 0;
  if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res))
    return -1;

  char msg[120];
  snprintf(msg, sizeof msg,
           "aginxos-trampoline: res crtc=%u conn=%u enc=%u fb=%u\n",
           res.count_crtcs, res.count_connectors, res.count_encoders,
           res.count_fbs);
  kmsg(msg);

  /* Prefer already-active CRTC (bootloader continuous splash). */
  for (uint32_t i = 0; i < res.count_crtcs; i++) {
    struct drm_mode_crtc c;
    memset(&c, 0, sizeof c);
    c.crtc_id = crtcs[i];
    if (ioctl(fd, DRM_IOCTL_MODE_GETCRTC, &c))
      continue;
    char cb[96];
    snprintf(cb, sizeof cb,
             "aginxos-trampoline: crtc%u id=%u valid=%u %ux%u\n", i, crtcs[i],
             c.mode_valid, c.mode.hdisplay, c.mode.vdisplay);
    kmsg(cb);
    if (!c.mode_valid || !c.mode.hdisplay || !c.mode.vdisplay)
      continue;
    *crtc_id_out = crtcs[i];
    *conn_id_out = res.count_connectors ? conns[0] : 0;
    *mode_out = c.mode;
    kmsg("aginxos-trampoline: using active CRTC mode\n");
    return 0;
  }

  for (uint32_t i = 0; i < res.count_connectors; i++) {
    struct drm_mode_get_connector conn;
    memset(&conn, 0, sizeof conn);
    conn.connector_id = conns[i];
    if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &conn))
      continue;

    uint32_t enc_ids[8];
    struct drm_mode_modeinfo modes[32];
    memset(enc_ids, 0, sizeof enc_ids);
    memset(modes, 0, sizeof modes);
    if (conn.count_encoders > 8)
      conn.count_encoders = 8;
    if (conn.count_modes > 32)
      conn.count_modes = 32;
    conn.encoders_ptr = (uint64_t)(uintptr_t)enc_ids;
    conn.modes_ptr = (uint64_t)(uintptr_t)modes;
    conn.props_ptr = 0;
    conn.prop_values_ptr = 0;
    if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &conn))
      continue;

    char cb[120];
    snprintf(cb, sizeof cb,
             "aginxos-trampoline: conn%u id=%u st=%u modes=%u enc=%u\n", i,
             conns[i], conn.connection, conn.count_modes, conn.count_encoders);
    kmsg(cb);

    if (conn.count_modes == 0)
      continue;

    uint32_t enc_id = conn.encoder_id ? conn.encoder_id : enc_ids[0];
    if (!enc_id && conn.count_encoders)
      enc_id = enc_ids[0];

    uint32_t crtc_id = 0;
    if (enc_id) {
      struct drm_mode_get_encoder enc;
      memset(&enc, 0, sizeof enc);
      enc.encoder_id = enc_id;
      if (ioctl(fd, DRM_IOCTL_MODE_GETENCODER, &enc) == 0) {
        crtc_id = enc.crtc_id;
        if (!crtc_id) {
          for (uint32_t c = 0; c < res.count_crtcs; c++) {
            if (enc.possible_crtcs & (1u << c)) {
              crtc_id = crtcs[c];
              break;
            }
          }
        }
      }
    }
    if (!crtc_id && res.count_crtcs)
      crtc_id = crtcs[0];
    if (!crtc_id)
      continue;

    *crtc_id_out = crtc_id;
    *conn_id_out = conns[i];
    *mode_out = modes[0];
    kmsg("aginxos-trampoline: using connector mode\n");
    return 0;
  }

  /* Last resort: hardcode Pixel 5 panel 1080x2340 if we have a CRTC id. */
  if (res.count_crtcs) {
    memset(mode_out, 0, sizeof *mode_out);
    mode_out->hdisplay = 1080;
    mode_out->hsync_start = 1080 + 32;
    mode_out->hsync_end = 1080 + 32 + 8;
    mode_out->htotal = 1080 + 32 + 8 + 32;
    mode_out->vdisplay = 2340;
    mode_out->vsync_start = 2340 + 8;
    mode_out->vsync_end = 2340 + 8 + 4;
    mode_out->vtotal = 2340 + 8 + 4 + 8;
    mode_out->clock = (mode_out->htotal * mode_out->vtotal * 60) / 1000;
    mode_out->vrefresh = 60;
    mode_out->type = 1; /* preferred */
    snprintf(mode_out->name, sizeof mode_out->name, "1080x2340");
    *crtc_id_out = crtcs[0];
    *conn_id_out = res.count_connectors ? conns[0] : 0;
    kmsg("aginxos-trampoline: using hardcoded 1080x2340\n");
    return 0;
  }

  kmsg("aginxos-trampoline: no usable mode\n");
  return -1;
}

static int add_fb(int fd, uint32_t w, uint32_t h, uint32_t pitch,
                  uint32_t handle, uint32_t *fb_id_out) {
  /* Prefer ADDFB2 with XRGB8888 (MSM common). */
  const uint32_t formats[] = {DRM_FORMAT_XRGB8888, DRM_FORMAT_ARGB8888,
                              DRM_FORMAT_XBGR8888, 0};
  for (int i = 0; formats[i]; i++) {
    struct drm_mode_fb_cmd2 fb2;
    memset(&fb2, 0, sizeof fb2);
    fb2.width = w;
    fb2.height = h;
    fb2.pixel_format = formats[i];
    fb2.handles[0] = handle;
    fb2.pitches[0] = pitch;
    if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fb2) == 0) {
      *fb_id_out = fb2.fb_id;
      char b[80];
      snprintf(b, sizeof b, "aginxos-trampoline: ADDFB2 fmt=%08x ok\n",
               formats[i]);
      kmsg(b);
      return 0;
    }
  }

  struct drm_mode_fb_cmd fb;
  memset(&fb, 0, sizeof fb);
  fb.width = w;
  fb.height = h;
  fb.pitch = pitch;
  fb.bpp = 32;
  fb.depth = 24;
  fb.handle = handle;
  if (ioctl(fd, DRM_IOCTL_MODE_ADDFB, &fb) == 0) {
    *fb_id_out = fb.fb_id;
    kmsg("aginxos-trampoline: ADDFB legacy ok\n");
    return 0;
  }
  char b[64];
  snprintf(b, sizeof b, "aginxos-trampoline: ADDFB errno=%d\n", errno);
  kmsg(b);
  return -1;
}

static int drm_solid(uint32_t color) {
  int fd = open_drm();
  if (fd < 0)
    return -1;

  uint32_t crtc_id = 0, conn_id = 0;
  struct drm_mode_modeinfo mode;
  memset(&mode, 0, sizeof mode);
  if (pick_mode(fd, &crtc_id, &conn_id, &mode) != 0) {
    close(fd);
    return -1;
  }

  uint32_t width = mode.hdisplay;
  uint32_t height = mode.vdisplay;
  char msg[128];
  snprintf(msg, sizeof msg,
           "aginxos-trampoline: mode %ux%u crtc=%u conn=%u color=%08x\n", width,
           height, crtc_id, conn_id, color);
  kmsg(msg);

  struct drm_mode_create_dumb dumb;
  memset(&dumb, 0, sizeof dumb);
  dumb.width = width;
  dumb.height = height;
  dumb.bpp = 32;
  if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb)) {
    snprintf(msg, sizeof msg, "aginxos-trampoline: CREATE_DUMB errno=%d\n",
             errno);
    kmsg(msg);
    close(fd);
    return -1;
  }

  uint32_t fb_id = 0;
  if (add_fb(fd, width, height, dumb.pitch, dumb.handle, &fb_id) != 0) {
    close(fd);
    return -1;
  }

  struct drm_mode_map_dumb map;
  memset(&map, 0, sizeof map);
  map.handle = dumb.handle;
  if (ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &map)) {
    kmsg("aginxos-trampoline: MAP_DUMB fail\n");
    close(fd);
    return -1;
  }

  void *ptr =
      mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, map.offset);
  if (ptr == MAP_FAILED) {
    kmsg("aginxos-trampoline: mmap fail\n");
    close(fd);
    return -1;
  }

  uint32_t pitch = dumb.pitch / 4;
  uint32_t *pix = ptr;
  uint32_t border = 0x00ffffff;
  uint32_t t = 64;
  for (uint32_t y = 0; y < height; y++) {
    for (uint32_t x = 0; x < width; x++) {
      int edge = x < t || y < t || x + t >= width || y + t >= height;
      /* big quadrant stripes if border region */
      pix[y * pitch + x] = edge ? border : color;
    }
  }
  /* Force a huge corner block (hard to miss). */
  for (uint32_t y = 0; y < 200 && y < height; y++)
    for (uint32_t x = 0; x < 200 && x < width; x++)
      pix[y * pitch + x] = 0x00ffffff;
  munmap(ptr, dumb.size);

  uint32_t conn_list[1] = {conn_id};
  struct drm_mode_crtc crtc;
  memset(&crtc, 0, sizeof crtc);
  crtc.crtc_id = crtc_id;
  crtc.fb_id = fb_id;
  crtc.mode = mode;
  crtc.mode_valid = 1;
  if (conn_id) {
    crtc.set_connectors_ptr = (uint64_t)(uintptr_t)conn_list;
    crtc.count_connectors = 1;
  }
  if (ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &crtc)) {
    int e1 = errno;
    crtc.set_connectors_ptr = 0;
    crtc.count_connectors = 0;
    if (ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &crtc)) {
      snprintf(msg, sizeof msg,
               "aginxos-trampoline: SETCRTC fail e1=%d e2=%d\n", e1, errno);
      kmsg(msg);
      close(fd);
      return -1;
    }
  }

  kmsg("aginxos-trampoline: DRM splash OK\n");
  /* keep fd open? close may drop master — keep briefly then close */
  close(fd);
  return 0;
}

/* --- USB gadget console (ffs.adb) ---
 * Mirrors the stock init.rc configfs sequence (see HARDWARE.md
 * "USB gadget recon"). ffs + configfs are built-in; adbd + bionic
 * runtime live at /system/bin/adbd in this same ramdisk.
 */
static void backlight_max(void);

static int wait_udc(char *out, size_t outsz, int tries) {
  for (int t = 0; t < tries; t++) { /* dwc3 + SMMU/PHY deferred probe can be slow */
    DIR *d = opendir("/sys/class/udc");
    if (d) {
      struct dirent *e;
      out[0] = 0;
      while ((e = readdir(d)) != NULL) {
        if (e->d_name[0] == '.')
          continue;
        snprintf(out, outsz, "%s", e->d_name);
        break;
      }
      closedir(d);
      if (out[0])
        return 0;
    }
    usleep(100000);
  }
  return -1;
}

/* --- USB bring-up diagnostics (/aginxos/usb-diag) ---
 * A one-shot module load never replays deferred probes the way stock's
 * staggered async init does (stock dwc3 probe only completes at 1.63s, after
 * the later smb5-charger load at 1.39s triggers a replay). This mode dumps
 * the live extcon + deferred-probe state, force-reprobes ssusb, then hands
 * off so the whole story lands in the ring buffer, readable from booted
 * Android via `su -c dmesg`. */

/* Read a small sysfs file and log it (newlines -> ';'). */
static void kmsg_file(const char *path, const char *tag) {
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0)
    return;
  char buf[512];
  ssize_t n = read(fd, buf, sizeof buf - 1);
  close(fd);
  if (n <= 0)
    return;
  buf[n] = 0;
  for (ssize_t i = 0; i < n; i++)
    if (buf[i] == '\n')
      buf[i] = ';';
  char out[640];
  snprintf(out, sizeof out, "aginxos-trampoline: %s %s: %s\n", tag, path, buf);
  kmsg(out);
}

/* Supplier snapshot: stock success is extcon0=eud extcon3=smb5 extcon4=pdphy
 * (USB=1). Anything missing here is why dwc3's probe defers forever. */
static void dump_extcon(void) {
  DIR *d = opendir("/sys/class/extcon");
  if (!d) {
    kmsg("aginxos-trampoline: no /sys/class/extcon dir\n");
    return;
  }
  struct dirent *e;
  while ((e = readdir(d)) != NULL) {
    if (e->d_name[0] == '.')
      continue;
    char p[160];
    snprintf(p, sizeof p, "/sys/class/extcon/%s/name", e->d_name);
    kmsg_file(p, "extcon");
    snprintf(p, sizeof p, "/sys/class/extcon/%s/state", e->d_name);
    kmsg_file(p, "extcon");
  }
  closedir(d);
}

/* Regulator names: ext_boost proves tps-regulator probed; smb5-vbus/smb5-vconn
 * only exist once qpnp-smb5 itself probed (they are its child regulators). */
static void dump_regulators(void) {
  DIR *d = opendir("/sys/class/regulator");
  if (!d) {
    kmsg("aginxos-trampoline: no /sys/class/regulator dir\n");
    return;
  }
  struct dirent *e;
  while ((e = readdir(d)) != NULL) {
    if (e->d_name[0] == '.')
      continue;
    char p[160];
    snprintf(p, sizeof p, "/sys/class/regulator/%s/name", e->d_name);
    kmsg_file(p, "regulator");
  }
  closedir(d);
}

/* The kernel's own view of who never finished probing (needs debugfs).
 * NOTE (2026-08-26): /sys/kernel/debug does not exist on this kernel's sysfs
 * (no mountpoint created — debugfs likely not built in), and mkdir on sysfs
 * is not permitted, which is why mounting there returned ENOENT. Mount at a
 * rootfs path instead. */
static void dump_deferred(void) {
  mkdir("/dbg", 0755);
  if (mount("none", "/dbg", "debugfs", 0, "") != 0 && errno != EBUSY) {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: debugfs mount errno=%d\n", errno);
    kmsg(b);
    return;
  }
  if (!exists("/dbg/devices_deferred")) {
    kmsg("aginxos-trampoline: deferred: no devices_deferred file\n");
    return;
  }
  kmsg_file("/dbg/devices_deferred", "deferred");
}

static void kick_probe(const char *dev) {
  int r = wf("/sys/bus/platform/drivers_probe", dev);
  char b[128];
  snprintf(b, sizeof b, "aginxos-trampoline: drivers_probe %s -> %d\n", dev, r);
  kmsg(b);
}

static void usb_diag(void) {
  kmsg("aginxos-trampoline: usb diag begin\n");
  if (!exists("/aginxos/modules.usb")) {
    kmsg("aginxos-trampoline: no /aginxos/modules.usb list\n");
    return;
  }
  load_list_file("/aginxos/modules.usb", 0);

  dump_extcon();
  dump_regulators();

  char udc[64];
  if (wait_udc(udc, sizeof udc, 100) == 0) { /* 10 s */
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: udc=%s PROBE VERDICT GREEN\n", udc);
    kmsg(b);
    return;
  }

  kmsg("aginxos-trampoline: no UDC after module load\n");
  {
    char b[112];
    snprintf(b, sizeof b,
             "aginxos-trampoline: ssusb bound to msm-dwc3: %s\n",
             exists("/sys/bus/platform/drivers/msm-dwc3/a600000.ssusb") ? "yes" : "no");
    kmsg(b);
  }
  dump_deferred();

  /* H2: force the deferred-probe replay that stock gets for free. */
  kick_probe("a600000.ssusb");
  if (exists("/sys/bus/platform/devices/a600000.dwc3"))
    kick_probe("a600000.dwc3");

  if (wait_udc(udc, sizeof udc, 100) == 0) { /* 10 s */
    char b[128];
    snprintf(b, sizeof b,
             "aginxos-trampoline: udc=%s AFTER drivers_probe - replay was the gap\n",
             udc);
    kmsg(b);
  } else {
    kmsg("aginxos-trampoline: still no UDC after drivers_probe kick\n");
    dump_deferred();
    dump_extcon();
    dump_regulators();
  }
  kmsg("aginxos-trampoline: diag done - modules stay loaded for Android handoff\n");
}

/* Tear the gadget down before Android handoff: stock init mounts its own
 * functionfs/configfs on the same paths, and a half-bound UDC or a leftover
 * ffs mount breaks Android's adbd (which is our only log channel afterwards).
 *
 * CRITICAL (v7-v9 bootloop, 2026-08-26): adbd is a daemon that never exits on
 * SIGTERM, and it is OUR child with root/cwd in the vendor-ramdisk rootfs.
 * first_stage_init's switch_root cannot free the old root while any process
 * holds it -> boot failure -> slot retries exhausted -> fastboot. Kill with
 * SIGKILL and reap before exec; verify the exit, not just the send. */
static pid_t g_adbd_pid;

static void kill_adbd(void) {
  if (g_adbd_pid <= 0)
    return;
  if (kill(g_adbd_pid, 0) != 0 && errno == ESRCH) {
    g_adbd_pid = 0; /* already gone */
    return;
  }
  kill(g_adbd_pid, SIGKILL);
  for (int i = 0; i < 100; i++) { /* reap or confirm death within 1s */
    if (kill(g_adbd_pid, 0) != 0 && errno == ESRCH)
      break;
    int st;
    pid_t r = waitpid(g_adbd_pid, &st, WNOHANG);
    if (r == g_adbd_pid || (r < 0 && errno == ECHILD))
      break;
    usleep(10000);
  }
  char b[96];
  snprintf(b, sizeof b, "aginxos-trampoline: adbd killed: %s\n",
           (kill(g_adbd_pid, 0) != 0 && errno == ESRCH) ? "yes" : "STILL ALIVE");
  kmsg(b);
  g_adbd_pid = 0;
}

static int g_bound; /* set only after a successful UDC bind */

static void cleanup_gadget(void) {
  kmsg("aginxos-trampoline: gadget cleanup begin\n");
  /* NEVER write "" to the UDC file unless WE bound it. v17 (mkdir g1 + this
   * unconditional write) bootlooped while v18 (no g1) booted - prime suspect
   * for a kernel panic in the unbind path of this 4.19 gadget code. */
  if (g_bound || exists("/aginxos/.usb-bound")) {
    /* v25: when usb_console ran in the forked child, the parent's copy of
     * g_bound is 0 — the child drops this flag file on a successful bind. */
    wf("/config/usb_gadget/g1/UDC", "");
  } else
    kmsg("aginxos-trampoline: never bound - skipping UDC unbind write\n");
  kill_adbd();
  umount2("/dev/usb-ffs/adb", MNT_DETACH);
  rmdir("/dev/usb-ffs/adb");
  rmdir("/dev/usb-ffs");
  unlink("/config/usb_gadget/g1/configs/b.1/ffs.adb");
  rmdir("/config/usb_gadget/g1/configs/b.1");
  rmdir("/config/usb_gadget/g1/functions/ffs.adb");
  rmdir("/config/usb_gadget/g1/functions");
  rmdir("/config/usb_gadget/g1/strings/0x409");
  rmdir("/config/usb_gadget/g1/strings");
  rmdir("/config/usb_gadget/g1/configs");
  rmdir("/config/usb_gadget/g1");
  rmdir("/config/usb_gadget");
  umount2("/config", MNT_DETACH);
  kmsg("aginxos-trampoline: gadget cleanup done\n");
}

/* mkdir that refuses to fail silently (2026-08-27 v25 postmortem: the ffs
 * symlink died with ENOENT because one of these had failed with no log line;
 * configfs default groups make EEXIST routine, everything else is a signal). */
static int mk(const char *path) {
  int r = mkdir(path, 0755);
  if (r != 0 && errno != EEXIST) {
    char b[160];
    snprintf(b, sizeof b, "aginxos-trampoline: mkdir %s errno=%d\n", path,
             errno);
    kmsg(b);
  }
  return r;
}

/* wf() with failure logging for the gadget-tree writes. */
static int wf_log(const char *path, const char *val) {
  int r = wf(path, val);
  if (r != 0) {
    char b[200];
    snprintf(b, sizeof b, "aginxos-trampoline: write %s errno=%d\n", path, -r);
    kmsg(b);
  }
  return r;
}

static void usb_console(void) {
  kmsg("aginxos-trampoline: usb console begin\n");

  /* 1. controller chain (topological per modules.dep); failures are logged
   *    by load_list_file and are non-fatal — worst case no UDC appears. */
  if (exists("/aginxos/usb-nomods")) {
    /* Bisect v21 (2026-08-27): mkdir g1 alone bootloops WITH our module chain
     * loaded (v17). This gate skips module loading entirely (no UDC will
     * ever appear, so wait_udc is bypassed) - separates "gadget registration
     * panics on its own" from "module chain + gadget interact". */
    kmsg("aginxos-trampoline: modules SKIPPED (usb-nomods bisect v21)\n");
  } else if (exists("/aginxos/modules.usb"))
    load_list_file("/aginxos/modules.usb", 0);
  else
    kmsg("aginxos-trampoline: no /aginxos/modules.usb list\n");

  /* 2. wait for the dwc3 UDC to register */
  char udc[64];
  int have_udc = exists("/aginxos/usb-nomods") ? 0
                                                : (wait_udc(udc, sizeof udc, 300) == 0);
  if (have_udc) {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: udc=%s\n", udc);
    kmsg(b);
    /* Runtime supplier state: pull-up/enumeration depends on smb5 seeing VBUS
     * and the role being peripheral, not just on dwc3_probe() succeeding. */
    dump_extcon();
  } else {
    kmsg("aginxos-trampoline: usb NO UDC — controller did not come up\n");
    dump_extcon();
    dump_regulators();
  }

  /* probe verdict mode: no display (msm_drm never modesets from rdinit on
   * redfin), no working ramoops, no LED without camera HAL. The only visible
   * channel is time-to-reboot: UDC found → reboot immediately (device visibly
   * restarts); timeout → HOLD forever (frozen logo = negative). */
  if (exists("/aginxos/usb-probe")) {
    if (have_udc) {
      kmsg("aginxos-trampoline: PROBE VERDICT GREEN — rebooting\n");
      sync();
      sleep(1);
      syscall(SYS_reboot, 0xfee1dead /*LINUX_REBOOT_MAGIC1*/,
              0x28121969 /*LINUX_REBOOT_MAGIC2*/, 0x01234567 /*RB_AUTOBOOT*/, NULL);
      for (;;)
        pause();
    }
    kmsg("aginxos-trampoline: PROBE VERDICT RED — holding forever\n");
    for (;;)
      sleep(1);
  }
  if (!have_udc)
    return;

  /* 3. configfs gadget g1 (stock recovery IDs)
   *
   * SETTLE BEFORE TREE (2026-08-27): creating /config/usb_gadget/g1 right
   * after the module loads bootloops the device (v17: mkdir g1 + modules =
   * reboot loop; v21: mkdir g1 with NO modules = fine). Our finit_module
   * spam leaves dwc3/smb5/pdphy probes still in flight when gadget
   * registration runs - stock never does this because its configfs writes
   * happen seconds after coldplug, with extcon settled. Give async probe 3s
   * to land before touching configfs. */
  sleep(3);
  dump_extcon();
  mkdir("/config", 0755);
  if (mount("none", "/config", "configfs", 0, "") != 0 && errno != EBUSY) {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: configfs mount errno=%d\n",
             errno);
    kmsg(b);
    return;
  }
  dump_dir("/config");
  if (exists("/aginxos/usb-configfs-only")) {
    /* Bisect v13 (2026-08-26): v12 proved the ffs mount + adbd fork + bind are
     * NOT the handoff breaker (all skipped, still bootlooped). This mode
     * mounts configfs and creates NOTHING under it - splits "mount" from
     * "gadget tree" (mkdirs/writes/symlink). */
    kmsg("aginxos-trampoline: configfs mounted, tree SKIPPED (usb-configfs-only bisect)\n");
    return;
  }
  mkdir("/config/usb_gadget", 0755);
  if (!exists("/config/usb_gadget")) {
    kmsg("aginxos-trampoline: no usb_gadget in configfs (CONFIG_USB_CONFIGFS?)\n");
    dump_dir("/config");
    return;
  }
  if (exists("/aginxos/usb-g1-only")) {
    /* Bisect v14 (2026-08-26): v13 proved configfs mount is safe. v12 proved
     * the gadget tree is the handoff breaker. Create ONLY the g1 directory -
     * if this still bootloops, mkdir g1 (gadget registration) is the culprit;
     * if Android boots, it is something further down (strings/functions/
     * configs/ffs.adb/symlink). */
    kmsg("aginxos-trampoline: g1 mkdir done, rest SKIPPED (usb-g1-only bisect)\n");
    dump_dir("/config/usb_gadget");
    return;
  }
  if (exists("/aginxos/usb-nog1")) {
    /* Bisect v18 (2026-08-27): mkdir g1 alone bootloops (v17). Control run:
     * stop exactly where v14 did (usb_gadget dir only, no g1) to confirm the
     * v14 success still reproduces - guards against an unrelated regression. */
    kmsg("aginxos-trampoline: usb_gadget dir only, NO g1 (usb-nog1 control)\n");
    return;
  }
  mkdir("/config/usb_gadget/g1", 0755);
  if (exists("/aginxos/usb-mkg1-only")) {
    /* Bisect v17 (2026-08-27): v14's gate sat BEFORE this mkdir, so g1 was
     * never actually tested - v15/v16 (mkdir g1 + writes) both bootlooped.
     * This gate isolates mkdir g1 itself with zero property writes. */
    kmsg("aginxos-trampoline: mkdir g1 done, no writes (usb-mkg1-only bisect)\n");
    return;
  }
  wf_log("/config/usb_gadget/g1/idVendor", "0x18d1");
  wf_log("/config/usb_gadget/g1/idProduct", "0xd001");
  if (exists("/aginxos/usb-vidpid-only")) {
    /* Bisect v16 (2026-08-27): v14 (g1 mkdir only) OK; v15 (g1 + 4 prop
     * writes) bootlooped. This gate keeps only idVendor+idProduct to split
     * the four writes in half. */
    char b[128];
    snprintf(b, sizeof b, "aginxos-trampoline: vid/pid written, rest SKIPPED (v16)\n");
    kmsg(b);
    return;
  }
  wf_log("/config/usb_gadget/g1/bcdDevice", "0x0100");
  wf_log("/config/usb_gadget/g1/bcdUSB", "0x0200");
  if (exists("/aginxos/usb-props-only")) {
    /* Bisect v15 (2026-08-26): g1 dir alone was safe (v14). This mode adds
     * the idVendor/idProduct/bcd* + strings writes but creates NO functions
     * or configs dirs - splits "property writes" from "tree structure". */
    kmsg("aginxos-trampoline: props written, no functions/configs (usb-props-only bisect)\n");
    return;
  }
  mk("/config/usb_gadget/g1/strings");
  mk("/config/usb_gadget/g1/strings/0x409");
  wf_log("/config/usb_gadget/g1/strings/0x409/serialnumber", "aginxosredfin");
  wf_log("/config/usb_gadget/g1/strings/0x409/manufacturer", "AginxOS");
  wf_log("/config/usb_gadget/g1/strings/0x409/product", "aginxos-redfin");
  mk("/config/usb_gadget/g1/functions");
  mk("/config/usb_gadget/g1/functions/ffs.adb");
  mk("/config/usb_gadget/g1/configs");
  mk("/config/usb_gadget/g1/configs/b.1");
  wf_log("/config/usb_gadget/g1/configs/b.1/MaxPower", "500");
  /* configfs symlink targets resolve via kern_path() from the CALLER's cwd
   * (v25: "../../functions/ffs.adb" resolved from / -> ENOENT, gadget build
   * silently aborted one step from the bind). Stock init.usb.rc uses an
   * absolute source path; so must we. */
  if (symlink("/config/usb_gadget/g1/functions/ffs.adb",
              "/config/usb_gadget/g1/configs/b.1/ffs.adb") != 0 &&
      errno != EEXIST) {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: ffs symlink errno=%d\n", errno);
    kmsg(b);
    return;
  }

  /* 4. functionfs + adbd (adbd must open endpoints before UDC bind) */
  if (exists("/aginxos/usb-noffs")) {
    /* Bisect v12 (2026-08-26): diag handoff works; v7-v11 (USBADB) all
     * bootlooped. v9 proved bind is not the cause. This mode stops after the
     * configfs gadget tree - if Android boots, the root cause is the ffs
     * mount or the adbd child, not configfs. */
    kmsg("aginxos-trampoline: ffs+adbd SKIPPED (usb-noffs bisect mode)\n");
    return;
  }
  mkdir("/dev/usb-ffs", 0755);
  mkdir("/dev/usb-ffs/adb", 0755);
  if (mount("adb", "/dev/usb-ffs/adb", "functionfs", 0, "uid=2000,gid=2000") !=
      0) {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: functionfs mount errno=%d\n",
             errno);
    kmsg(b);
    return;
  }
  /* v32: A14 adbd forces the RSA auth handshake because it decides via
   * __android_log_is_debuggable() -> property ro.debuggable, which reads
   * as "0" when no property area exists (the "ro.adb.secure" property of
   * older docs is gone — the string appears in none of this device's
   * binaries). The framework socket that would deliver trusted keys never
   * appears in the rdinit env, and this device's libadbd_auth.so contains
   * ZERO code references to /data/misc/adb/adb_keys (dead strings — the
   * file was seeded in v31 and provably ignored). So skip the handshake
   * entirely: stage a real property area (pulled from this device's own
   * Android boot, values patched by scripts/patch-prop-area.py) with
   * ro.debuggable=1 -> auth off, and ro.secure=0 -> should_drop_privileges()
   * keeps adbd root. */
  stage_property_area();
  ensure_devpts();
  /* adbd's shell service execs /system/bin/sh -c, but the vendor ramdisk
   * ships no shell (only adbd + libs) — an authorized connection would
   * immediately fail every command. The device's own toybox (pulled from
   * /system/bin, deps all present in the ramdisk lib64 set) serves as sh
   * and applets via symlink-name dispatch. getprop included: lets us read
   * back the staged property area from inside the rdinit console. */
  if (exists("/aginxos/toybox")) {
    static const char *const applets[] = {
        "sh", "getprop", "id",   "ls",  "cat", "dmesg", "uname",
        "ps", "env",     "echo", "pwd", "head", "grep", NULL,
    };
    for (unsigned i = 0; applets[i]; i++) {
      char p[40];
      snprintf(p, sizeof p, "/system/bin/%s", applets[i]);
      if (symlink("/aginxos/toybox", p) != 0 && errno != EEXIST)
        kmsg("aginxos-trampoline: toybox applet symlink failed\n");
    }
    kmsg("aginxos-trampoline: toybox linked as /system/bin/sh\n");
  }
  pid_t pid = fork();
  g_adbd_pid = pid;
  if (pid == 0) {
    /* v28: adbd died SIGABRT ~50 ms after exec (v27) and its reason died
     * with it — PID 1 has no console, so fd 2 went nowhere. Bionic's
     * linker and adbd's own fatal() both print to stderr; wire 0/1/2 to
     * /dev/kmsg (NO O_CLOEXEC — the fd must survive execve). */
    int kfd = open("/dev/kmsg", O_WRONLY);
    if (kfd >= 0) {
      dup2(kfd, 0);
      dup2(kfd, 1);
      dup2(kfd, 2);
      if (kfd > 2)
        close(kfd);
    }
    char *a[] = {"/system/bin/adbd", NULL};
    char *env[] = {"PATH=/system/bin", "LD_LIBRARY_PATH=/system/lib64", NULL};
    execve(a[0], a, env);
    _exit(127); /* exec failed — die quietly, parent logs via ep1 timeout */
  }
  for (int t = 0; t < 40; t++) { /* adbd writes ep0 + opens ep1/ep2 */
    int st;
    pid_t r = waitpid(pid, &st, WNOHANG);
    if (r == pid) {
      /* v26 postmortem prep: if adbd dies before opening ep0 (no property
       * area, linker/env issues), the ffs function has no descriptors and
       * the UDC bind cannot enumerate. Record HOW it died. */
      char b[128];
      if (WIFEXITED(st))
        snprintf(b, sizeof b, "aginxos-trampoline: adbd EXITED code=%d\n",
                 WEXITSTATUS(st));
      else if (WIFSIGNALED(st))
        snprintf(b, sizeof b, "aginxos-trampoline: adbd KILLED signal=%d\n",
                 WTERMSIG(st));
      else
        snprintf(b, sizeof b, "aginxos-trampoline: adbd status=%d\n", st);
      kmsg(b);
      g_adbd_pid = 0;
      break;
    }
    if (exists("/dev/usb-ffs/adb/ep1"))
      break;
    usleep(50000);
  }
  if (!exists("/dev/usb-ffs/adb/ep1"))
    kmsg("aginxos-trampoline: adbd did not open ep1 (exec ok?)\n");
  {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: adbd alive: %s\n",
             (g_adbd_pid > 0 && kill(g_adbd_pid, 0) == 0) ? "yes" : "no");
    kmsg(b);
  }

  /* 5. bind UDC - device enumerates on host now */
  if (exists("/aginxos/usb-nobind")) {
    /* Bisect mode (v9, 2026-08-26): binding the UDC was suspected of breaking
     * the first_stage handoff. Skip the bind, hand off, and read this run's
     * full kmsg from Android - it carries every diagnostic line above. */
    char p[128];
    kmsg("aginxos-trampoline: UDC bind SKIPPED (usb-nobind diag mode)\n");
    snprintf(p, sizeof p, "/sys/class/udc/%s/state", udc);
    kmsg_file(p, "udc-state");
    kmsg_file("/config/usb_gadget/g1/UDC", "gadget-udc");
    return;
  }
  if (wf("/config/usb_gadget/g1/UDC", udc) == 0) {
    g_bound = 1;
    /* v25: parent (PID 1) reads this — its g_bound copy stays 0 across fork. */
    int bfd = open("/aginxos/.usb-bound", O_WRONLY | O_CREAT | O_CLOEXEC, 0600);
    if (bfd >= 0)
      close(bfd);
    kmsg("aginxos-trampoline: usb gadget BOUND — adb should enumerate\n");
  } else {
    char b[96];
    snprintf(b, sizeof b, "aginxos-trampoline: UDC bind FAILED errno-ish=%d\n",
             -wf("/config/usb_gadget/g1/UDC", udc));
    kmsg(b);
  }
  /* 6. settle and record the kernel-side enumeration state (state goes
   *    "not attached" -> "addressed"/"configured" only if the host sees us).
   *    The long second window is a live-observation slot: if the gadget
   *    works, the host can see it (and us) before cleanup unbinds it. */
  {
    char p[128];
    sleep(3);
    snprintf(p, sizeof p, "/sys/class/udc/%s/state", udc);
    kmsg_file(p, "udc-state");
    snprintf(p, sizeof p, "/sys/class/udc/%s/current_speed", udc);
    kmsg_file(p, "udc-speed");
    kmsg_file("/config/usb_gadget/g1/UDC", "gadget-udc");
    sleep(22);
    snprintf(p, sizeof p, "/sys/class/udc/%s/state", udc);
    kmsg_file(p, "udc-state-late");
    snprintf(p, sizeof p, "/sys/class/udc/%s/current_speed", udc);
    kmsg_file(p, "udc-speed-late");
  }
}

static void backlight_max(void) {
  const char *paths[] = {
      "/sys/class/backlight/panel0-backlight/brightness",
      "/sys/class/backlight/panel0-backlight/max_brightness",
      NULL,
  };
  const char *br = paths[0];
  const char *mx = paths[1];
  char buf[16];
  int fd = open(mx, O_RDONLY);
  if (fd >= 0) {
    ssize_t n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n > 0) {
      buf[n] = 0;
      fd = open(br, O_WRONLY);
      if (fd >= 0) {
        write(fd, buf, strlen(buf));
        close(fd);
        kmsg("aginxos-trampoline: backlight max\n");
        return;
      }
    }
  }
  fd = open(br, O_WRONLY);
  if (fd >= 0) {
    write(fd, "4095\n", 5);
    close(fd);
  }
  /* Also try any backlight under class */
  DIR *d = opendir("/sys/class/backlight");
  if (d) {
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
      if (e->d_name[0] == '.')
        continue;
      char p[180];
      snprintf(p, sizeof p, "/sys/class/backlight/%s/brightness", e->d_name);
      fd = open(p, O_WRONLY);
      if (fd >= 0) {
        write(fd, "4095\n", 5);
        close(fd);
      }
    }
    closedir(d);
  }
}

static void splash_sequence(void) {
  if (!exists("/aginxos/splash")) {
    kmsg("aginxos-trampoline: splash disabled\n");
    return;
  }
  ensure_fs();
  dump_dir("/dev");
  dump_dir("/lib/modules");

  /*
   * Do NOT mmap /dev/mem at cont_splash_region — that bootlooped on redfin
   * (CONFIG_DEVMEM / reserved-memory is not a safe userspace FB).
   */

  /*
   * Module policy:
   *  - /aginxos/load-modules-loadfile → stock modules.load through msm_drm
   *  - /aginxos/load-modules + modules.allow → allow-list
   *  - /aginxos/load-modules-full → entire modules.load (riskier)
   */
  if (exists("/aginxos/load-modules-full") &&
      exists("/lib/modules/modules.load")) {
    kmsg("aginxos-trampoline: loading full modules.load\n");
    load_list_file("/lib/modules/modules.load", 0);
  } else if (exists("/aginxos/load-modules-loadfile") &&
             exists("/lib/modules/modules.load")) {
    kmsg("aginxos-trampoline: loading modules.load through msm_drm\n");
    load_list_file("/lib/modules/modules.load", 1);
  } else if (exists("/aginxos/load-modules") &&
             exists("/aginxos/modules.allow")) {
    kmsg("aginxos-trampoline: loading modules.allow\n");
    load_list_file("/aginxos/modules.allow", 0);
  } else {
    kmsg("aginxos-trampoline: splash without module load\n");
  }

  coldplug_drm();
  /* Settle probe */
  for (int i = 0; i < 50; i++) {
    if (exists("/dev/dri/card0") || exists("/sys/class/drm/card0"))
      break;
    if ((i % 10) == 0)
      coldplug_drm();
    usleep(100000);
  }
  dump_dir("/dev/dri");
  dump_dir("/sys/class/drm");
  dump_dir("/sys/class/backlight");
  dump_dir("/sys/class/graphics");
  backlight_max();

  /* One long green first (easy to notice), then optional extras if OK. */
  const uint32_t colors[] = {
      0x0000ff00, /* green */
      0x00ff0000, /* red */
      0x000000ff, /* blue */
  };
  int any_ok = 0;
  for (unsigned i = 0; i < sizeof colors / sizeof colors[0]; i++) {
    char b[72];
    snprintf(b, sizeof b, "aginxos-trampoline: frame %u color=%08x\n", i,
             colors[i]);
    kmsg(b);
    if (drm_solid(colors[i]) == 0) {
      any_ok = 1;
      sleep(i == 0 ? 4 : 2);
    } else {
      kmsg("aginxos-trampoline: frame paint failed\n");
      usleep(500000);
      coldplug_drm();
      if (drm_solid(colors[i]) == 0) {
        any_ok = 1;
        sleep(3);
      } else if (i == 0) {
        /* first frame failed — don't burn time on more colors */
        break;
      }
    }
  }
  if (any_ok) {
    kmsg("aginxos-trampoline: splash SUCCESS\n");
    sleep(1);
  } else {
    kmsg("aginxos-trampoline: splash FAILED all frames\n");
  }
}

int main(int argc, char **argv, char **envp) {
  (void)argc;
  (void)argv;
  kmsg("aginxos-trampoline: start v5\n");
  /* Do NOT mount proc/sysfs on the plain-handoff path. first_stage_init does
   * its own switch_root and an extra mount topology breaks it (regression
   * found 2026-08-26). Only the paths below that need /sys or /proc mount
   * their own copies via ensure_fs(). */

  /* Debug console first — it must exist before anything else can go wrong. */
  int usb_on = exists("/aginxos/usb-adb");
  int diag_on = exists("/aginxos/usb-diag");
  if (usb_on) {
    ensure_fs();
    /* v25 (2026-08-27): run the entire gadget bring-up in a CHILD process.
     * Every prior USB run (v6-v24) failed with no surviving log: any fault in
     * PID 1 — our own segfault just as much as a kernel panic — takes the
     * kernel down ("Attempted to kill init"), and the ring buffer dies on the
     * reboot (ramoops is dead on this unit). In a child, a userspace fault
     * only kills the child: PID 1 records the waitpid status and hands off,
     * so adb bugreport shows both WHERE the child stopped (kmsg stage
     * markers) and HOW it died (signal vs clean exit). Separates "our
     * userspace bug" from "kernel panics on configfs interaction" in one
     * flash. */
    pid_t c = fork();
    if (c == 0) {
      usb_console();
      /* adbd (if forked) is OUR child; PID 1's g_adbd_pid copy is 0, so the
       * parent's cleanup_gadget can never kill it. Die clean — except under
       * HOLD, where the whole point is keeping the console alive. */
      if (!exists("/aginxos/hold"))
        kill_adbd();
      _exit(0);
    }
    if (c < 0) {
      kmsg("aginxos-trampoline: fork failed - usb console inline\n");
      usb_console();
    } else {
      kmsg("aginxos-trampoline: usb child spawned\n");
      int st = 0, seen = 0;
      for (int t = 0; t < 1800; t++) { /* 180 s: ~25 s chain + 30 s observe */
        pid_t r = waitpid(c, &st, WNOHANG);
        if (r == c) {
          seen = 1;
          break;
        }
        if (r < 0 && errno == ECHILD)
          break;
        usleep(100000);
      }
      char b[128];
      if (seen) {
        if (WIFEXITED(st))
          snprintf(b, sizeof b, "aginxos-trampoline: usb child exited code=%d\n",
                   WEXITSTATUS(st));
        else if (WIFSIGNALED(st))
          snprintf(b, sizeof b,
                   "aginxos-trampoline: usb child DIED signal=%d%s\n",
                   WTERMSIG(st), WCOREDUMP(st) ? " (core)" : "");
        else
          snprintf(b, sizeof b, "aginxos-trampoline: usb child status=%d\n", st);
      } else {
        snprintf(b, sizeof b,
                 "aginxos-trampoline: usb child STUCK >180s - proceeding\n");
      }
      kmsg(b);
    }
  } else if (diag_on) {
    ensure_fs();
    usb_diag();
    /* Undo our mounts before handoff: first_stage builds its own topology and
     * extra mounted /proc+/sys broke its switch_root (2026-08-26). kmsg is
     * dead after this point (devtmpfs detached) — last diag line is logged
     * above, by design. */
    umount2("/sys/kernel/debug", MNT_DETACH);
    umount2("/sys", MNT_DETACH);
    umount2("/proc", MNT_DETACH);
    umount2("/dev", MNT_DETACH);
  }

  if (exists("/aginxos/hold")) {
    kmsg("aginxos-trampoline: HOLD (no handoff)\n");
    if (exists("/aginxos/splash"))
      splash_sequence();
    if (exists("/aginxos/aginxos-init")) {
      /* PID 1 takeover (2026-08-27): exec the Rust init in place. The usb
       * console already lives in its forked child, so exec here does not
       * touch it; /proc /sys /dev stay mounted and aginxos-init's
       * ensure_basics is idempotent. exec failure falls through to the C
       * hold loop — PID 1 must never exit. */
      kmsg("aginxos-trampoline: exec aginxos-init (PID 1 takeover)\n");
      char *a[] = {"/aginxos/aginxos-init", NULL};
      execve("/aginxos/aginxos-init", a, envp);
      kmsg("aginxos-trampoline: exec aginxos-init FAILED - C hold loop\n");
    }
    for (int n = 0;; n++) {
      if ((n % 10) == 0)
        kmsg("aginxos-trampoline: still holding\n");
      sleep(1);
    }
  }

  /* Unwind the gadget before Android builds its own USB stack on the same
   * controller — a leftover ffs mount or bound UDC breaks stock adbd, which
   * is our only channel for reading this run's ring buffer. */
  if (usb_on && exists("/aginxos/usb-nocleanup")) {
    /* Bisect v19 (2026-08-27): mkdir g1 then handoff = reboot loop (v17);
     * mkdir g1 + cleanup = same (v15/v16). Hypothesis: kernel panic, not a
     * userspace hang (a hung boot would not re-enter fastboot; slot-retry
     * exhaustion implies actual reboots). This gate skips cleanup entirely -
     * if v19 boots, the panic is in the TEARDOWN (rmdir gadget/umount
     * configfs -> dwc3 gadget teardown); if it still reboots, mkdir g1
     * itself panics. */
    kmsg("aginxos-trampoline: cleanup SKIPPED (usb-nocleanup bisect v19)\n");
  } else if (usb_on) {
    cleanup_gadget();
    /* Same regression class as the diag path (2026-08-26): first_stage_init's
     * switch_root breaks if our ensure_fs mounts are still live. The diag path
     * unmounts before exec; the usb path must too (found via v7 bootloop ->
     * fastboot fallback). kmsg is dead after this point, by design. */
    umount2("/sys/kernel/debug", MNT_DETACH);
    umount2("/sys", MNT_DETACH);
    umount2("/proc", MNT_DETACH);
    umount2("/dev", MNT_DETACH);
  }

  splash_sequence();

  /* Pre-loaded modules poison Android first_stage → unload first.
   * Exception: USB mode (console or diag) keeps the chain loaded — the
   * poisoning set is the display modules, and eud is load-only anyway
   * (rmmod eud panics this kernel). */
  if (g_nloaded > 0 && !usb_on && !diag_on)
    unload_loaded_modules();
  else if (g_nloaded > 0 && usb_on)
    kmsg("aginxos-trampoline: usb console on — skipping module unload\n");

  kmsg("aginxos-trampoline: exec first_stage\n");
  {
    char *a[] = {"/init", NULL};
    execve("/aginxos/first_stage_init", a, envp);
  }
  kmsg("aginxos-trampoline: execve failed\n");
  for (;;)
    sleep(1);
  return 1;
}
