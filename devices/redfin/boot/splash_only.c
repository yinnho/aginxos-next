/* Early-ish color splash for vendor init (after msm_drm is up). */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

static void kmsg(const char *s) {
  int fd = open("/dev/kmsg", O_WRONLY | O_CLOEXEC);
  if (fd < 0) return;
  write(fd, s, strlen(s));
  close(fd);
}

#define DRM_IOCTL_BASE 'd'
#define DRM_IOWR(nr, type) _IOC(_IOC_READ|_IOC_WRITE, DRM_IOCTL_BASE, (nr), sizeof(type))

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
#define DRM_IOCTL_MODE_CREATE_DUMB DRM_IOWR(0xB2, struct drm_mode_create_dumb)
#define DRM_IOCTL_MODE_MAP_DUMB DRM_IOWR(0xB3, struct drm_mode_map_dumb)
#define DRM_IOCTL_MODE_ADDFB2 DRM_IOWR(0xB8, struct drm_mode_fb_cmd2)
#define DRM_IOCTL_SET_MASTER _IO(DRM_IOCTL_BASE, 0x1e)
#define DRM_FORMAT_XRGB8888 0x34325258u

static int paint(uint32_t color) {
  int fd = -1;
  for (int t = 0; t < 100 && fd < 0; t++) {
    fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    if (fd < 0) usleep(50000);
  }
  if (fd < 0) { kmsg("aginxos-splash: no card0\n"); return -1; }
  ioctl(fd, DRM_IOCTL_SET_MASTER);

  struct drm_mode_card_res res;
  memset(&res, 0, sizeof res);
  if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res)) {
    kmsg("aginxos-splash: GETRESOURCES fail\n"); close(fd); return -1;
  }
  uint32_t crtcs[16], conns[16];
  if (res.count_crtcs > 16) res.count_crtcs = 16;
  if (res.count_connectors > 16) res.count_connectors = 16;
  res.crtc_id_ptr = (uint64_t)(uintptr_t)crtcs;
  res.connector_id_ptr = (uint64_t)(uintptr_t)conns;
  res.encoder_id_ptr = 0; res.fb_id_ptr = 0;
  if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res)) { close(fd); return -1; }

  uint32_t crtc_id = 0, conn_id = res.count_connectors ? conns[0] : 0;
  struct drm_mode_modeinfo mode; memset(&mode, 0, sizeof mode);
  for (uint32_t i = 0; i < res.count_crtcs; i++) {
    struct drm_mode_crtc c; memset(&c, 0, sizeof c);
    c.crtc_id = crtcs[i];
    if (ioctl(fd, DRM_IOCTL_MODE_GETCRTC, &c)) continue;
    if (!c.mode_valid || !c.mode.hdisplay) continue;
    crtc_id = crtcs[i]; mode = c.mode; break;
  }
  if (!crtc_id) {
    kmsg("aginxos-splash: no active CRTC, try 1080x2340\n");
    if (!res.count_crtcs) { close(fd); return -1; }
    crtc_id = crtcs[0];
    mode.hdisplay = 1080; mode.vdisplay = 2340;
    mode.hsync_start = 1112; mode.hsync_end = 1120; mode.htotal = 1152;
    mode.vsync_start = 2348; mode.vsync_end = 2352; mode.vtotal = 2360;
    mode.clock = (1152 * 2360 * 60) / 1000; mode.vrefresh = 60;
  }

  uint32_t w = mode.hdisplay, h = mode.vdisplay;
  char msg[96];
  snprintf(msg, sizeof msg, "aginxos-splash: %ux%u crtc=%u color=%08x\n", w, h, crtc_id, color);
  kmsg(msg);

  struct drm_mode_create_dumb dumb; memset(&dumb, 0, sizeof dumb);
  dumb.width = w; dumb.height = h; dumb.bpp = 32;
  if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb)) {
    kmsg("aginxos-splash: CREATE_DUMB fail\n"); close(fd); return -1;
  }
  struct drm_mode_fb_cmd2 fb2; memset(&fb2, 0, sizeof fb2);
  fb2.width = w; fb2.height = h; fb2.pixel_format = DRM_FORMAT_XRGB8888;
  fb2.handles[0] = dumb.handle; fb2.pitches[0] = dumb.pitch;
  if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fb2)) {
    kmsg("aginxos-splash: ADDFB2 fail\n"); close(fd); return -1;
  }
  struct drm_mode_map_dumb map; memset(&map, 0, sizeof map);
  map.handle = dumb.handle;
  if (ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &map)) { close(fd); return -1; }
  void *ptr = mmap(NULL, dumb.size, PROT_READ|PROT_WRITE, MAP_SHARED, fd, map.offset);
  if (ptr == MAP_FAILED) { close(fd); return -1; }
  uint32_t pitch = dumb.pitch / 4;
  uint32_t *pix = ptr;
  for (uint32_t y = 0; y < h; y++)
    for (uint32_t x = 0; x < w; x++) {
      int edge = x < 64 || y < 64 || x + 64 >= w || y + 64 >= h;
      pix[y * pitch + x] = edge ? 0x00ffffff : color;
    }
  munmap(ptr, dumb.size);

  uint32_t conn_list[1] = { conn_id };
  struct drm_mode_crtc crtc; memset(&crtc, 0, sizeof crtc);
  crtc.crtc_id = crtc_id; crtc.fb_id = fb2.fb_id; crtc.mode = mode; crtc.mode_valid = 1;
  if (conn_id) { crtc.set_connectors_ptr = (uint64_t)(uintptr_t)conn_list; crtc.count_connectors = 1; }
  if (ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &crtc)) {
    crtc.set_connectors_ptr = 0; crtc.count_connectors = 0;
    if (ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &crtc)) {
      kmsg("aginxos-splash: SETCRTC fail\n"); close(fd); return -1;
    }
  }
  kmsg("aginxos-splash: OK\n");
  close(fd);
  return 0;
}

int main(void) {
  kmsg("aginxos-splash: start\n");
  const uint32_t colors[] = { 0x0000ff00, 0x00ff0000, 0x000000ff };
  for (unsigned i = 0; i < 3; i++) {
    if (paint(colors[i]) == 0) sleep(2);
    else usleep(200000);
  }
  return 0;
}
