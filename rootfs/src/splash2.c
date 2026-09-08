/* DRM splash v2 — paints via the connector's real mode, not a synthetic one.
 * v1 failed because nothing ever did a KMS mode set on this panel (enabled=
 * disabled, no CRTC with mode_valid); the bootloader logo persists via
 * cont-splash scanout, independent of KMS. v2: GETCONNECTOR probe -> mode[0]
 * -> GETENCODER -> possible_crtcs -> dumb fb -> SETCRTC. Logs every step.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include <stdarg.h>
#include <stdlib.h>

static void kmsg(const char *s) {
  int fd = open("/dev/kmsg", O_WRONLY | O_CLOEXEC);
  if (fd < 0) return;
  write(fd, s, strlen(s));
  close(fd);
}
static void kmsgf(const char *fmt, ...) {
  char b[160];
  va_list ap;
  va_start(ap, fmt);
  vsnprintf(b, sizeof b, fmt, ap);
  va_end(ap);
  kmsg(b);
}

#define DRM_IOCTL_BASE 'd'
#define DRM_IOWR(nr, type) _IOC(_IOC_READ|_IOC_WRITE, DRM_IOCTL_BASE, (nr), sizeof(type))
#define DRM_IO(nr) _IO(DRM_IOCTL_BASE, (nr))

struct drm_mode_card_res {
  uint64_t fb_id_ptr, crtc_id_ptr, connector_id_ptr, encoder_id_ptr;
  uint32_t count_fbs, count_crtcs, count_connectors, count_encoders;
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
  uint32_t count_connectors, crtc_id, fb_id, x, y, gamma_size, mode_valid;
  struct drm_mode_modeinfo mode;
};
struct drm_mode_get_connector {
  uint64_t encoders_ptr, modes_ptr, props_ptr, prop_values_ptr;
  int count_modes, count_props, count_encoders;
  uint32_t encoder_id, connector_id, connector_type, connector_type_id;
  uint32_t pad;
};
struct drm_mode_get_encoder {
  uint32_t encoder_id, encoder_type, crtc_id;
  uint32_t possible_crtcs, possible_clones;
};
struct drm_mode_create_dumb {
  uint32_t height, width, bpp, flags, handle, pitch;
  uint64_t size;
};
struct drm_mode_map_dumb { uint32_t handle, pad; uint64_t offset; };
struct drm_mode_fb_cmd2 {
  uint32_t fb_id, width, height, pixel_format, flags;
  uint32_t handles[4], pitches[4], offsets[4];
  uint64_t modifier[4];
};
#define DRM_IOCTL_MODE_GETRESOURCES DRM_IOWR(0xA0, struct drm_mode_card_res)
#define DRM_IOCTL_MODE_GETCRTC DRM_IOWR(0xA1, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_SETCRTC DRM_IOWR(0xA2, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_GETENCODER DRM_IOWR(0xA6, struct drm_mode_get_encoder)
#define DRM_IOCTL_MODE_GETCONNECTOR DRM_IOWR(0xA7, struct drm_mode_get_connector)
#define DRM_IOCTL_MODE_CREATE_DUMB DRM_IOWR(0xB2, struct drm_mode_create_dumb)
#define DRM_IOCTL_MODE_MAP_DUMB DRM_IOWR(0xB3, struct drm_mode_map_dumb)
#define DRM_IOCTL_MODE_ADDFB2 DRM_IOWR(0xB8, struct drm_mode_fb_cmd2)
#define DRM_IOCTL_SET_MASTER DRM_IO(0x1e)
#define DRM_FORMAT_XRGB8888 0x34325258u

int main(int argc, char **argv) {
  uint32_t color = (argc > 1) ? (uint32_t)strtoul(argv[1], NULL, 16) : 0x0000ff00u;
  int hold = (argc > 2); /* hold = stay in fg, repaint on demand */

  int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
  if (fd < 0) { kmsg("splash2: no card0\n"); return 1; }
  ioctl(fd, DRM_IOCTL_SET_MASTER);

  struct drm_mode_card_res res;
  memset(&res, 0, sizeof res);
  if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res)) { kmsg("splash2: GETRESOURCES fail\n"); return 1; }
  uint32_t crtcs[16], conns[16];
  if (res.count_crtcs > 16) res.count_crtcs = 16;
  if (res.count_connectors > 16) res.count_connectors = 16;
  res.crtc_id_ptr = (uint64_t)(uintptr_t)crtcs;
  res.connector_id_ptr = (uint64_t)(uintptr_t)conns;
  res.encoder_id_ptr = 0;
  res.fb_id_ptr = 0;
  /* msm_drm (4.19 sde) rejects a second GETRESOURCES if count_fbs/encoders
   * are nonzero but their pointers are null - zero those counts too.
   */
  res.count_fbs = 0;
  res.count_encoders = 0;
  if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res)) {
    kmsgf("splash2: GETRESOURCES2 fail errno=%d\n", errno);
    return 1;
  }
  kmsgf("splash2: %u crtcs %u connectors\n", res.count_crtcs, res.count_connectors);

  /* Probe each connector; take a connected one WITH a bound encoder and
   * modes. Conn 46 (virtual) reports enc=0 and garbage modes - skip those.
   */
  uint32_t conn_id = 0, enc_id = 0;
  struct drm_mode_modeinfo mode;
  memset(&mode, 0, sizeof mode);
  struct drm_mode_modeinfo modes[8];
  uint64_t encs[8];
  for (uint32_t i = 0; i < res.count_connectors; i++) {
    struct drm_mode_get_connector gc;
    memset(&gc, 0, sizeof gc);
    gc.connector_id = conns[i];
    gc.encoders_ptr = (uint64_t)(uintptr_t)encs;
    gc.count_encoders = 8;
    gc.modes_ptr = (uint64_t)(uintptr_t)modes;
    gc.count_modes = 8;
    if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &gc)) continue;
    if (!gc.encoder_id || gc.count_modes < 1) continue;
    if (gc.connector_type == 0 /*DRM_MODE_CONNECTOR_Unknown*/) continue;
    /* take the first good one; prefer DSI-type (16) */
    if (!conn_id || gc.connector_type == 16) {
      conn_id = conns[i];
      enc_id = gc.encoder_id;
      mode = modes[0];
      if (gc.connector_type == 16) break;
    }
  }
  if (!conn_id) { kmsg("splash2: no enabled-path connector\n"); return 1; }
  kmsgf("splash2: conn=%u enc=%u mode=%ux%u '%s'\n", conn_id, enc_id,
        mode.hdisplay, mode.vdisplay, mode.name);

  /* Which CRTC can this encoder drive? */
  uint32_t crtc_id = 0;
  struct drm_mode_get_encoder ge;
  memset(&ge, 0, sizeof ge);
  ge.encoder_id = enc_id;
  if (!ioctl(fd, DRM_IOCTL_MODE_GETENCODER, &ge)) {
    if (ge.crtc_id) crtc_id = ge.crtc_id;
    else for (uint32_t c = 0; c < res.count_crtcs; c++)
      if (ge.possible_crtcs & (1u << c)) { crtc_id = crtcs[c]; break; }
  }
  if (!crtc_id) { kmsg("splash2: no crtc for encoder\n"); return 1; }
  kmsgf("splash2: crtc=%u\n", crtc_id);

  uint32_t w = mode.hdisplay, h = mode.vdisplay;
  struct drm_mode_create_dumb dumb;
  memset(&dumb, 0, sizeof dumb);
  dumb.width = w; dumb.height = h; dumb.bpp = 32;
  if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb)) { kmsg("splash2: DUMB fail\n"); return 1; }
  struct drm_mode_fb_cmd2 fb2;
  memset(&fb2, 0, sizeof fb2);
  fb2.width = w; fb2.height = h; fb2.pixel_format = DRM_FORMAT_XRGB8888;
  fb2.handles[0] = dumb.handle; fb2.pitches[0] = dumb.pitch;
  if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fb2)) { kmsg("splash2: ADDFB2 fail\n"); return 1; }
  struct drm_mode_map_dumb map;
  memset(&map, 0, sizeof map);
  map.handle = dumb.handle;
  if (ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &map)) { kmsg("splash2: MAP fail\n"); return 1; }
  uint32_t *pix = mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, map.offset);
  if (pix == MAP_FAILED) { kmsg("splash2: mmap fail\n"); return 1; }

  uint32_t pitch = dumb.pitch / 4;
  for (uint32_t y = 0; y < h; y++)
    for (uint32_t x = 0; x < w; x++) {
      int edge = x < 48 || y < 48 || x + 48 >= w || y + 48 >= h;
      pix[y * pitch + x] = edge ? 0x00ffffff : color;
    }

  uint32_t conn_list[1] = { conn_id };
  struct drm_mode_crtc sc;
  memset(&sc, 0, sizeof sc);
  sc.crtc_id = crtc_id; sc.fb_id = fb2.fb_id; sc.x = 0; sc.y = 0;
  sc.mode = mode; sc.mode_valid = 1;
  sc.set_connectors_ptr = (uint64_t)(uintptr_t)conn_list;
  sc.count_connectors = 1;
  int rc = ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &sc);
  kmsgf("splash2: SETCRTC rc=%d errno=%d %s\n", rc, errno, rc ? strerror(errno) : "OK");
  if (rc) {
    /* retry without the connector list (some drivers reject it) */
    sc.set_connectors_ptr = 0; sc.count_connectors = 0;
    rc = ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &sc);
    kmsgf("splash2: SETCRTC-retry rc=%d %s\n", rc, rc ? strerror(errno) : "OK");
  }
  if (hold) for (;;) sleep(3600);
  close(fd);
  return rc ? 1 : 0;
}
