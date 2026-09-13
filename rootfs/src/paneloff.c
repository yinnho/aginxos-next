/* aginx-panel-off — server-edition headless panel powerdown (2026-09-13).
 *
 * The problem: the bootloader paints the Google splash and the kernel's
 * smooth-takeover keeps that frame scanning out forever when no DRM client
 * ever shows up (bootcard retired from the image; term is an opt-in
 * package). On this OLED the "backlight" sysfs knob is a panel-brightness
 * command, not a power switch — bl_power=1/brightness=0 read back 0 while
 * the logo keeps glowing; fb0/blank is a no-op (atomic-only driver, no
 * legacy DPMS property, probed 2026-08-31).
 *
 * The trap (live receipt 2026-09-13): a bare null SETCRTC does NOTHING
 * here — the DRM objects already read enabled=disabled under smooth
 * takeover, so the disable path early-exits and the hardware keeps
 * scanning the bootloader's registers. The working sequence is the M15
 * path with an ownership grab first: real SETCRTC with a black dumb fb
 * (driver takes over the pipeline from the bootloader state) -> null
 * SETCRTC (encoder disable -> DSI off -> panel unprepare ->
 * dsi_backlight early-dpms hooks -> touch suspend).
 *
 * Timing: msm_drm creates card0 at insmod but the DSI panel registers
 * ~60s later and the fbdev initial modeset rides panel registration, so
 * we wait for a *connected* DSI connector before touching anything.
 *
 * SET_MASTER on this driver returns EINVAL (not EBUSY) when another
 * master holds the card — any failure is treated as busy and retried on
 * a fast poll (2026-09-06 receipt).
 *
 * --hold: keep the fd open (master held) after blanking, in case the
 * last-master-close fbdev restore ever re-enables the pipeline. Default
 * is off-and-exit; the live receipt decides which mode rcS bakes.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

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
  uint32_t count_modes, count_props, count_encoders;
  uint32_t encoder_id, connector_id, connector_type, connector_type_id;
  uint32_t connection, mm_width, mm_height, subpixel, pad;
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
struct drm_mode_rmfb { uint32_t fb_id; };

#define DRM_IOCTL_BASE 'd'
#define DRM_IOWR(nr, type) _IOWR(DRM_IOCTL_BASE, nr, type)
#define DRM_IO(nr) _IO(DRM_IOCTL_BASE, nr)
#define DRM_IOCTL_SET_MASTER DRM_IO(0x1e)
#define DRM_IOCTL_MODE_GETRESOURCES DRM_IOWR(0xA0, struct drm_mode_card_res)
#define DRM_IOCTL_MODE_SETCRTC DRM_IOWR(0xA2, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_GETENCODER DRM_IOWR(0xA6, struct drm_mode_get_encoder)
#define DRM_IOCTL_MODE_GETCONNECTOR DRM_IOWR(0xA7, struct drm_mode_get_connector)
#define DRM_IOCTL_MODE_CREATE_DUMB DRM_IOWR(0xB2, struct drm_mode_create_dumb)
#define DRM_IOCTL_MODE_MAP_DUMB DRM_IOWR(0xB3, struct drm_mode_map_dumb)
#define DRM_IOCTL_MODE_ADDFB2 DRM_IOWR(0xB8, struct drm_mode_fb_cmd2)
#define DRM_IOCTL_MODE_RMFB DRM_IOWR(0xAF, struct drm_mode_rmfb)

#define DRM_FORMAT_XRGB8888 0x34325258u /* 'XR24' */
#define DRM_MODE_CONNECTOR_DSI 16
#define DRM_MODE_CONNECTED 1

static void kmsg(const char *m) {
  int k = open("/dev/kmsg", O_WRONLY | O_CLOEXEC);
  if (k >= 0) { (void)!write(k, m, strlen(m)); close(k); }
}

/* Two-pass GETRESOURCES with the msm_drm quirk: the second call fails if
 * count_fbs/encoders are nonzero while their pointers stay null. */
static int get_resources(int fd, struct drm_mode_card_res *res,
                         uint32_t *crtcs, uint32_t *conns) {
  memset(res, 0, sizeof *res);
  if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, res)) return -1;
  if (res->count_crtcs > 16) res->count_crtcs = 16;
  if (res->count_connectors > 16) res->count_connectors = 16;
  res->crtc_id_ptr = (uint64_t)(uintptr_t)crtcs;
  res->connector_id_ptr = (uint64_t)(uintptr_t)conns;
  res->count_fbs = 0;
  res->count_encoders = 0;
  if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, res)) return -1;
  return 0;
}

int main(int argc, char **argv) {
  int hold = argc > 1 && strcmp(argv[1], "--hold") == 0;
  uint32_t crtcs[16], conns[16];
  struct drm_mode_card_res res;

  int n = 0, fd = -1;
  while (n < 90) {
    fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    if (fd >= 0) break;
    sleep(2);
    n++;
  }
  if (fd < 0) { kmsg("panel-off: no card0 after 180s\n"); return 1; }

  n = 0;
  while (ioctl(fd, DRM_IOCTL_SET_MASTER) != 0) {
    if (++n > 100) { kmsg("panel-off: SET_MASTER never granted\n"); return 1; }
    usleep(20000);
  }

  /* Wait for the DSI panel to finish registering and grab its mode +
   * encoder + crtc (bootcard probe path, DSI-preferred). */
  uint32_t conn_id = 0, crtc_id = 0;
  struct drm_mode_modeinfo mode;
  memset(&mode, 0, sizeof mode);
  int found = 0;
  for (n = 0; n < 90 && !found; n++) {
    if (get_resources(fd, &res, crtcs, conns)) break;
    for (uint32_t i = 0; i < res.count_connectors && !found; i++) {
      struct drm_mode_get_connector gc;
      struct drm_mode_modeinfo modes[8];
      uint64_t encs[8];
      memset(&gc, 0, sizeof gc);
      gc.connector_id = conns[i];
      gc.modes_ptr = (uint64_t)(uintptr_t)modes;
      gc.count_modes = 8;
      gc.encoders_ptr = (uint64_t)(uintptr_t)encs;
      gc.count_encoders = 8;
      if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &gc)) continue;
      if (gc.connector_type != DRM_MODE_CONNECTOR_DSI ||
          gc.connection != DRM_MODE_CONNECTED || gc.count_modes < 1)
        continue;
      uint32_t e = gc.encoder_id ? gc.encoder_id
                 : (gc.count_encoders > 0 ? (uint32_t)encs[0] : 0);
      if (!e) continue;
      struct drm_mode_get_encoder ge;
      memset(&ge, 0, sizeof ge);
      ge.encoder_id = e;
      if (ioctl(fd, DRM_IOCTL_MODE_GETENCODER, &ge)) continue;
      uint32_t crtc = ge.crtc_id;
      if (!crtc)
        for (uint32_t c = 0; c < res.count_crtcs; c++)
          if (ge.possible_crtcs & (1u << c)) { crtc = crtcs[c]; break; }
      if (!crtc) continue;
      conn_id = conns[i];
      crtc_id = crtc;
      mode = modes[0];
      found = 1;
    }
    if (!found) sleep(2);
  }
  if (!found) { kmsg("panel-off: no connected DSI after 180s\n"); return 1; }

  /* One black dumb fb, mapped + zeroed explicitly. */
  struct drm_mode_create_dumb dumb;
  memset(&dumb, 0, sizeof dumb);
  dumb.width = mode.hdisplay;
  dumb.height = mode.vdisplay;
  dumb.bpp = 32;
  if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb)) { kmsg("panel-off: DUMB fail\n"); return 1; }
  struct drm_mode_map_dumb md;
  memset(&md, 0, sizeof md);
  md.handle = dumb.handle;
  if (ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &md) == 0) {
    void *p = mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, (long)md.offset);
    if (p != MAP_FAILED) { memset(p, 0, dumb.size); munmap(p, dumb.size); }
  }
  struct drm_mode_fb_cmd2 fb2;
  memset(&fb2, 0, sizeof fb2);
  fb2.width = dumb.width;
  fb2.height = dumb.height;
  fb2.pixel_format = DRM_FORMAT_XRGB8888;
  fb2.handles[0] = dumb.handle;
  fb2.pitches[0] = dumb.pitch;
  if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fb2)) { kmsg("panel-off: ADDFB2 fail\n"); return 1; }

  /* Ownership grab: real modeset with the black frame — the driver takes
   * the pipeline over from the bootloader's smooth-takeover state. */
  uint64_t conn64 = conn_id;
  struct drm_mode_crtc sc;
  memset(&sc, 0, sizeof sc);
  sc.set_connectors_ptr = (uint64_t)(uintptr_t)&conn64;
  sc.count_connectors = 1;
  sc.crtc_id = crtc_id;
  sc.fb_id = fb2.fb_id;
  sc.mode_valid = 1;
  sc.mode = mode;
  if (ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &sc)) { kmsg("panel-off: SETCRTC-on fail\n"); return 1; }
  sleep(1); /* let the off-sequence see a stable enabled state */

  /* M15 kill: null SETCRTC takes the whole pipeline down. */
  memset(&sc, 0, sizeof sc);
  sc.crtc_id = crtc_id;
  if (ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &sc)) { kmsg("panel-off: SETCRTC-off fail\n"); return 1; }
  kmsg("panel-off: pipeline down\n");

  if (hold) for (;;) pause();
  return 0;
}
