// cam-shot (M19) — Qualcomm cam_req_mgr userspace.
//
// v0 (committed): sensor probe / bus sweep via CAM_SENSOR_PROBE_CMD packets.
//
// v1 (--stream): one RAW frame through the CamX-style pipeline, no CamX:
//   probe slot N real -> CREATE_SESSION -> ACQUIRE(sensor, csiphy, ife)
//   -> ACQUIRE_HW(ife, RDI-only in_port PHY_2/RDI_0) -> LINK(sensor+ife)
//   -> sensor CONFIG DEV packets (INIT=op2 global regs, CONFIG=op4 mode regs,
//      applied immediately by KMD; STREAMON=op0 MODE_SELECT, applied at
//      START_DEV) -> csiphy CONFIG_DEV_EXTERNAL + START -> ife CONFIG DEV
//      INIT(op0: clock/csid-clock/hfr/sensor-dim blobs) + UPDATE(op1 req1:
//      io_cfg RDI_0 + cam_sync fence) -> sensor START -> SCHED_REQ(1)
//      -> cam_sync WAIT on /dev/video4 -> read pixel buffer -> dump.
//   Register lists: default front (slot 2) = mainline imx355 1640x1232
//   2-lane 24MHz mode; --rear (slot 0) = the device's own vendor-bin imx363
//   tables (2016x1136 binned, 4-lane, 24MHz MCLK), extracted from the
//   chromatix module bin — see the imx363 block below for provenance.
//   All protocol structs mirrored from techpack/camera uapi (LineageOS
//   redbull) — packed where the kernel structs are packed, natural where not.
//
// v2 (--tpg): same pipeline with CAM_ISP_IFE_IN_RES_TPG — the CSID's own
//   generator is the source (VC 0xA / DT 0x2B, kernel-fixed), sensor and
//   csiphy are never touched. A frame here proves the IFE/RDI path; no
//   frame here means the defect is in our csid/ife arm, not the sensor.
//
// Power sequences and rails come from the device DT (dumped 2026-08-31,
// see HARDWARE.md M19): sensor@0 vio/vana/vdig 2.85/vaf + reset tlmm23 +
// mclk tlmm13; sensor@1 vio/vana/vdig1.1 + reset tlmm25 + mclk tlmm14;
// sensor@2 vio + custom_gpio1 (PM8150L gpio2) + reset tlmm21 + mclk tlmm15.
// Rail voltages: config_val=0 means "keep DT min/max" (VALIDATE_VOLTAGE
// rejects 0, kernel keeps DT values) — msm_camera_fill_vreg_params maps
// seq_type -> rail index by name, missing rail -> INVALID_VREG -> skipped.

#include <fcntl.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include "jpegenc.h"

/* ---- cam_defs.h ---- */
struct cam_control {
    uint32_t op_code;
    uint32_t size;
    uint32_t handle_type;
    uint32_t reserved;
    uint64_t handle;
};
#define VIDIOC_CAM_CONTROL 0xC01856C0 /* _IOWR('V', 192, 24) */
#define CAM_QUERY_CAP   0x101
#define CAM_HANDLE_USER_POINTER 1
#define CAM_HANDLE_MEM_HANDLE   2

struct cam_packet_header {
    uint32_t op_code, size;
    uint64_t request_id;
    uint32_t flags, padding;
};
struct cam_cmd_buf_desc {
    int32_t  mem_handle;
    uint32_t offset, size, length, type, meta_data;
};
struct cam_packet {
    struct cam_packet_header header;
    uint32_t cmd_buf_offset, num_cmd_buf;
    uint32_t io_configs_offset, num_io_configs;
    uint32_t patch_offset, num_patches;
    uint32_t kmd_cmd_buf_index, kmd_cmd_buf_offset;
    uint64_t payload[1];
};

/* ---- cam_req_mgr.h (video3 opcodes + mem mgr) ---- */
#define CAM_REQ_MGR_CREATE_DEV_NODES 0x10A
#define CAM_REQ_MGR_CREATE_SESSION  0x10B
#define CAM_REQ_MGR_DESTROY_SESSION 0x10C
#define CAM_REQ_MGR_LINK            0x10D
#define CAM_REQ_MGR_UNLINK          0x10E
#define CAM_REQ_MGR_SCHED_REQ       0x10F
#define CAM_REQ_MGR_ALLOC_BUF   0x112
#define CAM_REQ_MGR_MAP_BUF     0x113
#define CAM_REQ_MGR_RELEASE_BUF 0x114
#define CAM_REQ_MGR_CACHE_OPS   0x115
#define CAM_MEM_FLAG_KMD_ACCESS  (1 << 3)
#define CAM_MEM_FLAG_CMD_BUF_TYPE (1 << 6)
struct cam_mem_mgr_alloc_cmd {
    uint64_t len, align;
    int32_t  mmu_hdls[16];
    uint32_t num_hdl, flags;
    struct { uint32_t buf_handle; int32_t fd; uint64_t vaddr; } out;
};
struct cam_mem_mgr_release_cmd { int32_t buf_handle; uint32_t reserved; };

/* ================= v1 streaming (M19) ================= */

/* generic node opcodes — cam_defs.h */
#define CAM_ACQUIRE_DEV   0x102
#define CAM_START_DEV     0x103
#define CAM_STOP_DEV      0x104
#define CAM_CONFIG_DEV    0x105
#define CAM_RELEASE_DEV   0x106
#define CAM_ACQUIRE_HW    0x151
#define CAM_CONFIG_DEV_EXTERNAL 0x201
#define CAM_API_COMPAT_CONSTANT 0xFEFEFEFE

struct cam_acquire_dev_cmd {
    int32_t  session_handle, dev_handle;
    uint32_t handle_type, num_resources;
    uint64_t resource_hdl;
};
struct cam_start_stop_dev_cmd { int32_t session_handle, dev_handle; };
struct cam_release_dev_cmd    { int32_t session_handle, dev_handle; };
struct cam_config_dev_cmd {
    int32_t  session_handle, dev_handle;
    uint64_t offset, packet_handle;
};

/* req mgr payloads — cam_req_mgr.h (natural alignment, not packed) */
struct cam_req_mgr_session_info { int32_t session_hdl, reserved; };
#define CAM_REQ_MGR_MAX_HANDLES 64
struct cam_req_mgr_link_info {
    int32_t  session_hdl;
    uint32_t num_devices;
    int32_t  dev_hdls[CAM_REQ_MGR_MAX_HANDLES];
    int32_t  link_hdl;
};
struct cam_req_mgr_unlink_info { int32_t session_hdl, link_hdl; };
struct cam_req_mgr_sched_request {
    int32_t session_hdl, link_hdl, bubble_enable, sync_mode,
            additional_timeout, reserved;
    int64_t req_id;
};

/* sensor/csiphy acquire — cam_sensor.h (packed) */
struct cam_sensor_acquire_dev {
    uint32_t session_handle, device_handle, handle_type, reserved;
    uint64_t info_handle;
} __attribute__((packed));
struct cam_csiphy_acquire_dev_info { uint32_t combo_mode, reserved; }
    __attribute__((packed));
struct cam_csiphy_info {
    uint16_t lane_mask, lane_assign;
    uint8_t  csiphy_3phase, combo_mode, lane_cnt, secure_mode;
    uint64_t settle_time, data_rate;
} __attribute__((packed));
/* cam_csiphy_query_cap_t { u32 slot_info; u16 reserved } == same layout as
 * the sensor querycap head; reuse cam_sensor_query_cap for both. */

/* IFE acquire — cam_defs.h v2 (CAM_MAX_ACQ_RES=5, CAM_MAX_HW_SPLIT=3) */
struct cam_acquire_hw_cmd_v2 {
    uint32_t struct_version, reserved;
    int32_t  session_handle, dev_handle;
    uint32_t handle_type, data_size;
    uint64_t resource_hdl;
    struct {
        uint32_t acquired_hw_id[5];
        uint32_t acquired_hw_path[5][3];
        uint32_t valid_acquired_hw;
    } hw_info;
};

/* cam_isp.h — NOT packed (all-natural u32/u64) */
struct cam_isp_out_port_info {
    uint32_t res_type, format, width, height, comp_grp_id, split_point,
             secure_mode, reserved;
};
struct cam_isp_in_port_info {
    uint32_t res_type, lane_type, lane_num, lane_cfg, vc, dt, format,
             test_pattern, usage_type, left_start, left_stop, left_width,
             right_start, right_stop, right_width, line_start, line_stop,
             height, pixel_clk, batch_size, dsp_mode, hbi_cnt, reserved,
             num_out_res;
    struct cam_isp_out_port_info data[1];
};
struct cam_isp_acquire_hw_info {
    uint16_t common_info_version, common_info_size;
    uint32_t common_info_offset, num_inputs, input_info_version,
             input_info_size, input_info_offset;
    uint64_t data;   /* in_port blob starts here (offset 24, size 32) */
};

/* io config — cam_defs.h (natural alignment) */
struct cam_plane_cfg {
    uint32_t width, height, plane_stride, slice_height, meta_stride,
             meta_size, meta_offset, packer_config, mode_config,
             tile_config, h_init, v_init;
};
struct cam_buf_io_cfg {
    int32_t  mem_handle[3];
    uint32_t offsets[3];
    struct cam_plane_cfg planes[3];
    uint32_t format, color_space, color_pattern, bpp, rotation,
             resource_type;
    int32_t  fence, early_fence;
    struct cam_cmd_buf_desc aux_cmd_buf;
    uint32_t direction, batch_size, subsample_pattern, subsample_period,
             framedrop_pattern, framedrop_period, flag, padding;
};
#define CAM_BUF_OUTPUT 2

/* isp QUERY_CAP — two-step: caps_handle points at cam_isp_query_cap_cmd,
 * kernel writes it back including the SMMU handles userspace must name in
 * cam_mem_mgr_alloc_cmd.mmu_hdls for any HW-mapped (pixel) buffer. */
struct cam_query_cap_cmd { uint32_t size, handle_type; uint64_t caps_handle; };
struct cam_iommu_handle { int32_t non_secure, secure; };
struct cam_hw_version { uint32_t major, minor, incr, reserved; };
struct cam_isp_dev_cap_info {
    uint32_t hw_type, reserved;
    struct cam_hw_version hw_version;
};
#define CAM_ISP_HW_MAX 5
struct cam_isp_query_cap_cmd {
    struct cam_iommu_handle device_iommu, cdm_iommu;
    int32_t num_dev;
    uint32_t reserved;
    struct cam_isp_dev_cap_info dev_caps[CAM_ISP_HW_MAX];
};

/* generic blob payloads — cam_isp.h (packed) */
struct cam_isp_clock_config {
    uint32_t usage_type, num_rdi;
    uint64_t left_pix_hz, right_pix_hz, rdi_hz[1];
} __attribute__((packed));
struct cam_isp_csid_clock_config { uint64_t csid_clock; }
    __attribute__((packed));
/* cam_cpas.h: one AXI path vote; cam_isp.h: V2 wrapper (packed) */
struct cam_axi_per_path_bw_vote {
    uint32_t usage_data, transac_type, path_data_type, reserved;
    uint64_t camnoc_bw, mnoc_ab_bw, mnoc_ib_bw, ddr_ab_bw, ddr_ib_bw;
} __attribute__((packed));
struct cam_isp_bw_config_v2 {
    uint32_t usage_type, num_paths;
    struct cam_axi_per_path_bw_vote axi_path[1];
} __attribute__((packed));
struct cam_isp_port_hfr_config {
    uint32_t resource_type, subsample_pattern, subsample_period,
             framedrop_pattern, framedrop_period, reserved;
} __attribute__((packed));
struct cam_isp_resource_hfr_config {
    uint32_t num_ports, reserved;
    struct cam_isp_port_hfr_config port[1];
} __attribute__((packed));
struct cam_isp_sensor_dimension {
    uint32_t width, height, measure_enabled;
} __attribute__((packed));
struct cam_isp_sensor_config_blob {
    struct cam_isp_sensor_dimension ppp_path, ipp_path, rdi_path[4];
    uint32_t hbi, vbi;
} __attribute__((packed));   /* 80 B: kernel cam_isp_sensor_config, RDI_MAX=4 */

/* IFE resource ids — uapi cam_isp_ife.h */
#define CAM_ISP_IFE_IN_RES_TPG  0x4000
#define CAM_ISP_IFE_IN_RES_BASE 0x4000
#define CAM_ISP_IFE_IN_RES_PHY_2  0x4003  /* = BASE+3: phy idx 2 (front) */
#define CAM_ISP_IFE_OUT_RES_RDI_0 0x3006
#define CAM_ISP_IFE_OUT_RES_FULL 0x3000
#define CAM_ISP_IFE_OUT_RES_RAW_DUMP 0x3003
/* ISP packet meta / blob types — cam_isp.h */
#define CAM_ISP_PACKET_META_BASE              0
/* raw CDM command payload, appended verbatim as a CDM BL entry
 * (cam_isp_add_command_buffers: META_COMMON -> hw_update_entry, CAM_ISP_IQ_BL).
 * This is the only way userspace reaches the VFE ISP modules (BLS/demosaic/
 * gamma/CCM/CSC and the module CGC overrides) — the kernel never programs
 * them itself. Payload must carry its own ChangeBase (base = the acquired
 * IFE's camera-SS-relative reg base, e.g. IFE1 = 0xB6000). */
#define CAM_ISP_PACKET_META_COMMON            3
#define CAM_ISP_PACKET_META_GENERIC_BLOB_COMMON 12
#define CAM_ISP_GENERIC_BLOB_TYPE_HFR_CONFIG            0
#define CAM_ISP_GENERIC_BLOB_TYPE_CLOCK_CONFIG          1
#define CAM_ISP_GENERIC_BLOB_TYPE_CSID_CLOCK_CONFIG     4
#define CAM_ISP_GENERIC_BLOB_TYPE_SENSOR_DIMENSION_CONFIG 11
/* formats — cam_defs.h */
#define CAM_FORMAT_MIPI_RAW_10 3
#define CAM_FORMAT_PLAIN16_10 14
#define CAM_FORMAT_NV12 32

/* cam_sync — uapi cam_sync.h (video4). NB: _IOWR('V',192,24) encodes to the
 * same nr as VIDIOC_CAM_CONTROL; the sync node reads cam_private_ioctl_arg. */
struct cam_private_ioctl_arg {
    uint32_t id, size, result, reserved;
    uint64_t ioctl_ptr;
};
struct cam_sync_info { char name[64]; int32_t sync_obj; };
struct cam_sync_wait { int32_t sync_obj; uint32_t reserved; uint64_t timeout_ms; };
#define CAM_SYNC_CREATE 0
#define CAM_SYNC_DESTROY 1
#define CAM_SYNC_WAIT 6

/* media entity types — cam_defs.h */
#define CAM_CSIPHY_DEVICE_TYPE 0x10008
#define CAM_ISP_DEVICE_TYPE    0x10002

/* I2C write mosaic — cam_sensor.h (packed). A random-wr command is
 * header{count, op_code, cmd_type, data_type, addr_type} followed by count
 * {u32 reg_addr, u32 reg_data}; the KMD parser walks the cmd buffer and
 * advances by 8 + 8*count bytes per command. */
struct i2c_rdwr_header {
    uint32_t count;
    uint8_t  op_code, cmd_type, data_type, addr_type;
} __attribute__((packed));
struct i2c_random_wr_payload { uint32_t reg_addr, reg_data; }
    __attribute__((packed));
#define CMD_I2C_RNDM_WR 5
#define I2C_OP_RNDM_WR  1
#define I2C_TYPE_BYTE   1

/* ---- cam_sensor.h ---- */
#define CAM_SENSOR_PROBE_CMD (0x109 + 1)
struct cam_cmd_i2c_info { uint32_t slave_addr; uint8_t i2c_freq_mode,
    cmd_type; uint16_t reserved; } __attribute__((packed));
struct cam_cmd_probe {
    uint8_t data_type, addr_type, op_code, cmd_type;
    uint32_t reg_addr, expected_data, data_mask;
    uint16_t camera_id; uint8_t fw_update_flag; uint16_t reserved;
} __attribute__((packed));
/* READREG payload (cam_sensor.h, packed): reg_addr to read, reg_data =
 * byte count (<=8), query_data_handle = user address the KMD copy_to_user()s
 * the raw CCI bytes into — the read happens synchronously inside the
 * CAM_CONFIG_DEV ioctl (cam_sensor_read_reg). */
struct cam_cmd_get_sensor_data {
    uint32_t reg_addr, reg_data;
    uint64_t query_size_handle, query_data_handle;
} __attribute__((packed));
struct cam_power_settings {
    uint16_t power_seq_type, reserved;
    uint32_t config_val_low, config_val_high;
} __attribute__((packed));
struct cam_cmd_power {
    uint32_t count; uint8_t reserved, cmd_type; uint16_t more_reserved;
    struct cam_power_settings power_settings[1];
} __attribute__((packed));
struct cam_cmd_unconditional_wait {
    int16_t delay, reserved; uint8_t op_code, cmd_type; uint16_t reserved1;
} __attribute__((packed));
struct cam_sensor_query_cap {
    uint32_t slot_info, secure_camera, pos_pitch, pos_roll, pos_yaw,
        actuator_slot_id, eeprom_slot_id, ois_slot_id, flash_slot_id,
        csiphy_slot_id;
} __attribute__((packed));

/* enums from cam_sensor_cmn_header.h */
enum { SENSOR_MCLK = 0, SENSOR_VANA, SENSOR_VDIG, SENSOR_VIO, SENSOR_VAF,
       SENSOR_VAF_PWDM, SENSOR_CUSTOM_REG1, SENSOR_CUSTOM_REG2,
       SENSOR_RESET = 8, SENSOR_STANDBY, SENSOR_CUSTOM_GPIO1 = 10 };
#define CMD_PWR_UP   2
#define CMD_PWR_DOWN 3
#define CMD_I2C_INFO 4
#define CMD_PROBE    1
#define CMD_WAIT     9
#define WAIT_SW_UCND 3
#define I2C_TYPE_WORD 2
#define I2C_FREQ_FAST 1

/* media controller bits (same struct as media-topo, 4.19 uapi) */
struct media_entity_desc {
    uint32_t id;
    char name[32];
    uint32_t type, revision, flags, group_id;
    uint16_t pads, links;
    uint32_t reserved[4];
    union {
        struct { uint32_t major, minor; } v4l;
        uint8_t raw[184];
    } dev;
};
#define MEDIA_IOC_ENUM_ENTITIES _IOWR('|', 0x01, struct media_entity_desc)
#define CAM_SENSOR_DEVICE_TYPE 0x10001
#define MEDIA_ENT_ID_FLAG_NEXT ((uint32_t)1 << 31)

#define MAX_SUBDEV 8

struct slot_cfg {
    const char *name;
    uint32_t addr;   /* 8-bit CCI slave address for the real probe */
    /* power-up steps: seq_type, config_val, delay_ms. config_val is 0 for
     * rails ("keep DT voltages") but must be the rate in Hz for SENSOR_MCLK:
     * the kernel only clk_set_rate()s cam_clk when config_val != 0 — with 0
     * it runs at the camcc default ("set cam_clk, rate 0", observed
     * 2026-09-01) and the sensor PLL locks to an unknown reference. DT
     * clock-rates = 24 MHz for all three sensor nodes. */
    struct { uint8_t seq; uint32_t cfg; uint16_t delay; } up[8];
    int n_up;
    /* power-down steps (applied in given order) */
    struct { uint8_t seq; uint32_t cfg; uint16_t delay; } down[8];
    int n_down;
};

static struct slot_cfg slots[3] = {
    /* slot 0 rails (DT phandles resolved): cam_vio=slg51000 ldo7 1.8V,
     * cam_vana=ldo3, cam_vdig=ldo1, cam_v_custom1=ldo4, cam_v_custom2=ldo6,
     * cam_vaf=gpio-regulator@0 "camera_ldo" (pm8150l gpio8 camera_rear_vcm_en).
     * The kernel power-up executor (cam_sensor_core_power_up) only enables
     * rails that appear in the seq — no fallback — so omitting VIO leaves
     * the sensor's I2C/DOVDD unpowered and the chip-ID read NACKs.
     * Address: IMX3xx latches one of two slave addrs from the INCK/XCLR
     * power-on timing; our seq (MCLK before XCLR) latches 0x20 — observed
     * via full-bus sweep 2026-09-01, chip id 0x363 read at 0x20. */
    [0] = { "rear-main", .addr = 0x20, .up = {
        { SENSOR_VIO,         0, 1 }, { SENSOR_VANA, 0, 1 },
        { SENSOR_VAF,         0, 0 }, { SENSOR_VDIG, 0, 1 },
        { SENSOR_CUSTOM_REG1, 0, 1 }, { SENSOR_CUSTOM_REG2, 0, 1 },
        { SENSOR_MCLK, 24000000, 1 }, { SENSOR_RESET, 1, 5 } },
        .n_up = 8, .down = {
        { SENSOR_MCLK,        0, 1 }, { SENSOR_RESET, 0, 1 },
        { SENSOR_CUSTOM_REG2, 0, 1 }, { SENSOR_CUSTOM_REG1, 0, 1 },
        { SENSOR_VDIG,        0, 1 }, { SENSOR_VAF,  0, 1 },
        { SENSOR_VANA,        0, 1 }, { SENSOR_VIO,  0, 1 } }, .n_down = 8 },
    [1] = { "rear-uw", .addr = 0x34, .up = {
        { SENSOR_VIO,   0, 1 }, { SENSOR_VANA,  0, 1 },
        { SENSOR_VDIG,  0, 1 }, { SENSOR_RESET, 1, 8 },
        { SENSOR_MCLK, 24000000, 1 } }, .n_up = 5, .down = {
        { SENSOR_MCLK,  0, 1 }, { SENSOR_RESET, 0, 5 },
        { SENSOR_VDIG,  0, 1 }, { SENSOR_VANA,  0, 1 },
        { SENSOR_VIO,   0, 1 } }, .n_down = 5 },
    [2] = { "front", .addr = 0x34, .up = {
        /* DT cam-sensor@2 (front imx355, cci1 master 0, csiphy 2, roll 270
         * / yaw 0). The node carries ONLY cam_vio + cam_clk regulators, so
         * VANA/VDIG seq entries match nothing in msm_camera_fill_vreg_params
         * and the kernel skips them silently (INVALID_VREG — confirmed
         * 2026-08-31: fill printed only "j: 0 cam_vio"; ldo2/ldo5 belong to
         * the rear ultrawide @1).
         *
         * Stock imx355_module.bin power-up (re-decoded 2026-09-01: 2-u64
         * header, (type, cfg, delay) u64-triples, 1-u64 trailer): GPIO1=1
         * d1 -> MCLK 24 MHz d1 -> RESET=1 d12; down: MCLK 0 -> GPIO1 0 ->
         * VIO 0. GPIO1 = CUSTOM_GPIO1 = pm8150l gpio2 (1101), @2's own DT
         * pin (en_rwcam is gpio10 — NOT this one), the module's master
         * enable, active high. INCK must be stable BEFORE XCLR releases.
         *
         * cap16 caveat: that run flashed a STALE binary — kmsg showed the
         * old bad table (GPIO1 cfg=0 + junk MCLK cfg=1 before the 24 MHz
         * entry), so GPIO1=0 was what actually ran: 3 NACKed id reads then
         * a good 0x355 on the retry, full config applied, still no MIPI.
         * GPIO1=1 has never actually run — verify the binary's own table
         * in kmsg "cam_sensor_update_power_settings" before trusting any
         * result. */
        { SENSOR_VIO,          0, 1 },
        { SENSOR_CUSTOM_GPIO1, 1, 1 },
        { SENSOR_MCLK,  24000000, 1 },
        { SENSOR_RESET,        1, 12 } }, .n_up = 4, .down = {
        { SENSOR_MCLK,         0, 1 },
        { SENSOR_RESET,        0, 1 },
        { SENSOR_CUSTOM_GPIO1, 0, 1 },
        { SENSOR_VIO,          0, 1 } }, .n_down = 4 },
};

/* chip ids observed on this device (HARDWARE.md 2026-08-31) */
static const uint32_t slot_id[3] = { 0x363, 0x481, 0x355 };

static const uint32_t try_addrs[] = { 0x34, 0x20, 0x10, 0x6E, 0x6C, 0x36 };

/* ---- imx355 register lists (mainline drivers/media/i2c/imx355.c) ----
 * Front camera, slot 2, slave 0x34, chip id 0x355 @ reg 0x16.
 * Mode: 1640x1232 (crop 3280x2464 @0,0, 2x2 binning), 2-lane DPHY,
 * 24 MHz MCLK -> extclk 0x1800, pll_op_mul 111, prediv 3 (888 Mbps/lane,
 * link freq 444 MHz, pixel rate 177.6 MPix/s), fll 1306, llp 1836, RAW10.
 * Width 8 = 8-bit reg, 16 = 16-bit reg; the packet builder groups by width
 * into separate I2C random-write commands. */
struct wreg { uint16_t addr; uint16_t val; uint8_t width; };

/* imx355_global_regs — sent as INITIAL_CONFIG (applied at CONFIG_DEV) */
static const struct wreg imx355_global[] = {
    {0x304e, 0x03, 8}, {0x4348, 0x16, 8}, {0x4350, 0x19, 8},
    {0x4408, 0x0a, 8}, {0x440c, 0x0b, 8}, {0x4411, 0x5f, 8},
    {0x4412, 0x2c, 8}, {0x4623, 0x00, 8}, {0x462c, 0x0f, 8},
    {0x462d, 0x00, 8}, {0x462e, 0x00, 8}, {0x4684, 0x54, 8},
    {0x480a, 0x07, 8}, {0x4908, 0x07, 8}, {0x4909, 0x07, 8},
    {0x490d, 0x0a, 8}, {0x491e, 0x0f, 8}, {0x4921, 0x06, 8},
    {0x4923, 0x28, 8}, {0x4924, 0x28, 8}, {0x4925, 0x29, 8},
    {0x4926, 0x29, 8}, {0x4927, 0x1f, 8}, {0x4928, 0x20, 8},
    {0x4929, 0x20, 8}, {0x492a, 0x20, 8}, {0x492c, 0x05, 8},
    {0x492d, 0x06, 8}, {0x492e, 0x06, 8}, {0x492f, 0x06, 8},
    {0x4930, 0x03, 8}, {0x4931, 0x04, 8}, {0x4932, 0x04, 8},
    {0x4933, 0x05, 8}, {0x595e, 0x01, 8}, {0x5963, 0x01, 8},
    {0x3030, 0x01, 8}, {0x3031, 0x01, 8}, {0x3045, 0x01, 8},
    {0x4010, 0x00, 8}, {0x4011, 0x00, 8}, {0x4012, 0x00, 8},
    {0x4013, 0x01, 8}, {0x68a8, 0xfe, 8}, {0x68a9, 0xff, 8},
    {0x6888, 0x00, 8}, {0x6889, 0x00, 8}, {0x68b0, 0x00, 8},
    {0x3058, 0x00, 8}, {0x305a, 0x00, 8},
    {0x0112, 0x0a, 8},   /* CST_SZ: 10-bit addr/data (RAW10) */
    {0x0113, 0x0a, 8},
    {0x0301, 0x05, 8},   /* PLL_IVT_PCK_DIV */
    {0x0303, 0x01, 8},   /* PLL_IVT_SYSCK_DIV (rewritten at streamon) */
    {0x0305, 0x02, 8}, {0x0306, 0x00, 8}, {0x0307, 0x78, 8},
    {0x030b, 0x01, 8},
    {0x030d, 0x02, 8},   /* PLL_OP_PREDIV default (rewritten at streamon) */
    {0x0310, 0x00, 8}, {0x0220, 0x00, 8}, {0x0222, 0x01, 8},
    {0x3088, 0x04, 8}, {0x6813, 0x02, 8}, {0x6835, 0x07, 8},
    {0x6836, 0x01, 8}, {0x6837, 0x04, 8}, {0x684d, 0x07, 8},
    {0x684e, 0x01, 8}, {0x684f, 0x04, 8},
};

/* streamon body — mode regs + crop + binning + PLL + MIPI + timing.
 * Sent as CONFIG (applied at CONFIG_DEV), in mainline order. */
static const struct wreg imx355_cfg[] = {
    {0x0700, 0x00, 8},   /* mode regs */
    {0x0701, 0x10, 8},
    {0x0344, 0, 16},     /* crop X_ADD_START */
    {0x0346, 0, 16},     /* crop Y_ADD_START */
    {0x0348, 3279, 16},  /* X_ADD_END (3280-1) */
    {0x034a, 2463, 16},  /* Y_ADD_END (2464-1) */
    {0x034c, 1640, 16},  /* X_OUT_SIZE */
    {0x034e, 1232, 16},  /* Y_OUT_SIZE */
    {0x0900, 0x01, 8},   /* binning mode (2x2 -> 0x22 != 0x11 -> 0x01) */
    {0x0901, 0x22, 8},   /* binning type */
    {0x0902, 0x00, 8},   /* binning weighting */
    /* MCLK: the DT clock-rates for this sensor ask for 24 MHz, so that is
     * the default (extclk 0x1800, mul 111, prediv 3, 888 Mbps/lane, link
     * 444 MHz). A 19.2 MHz variant (0x1333/92/2 -> 883.2 Mbps) exists in
     * mainline and stays reachable with --mclk 19; a frame-period
     * measurement once suggested 19.2 MHz but was later shown unreliable
     * (mixed print sites), so treat 24 MHz as the recorded default. */
    {0x0136, 0x1800, 16},/* EXTCLK_FREQ 24.0 MHz (8.8 fixed point) */
    {0x030e, 111, 16},   /* PLL_OP_MUL (24 MHz, 2-lane) */
    {0x030d, 3, 8},      /* PLL_OP_PREDIV (24 MHz, 2-lane) */
    {0x0303, 2, 8},      /* PLL_IVT_SYSCK_DIV (2-lane) */
    {0x0114, 1, 8},      /* LANE_SEL: 2 lanes */
    {0x0820, 1776, 16},  /* REQ_LINK_BIT_RATE MHz (444*2*2) */
    {0x3070, 1, 8},      /* DPGA_USE_GLOBAL_GAIN */
    {0x0342, 1836, 16},  /* LLP */
    {0x0340, 1306, 16},  /* FLL */
    {0x0202, 1000, 16},  /* exposure */
    {0x0204, 0, 16},     /* analog gain */
    {0x020e, 256, 16},   /* digital gain 1.0x (8.8) */
    {0x0600, 0, 16},     /* test pattern off */
};

static const struct wreg imx355_streamon[] = { {0x0100, 1, 8} };
static const struct wreg imx355_streamoff[] = { {0x0100, 0, 8} };

/* ---- imx355 vendor-bin tables (front, slot 2) ----
 * Same decoder, the device's own imx355_module.bin: initSettings #704
 * (50 writes) and mode regSetting #351 = 1640x925 2x2-binned, fll 0x0a36
 * =2614, llp 0x072c=1836, PLL 0x0305<-2 / 0x0307<-0x78(120) -> VCO
 * 24/2*120 = 1440 MHz, /10 (0x0301<-5 x 0x030d<-2) = 144 MHz pck; frame
 * 2614*1836*30fps = 144 MHz — self-consistent at 30 fps. REQ_LINK_BIT_
 * RATE 0x0820<-0x05a0 = 1440 Mbps TOTAL = 360 Mbps/lane over 4 lanes.
 * KEY DIFFERENCE vs mainline imx355.c: the vendor bin sets 0x0114<-3 =
 * 4 data lanes (mainline says 2). This device wires the front sensor on
 * 4 lanes — the old 2-lane path was misconfigured. Verbatim, no PLL
 * patching (24 MHz MCLK from DT cam-sensor@2 clock-rates 0x16e3600). */
static const struct wreg imx355_vinit[] = {
    {0x0137, 0x00, 8}, {0x304e, 0x03, 8}, {0x4348, 0x16, 8},
    {0x4350, 0x19, 8}, {0x4408, 0x0a, 8}, {0x440c, 0x0b, 8},
    {0x4411, 0x5f, 8}, {0x4412, 0x2c, 8}, {0x4623, 0x00, 8},
    {0x462c, 0x0f, 8}, {0x462d, 0x00, 8}, {0x462e, 0x00, 8},
    {0x4684, 0x54, 8}, {0x480a, 0x07, 8}, {0x4908, 0x07, 8},
    {0x4909, 0x07, 8}, {0x490d, 0x0a, 8}, {0x491e, 0x0f, 8},
    {0x4921, 0x06, 8}, {0x4923, 0x28, 8}, {0x4924, 0x28, 8},
    {0x4925, 0x29, 8}, {0x4926, 0x29, 8}, {0x4927, 0x1f, 8},
    {0x4928, 0x20, 8}, {0x4929, 0x20, 8}, {0x492a, 0x20, 8},
    {0x492c, 0x05, 8}, {0x492d, 0x06, 8}, {0x492e, 0x06, 8},
    {0x492f, 0x06, 8}, {0x4930, 0x03, 8}, {0x4931, 0x04, 8},
    {0x4932, 0x04, 8}, {0x4933, 0x05, 8}, {0x595e, 0x01, 8},
    {0x5963, 0x01, 8}, {0x0101, 0x03, 8}, {0x4010, 0x00, 8},
    {0x4011, 0x00, 8}, {0x4012, 0x00, 8}, {0x4013, 0x00, 8},
    {0x68a8, 0xfe, 8}, {0x68a9, 0xff, 8}, {0x6888, 0x00, 8},
    {0x6889, 0x00, 8}, {0x3058, 0x00, 8}, {0x305a, 0x00, 8},
    {0x68b0, 0x00, 8}, {0x3044, 0x00, 8},
};

/* mode #351 (1640x925): byte-split 16-bit regs as the bin writes them */
static const struct wreg imx355_vcfg[] = {
    {0x0113, 0x0a, 8},   /* CST_SZ 10-bit */
    {0x0114, 0x03, 8},   /* LANE_SEL: 4 lanes (vendor; mainline says 2!) */
    {0x0342, 0x07, 8}, {0x0343, 0x2c, 8},   /* LLP 1836 */
    {0x0340, 0x0a, 8}, {0x0341, 0x36, 8},   /* FLL 2614 */
    {0x0344, 0x00, 8}, {0x0345, 0x00, 8},   /* crop X 0 */
    {0x0346, 0x01, 8}, {0x0347, 0x30, 8},   /* crop Y 304 */
    {0x0348, 0x0c, 8}, {0x0349, 0xcf, 8},   /* X end 3279 */
    {0x034a, 0x08, 8}, {0x034b, 0x67, 8},   /* Y end 2151 */
    {0x0220, 0x00, 8}, {0x0222, 0x01, 8},
    {0x0900, 0x01, 8}, {0x0901, 0x22, 8}, {0x0902, 0x00, 8},
    {0x034c, 0x06, 8}, {0x034d, 0x68, 8},   /* X_OUT 1640 */
    {0x034e, 0x03, 8}, {0x034f, 0x9c, 8},   /* Y_OUT 924 */
    {0x0301, 0x05, 8}, {0x0303, 0x01, 8}, {0x0305, 0x02, 8},
    {0x0306, 0x00, 8}, {0x0307, 0x78, 8},   /* PLL mult 120 */
    {0x030b, 0x01, 8}, {0x030d, 0x02, 8}, {0x030e, 0x00, 8},
    {0x030f, 0x1e, 8}, {0x0310, 0x00, 8},
    {0x0700, 0x00, 8}, {0x0701, 0x10, 8},
    {0x0820, 0x05, 8}, {0x0821, 0xa0, 8},   /* REQ_LINK 1440 Mbps total */
    {0x3088, 0x02, 8},
    {0x6813, 0x01, 8},
    {0x6835, 0x00, 8}, {0x6836, 0x01, 8}, {0x6837, 0x02, 8},
    {0x684d, 0x00, 8}, {0x684e, 0x01, 8}, {0x684f, 0x02, 8},
    {0x0202, 0x0a, 8}, {0x0203, 0x2c, 8},   /* exposure 2600 */
    {0x0204, 0x00, 8}, {0x0205, 0x00, 8},
    {0x020e, 0x01, 8}, {0x020f, 0x00, 8},   /* digital gain 1.0x */
};

/* ---- imx481 register lists (vendor bin) ----
 * Rear ultra-wide, slot 1, slave 0x34, chip id 0x481 @ reg 0x0016.
 * Source: the device's /vendor/lib64/camera/com.qti.sensormodule.
 * metric_imx481_lito2.bin, same decoder as the imx363/imx355 tables
 * (Parameter Parser V2 TOC; init = the bin's 209-write initSettings).
 * mode3 = regSetting#1301, the ~30fps binned 2328x1310 mode:
 * fll 0x0760=1888, llp 0x1400=5120, crop (0,438)-(4655,3057), 4 lanes
 * (0x0114=3). PLL (Sony rule proven on this device's imx355+imx363:
 * lane Mbps = INCK/0x030d * (0x030e<<8|0x030f)) = 24/15*439 = 702.4
 * Mbps/lane -> pck = 702.4M*4/10 = 281 MHz, fps = 281M/(5120*1888)
 * = 29.1. Mode[4] (2016x1136) computes to ~120 fps with its PLL
 * (24/3*230 = 1840 Mbps/lane) — the vendor's fast mode; mode0 full-
 * res cross-checks the rule exactly (24/4*218 = 1308 = 523.2 MHz
 * pck x 10 / 4, the rate the bin's own resolutionData declares).
 * The bin never writes 0x0820 REQ_LINK or 0x0136 INCK — verbatim, like
 * the rear --rawvendor path. Exposure 0x0202=0x0494 lines, gain 1x. */

static const struct wreg imx481_init[] = {
    {0x0137, 0x00, 8},
    {0x3c7e, 0x01, 8},
    {0x3c7f, 0x06, 8},
    {0x0101, 0x03, 8},
    {0x3f7f, 0x01, 8},
    {0x531c, 0x01, 8},
    {0x531d, 0x02, 8},
    {0x531e, 0x04, 8},
    {0x5928, 0x00, 8},
    {0x5929, 0x2f, 8},
    {0x592a, 0x00, 8},
    {0x592b, 0x85, 8},
    {0x592c, 0x00, 8},
    {0x592d, 0x32, 8},
    {0x592e, 0x00, 8},
    {0x592f, 0x88, 8},
    {0x5930, 0x00, 8},
    {0x5931, 0x3d, 8},
    {0x5932, 0x00, 8},
    {0x5933, 0x93, 8},
    {0x5938, 0x00, 8},
    {0x5939, 0x24, 8},
    {0x593a, 0x00, 8},
    {0x593b, 0x7a, 8},
    {0x593c, 0x00, 8},
    {0x593d, 0x24, 8},
    {0x593e, 0x00, 8},
    {0x593f, 0x7a, 8},
    {0x5940, 0x00, 8},
    {0x5941, 0x2f, 8},
    {0x5942, 0x00, 8},
    {0x5943, 0x85, 8},
    {0x5e12, 0x00, 8},
    {0x5e13, 0x23, 8},
    {0x5f06, 0x08, 8},
    {0x5f07, 0x81, 8},
    {0x5f0b, 0xeb, 8},
    {0x5f0c, 0xae, 8},
    {0x5f0d, 0x15, 8},
    {0x5f0e, 0x6e, 8},
    {0x5f0f, 0x03, 8},
    {0x5f10, 0xa5, 8},
    {0x5f11, 0xc6, 8},
    {0x5f12, 0x92, 8},
    {0x5f13, 0xb9, 8},
    {0x5f14, 0x5e, 8},
    {0x5f17, 0x5e, 8},
    {0x5f18, 0xdc, 8},
    {0x5f19, 0x23, 8},
    {0x5f1a, 0xdb, 8},
    {0x5f1b, 0xc7, 8},
    {0x5f1c, 0x5b, 8},
    {0x5f1d, 0x7e, 8},
    {0x5f1e, 0x20, 8},
    {0x5f1f, 0x51, 8},
    {0x5f20, 0xa2, 8},
    {0x5f21, 0x46, 8},
    {0x5f22, 0x87, 8},
    {0x5f23, 0x2c, 8},
    {0x5f24, 0x1d, 8},
    {0x5f25, 0x10, 8},
    {0x5f26, 0x76, 8},
    {0x5f27, 0xa1, 8},
    {0x5f28, 0xc6, 8},
    {0x5f29, 0x07, 8},
    {0x5f2a, 0x1a, 8},
    {0x5f2b, 0x1c, 8},
    {0x5f2c, 0xa8, 8},
    {0x5f2d, 0x76, 8},
    {0x5f2e, 0x61, 8},
    {0x5f2f, 0xc6, 8},
    {0x5f30, 0x87, 8},
    {0x5f31, 0x2c, 8},
    {0x5f32, 0x1d, 8},
    {0x5f33, 0x10, 8},
    {0x5f34, 0x76, 8},
    {0x5f35, 0xa1, 8},
    {0x5f36, 0xc6, 8},
    {0x5f37, 0x07, 8},
    {0x5f38, 0x1a, 8},
    {0x5f39, 0x1c, 8},
    {0x5f3a, 0xa8, 8},
    {0x5f3b, 0x76, 8},
    {0x5f3c, 0xa1, 8},
    {0x5f3d, 0xc6, 8},
    {0x5f3e, 0x87, 8},
    {0x5f3f, 0x2c, 8},
    {0x5f40, 0x1d, 8},
    {0x5f41, 0x10, 8},
    {0x5f42, 0x76, 8},
    {0x5f43, 0xa1, 8},
    {0x5f44, 0xc6, 8},
    {0x5f45, 0x07, 8},
    {0x5f46, 0x2a, 8},
    {0x5f47, 0x1d, 8},
    {0x5f48, 0x08, 8},
    {0x5f49, 0x76, 8},
    {0x5f4a, 0x81, 8},
    {0x5f4b, 0xc0, 8},
    {0x5f75, 0x27, 8},
    {0x5f76, 0xee, 8},
    {0x5f77, 0xee, 8},
    {0x5f78, 0xee, 8},
    {0x5f79, 0xe5, 8},
    {0x7990, 0x01, 8},
    {0x7993, 0x5d, 8},
    {0x7994, 0x5d, 8},
    {0x7995, 0xa1, 8},
    {0x799a, 0x01, 8},
    {0x799d, 0x00, 8},
    {0x8169, 0x01, 8},
    {0x8359, 0x01, 8},
    {0x88c7, 0x00, 8},
    {0x88d4, 0x03, 8},
    {0x9300, 0x2a, 8},
    {0x9301, 0x24, 8},
    {0x9302, 0x1e, 8},
    {0x9304, 0x2c, 8},
    {0x9305, 0x23, 8},
    {0x9306, 0x1f, 8},
    {0x9308, 0x2d, 8},
    {0x9309, 0x28, 8},
    {0x930a, 0x26, 8},
    {0x930c, 0x2e, 8},
    {0x930d, 0x2c, 8},
    {0x930e, 0x23, 8},
    {0x9310, 0x2e, 8},
    {0x9311, 0x28, 8},
    {0x9312, 0x23, 8},
    {0x9314, 0x31, 8},
    {0x9315, 0x31, 8},
    {0x9316, 0x2c, 8},
    {0x9317, 0x19, 8},
    {0x9960, 0x00, 8},
    {0x9963, 0x64, 8},
    {0x9964, 0x50, 8},
    {0xa391, 0x04, 8},
    {0xb046, 0x01, 8},
    {0xb048, 0x01, 8},
    {0x42b0, 0x00, 8},
    {0x4bd7, 0x14, 8},
    {0x42aa, 0xff, 8},
    {0x428a, 0x00, 8},
    {0x510c, 0x01, 8},
    {0x8145, 0x00, 8},
    {0x8146, 0x04, 8},
    {0x8341, 0x00, 8},
    {0x8343, 0x08, 8},
    {0xa801, 0x00, 8},
    {0xa802, 0x00, 8},
    {0xa903, 0x00, 8},
    {0xa905, 0x00, 8},
    {0xa909, 0x00, 8},
    {0xa90b, 0x00, 8},
    {0xa925, 0x02, 8},
    {0xa927, 0x02, 8},
    {0xa929, 0x02, 8},
    {0xa92b, 0x00, 8},
    {0xa92d, 0x00, 8},
    {0xa92f, 0x00, 8},
    {0xa933, 0x00, 8},
    {0xa935, 0x00, 8},
    {0xa939, 0x00, 8},
    {0xa93b, 0x00, 8},
    {0xa955, 0x02, 8},
    {0xa957, 0x02, 8},
    {0xa959, 0x02, 8},
    {0xa95b, 0x00, 8},
    {0xa95d, 0x00, 8},
    {0xa95f, 0x00, 8},
    {0xa963, 0x00, 8},
    {0xa965, 0x00, 8},
    {0xa969, 0x00, 8},
    {0xa96b, 0x00, 8},
    {0xa985, 0x02, 8},
    {0xa987, 0x02, 8},
    {0xa989, 0x02, 8},
    {0xa98b, 0x00, 8},
    {0xa98d, 0x00, 8},
    {0xa98f, 0x00, 8},
    {0xaa06, 0x3f, 8},
    {0xaa07, 0x05, 8},
    {0xaa08, 0x04, 8},
    {0xaa12, 0x3f, 8},
    {0xaa13, 0x04, 8},
    {0xaa14, 0x03, 8},
    {0xab55, 0x02, 8},
    {0xab57, 0x01, 8},
    {0xab59, 0x01, 8},
    {0xabb4, 0x00, 8},
    {0xabb5, 0x01, 8},
    {0xabb6, 0x00, 8},
    {0xabb7, 0x01, 8},
    {0xabb8, 0x00, 8},
    {0xabb9, 0x01, 8},
    {0xae08, 0x00, 8},
    {0xae0b, 0x00, 8},
    {0xae0e, 0x00, 8},
    {0xae11, 0x00, 8},
    {0xae14, 0x00, 8},
    {0xae1a, 0x00, 8},
    {0xae2e, 0x00, 8},
    {0xae31, 0x00, 8},
    {0xae37, 0x00, 8},
    {0xae40, 0x00, 8},
    {0xae54, 0x00, 8},
    {0xae57, 0x00, 8},
    {0xae5d, 0x00, 8},
    {0xae66, 0x00, 8},
};
static const struct wreg imx481_mode3[] = {
    {0x0113, 0x0a, 8},
    {0x0114, 0x03, 8},
    {0x0342, 0x14, 8},
    {0x0343, 0x00, 8},
    {0x0340, 0x07, 8},
    {0x0341, 0x60, 8},
    {0x0344, 0x00, 8},
    {0x0345, 0x00, 8},
    {0x0346, 0x01, 8},
    {0x0347, 0xb6, 8},
    {0x0348, 0x12, 8},
    {0x0349, 0x2f, 8},
    {0x034a, 0x0b, 8},
    {0x034b, 0xf1, 8},
    {0x0381, 0x01, 8},
    {0x0383, 0x01, 8},
    {0x0385, 0x01, 8},
    {0x0387, 0x01, 8},
    {0x0900, 0x01, 8},
    {0x0901, 0x22, 8},
    {0x0902, 0x0a, 8},
    {0x3f4c, 0x05, 8},
    {0x3f4d, 0x03, 8},
    {0x0408, 0x00, 8},
    {0x0409, 0x00, 8},
    {0x040a, 0x00, 8},
    {0x040b, 0x00, 8},
    {0x040c, 0x09, 8},
    {0x040d, 0x18, 8},
    {0x040e, 0x05, 8},
    {0x040f, 0x1e, 8},
    {0x034c, 0x09, 8},
    {0x034d, 0x18, 8},
    {0x034e, 0x05, 8},
    {0x034f, 0x1e, 8},
    {0x0301, 0x06, 8},
    {0x0303, 0x02, 8},
    {0x0305, 0x04, 8},
    {0x0306, 0x01, 8},
    {0x0307, 0x22, 8},
    {0x030b, 0x01, 8},
    {0x030d, 0x0f, 8},
    {0x030e, 0x01, 8},
    {0x030f, 0xb7, 8},
    {0x0310, 0x01, 8},
    {0x3e20, 0x01, 8},
    {0x3e37, 0x01, 8},
    {0x3e3b, 0x00, 8},
    {0x38a3, 0x02, 8},
    {0x38ac, 0x01, 8},
    {0x38ad, 0x01, 8},
    {0x38ae, 0x01, 8},
    {0x38af, 0x01, 8},
    {0x38b0, 0x01, 8},
    {0x38b1, 0x01, 8},
    {0x38b2, 0x01, 8},
    {0x38b3, 0x01, 8},
    {0x38b4, 0x03, 8},
    {0x38b5, 0xa4, 8},
    {0x38b6, 0x02, 8},
    {0x38b7, 0x0c, 8},
    {0x38b8, 0x05, 8},
    {0x38b9, 0x76, 8},
    {0x38ba, 0x03, 8},
    {0x38bb, 0x12, 8},
    {0x38bc, 0x03, 8},
    {0x38bd, 0x6a, 8},
    {0x38be, 0x01, 8},
    {0x38bf, 0xec, 8},
    {0x38c0, 0x05, 8},
    {0x38c1, 0xb0, 8},
    {0x38c2, 0x03, 8},
    {0x38c3, 0x34, 8},
    {0x38c4, 0x03, 8},
    {0x38c5, 0x2e, 8},
    {0x38c6, 0x01, 8},
    {0x38c7, 0xca, 8},
    {0x38c8, 0x05, 8},
    {0x38c9, 0xe8, 8},
    {0x38ca, 0x03, 8},
    {0x38cb, 0x54, 8},
    {0x38cc, 0x02, 8},
    {0x38cd, 0xba, 8},
    {0x38ce, 0x01, 8},
    {0x38cf, 0x8a, 8},
    {0x38d0, 0x06, 8},
    {0x38d1, 0x5e, 8},
    {0x38d2, 0x03, 8},
    {0x38d3, 0x96, 8},
    {0x38d4, 0x03, 8},
    {0x38d5, 0x2e, 8},
    {0x38d6, 0x01, 8},
    {0x38d7, 0xe4, 8},
    {0x38d8, 0x05, 8},
    {0x38d9, 0x3a, 8},
    {0x38da, 0x03, 8},
    {0x38db, 0x12, 8},
    {0x38dc, 0x03, 8},
    {0x38dd, 0xc6, 8},
    {0x38de, 0x01, 8},
    {0x38df, 0xe4, 8},
    {0x38e0, 0x05, 8},
    {0x38e1, 0xd2, 8},
    {0x38e2, 0x03, 8},
    {0x38e3, 0x12, 8},
    {0x38e4, 0x03, 8},
    {0x38e5, 0x2e, 8},
    {0x38e6, 0x02, 8},
    {0x38e7, 0x0c, 8},
    {0x38e8, 0x05, 8},
    {0x38e9, 0x3a, 8},
    {0x38ea, 0x03, 8},
    {0x38eb, 0x3a, 8},
    {0x38ec, 0x03, 8},
    {0x38ed, 0xc6, 8},
    {0x38ee, 0x02, 8},
    {0x38ef, 0x0c, 8},
    {0x38f0, 0x05, 8},
    {0x38f1, 0xea, 8},
    {0x38f2, 0x03, 8},
    {0x38f3, 0x3a, 8},
    {0x3f78, 0x02, 8},
    {0x3f79, 0x0b, 8},
    {0x3ffe, 0x00, 8},
    {0x3fff, 0x14, 8},
    {0x5f0a, 0xb2, 8},
    {0xa828, 0x03, 8},
    {0xa829, 0x03, 8},
    {0xa84f, 0x01, 8},
    {0xa850, 0x01, 8},
    {0xb2df, 0x12, 8},
    {0xb2e5, 0x06, 8},
    {0x0202, 0x07, 8},
    {0x0203, 0x4e, 8},
    {0x0204, 0x00, 8},
    {0x0205, 0x00, 8},
    {0x020e, 0x01, 8},
    {0x020f, 0x00, 8},
};

/* ---- imx363 register lists (vendor bin, NOT mainline) ----
 * Rear camera, slot 0, slave 0x20 (INCK/XCLR latch, observed), chip id
 * 0x363 @ reg 0x0016. Source: the device's own vendor chromatix bin
 * /vendor/lib64/camera/com.qti.sensormodule.*imx363*.bin — a "Parameter
 * Parser V2.0.0" container decoded by hand (TOC of 72B entries + per-
 * element u64 streams; pairing rule addr[k] <- value of element k-1,
 * verified against mainline imx355.c register-for-register). The vendor
 * bin is this module's own tuning — the borrowed ChromeOS imx355 global
 * list corrupted the front module's MIPI TX, so vendor tables are the
 * only trustworthy source.
 *
 * MCLK: rear DT cam-sensor@0 clock-rates = 0x16e3600 = 24 MHz, same as
 * cam-shot's power table. Vendor PLL for this mode: mult 0x0307=207,
 * prediv 0x0305=4 -> VCO 24/4*207 = 1248 MHz, /sysck(2)/pck(3) = 208 MHz
 * pixel clock; FLL 0x0674=1652, LLP 0x1050=4176 -> 33.2 ms/frame ~= 30 fps
 * — the tables are self-consistent for 24 MHz INCK, no retuning needed.
 * MIPI (Sony rule link/lane = pck*10/lanes, cross-checked on imx355):
 * 520 Mbps/lane over 4 DPHY lanes. Exposure 0x0666 lines (~32.9 ms). */

/* initSettings #2958 in the bin TOC: 29 single-byte writes, applied in
 * standby as INITIAL_CONFIG. 0x0112/0x0113 CST_SZ (10-bit) prepended —
 * the bin's head pair is the 16-bit EXTCLK write whose hi byte 0x0136
 * falls off the k-1 pairing edge (lo 0x0137<-0x00 matches EXTCLK 0x1800
 * = 24 MHz in 8.8 fixed point). */
static const struct wreg imx363_init[] = {
    {0x0112, 0x0a, 8},
    {0x0137, 0x00, 8},
    {0x31a3, 0x00, 8},
    {0x64d4, 0x01, 8}, {0x64d5, 0xaa, 8}, {0x64d6, 0x01, 8},
    {0x64d7, 0xa9, 8}, {0x64d8, 0x01, 8}, {0x64d9, 0xa5, 8},
    {0x64da, 0x01, 8}, {0x64db, 0xa1, 8},
    {0x720a, 0x24, 8}, {0x720b, 0x89, 8}, {0x720c, 0x85, 8},
    {0x720d, 0xa1, 8}, {0x720e, 0x6e, 8},
    {0x729c, 0x59, 8},
    {0x817c, 0xff, 8}, {0x817d, 0x80, 8},
    {0x9348, 0x96, 8}, {0x934b, 0x8c, 8}, {0x934c, 0x82, 8},
    {0x9353, 0xaa, 8}, {0x9354, 0xaa, 8},
    {0x5872, 0x00, 8}, {0x5873, 0x0c, 8},
    {0x4b67, 0xff, 8}, {0x4bd0, 0x00, 8},
    {0x0138, 0x00, 8},
    {0x5d0c, 0x01, 8},
};

/* mode regSetting #544: 2016x1136 16:9, 2x2 binning (0x0900<-01/0x0901<-22),
 * crop (0,376)-(4031,2647) of the 4032x3024 array, RAW10, 4 lanes
 * (0x0114<-3), PLL mult 207. First real-sensor target: smallest sane
 * frame (2.86 MB), 30 fps at 24 MHz MCLK. */
static const struct wreg imx363_cfg[] = {
    {0x0113, 0x0a, 8},
    {0x0114, 0x03, 8},    /* LANE_SEL: 4 lanes */
    {0x0220, 0x00, 8}, {0x0221, 0x11, 8},   /* digital gain 1.06x */
    {0x0340, 0x06, 8}, {0x0341, 0x74, 8},   /* FLL 1652 */
    {0x0342, 0x10, 8}, {0x0343, 0x50, 8},   /* LLP 4176 */
    {0x0381, 0x01, 8}, {0x0383, 0x01, 8},
    {0x0385, 0x01, 8}, {0x0387, 0x01, 8},
    {0x0900, 0x01, 8},    /* binning mode */
    {0x0901, 0x22, 8},    /* binning type 2x2 */
    {0x30e4, 0x00, 8}, {0x30e8, 0x00, 8}, {0x30ea, 0x09, 8},
    {0x30f4, 0x01, 8}, {0x30f5, 0xcc, 8},
    {0x30f6, 0x00, 8}, {0x30f7, 0x14, 8},
    {0x31a0, 0x03, 8}, {0x31a5, 0x00, 8}, {0x31a6, 0x00, 8},
    {0x560f, 0xe6, 8},
    {0x5856, 0x04, 8}, {0x58d0, 0x0e, 8},
    {0x734a, 0x23, 8}, {0x734f, 0x64, 8}, {0x7441, 0x5a, 8},
    {0x7914, 0x02, 8}, {0x7928, 0x08, 8}, {0x7929, 0x08, 8},
    {0x793f, 0x02, 8},
    {0xbc7b, 0x2c, 8},
    {0x0344, 0x00, 8}, {0x0345, 0x00, 8},   /* X_ADD_START 0 */
    {0x0346, 0x01, 8}, {0x0347, 0x78, 8},   /* Y_ADD_START 376 */
    {0x0348, 0x0f, 8}, {0x0349, 0xbf, 8},   /* X_ADD_END 4031 */
    {0x034a, 0x0a, 8}, {0x034b, 0x57, 8},   /* Y_ADD_END 2647 */
    {0x034c, 0x07, 8}, {0x034d, 0xe0, 8},   /* X_OUT_SIZE 2016 */
    {0x034e, 0x04, 8}, {0x034f, 0x70, 8},   /* Y_OUT_SIZE 1136 */
    {0x0101, 0x03, 8},
    {0x0408, 0x00, 8}, {0x0409, 0x00, 8},
    {0x040a, 0x00, 8}, {0x040b, 0x00, 8},
    {0x040c, 0x07, 8}, {0x040d, 0xe0, 8},   /* DOL out width 2016 */
    {0x040e, 0x04, 8}, {0x040f, 0x70, 8},
    {0x319c, 0x00, 8}, {0x7819, 0x00, 8},
    {0x8118, 0x00, 8}, {0x8119, 0x02, 8}, {0x811b, 0x01, 8},
    {0x0301, 0x03, 8},    /* IVT_PCK_DIV */
    {0x0303, 0x02, 8},    /* IVT_SYSCK_DIV */
    {0x0305, 0x04, 8},    /* IVT_PREPLLCK_DIV */
    {0x0306, 0x00, 8}, {0x0307, 0xcf, 8},   /* PLL mult 207 */
    {0x0309, 0x0a, 8}, {0x030b, 0x01, 8},
    {0x030d, 0x04, 8}, {0x030e, 0x01, 8}, {0x030f, 0x32, 8},
    {0x0310, 0x01, 8},
    {0x0202, 0x06, 8}, {0x0203, 0x66, 8},   /* exposure 1638 lines */
    {0x0224, 0x01, 8}, {0x0225, 0xf4, 8},
    {0x0204, 0x00, 8}, {0x0205, 0x00, 8},   /* analog gain 0 */
    {0x0216, 0x00, 8}, {0x0217, 0x00, 8},
    {0x020e, 0x01, 8}, {0x020f, 0x00, 8},   /* digital gain */
    {0x0226, 0x00, 8}, {0x0227, 0x00, 8},
};

/* Vendor-bin mode #2610 (decoded 2026-09-01): same 2016x1136 output, but the
 * MIPI lane rate is 24 MHz/0x030d(4)*OP_MUL(0x00bc=188) = 1128 Mbps/lane vs
 * mode #544's 1836. KEY: 0x0307 is the *pixel-clock* PLL mult (pck check:
 * 24*78/2/3 = 312 MHz = llp 4176 * fll 2488 * 30 fps exactly) while the lane
 * rate lives in 0x030e:0x030f — so the earlier --halfrate (0x0307 207->104)
 * never lowered the MIPI rate and the rate-margin hypothesis was never
 * actually tested. If headers ECC-clean at 1128 Mbps, corruption is SI at
 * 1836, not protocol/config. */
static const struct wreg imx363_mode2610[] = {
    {0x0113, 0x0a, 8}, {0x0114, 0x03, 8},
    {0x0220, 0x00, 8}, {0x0221, 0x11, 8},
    {0x0340, 0x09, 8}, {0x0341, 0xb8, 8},   /* fll 2488 */
    {0x0342, 0x10, 8}, {0x0343, 0x50, 8},   /* llp 4176 */
    {0x0381, 0x01, 8}, {0x0383, 0x01, 8},
    {0x0385, 0x01, 8}, {0x0387, 0x01, 8},
    {0x0900, 0x01, 8}, {0x0901, 0x22, 8},
    {0x30e4, 0x00, 8}, {0x30e8, 0x00, 8}, {0x30ea, 0x09, 8},
    {0x30f4, 0x01, 8}, {0x30f5, 0xcc, 8},
    {0x30f6, 0x00, 8}, {0x30f7, 0x14, 8},
    {0x31a0, 0x03, 8}, {0x31a5, 0x00, 8}, {0x31a6, 0x00, 8},
    {0x560f, 0xe6, 8}, {0x5856, 0x04, 8}, {0x58d0, 0x0e, 8},
    {0x734a, 0x23, 8}, {0x734f, 0x64, 8}, {0x7441, 0x5a, 8},
    {0x7914, 0x02, 8}, {0x7928, 0x08, 8}, {0x7929, 0x08, 8},
    {0x793f, 0x02, 8}, {0xbc7b, 0x2c, 8},
    {0x0344, 0x00, 8}, {0x0345, 0x00, 8},   /* x start 0 */
    {0x0346, 0x01, 8}, {0x0347, 0x78, 8},   /* y start 376 */
    {0x0348, 0x0f, 8}, {0x0349, 0xbf, 8},   /* x end 4031 */
    {0x034a, 0x0a, 8}, {0x034b, 0x57, 8},   /* y end 2647 */
    {0x034c, 0x07, 8}, {0x034d, 0xe0, 8},   /* out x 2016 */
    {0x034e, 0x04, 8}, {0x034f, 0x70, 8},   /* out y 1136 */
    {0x0101, 0x03, 8},
    {0x0408, 0x00, 8}, {0x0409, 0x00, 8},
    {0x040a, 0x00, 8}, {0x040b, 0x00, 8},
    {0x040c, 0x07, 8}, {0x040d, 0xe0, 8},
    {0x040e, 0x04, 8}, {0x040f, 0x70, 8},
    {0x319c, 0x00, 8}, {0x7819, 0x00, 8},
    {0x8118, 0x00, 8}, {0x8119, 0x02, 8}, {0x811b, 0x01, 8},
    {0x0301, 0x03, 8}, {0x0303, 0x02, 8},
    {0x0305, 0x04, 8}, {0x0306, 0x00, 8},
    {0x0307, 0x4e, 8},                       /* pck VCO mult 78 */
    {0x0309, 0x0a, 8},
    {0x030b, 0x02, 8},
    {0x030d, 0x04, 8},                       /* OP prediv */
    {0x030e, 0x00, 8}, {0x030f, 0xbc, 8},   /* OP_MUL 188 -> 1128 Mbps/lane */
    {0x0310, 0x01, 8},
    {0x0202, 0x09, 8}, {0x0203, 0xaa, 8},   /* exposure 2474 lines */
    {0x0224, 0x01, 8}, {0x0225, 0xf4, 8},
    {0x0204, 0x00, 8}, {0x0205, 0x00, 8},
    {0x0216, 0x00, 8}, {0x0217, 0x00, 8},
    {0x020e, 0x01, 8}, {0x020f, 0x00, 8},
    {0x0226, 0x01, 8}, {0x0227, 0x00, 8},
};

/* masterSettings (bin TOC #2900, decoded 2026-09-01): 8 writes the vendor
 * applies when @0 runs as sync MASTER — which is its stock role for the
 * rear trio (slaveSettings differ ONLY in 0x30a1<-0 / 0x5875<-0). Without
 * them the imx363's external-sync block sits at reset defaults and gates
 * frame readout: clock lane runs (PLL+PHY up, rx 0xd040ff) but no valid
 * long packets — the exact pre-master state observed 2026-09-01. The
 * stream's 9th element (value 9) pairs circularly with dropped head addr
 * 0x30a0; held back until the 8 proven writes prove insufficient. */
static const struct wreg imx363_master[] = {
    {0x30a1, 0x01, 8},
    {0x5875, 0x01, 8},
    {0x5879, 0x01, 8},
    {0x3310, 0x01, 8},
    {0x5874, 0x01, 8},
    {0x3316, 0x0c, 8},
    {0x3317, 0x38, 8},
    {0x0350, 0x00, 8},
};

/* IMX363 stream-on/off is the Sony MODE_SELECT 0x0100, same convention
 * as imx355 — reuse the imx355_streamon/off tables for both sensors. */

/* ---- helpers ---- */
/* NB: video3 (cam_req_mgr) checks size == sizeof(payload struct), so the
 * size field must carry the payload size, not sizeof(cam_control). */
static int cam_ioctl(int fd, uint32_t op, void *arg, uint32_t htype,
                     uint32_t size)
{
    struct cam_control ctl = { .op_code = op, .size = size,
        .handle_type = htype, .reserved = 0,
        .handle = (uint64_t)(uintptr_t)arg };
    return ioctl(fd, VIDIOC_CAM_CONTROL, &ctl);
}

static int alloc_buf(int video_fd, uint64_t len, uint64_t align,
                     struct cam_mem_mgr_alloc_cmd *out)
{
    memset(out, 0, sizeof(*out));
    out->len = len;
    out->align = align;
    out->num_hdl = 0;
    out->flags = CAM_MEM_FLAG_KMD_ACCESS | CAM_MEM_FLAG_CMD_BUF_TYPE;
    if (cam_ioctl(video_fd, CAM_REQ_MGR_ALLOC_BUF, out,
                  CAM_HANDLE_USER_POINTER, sizeof(*out)) < 0) {
        fprintf(stderr, "alloc_buf(%llu) failed: %s\n",
            (unsigned long long)len, strerror(errno));
        return -1;
    }
    return 0;
}

static void release_buf(int video_fd, uint32_t hdl)
{
    struct cam_mem_mgr_release_cmd rel = { .buf_handle = (int32_t)hdl };
    cam_ioctl(video_fd, CAM_REQ_MGR_RELEASE_BUF, &rel,
              CAM_HANDLE_USER_POINTER, sizeof(rel));
}

static void *map_fd(int fd, size_t len)
{
    void *p = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (p == MAP_FAILED)
        fprintf(stderr, "mmap failed: %s\n", strerror(errno));
    return p;
}

/* kmsg tail: print CAM_SENSOR lines newer than the given monotonic us. */
static double kmsg_drain(int fd, double since_us)
{
    char buf[512];
    double max = since_us;
    while (1) {
        ssize_t n = read(fd, buf, sizeof(buf) - 1);
        if (n <= 0)
            break;
        buf[n] = 0;
        double ts;
        if (sscanf(buf, "%*d,%*llu,%lf;%*s", &ts) != 1) {
            /* fall back: whole line after ';' */
            char *semi = strchr(buf, ';');
            if (!semi)
                continue;
            ts = max;
        }
        if (ts > max)
            max = ts;
        if (ts < since_us)
            continue;
        char *semi = strchr(buf, ';');
        if (semi && (strstr(semi, "CAM-SENSOR") || strstr(semi, "cam_req") ||
                     strstr(semi, "CAM-UTIL") || strstr(semi, "CAM-CCI") ||
                     strstr(semi, "slg51000")))
            printf("    kmsg: %s", semi + 1);
    }
    return max;
}

/* classify camera kmsg lines since ts (quiet): ACK / NACK / bus wedge */
#define KMSG_QUIET 0
#define KMSG_NACK  1
#define KMSG_HIT   2
#define KMSG_WEDGE 3
static int kmsg_classify(int fd, double since_us, uint32_t *hit_id,
                         double *out_max)
{
    char buf[512];
    int st = KMSG_QUIET;
    double max = since_us;
    if (hit_id)
        *hit_id = 0;
    while (1) {
        ssize_t n = read(fd, buf, sizeof(buf) - 1);
        if (n <= 0)
            break;
        buf[n] = 0;
        double ts;
        if (sscanf(buf, "%*d,%*llu,%lf;%*s", &ts) != 1) {
            char *semi = strchr(buf, ';');
            if (!semi)
                continue;
            ts = max;
        }
        if (ts > max)
            max = ts;
        if (ts < since_us)
            continue;
        char *semi = strchr(buf, ';');
        if (!semi || !strstr(semi, "CAM-"))
            continue;   /* only camera-driver lines (adbd spam says "timeout") */
        char *m = strstr(semi, "read id: 0x");
        if (m) {
            uint32_t id = (uint32_t)strtoul(m + 11, 0, 16);
            if (id) {
                st = KMSG_HIT;
                if (hit_id)
                    *hit_id = id;
            } else if (st < KMSG_NACK) {
                st = KMSG_NACK;
            }
        }
        if (strstr(semi, "-110") || strstr(semi, "timeout")) {
            printf("    wedge-match line: %s", semi + 1);
            st = KMSG_WEDGE;
        }
    }
    if (out_max)
        *out_max = max;
    return st;
}

/* ---- packet build ---- */
struct bufs {
    struct cam_mem_mgr_alloc_cmd pkt, c0, c1;
    void *p_pkt, *p_c0, *p_c1;
    int pkt_fd, c0_fd, c1_fd;
};

static void bufs_free(int video_fd, struct bufs *b)
{
    if (b->p_pkt) munmap(b->p_pkt, 4096);
    if (b->p_c0)  munmap(b->p_c0, 256);
    if (b->p_c1)  munmap(b->p_c1, 1024);
    if (b->pkt.out.buf_handle) release_buf(video_fd, b->pkt.out.buf_handle);
    if (b->c0.out.buf_handle)  release_buf(video_fd, b->c0.out.buf_handle);
    if (b->c1.out.buf_handle)  release_buf(video_fd, b->c1.out.buf_handle);
    if (b->pkt_fd > 0) close(b->pkt_fd);
    if (b->c0_fd > 0)  close(b->c0_fd);
    if (b->c1_fd > 0)  close(b->c1_fd);
}

static double g_extclk_mhz; /* fwd: defined+initialized at ~line 1400 */

static int probe_once(int video_fd, int sd_fd, int slot, uint32_t slave,
                      uint32_t reg, uint32_t expected)
{
    struct bufs b;
    memset(&b, 0, sizeof(b));
    int rc = -1;
    if (alloc_buf(video_fd, 4096, 4096, &b.pkt) < 0) goto out;
    if (alloc_buf(video_fd, 256, 8, &b.c0) < 0) goto out;
    if (alloc_buf(video_fd, 1024, 8, &b.c1) < 0) goto out;
    b.pkt_fd = b.pkt.out.fd; b.c0_fd = b.c0.out.fd; b.c1_fd = b.c1.out.fd;
    b.p_pkt = map_fd(b.pkt_fd, 4096);
    b.p_c0 = map_fd(b.c0_fd, 256);
    b.p_c1 = map_fd(b.c1_fd, 1024);
    if (!b.p_pkt || !b.p_c0 || !b.p_c1) goto out;
    memset(b.p_pkt, 0, 4096); memset(b.p_c0, 0, 256); memset(b.p_c1, 0, 1024);

    /* cmd buf 0: i2c info + probe */
    struct cam_cmd_i2c_info *i2c = b.p_c0;
    i2c->slave_addr = slave;
    i2c->i2c_freq_mode = I2C_FREQ_FAST;
    i2c->cmd_type = CMD_I2C_INFO;
    struct cam_cmd_probe *pr = (void *)((char *)b.p_c0 + sizeof(*i2c));
    pr->data_type = I2C_TYPE_WORD;
    pr->addr_type = I2C_TYPE_WORD;
    pr->cmd_type = CMD_PROBE;
    pr->reg_addr = reg;
    /* data_mask=0 lets cam_sensor_id_by_mask apply its ~0 fallback, i.e. a
     * FULL 16-bit id compare (verified in cam_sensor_core.c:695 — an early
     * theory that mask=0 made the compare vacuous was wrong). cap16's probe
     * "rc=0" was real: 3 NACKed id reads, then 0x355 matched on the retry
     * ("Probe success,slot:2"). */
    pr->expected_data = expected;
    pr->data_mask = 0;
    pr->camera_id = (uint16_t)slot;
    uint32_t c0_len = sizeof(*i2c) + sizeof(*pr);

    /* cmd buf 1: power blob — one PWR_UP(count=1)+WAIT per step, then downs */
    struct slot_cfg *sc = &slots[slot];
    uint8_t *q = b.p_c1;
    for (int i = 0; i < sc->n_up; i++) {
        struct cam_cmd_power *pw = (void *)q;
        uint32_t cfg = sc->up[i].cfg;
        /* --mclk also retargets the power-table MCLK entry (otherwise the
         * flag only retunes PLL math and the kernel still programs 24 MHz).
         * Feeding a deliberately wrong INCK is a live-pin discriminator: a
         * sensor with a working INCK mistimes its PLL and the CSID shows
         * error IRQs; a dead pad stays rx=0. */
        if (sc->up[i].seq == SENSOR_MCLK && g_extclk_mhz != 24.0)
            cfg = (uint32_t)(g_extclk_mhz * 1000000.0);
        pw->count = 1;
        pw->cmd_type = CMD_PWR_UP;
        pw->power_settings[0].power_seq_type = sc->up[i].seq;
        pw->power_settings[0].config_val_low = cfg;
        q += sizeof(*pw);
        if (sc->up[i].delay) {
            struct cam_cmd_unconditional_wait *w = (void *)q;
            w->delay = sc->up[i].delay;
            w->op_code = WAIT_SW_UCND;
            w->cmd_type = CMD_WAIT;
            q += sizeof(*w);
        }
    }
    for (int i = 0; i < sc->n_down; i++) {
        struct cam_cmd_power *pw = (void *)q;
        pw->count = 1;
        pw->cmd_type = CMD_PWR_DOWN;
        pw->power_settings[0].power_seq_type = sc->down[i].seq;
        pw->power_settings[0].config_val_low = sc->down[i].cfg;
        q += sizeof(*pw);
        if (sc->down[i].delay) {
            struct cam_cmd_unconditional_wait *w = (void *)q;
            w->delay = sc->down[i].delay;
            w->op_code = WAIT_SW_UCND;
            w->cmd_type = CMD_WAIT;
            q += sizeof(*w);
        }
    }
    uint32_t c1_len = (uint32_t)(q - (uint8_t *)b.p_c1);

    /* packet: header + 2 cmd descs at payload (cmd_buf_offset = 0) */
    struct cam_packet *pkt = b.p_pkt;
    pkt->header.op_code = 0;
    pkt->header.size = sizeof(*pkt) + 2 * sizeof(struct cam_cmd_buf_desc);
    pkt->num_cmd_buf = 2;
    struct cam_cmd_buf_desc *desc = (void *)pkt->payload;
    desc[0].mem_handle = (int32_t)b.c0.out.buf_handle;
    desc[0].size = 256; desc[0].length = c0_len;
    desc[1].mem_handle = (int32_t)b.c1.out.buf_handle;
    desc[1].size = 1024; desc[1].length = c1_len;

    rc = cam_ioctl(sd_fd, CAM_SENSOR_PROBE_CMD,
                   (void *)(uintptr_t)b.pkt.out.buf_handle,
                   CAM_HANDLE_MEM_HANDLE, sizeof(struct cam_control));
out:
    bufs_free(video_fd, &b);
    return rc;
}

/* find sensor subdev nodes via media2 (or media3..) entity table */
static int find_sensor_nodes(int sd_slot_fd[MAX_SUBDEV], int sd_slot[MAX_SUBDEV])
{
    int n = 0;
    for (int mi = 0; mi < 8 && n < MAX_SUBDEV; mi++) {
        char path[32];
        snprintf(path, sizeof(path), "/dev/media%d", mi);
        int mfd = open(path, O_RDONLY);
        if (mfd < 0)
            continue;
        uint32_t id = 0;
        for (;;) {
            struct media_entity_desc ent;
            memset(&ent, 0, sizeof(ent));
            ent.id = id | MEDIA_ENT_ID_FLAG_NEXT;
            if (ioctl(mfd, MEDIA_IOC_ENUM_ENTITIES, &ent) < 0) {
                if (id == 0)
                    fprintf(stderr, "%s: enum failed: %s\n", path,
                        strerror(errno));
                break;
            }
            id = ent.id;
            if (ent.type == CAM_SENSOR_DEVICE_TYPE &&
                ent.dev.v4l.major != 0) {
                /* map major:minor -> /dev/v4l-subdevX via sysfs */
                char want[32];
                snprintf(want, sizeof(want), "%u:%u",
                    ent.dev.v4l.major, ent.dev.v4l.minor);
                for (int s = 0; s < 32; s++) {
                    char sf[96], dv[32];
                    snprintf(sf, sizeof(sf),
                        "/sys/class/video4linux/v4l-subdev%d/dev", s);
                    int df = open(sf, O_RDONLY);
                    if (df < 0)
                        continue;
                    ssize_t r = read(df, dv, sizeof(dv) - 1);
                    close(df);
                    if (r <= 0)
                        continue;
                    dv[r] = 0;
                    size_t wl = strlen(want);
                    if (strncmp(dv, want, wl) == 0 &&
                        (dv[wl] == '\n' || dv[wl] == 0)) {
                        char devp[32];
                        snprintf(devp, sizeof(devp),
                            "/dev/v4l-subdev%d", s);
                        int fd = open(devp, O_RDWR);
                        if (fd >= 0) {
                            struct cam_sensor_query_cap cap;
                            memset(&cap, 0, sizeof(cap));
                            if (cam_ioctl(fd, CAM_QUERY_CAP, &cap,
                                          CAM_HANDLE_USER_POINTER,
                                          sizeof(cap)) == 0) {
                                sd_slot_fd[n] = fd;
                                sd_slot[n] = (int)cap.slot_info;
                                printf("slot %d: %s (entity %s, csiphy %d, "
                                    "eeprom %d, actuator %d, ois %d)\n",
                                    cap.slot_info, devp, ent.name,
                                    cap.csiphy_slot_id, cap.eeprom_slot_id,
                                    cap.actuator_slot_id, cap.ois_slot_id);
                                n++;
                            } else {
                                printf("slot ?: %s querycap %s\n", devp,
                                    strerror(errno));
                            }
                        }
                        break;
                    }
                }
            }
        }
        close(mfd);
    }
    return n;
}

/* ============ v1: one RAW frame through cam_req_mgr / IFE RDI ============ */

/* monotonic seconds — kmsg ts is the same clock in us, so prints align */
static double mono(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec / 1e9;
}

/* kmsg tail for streaming: print every camera-stack line */
static double stream_kmsg(int fd, double since_us)
{
    char buf[512];
    double max = since_us;
    while (1) {
        ssize_t n = read(fd, buf, sizeof(buf) - 1);
        if (n <= 0)
            break;
        buf[n] = 0;
        double ts;
        if (sscanf(buf, "%*d,%*llu,%lf;%*s", &ts) != 1) {
            char *s = strchr(buf, ';');
            if (!s)
                continue;
            ts = max;
        }
        if (ts > max)
            max = ts;
        if (ts < since_us)
            continue;
        char *semi = strchr(buf, ';');
        if (semi && strstr(semi, "CAM-"))
            printf("    kmsg[%9.3f]: %s", ts / 1e6, semi + 1);
    }
    return max;
}

/* find the subdev node for an entity type; want_slot >= 0 also matches the
 * querycap slot_info (sensor/csiphy). Returns fd or -1. */
static char g_isp_node[32];
static int g_hold_fds[2];
static int find_subdev_by_type(uint32_t type, int want_slot, int *out_slot)
{
    for (int mi = 0; mi < 8; mi++) {
        char path[32];
        snprintf(path, sizeof(path), "/dev/media%d", mi);
        int mfd = open(path, O_RDONLY);
        if (mfd < 0)
            continue;
        uint32_t id = 0;
        for (;;) {
            struct media_entity_desc ent;
            memset(&ent, 0, sizeof(ent));
            ent.id = id | MEDIA_ENT_ID_FLAG_NEXT;
            if (ioctl(mfd, MEDIA_IOC_ENUM_ENTITIES, &ent) < 0)
                break;
            id = ent.id;
            if (ent.type != type || ent.dev.v4l.major == 0)
                continue;
            char want[32];
            snprintf(want, sizeof(want), "%u:%u",
                ent.dev.v4l.major, ent.dev.v4l.minor);
            for (int s = 0; s < 32; s++) {
                char sf[96], dv[64], devp[32];
                snprintf(sf, sizeof(sf),
                    "/sys/class/video4linux/v4l-subdev%d/dev", s);
                int df = open(sf, O_RDONLY);
                if (df < 0)
                    continue;
                ssize_t r = read(df, dv, sizeof(dv) - 1);
                close(df);
                if (r <= 0)
                    continue;
                dv[r] = 0;
                size_t wl = strlen(want);
                if (strncmp(dv, want, wl) != 0 ||
                    (dv[wl] != '\n' && dv[wl] != 0))
                    continue;
                snprintf(devp, sizeof(devp), "/dev/v4l-subdev%d", s);
                int fd = open(devp, O_RDWR);
                if (fd < 0)
                    break;
                if (want_slot >= 0) {
                    struct cam_sensor_query_cap cap;
                    memset(&cap, 0, sizeof(cap));
                    if (cam_ioctl(fd, CAM_QUERY_CAP, &cap,
                                  CAM_HANDLE_USER_POINTER,
                                  sizeof(cap)) < 0 ||
                        (int)cap.slot_info != want_slot) {
                        close(fd);
                        break;
                    }
                    if (out_slot)
                        *out_slot = (int)cap.slot_info;
                }
                if (type == CAM_ISP_DEVICE_TYPE)
                    snprintf(g_isp_node, sizeof(g_isp_node), "%s", devp);
                printf("  node %s = %s\n",
                    type == CAM_SENSOR_DEVICE_TYPE ? "sensor" :
                    type == CAM_CSIPHY_DEVICE_TYPE ? "csiphy" : "isp",
                    devp);
                close(mfd);
                return fd;
            }
        }
        close(mfd);
    }
    return -1;
}

/* --force-ife support: pre-acquire throwaway ISP contexts whose lane_cfg
 * deliberately mismatches the real one, so the hw mgr's descending CSID scan
 * (highest free first for single-IFE ctx) skips the occupied CSIDs and lands
 * the real context on the requested IFE. Each throwaway is a full open() of
 * the isp node (own ctx) + ACQUIRE_DEV + ACQUIRE_HW; never linked or started. */
static int hold_higher_csids(uint32_t session, int n, uint32_t res_type,
    uint32_t lane_num, uint32_t dt, uint32_t width, uint32_t height)
{
    static const uint32_t hold_cfg[2] = { 0x5, 0xa };
    for (int k = 0; k < n; k++) {
        int fd = open(g_isp_node, O_RDWR);
        if (fd < 0) {
            fprintf(stderr, "hold: open %s: %s\n", g_isp_node, strerror(errno));
            return -1;
        }
        struct cam_acquire_dev_cmd iacq;
        memset(&iacq, 0, sizeof(iacq));
        iacq.session_handle = (int32_t)session;
        iacq.handle_type = CAM_HANDLE_USER_POINTER;
        iacq.num_resources = CAM_API_COMPAT_CONSTANT;
        if (cam_ioctl(fd, CAM_ACQUIRE_DEV, &iacq,
                      CAM_HANDLE_USER_POINTER, sizeof(iacq)) < 0) {
            fprintf(stderr, "hold: ACQUIRE_DEV: %s\n", strerror(errno));
            close(fd);
            return -1;
        }
        uint8_t blob[256];
        memset(blob, 0, sizeof(blob));
        struct cam_isp_acquire_hw_info *ah = (void *)blob;
        struct cam_isp_in_port_info *port = (void *)&ah->data;
        ah->common_info_version = 0x1000;
        ah->common_info_size = sizeof(*port);
        ah->num_inputs = 1;
        ah->input_info_version = 0x2000;
        ah->input_info_size = sizeof(*port);
        ah->input_info_offset = 0;
        port->res_type = res_type;
        port->lane_type = 0;
        port->lane_num = lane_num;
        port->lane_cfg = hold_cfg[k];
        port->vc = 0;
        port->dt = dt;
        port->format = CAM_FORMAT_MIPI_RAW_10;
        port->usage_type = 0;
        port->left_width = width;
        port->height = height;
        port->pixel_clk = 0;
        port->num_out_res = 1;
        port->data[0].res_type = CAM_ISP_IFE_OUT_RES_RDI_0;
        port->data[0].format = CAM_FORMAT_MIPI_RAW_10;
        port->data[0].width = width;
        port->data[0].height = height;
        struct cam_acquire_hw_cmd_v2 ahw;
        memset(&ahw, 0, sizeof(ahw));
        ahw.struct_version = 2;
        ahw.session_handle = (int32_t)session;
        ahw.dev_handle = iacq.dev_handle;
        ahw.handle_type = CAM_HANDLE_USER_POINTER;
        ahw.data_size = 24 + (uint32_t)sizeof(*port);
        ahw.resource_hdl = (uint64_t)(uintptr_t)blob;
        if (cam_ioctl(fd, CAM_ACQUIRE_HW, &ahw,
                      CAM_HANDLE_USER_POINTER, sizeof(ahw)) < 0) {
            fprintf(stderr, "hold: ACQUIRE_HW (lane_cfg 0x%x): %s\n",
                hold_cfg[k], strerror(errno));
            close(fd);
            return -1;
        }
        printf("hold ctx %d ok (lane_cfg 0x%x, hw id mask 0x%x)\n",
            k, hold_cfg[k], ahw.hw_info.acquired_hw_id[0]);
        g_hold_fds[k] = fd;
    }
    return 0;
}

/* build I2C random-write mosaic into buf; returns bytes used */
static size_t build_i2c_writes(uint8_t *buf, const struct wreg *r, int n)
{
    uint8_t *p = buf;
    int i = 0;
    while (i < n) {
        int j = i;
        while (j < n && r[j].width == r[i].width)
            j++;
        struct i2c_rdwr_header *h = (void *)p;
        h->count = (uint32_t)(j - i);
        h->op_code = I2C_OP_RNDM_WR;
        h->cmd_type = CMD_I2C_RNDM_WR;
        h->data_type = r[i].width == 16 ? I2C_TYPE_WORD : I2C_TYPE_BYTE;
        /* imx355 register ADDRESSES are always 16-bit, only data width
         * varies — with BYTE addr the CCI truncates 0x030d to register
         * 0x0d and the write is silently dropped (observed: every 8-bit
         * reg read back its old value while all 16-bit regs landed). */
        h->addr_type = I2C_TYPE_WORD;
        struct i2c_random_wr_payload *pl = (void *)(p + sizeof(*h));
        for (int k = i; k < j; k++) {
            pl[k - i].reg_addr = r[k].addr;
            pl[k - i].reg_data = r[k].val;
        }
        p += sizeof(*h) + sizeof(*pl) * (uint32_t)(j - i);
        i = j;
    }
    return (size_t)(p - buf);
}

/* one shot buffer set (packet + i2c cmd buf), reusable across packets */
struct shot_bufs {
    struct cam_mem_mgr_alloc_cmd pkt, cmd;
    void *p_pkt, *p_cmd;
    int pkt_fd, cmd_fd;
    size_t cmd_cap;
};

static void shot_free(int video_fd, struct shot_bufs *b)
{
    if (b->p_pkt) munmap(b->p_pkt, 4096);
    if (b->p_cmd) munmap(b->p_cmd, b->cmd_cap);
    if (b->pkt.out.buf_handle) release_buf(video_fd, b->pkt.out.buf_handle);
    if (b->cmd.out.buf_handle) release_buf(video_fd, b->cmd.out.buf_handle);
    if (b->pkt_fd > 0) close(b->pkt_fd);
    if (b->cmd_fd > 0) close(b->cmd_fd);
    memset(b, 0, sizeof(*b));
}

/* mmu_hdl > 0: cmd buf is also HW-mapped into that SMMU ctx (the CDM reads
 * the kmd/BL region through its own iommu; without it submit_bl fails its
 * sanity check — observed). mmu_hdl <= 0: CPU-only (sensor packets). */
static int shot_alloc(int video_fd, struct shot_bufs *b, size_t cmd_cap,
                      int32_t mmu_hdl)
{
    memset(b, 0, sizeof(*b));
    b->cmd_cap = (cmd_cap + 4095) & ~4095UL;
    if (alloc_buf(video_fd, 4096, 4096, &b->pkt) < 0)
        return -1;
    memset(&b->cmd, 0, sizeof(b->cmd));
    b->cmd.len = b->cmd_cap;
    b->cmd.align = 8;
    if (mmu_hdl > 0) {
        b->cmd.mmu_hdls[0] = mmu_hdl;
        b->cmd.num_hdl = 1;
        b->cmd.flags = 0x49;  /* KMD_ACCESS|CMD_BUF_TYPE|HW_READ_WRITE */
    } else {
        b->cmd.flags = 0x48;  /* KMD_ACCESS|CMD_BUF_TYPE */
    }
    if (cam_ioctl(video_fd, CAM_REQ_MGR_ALLOC_BUF, &b->cmd,
                  CAM_HANDLE_USER_POINTER, sizeof(b->cmd)) < 0) {
        fprintf(stderr, "cmd alloc_buf(%zu): %s\n", b->cmd_cap,
            strerror(errno));
        shot_free(video_fd, b);
        return -1;
    }
    b->pkt_fd = b->pkt.out.fd;
    b->cmd_fd = b->cmd.out.fd;
    b->p_pkt = map_fd(b->pkt_fd, 4096);
    b->p_cmd = map_fd(b->cmd_fd, b->cmd_cap);
    if (!b->p_pkt || !b->p_cmd) {
        shot_free(video_fd, b);
        return -1;
    }
    memset(b->p_pkt, 0, 4096);
    memset(b->p_cmd, 0, b->cmd_cap);
    return 0;
}

/* sensor CONFIG_DEV packet: header + 1 cmd desc holding the i2c mosaic.
 * The KMD parses exactly one cmd buffer per config packet. opcodes:
 * 0=STREAMON(applied at START_DEV) 2=INITIAL_CONFIG 4=CONFIG 5=STREAMOFF
 * (2/4/5 applied immediately). */
static int sensor_config(int video_fd, int sd_fd, struct shot_bufs *b,
                         uint32_t session, uint32_t dev_hdl,
                         uint32_t op, const struct wreg *regs, int n)
{
    size_t used = build_i2c_writes(b->p_cmd, regs, n);
    memset(b->p_pkt, 0, 512);
    struct cam_packet *pk = b->p_pkt;
    pk->header.op_code = op;
    pk->header.request_id = 0;
    pk->header.size = (uint32_t)(sizeof(*pk) + sizeof(struct cam_cmd_buf_desc));
    pk->num_cmd_buf = 1;
    struct cam_cmd_buf_desc *d = (void *)pk->payload;
    d->mem_handle = (int32_t)b->cmd.out.buf_handle;
    d->size = (uint32_t)b->cmd_cap;
    d->length = (uint32_t)used;
    struct cam_config_dev_cmd cfg = {
        .session_handle = (int32_t)session, .dev_handle = (int32_t)dev_hdl,
        .offset = 0, .packet_handle = b->pkt.out.buf_handle };
    return cam_ioctl(sd_fd, CAM_CONFIG_DEV, &cfg,
                     CAM_HANDLE_USER_POINTER, sizeof(cfg));
}

/* sensor register readback (op 6 READREG): one cmd buf holding
 * cam_cmd_get_sensor_data; the KMD does the CCI read inside the ioctl and
 * copy_to_user()s nbytes raw bytes into our stack var. Reuses the sensor
 * shot bufs (settings were already copied out at parse time). */
static int sensor_readreg(int sd_fd, struct shot_bufs *b, uint32_t session,
                          uint32_t dev_hdl, uint32_t addr, int nbytes,
                          uint8_t *out)
{
    struct cam_cmd_get_sensor_data *gs = b->p_cmd;
    memset(b->p_cmd, 0, 64);
    gs->reg_addr = addr;
    gs->reg_data = (uint32_t)nbytes;
    gs->query_data_handle = (uint64_t)(uintptr_t)out;
    memset(b->p_pkt, 0, 512);
    struct cam_packet *pk = b->p_pkt;
    pk->header.op_code = 6;   /* CAM_SENSOR_PACKET_OPCODE_SENSOR_READREG */
    pk->header.request_id = 0;
    pk->header.size = (uint32_t)(sizeof(*pk) + sizeof(struct cam_cmd_buf_desc));
    pk->num_cmd_buf = 1;
    struct cam_cmd_buf_desc *d = (void *)pk->payload;
    d->mem_handle = (int32_t)b->cmd.out.buf_handle;
    d->size = (uint32_t)b->cmd_cap;
    d->length = sizeof(*gs);
    struct cam_config_dev_cmd cfg = {
        .session_handle = (int32_t)session, .dev_handle = (int32_t)dev_hdl,
        .offset = 0, .packet_handle = b->pkt.out.buf_handle };
    if (cam_ioctl(sd_fd, CAM_CONFIG_DEV, &cfg,
                  CAM_HANDLE_USER_POINTER, sizeof(cfg)) < 0) {
        fprintf(stderr, "READREG 0x%04x: %s\n", addr, strerror(errno));
        return -1;
    }
    return nbytes;
}

/* readback table: 8-bit regs read 1 byte, 16-bit read 2 (bus order MSB
 * first). expect = value as written in the reg tables. */
static const struct rbreg {
    uint32_t addr; uint32_t val; uint8_t n; const char *name;
} rbregs[] = {
    {0x304e, 0x03,   1, "global 0x304e"},  /* INIT-only 8-bit write */
    {0x0136, 0x1800, 2, "EXTCLK 24MHz"},
    {0x0114, 0x0001, 1, "LANE_SEL 2"},
    {0x030e, 111,    2, "PLL_OP_MUL"},
    {0x030d, 3,      1, "PLL_OP_PREDIV"},
    {0x0303, 2,      1, "IVT_SYSCK_DIV"},
    {0x0820, 1776,   2, "REQ_LINK_BIT_RATE"},
    {0x0342, 1836,   2, "LLP"},
    {0x0340, 1306,   2, "FLL"},
    {0x0100, 1,      1, "MODE_SELECT"},
};

/* imx363 readback (rear, slot 0): expectations straight from the vendor
 * bin tables — no PLL patching, they are written verbatim. The mode
 * table splits 16-bit values into byte writes, so read back the byte
 * registers the table actually wrote (hi,lo of FLL/size, lane sel). */
static const struct rbreg rbregs363[] = {
    {0x0112, 0x0a,   1, "CST_SZ 10-bit"},
    {0x0114, 0x0003, 1, "LANE_SEL 4"},
    {0x0301, 0x03,  1, "PLL pck div 3"},
    {0x0303, 0x02,  1, "PLL sysck div 2"},
    {0x0305, 0x04,  1, "PLL prediv 4"},
    {0x0306, 0x00,  1, "PLL mult hi"},
    {0x0307, 0xcf,   1, "PLL mult 207"},
    {0x0136, 0x18,  1, "INCK freq hi (0x1800=24M)"},
    {0x0137, 0x00,  1, "INCK freq lo"},
    {0x0820, 0x13,  1, "REQ_LINK_BIT_RATE hi"},
    {0x0821, 0x68,  1, "REQ_LINK_BIT_RATE lo"},
    {0x0309, 0x0a,  1, "PLL 0309"},
    {0x030b, 0x01,  1, "PLL 030b"},
    {0x030d, 0x04,  1, "PLL 030d"},
    {0x030e, 0x01,  1, "PLL 030e"},
    {0x030f, 0x32,  1, "PLL mipi div"},
    {0x0310, 0x01,  1, "PLL 0310"},
    {0x0342, 0x10,  1, "LLP hi"},
    {0x0343, 0x50,  1, "LLP lo"},
    {0x0340, 0x06,   1, "FLL hi"},
    {0x0341, 0x74,   1, "FLL lo"},
    {0x034c, 0x07,   1, "X_OUT hi"},
    {0x034d, 0xe0,   1, "X_OUT lo"},
    {0x034e, 0x04,   1, "Y_OUT hi"},
    {0x034f, 0x70,   1, "Y_OUT lo"},
    {0x0901, 0x22,   1, "binning 2x2"},
    {0x0100, 1,      1, "MODE_SELECT"},
};

/* imx355 vendor readback (front, slot 2): byte regs the vendor mode
 * table actually wrote (mirrors rbregs363 style). */
static const struct rbreg rbregs355v[] = {
    {0x0113, 0x0a,   1, "CST_SZ 10-bit"},
    {0x0114, 0x0003, 1, "LANE_SEL 4"},
    {0x0307, 0x78,   1, "PLL mult 120"},
    {0x0340, 0x0a,   1, "FLL hi"},
    {0x0341, 0x36,   1, "FLL lo"},
    {0x034c, 0x06,   1, "X_OUT hi"},
    {0x034d, 0x68,   1, "X_OUT lo"},
    {0x034e, 0x03,   1, "Y_OUT hi"},
    {0x034f, 0x9c,   1, "Y_OUT lo"},
    {0x0901, 0x22,   1, "binning 2x2"},
    {0x0100, 1,      1, "MODE_SELECT"},
};

/* active sensor slot for table selection (run_stream sets it) */
static int g_slot = 2;

/* MCLK input actually fed to the sensor: DT asks for 24 MHz (default);
 * --mclk 19 selects the 19.2 MHz parameter set. Chosen before packets are
 * built so both the write tables and the readback expectations follow. */
static int g_mclk24 = 1;
/* --mclk <MHz>: the EXTCLK the sensor actually receives. Generic PLL path:
 * PREDIV=1 MUL=30 → VCO = 30*f Mbps/lane for ANY f, and the CSIPHY data
 * rate is told the same number, so a sweep over f hypotheses needs only
 * self-consistent configs (observed idea 2026-09-01: real MCLK proved to
 * be neither 24 nor 19.2 — both tuned sets stream the same ECC garbage). */
static double g_extclk_mhz = 24.0;

/* Lane count. The vendor module info (mipiFlags in
 * /vendor/lib64/camera/com.qti.sensormodule.primax_imx355_lito2.bin) says
 * this module runs clk + 4 data lanes, so 4 is the default; the mainline
 * driver's 2-lane set stays reachable with --lanes 2. */
static int g_lanes = 4;

/* CSID CSI2_RX_CFG0 DL_INPUT_SEL fields, 4 bits per lane (uapi
 * cam_isp_in_port_info.lane_cfg "4 bits per lane"; kernel writes it
 * lane_cfg<<4 unmasked): lane_cfg[3:0]=DL0 sel, [7:4]=DL1, [11:8]=DL2,
 * [15:12]=DL3. 0 is NOT identity — it makes every logical lane read
 * physical D0 and the striped long-packet headers garble (FS/FE survive:
 * broadcast shorts are lane-permutation-invariant). Canonical identity is
 * 0x3210; observed 2026-09-01: with it the link is clean (WARNING_ECC-only,
 * LONG_PKT VC:0 DT:0x2B WC:2520, RDI0 SOF+EOF per frame, 2860495/2862720
 * nonzero bytes — first real frame). */
static uint32_t g_lanecfg = 0x3210;
/* --force-ife N: land the real context on CSID/IFE N instead of the mgr's
 * default pick. cam_ife_hw_mgr_acquire_csid_hw walks CSIDs from the HIGHEST
 * index down for a single-IFE context (is_start_lower_idx=false), so we always
 * get IFE2 (the highest probed); an occupied CSID is only reused when
 * lane_cfg/lane_type/lane_num all match, so pre-acquiring throwaway contexts
 * with a different lane_cfg (0x5 / 0xa) on the higher CSIDs makes the real
 * acquire fall through to N. Throwaways are never linked/configured/started —
 * they only hold a CID + RDI reservation. */
static int g_force_ife = -1;
/* --noglobal: skip the 70-reg global init (mainline imx355 list, sourced
 * from a ChromeOS module build) and write only the per-mode config. A/B for
 * whether the borrowed global tuning is what corrupts this primax module's
 * MIPI TX (all configs stream garbage with it; observed 2026-09-01). */
static int g_noglobal;
/* --nostarton: see imx355_streamoff above. */
static int g_nostarton;
/* --halfrate (rear only): halve the vendor PLL multiplier (0x0307 207->104)
 * to drop the MIPI lane rate ~2x (260 Mbps/lane, ~15 fps). Discriminates
 * "rate too high for our CSID clock vote" from "link systematically
 * corrupt": every packet ECC-fails at the vendor rate (observed
 * 2026-09-01, ~line-rate error IRQs at the modeled 20us line time). */
static int g_halfrate;
/* --rawvendor: apply the rear imx363 tables exactly as the vendor bin
 * decodes them — skip our appended 0x0136 (INCK) / 0x0820 (REQ_LINK)
 * writes that the bin never makes (see the block at the rear CONFIG). */
static int g_rawvendor;
static int g_slowrear;
static int g_rear564;
static int g_keep0112;
static unsigned g_cit;        /* coarse integration lines, 0 = mode default */
static double g_gain = 1.0;   /* analog gain multiplier 1..16 */
static double g_dgain = 1.0;  /* digital gain multiplier 1..16 */
static int g_png;             /* also write /tmp/frame.png (gray8) */
/* --jpeg [q]: also encode the frame as JPEG (default color, q85).
 * --jpeg-gray / --jpeg-out override. Output path defaults to the raw dump
 * path with .raw swapped for .jpg. */
static int g_jpeg_q;
static int g_jpeg_color = 1;
/* white balance: gray-world auto by default (imx363 measures G/R=1.58,
 * G/B=1.51 with no WB — the RGGB G-double-sample cast, 2026-09-01);
 * --wb r,g,b takes over manually, --wb off disables */
static int g_wb_auto = 1;
static float g_wb_r = 1.0f, g_wb_g = 1.0f, g_wb_b = 1.0f;
static const char *g_jpeg_out;
/* --frames N: queue N requests back-to-back before START (burst). Each
 * frame gets its own pixel buffer + fence; fences are waited in order
 * (frames land at sensor frame rate). Outputs gain a -<i> suffix. */
static int g_frames = 1;
/* --roll: queue req i+1 only after fence i signals (rolling submission)
 * instead of pre-queueing all N before START. Default 0 — rolling loses
 * the SOF race (see the queue-section note). */
static int g_roll;

/* replace-or-append one byte register in a CONFIG table (cfg_regs is sized
 * 128; callers stay far below that) */
static void cfg_override(struct wreg *r, size_t *n, uint16_t addr, uint8_t val)
{
    for (size_t i = 0; i < *n; i++) {
        if (r[i].addr == addr) {
            r[i].val = val;
            return;
        }
    }
    r[*n].addr = addr;
    r[*n].val = val;
    r[*n].width = 8;
    (*n)++;
}
/* --tpg: arm the CSID's built-in test pattern generator instead of the PHY
 * RX (CAM_ISP_IFE_IN_RES_TPG). The whole sensor side (probe, power, register
 * lists, csiphy) is skipped, so a frame proves the IFE/RDI pipeline alone —
 * the discriminator between "pipeline broken" and "sensor silent" (the
 * sensor's own 0x0600 pattern via --tp still needs PLL+start, so it never
 * exercised this). Kernel fixes TPG traffic at VC 0xA / DT 0x2B. */
static int g_tpg;
/* --railhelper: before powering the target slot, acquire the REAR sensor
 * (slot 0) and hold its INIT in the same session. The front module's analog
 * supply lives on rails only @0's regulator list references — the SLG51000
 * camera PMIC (ldo1..6) and the gpio-switched 2.85V "camera_ldo" — so a
 * front-only session raises nothing but SLG ldo7 (VIO, matrix 0x40) and the
 * imx355 acks I2C (IF runs on VIO) with a dead PLL/MIPI. A rear session
 * powers all of it (matrix 0x7f + camera_ldo observed 2026-09-01); holding
 * the rear's INIT keeps those rails up across the front's own power-up. The
 * helper device is never linked or started. */
static int g_railhelper;
/* --verify: full mode-table readback (every reg written vs read). */
static int g_verify;
/* --bw: append BW_CONFIG_V2 blob (IFE_RDI0 WRITE vote) to the INIT packet. */
static int g_bw;
/* --vc / --dt: RDI0's mapped virtual channel / data type in the IFE
 * in_port (default 0 / 0x2B RAW10). Diagnostic sweep: if the sensor's
 * image packets arrive on a different VC/DT the CSID flags
 * UNMAPPED_VC_DT and drops every line → STREAM_UNDERFLOW, zero buffer. */
static uint32_t g_vc = 0;
static uint32_t g_dt = 0x2B;   /* RAW10 */
/* --pix: IFE PIX processing path instead of the RDI raw dump. Out res
 * CAM_ISP_IFE_OUT_RES_FULL + NV12 (two write masters: Y w x h, C w x h/2)
 * makes the hw mgr acquire the CSID IPP (preprocess_port: any non-RDI,
 * non-2PD/LCR out counts as ipp) -> VFE CAMIF -> the hardware ISP module
 * chain. The kernel wires CAMIF/top/bus only — cam_vfe_top_ver2.c and
 * cam_vfe_camif_ver2.c contain NO module programming (no black level, WB,
 * demosaic, gamma, CCM writes anywhere), and the hw-mgr blob list has no
 * module-setting blobs: the module register config is expected as raw CDM
 * command payloads from userspace (CAM_ISP_PACKET_META_COMMON cmd bufs,
 * what stock CHI ships from Chromatix). First light runs kernel-config
 * only and observes what an unconfigured module chain does. */
static int g_pix;
/* --pix-raw: PIX context (CSID IPP -> CAMIF -> CGC unlock) but the bus out
 * is RAW_DUMP + PLAIN16_10 — a tap upstream of the demosaic/gamma/CCM/CSC
 * chain. Diagnostic bisect: if RAW_DUMP lands a frame while FULL/NV12 stays
 * silent, the blocker is the unconfigured module chain, not the CAMIF link. */
static int g_pixraw;
/* camera-SS-relative reg base of the IFE the hw mgr actually acquired, for
 * the ChangeBase word in userspace CDM payloads. Verified from the kernel's
 * own kmd CDM dump: PIX lands on IFE1 (acquire print "Acquired single
 * IFE[1 -1]"), whose changebase word is 0x08_0B6000. Overridable for
 * --force-ife experiments once IFE0's base is measured. */
static uint32_t g_ife_base = 0x0B6000u;
/* pix-mode geometry (run_stream sets; fill_out_io reads) */
static uint32_t g_pw, g_ph, g_pstride, g_pcoff;

/* per (mclk, lanes) PLL tuple: [mpy, prediv, sysck, link_total, lane_sel] */
static void pll_params(uint32_t *m19)
{
    /* generic: MUL=30 PREDIV=1 → VCO = 30*extclk Mbps (720 @24M, the same
     * VCO as the legacy tuned 4-lane set); REQ_LINK = VCO*lanes. The CSIPHY
     * data rate is told the same per-lane number, so any EXTCLK hypothesis
     * is a self-consistent config. */
    uint32_t vco = (uint32_t)(30.0 * g_extclk_mhz + 0.5);
    m19[0] = 30;                          /* PLL_OP_MUL */
    m19[1] = 1;                           /* PLL_OP_PREDIV */
    m19[2] = g_lanes == 4 ? 1 : 2;        /* IVT_SYSCK_DIV */
    m19[3] = vco * (uint32_t)g_lanes;     /* REQ_LINK_BIT_RATE total Mbps */
    m19[4] = (uint32_t)g_lanes - 1;       /* LANE_SEL */
}

static uint16_t extclk_reg(void)
{
    return (uint16_t)(g_extclk_mhz * 256.0 + 0.5);  /* 8.8 fixed point */
}

static void mclk_expect(uint32_t addr, uint32_t *val)
{
    (void)addr; (void)val;
    return;   /* both slots run vendor-bin tables verbatim, no patching */
}

static void sensor_readback(int sd_fd, struct shot_bufs *b, uint32_t session,
                            uint32_t dev_hdl, const char *when)
{    const struct rbreg *rbs = g_slot == 0 ? rbregs363 : rbregs355v;
    const size_t n_rbs = g_slot == 0
        ? sizeof(rbregs363) / sizeof(rbregs363[0])
        : sizeof(rbregs355v) / sizeof(rbregs355v[0]);
    printf("== sensor readback (%s) ==\n", when);
    for (size_t i = 0; i < n_rbs; i++) {
        uint8_t v[8] = {0xEE, 0xEE, 0xEE, 0xEE};
        if (sensor_readreg(sd_fd, b, session, dev_hdl, rbs[i].addr,
                           rbs[i].n, v) < 0) {
            printf("  0x%04x read FAILED (sensor not answering?)\n",
                rbs[i].addr);
            break;
        }
        uint32_t got = rbs[i].n == 2
            ? (uint32_t)((v[0] << 8) | v[1]) : v[0];
        uint32_t expect = rbs[i].val;
        mclk_expect(rbs[i].addr, &expect);
        printf("  0x%04x %-18s = 0x%0*x (expect 0x%x) %s\n",
            rbs[i].addr, rbs[i].name, rbs[i].n == 2 ? 4 : 2,
            got, expect,
            got == expect ? "ok" : "MISMATCH");
    }
}

/* --verify: read back EVERY register the mode table wrote (the wreg tables
 * are byte-per-entry, so a 1-byte read at each addr compares exactly). The
 * curated rb lists above sample 11 of ~51 writes; a single dropped I2C write
 * in the PLL divider block (0x0301/0x0303/0x0305...) would scramble MIPI TX
 * timing while every sampled reg still reads back clean. */
static void verify_cfg_table(int sd_fd, struct shot_bufs *b, uint32_t session,
                             uint32_t dev_hdl, const struct wreg *t, size_t n,
                             const char *when)
{
    printf("== full mode-table readback (%s): %zu regs ==\n", when, n);
    int bad = 0;
    for (size_t i = 0; i < n; i++) {
        uint8_t v = 0xEE;
        if (sensor_readreg(sd_fd, b, session, dev_hdl, t[i].addr, 1, &v) < 0) {
            printf("  0x%04x read FAILED\n", t[i].addr);
            bad++;
            break;
        }
        if (v != (uint8_t)t[i].val) {
            printf("  0x%04x = 0x%02x expect 0x%02x MISMATCH\n",
                t[i].addr, v, t[i].val);
            bad++;
        }
    }
    printf("verify: %d/%zu mismatch\n", bad, n);
}

/* dump + pattern analysis of the RDI pixel buffer. On failed runs the partial
 * content distinguishes scrambling modes: lane-swap leaves structured repeats
 * (valid bytes every Nth position), analog noise leaves spread corruption,
 * and an all-zero buffer means the path never wrote. Returns nonzero count. */
static size_t inspect_buf(const uint8_t *p8, size_t n, const char *path,
                          const char *what)
{
    size_t nz = 0;
    uint8_t mn = 255, mx = 0;
    size_t first_nz = SIZE_MAX;
    for (size_t i = 0; i < n; i++) {
        if (p8[i]) {
            nz++;
            if (first_nz == SIZE_MAX)
                first_nz = i;
        }
        if (p8[i] < mn) mn = p8[i];
        if (p8[i] > mx) mx = p8[i];
    }
    printf("buffer(%s): nonzero %zu/%zu, byte range 0x%02x..0x%02x, first nz @%zu\n",
        what, nz, n, mn, mx, first_nz == SIZE_MAX ? n : first_nz);
    printf("first 64 B @0:");
    for (int i = 0; i < 64; i++)
        printf(" %02x", p8[i]);
    printf("\n");
    if (first_nz != SIZE_MAX) {
        printf("first 64 B @%zu:", first_nz);
        for (int i = 0; i < 64 && first_nz + (size_t)i < n; i++)
            printf(" %02x", p8[first_nz + (size_t)i]);
        printf("\n");
        /* mod-5 histogram of nonzero bytes over the first 4 KB of content:
         * RAW10 packs as 5-byte groups (4 pixels), so peaks at specific
         * residues = intact groups at a shifted phase. */
        size_t hist[5] = {0};
        size_t lim = first_nz + 4096 < n ? first_nz + 4096 : n;
        for (size_t i = first_nz; i < lim; i++)
            if (p8[i])
                hist[(i - first_nz) % 5]++;
        printf("nonzero mod-5 histogram (first 4KB): %zu %zu %zu %zu %zu\n",
            hist[0], hist[1], hist[2], hist[3], hist[4]);
    }
    if (path && path[0]) {
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd >= 0) {
            size_t off = 0;
            while (off < n) {
                ssize_t w = write(fd, p8 + off, n - off);
                if (w <= 0)
                    break;
                off += (size_t)w;
            }
            printf("dumped %zu B -> %s\n", off, path);
            close(fd);
        } else {
            fprintf(stderr, "open %s: %s\n", path, strerror(errno));
        }
    }
    return nz;
}

/* ---- minimal grayscale PNG writer ----
 * 8-bit gray, zlib stored (uncompressed) DEFLATE blocks: no zlib dependency,
 * output ~width*height + 0.03% overhead. Fine for pulling a frame off the
 * phone; swap in a real compressor when upload bandwidth matters.
 * bitwise crc32 (continuable) + adler32, ~20 MB/s — fine for one frame. */
static uint32_t crc32_upd(const uint8_t *d, size_t n, uint32_t crc)
{
    crc = ~crc;
    for (size_t i = 0; i < n; i++) {
        crc ^= d[i];
        for (int k = 0; k < 8; k++)
            crc = (crc >> 1) ^ (0xEDB88320u & (uint32_t)(-(int32_t)(crc & 1)));
    }
    return ~crc;
}

static int wr_all(int fd, const void *buf, size_t n)
{
    const uint8_t *p = buf;
    while (n) {
        ssize_t w = write(fd, p, n);
        if (w <= 0) return -1;
        p += w;
        n -= (size_t)w;
    }
    return 0;
}

static int png_chunk(int fd, const char type[4], const uint8_t *data, size_t n)
{
    uint8_t hdr[8] = {
        (uint8_t)(n >> 24), (uint8_t)(n >> 16), (uint8_t)(n >> 8), (uint8_t)n,
        (uint8_t)type[0], (uint8_t)type[1], (uint8_t)type[2], (uint8_t)type[3],
    };
    uint32_t crc = crc32_upd(data, n, crc32_upd(hdr + 4, 4, 0));
    uint8_t tail[4] = {
        (uint8_t)(crc >> 24), (uint8_t)(crc >> 16), (uint8_t)(crc >> 8), (uint8_t)crc,
    };
    if (wr_all(fd, hdr, 8) < 0 || (n && wr_all(fd, data, n) < 0) ||
        wr_all(fd, tail, 4) < 0)
        return -1;
    return 0;
}

/* RAW10 RDI buffer (stride bytes/row, 5-byte groups = 4 px, MSB byte carries
 * bits[9:2] so gray8 = first 4 bytes of each group) -> PNG. */
static void dump_png(const uint8_t *rdi, uint32_t w, uint32_t h, uint32_t stride,
                     const char *path)
{
    size_t raw_len = (size_t)(w + 1) * h;   /* filter byte 0 per row */
    uint8_t *rows = malloc(raw_len);
    if (!rows) {
        fprintf(stderr, "png: no mem for %zu B\n", raw_len);
        return;
    }
    for (uint32_t r = 0; r < h; r++) {
        uint8_t *out = rows + (size_t)r * (w + 1);
        *out++ = 0;   /* filter none */
        const uint8_t *in = rdi + (size_t)r * stride;
        for (uint32_t xb = 0; xb + 4 < stride; xb += 5) {
            *out++ = in[xb];
            *out++ = in[xb + 1];
            *out++ = in[xb + 2];
            *out++ = in[xb + 3];
        }
    }
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        fprintf(stderr, "open %s: %s\n", path, strerror(errno));
        free(rows);
        return;
    }
    static const uint8_t sig[8] = {137, 80, 78, 71, 13, 10, 26, 10};
    uint8_t ihdr[13];
    ihdr[0] = (uint8_t)(w >> 24); ihdr[1] = (uint8_t)(w >> 16);
    ihdr[2] = (uint8_t)(w >> 8);  ihdr[3] = (uint8_t)w;
    ihdr[4] = (uint8_t)(h >> 24); ihdr[5] = (uint8_t)(h >> 16);
    ihdr[6] = (uint8_t)(h >> 8);  ihdr[7] = (uint8_t)h;
    ihdr[8] = 8;    /* bit depth */
    ihdr[9] = 0;    /* gray */
    ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
    int bad = wr_all(fd, sig, 8) || png_chunk(fd, "IHDR", ihdr, 13);
    /* zlib stream in one IDAT chunk: 0x78 0x01 + stored blocks + adler32 */
    size_t nblk = (raw_len + 65534) / 65535;
    size_t idat_len = 2 + nblk * 5 + raw_len + 4;
    uint8_t *idat = malloc(idat_len);
    if (!idat) {
        fprintf(stderr, "png: no mem for %zu B idat\n", idat_len);
        close(fd);
        free(rows);
        return;
    }
    size_t io = 0;
    idat[io++] = 0x78;
    idat[io++] = 0x01;
    uint32_t adler_a = 1, adler_b = 0;
    size_t off = 0;
    while (off < raw_len) {
        size_t chunk = raw_len - off > 65535 ? 65535 : raw_len - off;
        idat[io++] = (uint8_t)(off + chunk >= raw_len ? 1 : 0);
        idat[io++] = (uint8_t)chunk;
        idat[io++] = (uint8_t)(chunk >> 8);
        idat[io++] = (uint8_t)~chunk;
        idat[io++] = (uint8_t)(~chunk >> 8);
        memcpy(idat + io, rows + off, chunk);
        for (size_t i = 0; i < chunk; i++) {
            adler_a = (adler_a + rows[off + i]) % 65521;
            adler_b = (adler_b + adler_a) % 65521;
        }
        io += chunk;
        off += chunk;
    }
    idat[io++] = (uint8_t)(adler_b >> 8);
    idat[io++] = (uint8_t)adler_b;
    idat[io++] = (uint8_t)(adler_a >> 8);
    idat[io++] = (uint8_t)adler_a;
    bad = bad || png_chunk(fd, "IDAT", idat, io) ||
          png_chunk(fd, "IEND", NULL, 0);
    free(idat);
    close(fd);
    free(rows);
    if (bad) {
        fprintf(stderr, "png: write failed\n");
        return;
    }
    printf("png: %ux%u gray8, %zu B payload -> %s\n", w, h, raw_len, path);
}

/* ---- JPEG path (M19c) ----
 * RAW10 RDI -> gray8 (bits[9:2]) -> optional bilinear debayer (RGGB —
 * verified correct phase for imx363 and imx481 on device 2026-09-01;
 * imx355 unverifiable, lens was blocked) -> jpegenc.h. */
static uint8_t *cs_raw10_gray(const uint8_t *raw, uint32_t w, uint32_t h,
                              uint32_t stride)
{
    uint8_t *g = malloc((size_t)w * h);
    if (!g) return NULL;
    for (uint32_t y = 0; y < h; y++) {
        const uint8_t *r = raw + (size_t)y * stride;
        for (uint32_t x = 0; x + 4 <= w; x += 4) {
            const uint8_t *p = r + (size_t)(x / 4) * 5;
            g[(size_t)y * w + x + 0] = p[0];
            g[(size_t)y * w + x + 1] = p[1];
            g[(size_t)y * w + x + 2] = p[2];
            g[(size_t)y * w + x + 3] = p[3];
        }
    }
    return g;
}

static uint8_t cs_at(const uint8_t *g, uint32_t w, uint32_t h,
                     int64_t x, int64_t y)
{
    if (x < 0) x = 0;
    if (y < 0) y = 0;
    if (x >= (int64_t)w) x = w - 1;
    if (y >= (int64_t)h) y = h - 1;
    return g[(size_t)y * w + (size_t)x];
}

/* gray-world white balance: per-site means of the RGGB pattern (R at
 * even/even, B at odd/odd, G on the cross), gains normalizing all three
 * to the brightest site mean — brightness is preserved (gains >= 1) so
 * this never darkens the exposure we tuned with --gain/--dgain */
static void wb_measure(const uint8_t *g, uint32_t w, uint32_t h, float wb[3])
{
    double sr = 0, sg = 0, sb = 0;
    uint64_t nr = 0;
    for (uint32_t y = 0; y + 1 < h; y += 2)
        for (uint32_t x = 0; x + 1 < w; x += 2) {
            size_t i0 = (size_t)y * w + x;
            sr += g[i0];
            sg += g[i0 + 1] + g[i0 + w];   /* 2 G samples per quad */
            sb += g[i0 + w + 1];
            nr++;
        }
    if (!nr) { wb[0] = wb[1] = wb[2] = 1.0f; return; }
    double mr = sr / nr, mg = sg / (2.0 * nr), mb = sb / nr;
    if (mr < 1.0 || mg < 1.0 || mb < 1.0) {
        wb[0] = wb[1] = wb[2] = 1.0f;      /* black frame — nothing to balance */
        return;
    }
    double top = mr > mg ? (mr > mb ? mr : mb) : (mg > mb ? mg : mb);
    wb[0] = (float)(top / mr);
    wb[1] = (float)(top / mg);
    wb[2] = (float)(top / mb);
    for (int k = 0; k < 3; k++)
        if (wb[k] > 4.0f) wb[k] = 4.0f;    /* keep tinted scenes sane */
}

static uint8_t *cs_debayer(const uint8_t *g, uint32_t w, uint32_t h,
                           const float wb[3])
{
    uint8_t *rgb = malloc((size_t)w * h * 3);
    if (!rgb) return NULL;
    for (uint64_t y = 0; y < h; y++)
        for (uint64_t x = 0; x < w; x++) {
            /* RGGB: even row = R G, odd row = G B */
            int site = !(y & 1) ? (!(x & 1) ? 0 : 1) : (!(x & 1) ? 1 : 2);
            int R, G, B;
            int l = cs_at(g, w, h, x - 1, y), r = cs_at(g, w, h, x + 1, y);
            int u = cs_at(g, w, h, x, y - 1), d = cs_at(g, w, h, x, y + 1);
            int ul = cs_at(g, w, h, x - 1, y - 1), ur = cs_at(g, w, h, x + 1, y - 1);
            int dl = cs_at(g, w, h, x - 1, y + 1), dr = cs_at(g, w, h, x + 1, y + 1);
            if (site == 0) {          /* R site */
                R = g[(size_t)y * w + x];
                G = (l + r + u + d) / 4;
                B = (ul + ur + dl + dr) / 4;
            } else if (site == 2) {   /* B site */
                B = g[(size_t)y * w + x];
                G = (l + r + u + d) / 4;
                R = (ul + ur + dl + dr) / 4;
            } else if (!(y & 1)) {    /* G on R row: R left/right */
                G = g[(size_t)y * w + x];
                R = (l + r) / 2;
                B = (u + d) / 2;
            } else {                  /* G on B row: B left/right */
                G = g[(size_t)y * w + x];
                B = (l + r) / 2;
                R = (u + d) / 2;
            }
            int Rc = (int)(R * wb[0] + 0.5f);
            int Gc = (int)(G * wb[1] + 0.5f);
            int Bc = (int)(B * wb[2] + 0.5f);
            uint8_t *p = rgb + ((size_t)y * w + x) * 3;
            p[0] = (uint8_t)(Rc > 255 ? 255 : Rc);
            p[1] = (uint8_t)(Gc > 255 ? 255 : Gc);
            p[2] = (uint8_t)(Bc > 255 ? 255 : Bc);
        }
    return rgb;
}

static void dump_jpeg(const uint8_t *rdi, uint32_t w, uint32_t h,
                      uint32_t stride, const char *raw_path)
{
    char def[512];
    const char *path = g_jpeg_out;
    if (!path) {
        snprintf(def, sizeof def, "%s", raw_path);
        char *dot = strrchr(def, '.');
        if (dot && !strcmp(dot, ".raw")) strcpy(dot, ".jpg");
        else strcat(def, ".jpg");
        path = def;
    }
    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);

    uint8_t *g = cs_raw10_gray(rdi, w, h, stride);
    if (!g) { fprintf(stderr, "jpeg: no mem for gray\n"); return; }
    size_t cap = (size_t)w * h * 3 + 65536;
    uint8_t *out = malloc(cap);
    if (!out) { fprintf(stderr, "jpeg: no mem for %zu B out\n", cap); free(g); return; }
    ssize_t n;
    if (g_jpeg_color) {
        float wb[3] = { g_wb_r, g_wb_g, g_wb_b };
        if (g_wb_auto) {
            wb_measure(g, w, h, wb);
            printf("wb: auto r=%.2f g=%.2f b=%.2f\n", wb[0], wb[1], wb[2]);
        }
        uint8_t *rgb = cs_debayer(g, w, h, wb);
        free(g);
        if (!rgb) { fprintf(stderr, "jpeg: no mem for rgb\n"); free(out); return; }
        n = jpeg_encode_rgb24(rgb, (int)w, (int)h, (int)w * 3, g_jpeg_q,
                              out, cap);
        free(rgb);
    } else {
        n = jpeg_encode_gray8(g, (int)w, (int)h, (int)w, g_jpeg_q,
                              out, (size_t)w * h + 65536);
        free(g);
    }
    if (n < 0) { fprintf(stderr, "jpeg: encode overflow\n"); free(out); return; }
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        fprintf(stderr, "open %s: %s\n", path, strerror(errno));
        free(out);
        return;
    }
    int bad = wr_all(fd, out, (size_t)n) < 0;
    close(fd);
    free(out);
    clock_gettime(CLOCK_MONOTONIC, &b);
    double t = (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9;
    if (bad) { fprintf(stderr, "jpeg: write failed\n"); return; }
    printf("jpeg: %ux%u %s q%d -> %zd B (%.2f bpp) in %.3f s -> %s\n",
           w, h, g_jpeg_color ? "color" : "gray", g_jpeg_q, n,
           (double)n * 8 / ((double)w * h), t, path);
}

/* numbered output path for frame `frameno` of `total` (suffix -<n>
 * when bursting) */
static void frame_path(char *out, size_t outsz, const char *base,
                       int frameno, int total)
{
    if (total == 1) {
        snprintf(out, outsz, "%s", base);
        return;
    }
    const char *dot = strrchr(base, '.');
    if (dot)
        snprintf(out, outsz, "%.*s-%d%s",
                 (int)(dot - base), base, frameno, dot);
    else
        snprintf(out, outsz, "%s-%d", base, frameno);
}

/* heavy pass for one landed frame: buffer stats + optional raw dump +
 * optional PNG/JPEG. dump_raw==0 keeps the stats but skips the multi-MB
 * write. Returns nonzero bytes seen. */
static size_t process_frame(const uint8_t *map, size_t len,
                            const char *out_path, int frameno, int total,
                            uint32_t width, uint32_t height, uint32_t stride,
                            int dump_raw)
{
    char fpath[512];
    frame_path(fpath, sizeof fpath, out_path, frameno, total);
    if (g_pix && !g_pixraw) {
        /* first light: dump the NV12 raw and report Y/C coverage — the
         * PNG/JPEG paths assume RAW10 geometry. Decode on host until the
         * PIX output is trusted. */
        size_t nz = inspect_buf(map, len, fpath, "pix");
        if (nz) {
            size_t yn = (size_t)g_pstride * g_ph, cn = (size_t)g_pstride * (g_ph / 2);
            double sum = 0;
            for (size_t i = 0; i < yn; i++) sum += map[i];
            size_t cs = 0;
            const uint8_t *c = map + g_pcoff;
            for (size_t i = 0; i < cn; i++)
                if (c[i]) cs++;
            printf("pix frame %d/%d: Y mean %.1f, C nonzero %zu/%zu\n",
                   frameno, total, sum / (double)yn, cs, cn);
        }
        return nz;
    }
    size_t nz = inspect_buf(map, len, dump_raw ? fpath : NULL, "frame");
    if (nz && g_png) {
        char ppath[512];
        if (total == 1)
            snprintf(ppath, sizeof ppath, "/tmp/frame.png");
        else
            snprintf(ppath, sizeof ppath, "/tmp/frame-%d.png", frameno);
        dump_png(map, width, height, stride, ppath);
    }
    if (nz && g_jpeg_q > 0)
        dump_jpeg(map, width, height, stride, fpath);
    return nz;
}

/* dump the kernel-built CDM command region of an UPDATE packet's kmd
 * scratch: the RDI write-master base address lives in there. Scan u32s for
 * IOVA-looking values plus a raw prefix for eyeballing. */
static void dump_kmd(struct shot_bufs *b, int req)
{
    uint32_t half = (uint32_t)(b->cmd_cap / 2);
    uint32_t *k = (uint32_t *)((uint8_t *)b->p_cmd + half);
    printf("    kmd[%d] aligned iova-like:", req);
    for (uint32_t i = 0; i < 64; i++)
        if (k[i] >= 0xd0000000u && k[i] < 0xf0000000u && (k[i] & 0xfff) == 0)
            printf(" [%u]=0x%x", i, k[i]);
    printf(" | near:");
    for (uint32_t i = 0; i < 64; i++)
        if (k[i] >= 0xd0000000u && k[i] < 0xf0000000u && (k[i] & 0xfff))
            printf(" [%u]=0x%x", i, k[i]);
    printf("\n    kmd[%d] words 0..31:", req);
    for (uint32_t i = 0; i < 32; i++) printf(" %08x", k[i]);
    printf("\n");
}

/* sensor NOP packet for req_id — registers the request with the req mgr
 * (every linked device must add a request before it can be applied). */
static int sensor_nop(int video_fd, int sd_fd, struct shot_bufs *b,
                      uint32_t session, uint32_t dev_hdl, int64_t req_id)
{
    memset(b->p_pkt, 0, 512);
    struct cam_packet *pk = b->p_pkt;
    pk->header.op_code = 127;
    pk->header.request_id = (uint64_t)req_id;
    pk->header.size = (uint32_t)sizeof(*pk);
    pk->num_cmd_buf = 0;
    struct cam_config_dev_cmd cfg = {
        .session_handle = (int32_t)session, .dev_handle = (int32_t)dev_hdl,
        .offset = 0, .packet_handle = b->pkt.out.buf_handle };
    return cam_ioctl(sd_fd, CAM_CONFIG_DEV, &cfg,
                     CAM_HANDLE_USER_POINTER, sizeof(cfg));
}

/* IFE packet builder. num_cmd = 1 (kmd only, length 0 => whole buf is KMD
 * scratch for CDM commands), 2 (kmd + generic-blob cmd, meta 12) or 3 (kmd +
 * blob + raw CDM payload cmd, meta 3 — the userspace ISP-module programming
 * channel; only the INIT packet uses it). The CDM payload words sit right
 * after the blob in the [0, half) region.
 * io_cfg: optional output port entry. */
static int isp_config(int video_fd, int isp_fd, struct shot_bufs *b,
                      uint32_t session, uint32_t dev_hdl, uint32_t op,
                      int64_t req_id, size_t blob_len,
                      const uint32_t *cdm, uint32_t cdm_words,
                      int n_io, const struct cam_buf_io_cfg *io)
{
    memset(b->p_pkt, 0, 1024);
    int ndesc = 1 + (blob_len ? 1 : 0) + (cdm_words ? 1 : 0);
    struct cam_packet *pk = b->p_pkt;
    pk->header.op_code = op;
    pk->header.request_id = (uint64_t)req_id;
    pk->header.size = (uint32_t)(sizeof(*pk) +
        ndesc * sizeof(struct cam_cmd_buf_desc) +
        n_io * sizeof(struct cam_buf_io_cfg));
    pk->num_cmd_buf = ndesc;
    pk->kmd_cmd_buf_index = 0;
    pk->kmd_cmd_buf_offset = 0;
    struct cam_cmd_buf_desc *d = (void *)pk->payload;
    /* cmd buffer layout: [0, half) carries the blob payload (desc 1) and the
     * raw CDM payload (desc 2), [half, cap) is kmd scratch where the hw mgr
     * builds CDM commands. desc 0 (kmd) has length 0 -> add_command_buffers
     * skips it. */
    uint32_t half = (uint32_t)(b->cmd_cap / 2);
    d[0].mem_handle = (int32_t)b->cmd.out.buf_handle;
    d[0].offset = half;
    d[0].size = half;
    d[0].length = 0;
    if (blob_len) {
        d[1].mem_handle = (int32_t)b->cmd.out.buf_handle;
        d[1].offset = 0;
        d[1].size = half;
        d[1].length = (uint32_t)blob_len;
        d[1].meta_data = CAM_ISP_PACKET_META_GENERIC_BLOB_COMMON;
    }
    if (cdm_words) {
        int di = blob_len ? 2 : 1;
        d[di].mem_handle = (int32_t)b->cmd.out.buf_handle;
        d[di].offset = (uint32_t)blob_len;  /* right after the blob */
        d[di].size = half - (uint32_t)blob_len;
        d[di].length = cdm_words * 4;
        d[di].meta_data = CAM_ISP_PACKET_META_COMMON;
        /* the desc points into the mapped cmd buffer — the payload itself
         * must live there (first run left it on the stack: the CDM fetched
         * zeros at the BL iova and raised Invalid-command, 2026-09-02). */
        memcpy((uint8_t *)b->p_cmd + blob_len, cdm, cdm_words * 4);
    }
    if (n_io) {
        pk->io_configs_offset =
            ndesc * sizeof(struct cam_cmd_buf_desc);
        pk->num_io_configs = n_io;
        memcpy((uint8_t *)pk->payload + pk->io_configs_offset, io,
            n_io * sizeof(*io));
    }
    struct cam_config_dev_cmd cfg = {
        .session_handle = (int32_t)session, .dev_handle = (int32_t)dev_hdl,
        .offset = 0, .packet_handle = b->pkt.out.buf_handle };
    return cam_ioctl(isp_fd, CAM_CONFIG_DEV, &cfg,
                     CAM_HANDLE_USER_POINTER, sizeof(cfg));
}

/* append one generic blob (word = size<<8 | type, payload padded to 4) */
static size_t blob_add(uint8_t *p, uint32_t type, const void *payload,
                       size_t len)
{
    uint32_t hdr = ((uint32_t)len << 8) | type;
    memcpy(p, &hdr, 4);
    memcpy(p + 4, payload, len);
    size_t pad = (4 - (len & 3)) & 3;
    memset(p + 4 + len, 0, pad);
    return 4 + len + pad;
}

/* output io config for one request. RDI: single-plane RAW10. PIX: NV12 with
 * Y and C declared as two planes of the same buffer (the kernel's plane loop
 * breaks at mem_handle 0; same handle twice = two planes). bus_ver2 takes
 * each plane's stride/slice_height from here and checks the stride is
 * 16-aligned (cam_vfe_bus_ver2_update_wm). */
static void fill_out_io(struct cam_buf_io_cfg *io, uint32_t buf_handle,
                        uint32_t fence, uint32_t width, uint32_t height,
                        uint32_t raw_stride)
{
    memset(io, 0, sizeof(*io));
    io->fence = (int32_t)fence;
    io->direction = CAM_BUF_OUTPUT;
    io->subsample_pattern = 1;
    io->subsample_period = 1;
    io->framedrop_pattern = 1;
    io->framedrop_period = 1;
    if (g_pixraw) {
        /* RAW_DUMP tap: single plane, 2 B/px, upstream of the YUV chain */
        io->mem_handle[0] = (int32_t)buf_handle;
        io->offsets[0] = 0;
        io->planes[0].width = width;
        io->planes[0].height = height;
        io->planes[0].plane_stride = g_pstride;
        io->planes[0].slice_height = height;
        io->format = CAM_FORMAT_PLAIN16_10;
        io->bpp = 16;
        io->resource_type = CAM_ISP_IFE_OUT_RES_RAW_DUMP;
    } else if (g_pix) {
        io->mem_handle[0] = (int32_t)buf_handle;
        io->mem_handle[1] = (int32_t)buf_handle;
        io->offsets[0] = 0;
        io->offsets[1] = g_pcoff;
        io->planes[0].width = width;
        io->planes[0].height = height;
        io->planes[0].plane_stride = g_pstride;
        io->planes[0].slice_height = height;
        io->planes[1].width = width;
        io->planes[1].height = height / 2;
        io->planes[1].plane_stride = g_pstride;
        io->planes[1].slice_height = height / 2;
        io->format = CAM_FORMAT_NV12;
        io->bpp = 8;
        io->resource_type = CAM_ISP_IFE_OUT_RES_FULL;
    } else {
        io->mem_handle[0] = (int32_t)buf_handle;
        io->offsets[0] = 0;
        io->planes[0].width = width;
        io->planes[0].height = height;
        io->planes[0].plane_stride = raw_stride;
        io->planes[0].slice_height = height;
        io->format = CAM_FORMAT_MIPI_RAW_10;
        io->bpp = 10;
        io->resource_type = CAM_ISP_IFE_OUT_RES_RDI_0;
    }
}

static int run_stream(int slot, const char *out_path, int wait_ms,
                      uint32_t settle_cnt, uint32_t tp, int rb, int nframes)
{
    int rc = 1, video_fd = -1, sync_fd = -1, kmsg = -1;
    int sensor_fd = -1, csiphy_fd = -1, isp_fd = -1;
    int rail_fd = -1;
    int out_fd = -1;
    uint32_t session = 0, sensor_hdl = 0, csiphy_hdl = 0, isp_hdl = 0,
             link_hdl = 0, rail_hdl = 0;
    struct shot_bufs sb = {0}, ib = {0};
    /* per-frame pixel buffers + fences (--frames N). The kernel's IFE
     * UPDATE packet pool tops out ~19 packets (observed 2026-09-01:
     * "isp UPDATE packet 20: Out of memory" pre-queueing 150), so the
     * pre-queue window is MAXF and longer runs recycle slots. */
    enum { MAXF = 16 };
    /* per-request UPDATE packets: the IFE hw mgr builds CDM commands into
     * the packet's own kmd scratch at submit and derefs them at SOF apply
     * — sharing one buffer across queued requests made req 2 apply fail
     * with rc -5 (observed 2026-09-01). One shot_bufs per request. */
    struct shot_bufs ub[MAXF];
    struct cam_mem_mgr_alloc_cmd pix[MAXF];
    void *pix_map[MAXF];
    int pix_mfd[MAXF];
    uint32_t sync_obj[MAXF];
    for (int fi = 0; fi < MAXF; fi++) {
        memset(&pix[fi], 0, sizeof(pix[fi]));
        memset(&ub[fi], 0, sizeof(ub[fi]));
        pix_map[fi] = MAP_FAILED;
        pix_mfd[fi] = -1;
        sync_obj[fi] = 0;
    }
    /* buffers/fences exist for `window` frames; nframes > MAXF runs RING
     * mode — each slot is recycled for request f+window the moment its
     * fence signals (see the wait loop). */
    int window = nframes < (int)MAXF ? nframes : (int)MAXF;
    int ring = nframes > (int)MAXF;
    double kt = 0;
    /* mode dims: slot 0 (rear imx363) = vendor-bin 2016x1136 binned mode
     * (2.86 MB RAW10); slot 1 (UW imx481) = vendor-bin mode #1301
     * 2328x1310 binned (3.81 MB RAW10, pck 281 MHz); slot 2 (front imx355)
     * = vendor-bin 1640x925 binned, pck 144 MHz (fll 2614 x llp 1836 x
     * 30 fps). */
    g_slot = slot;
    uint32_t width = 1640, height = 925, stride = 2050;
    uint32_t pixel_clk = 144000000, hbi = 1836, vbi = 2614;
    if (slot == 1) {
        width = 2328; height = 1310; stride = 2910;
        /* 24/15*439 = 702.4 Mbps/lane over 4 lanes -> pck 280.96 MHz;
         * fll 1888 x llp 5120 -> 29.1 fps (timing model must match the
         * applied mode table, see the imx481_mode3 note) */
        pixel_clk = 280960000; hbi = 5120; vbi = 1888;
    }
    if (slot == 0) {
        width = 2016; height = 1136; stride = 2520;
        /* timing model must match the applied mode table: #544 fll=1652
         * pck=208M; #2610 (slowrear/rear564) fll=2488, pck = 4176*2488*30
         * ~= 312M. Feeding #544 numbers under #2610 told the IFE to expect
         * EOF a frame and a half early — CCIF-violation-shaped. */
        if (g_slowrear) {
            pixel_clk = 312000000; hbi = 4176; vbi = 2488;
        } else {
            pixel_clk = 208000000; hbi = 4176; vbi = 1652;
        }
    }
    const uint32_t dt = g_dt;
    /* pix-mode geometry first (NV12 in one buffer: Y plane w x h at offset 0,
     * C plane w x ceil(h/2) at a 4K-aligned offset after Y — bus_ver2 takes
     * both plane addresses from the io config and halves the C-plane WM
     * height itself, cam_vfe_bus_ver2_get_res: PLANE_C height /= 2). */
    g_pw = width; g_ph = height;
    /* pix-mode plane stride: NV12 Y stride (1 B/px) or RAW_DUMP PLAIN16_10
     * (2 B/px). ALIGNUP 16 — kernel warns otherwise. */
    g_pstride = g_pixraw ? ((width * 2 + 15) & ~15u) : ((width + 15) & ~15u);
    g_pcoff = (g_pstride * height + 4095u) & ~4095u;
    const uint64_t pixbuf_len = g_pixraw
        ? (uint64_t)g_pstride * height
        : g_pix
        ? (uint64_t)g_pcoff + (uint64_t)g_pstride * ((height + 1) / 2)
        : (uint64_t)stride * height;

    setvbuf(stdout, NULL, _IOLBF, 0);
    video_fd = open("/dev/video3", O_RDWR);
    sync_fd = open("/dev/video4", O_RDWR);
    kmsg = open("/dev/kmsg", O_RDONLY | O_NONBLOCK);
    if (kmsg >= 0)
        kt = kmsg_drain(kmsg, 0);

    printf("== stream slot %d (%s), %ux%u %s (stride %u) ==\n",
        slot, slots[slot].name, width, height,
        g_pix ? (g_pixraw ? "PLAIN16 via PIX RAW_DUMP" : "NV12 via IFE PIX")
              : "RAW10 RDI", g_pix ? g_pstride : stride);

    /* locate the three nodes: sensor by slot; its querycap says which
     * csiphy index serves that slot; isp by entity type */
    sensor_fd = find_subdev_by_type(CAM_SENSOR_DEVICE_TYPE, slot, NULL);
    int phy_idx = -1;
    if (sensor_fd >= 0) {
        struct cam_sensor_query_cap cap;
        memset(&cap, 0, sizeof(cap));
        if (cam_ioctl(sensor_fd, CAM_QUERY_CAP, &cap, CAM_HANDLE_USER_POINTER,
                      sizeof(cap)) == 0)
            phy_idx = (int)cap.csiphy_slot_id;
    }
    csiphy_fd = phy_idx >= 0
        ? find_subdev_by_type(CAM_CSIPHY_DEVICE_TYPE, phy_idx, NULL) : -1;
    isp_fd = find_subdev_by_type(CAM_ISP_DEVICE_TYPE, -1, NULL);
    if (sensor_fd < 0 || csiphy_fd < 0 || isp_fd < 0) {
        fprintf(stderr, "node discovery failed: sensor=%d csiphy=%d(%d) isp=%d\n",
            sensor_fd, csiphy_fd, phy_idx, isp_fd);
        goto out;
    }

    /* 1. real probe (expected id -> is_probe_succeed=1; powers down after).
     * Runs in --tpg too: the kernel rejects the sensor ACQUIRE_DEV with
     * EINVAL while is_probe_succeed==0 (fresh boot), and the vendor TPG
     * topology keeps the (idle, powered) sensor in the link. TPG mode
     * differs only from here on: no regs, no mode config, no streamon,
     * no csiphy. */
    int prc = 0;
    printf("probe %s @0x%02x expect 0x%04x: ", slots[slot].name,
        slots[slot].addr, slot_id[slot]);
    fflush(stdout);
    for (int attempt = 1; attempt <= 3; attempt++) {
        double tp0 = kt;
        prc = probe_once(video_fd, sensor_fd, slot, slots[slot].addr,
                         0x0016, slot_id[slot]);
        kt = kmsg_drain(kmsg, tp0);
        printf("rc=%d (%s)%s", prc, prc ? strerror(errno) : "OK",
            attempt < 3 && prc ? "; retry " : "\n");
        if (prc == 0)
            break;
        sleep(2);
    }
    if (prc != 0) {
        /* without is_probe_succeed=1 the sensor driver rejects the INIT
         * packet; no point continuing */
        fprintf(stderr, "probe failed, aborting\n");
        rc = 2;
        goto out;
    }
    if (g_tpg)
        printf("tpg mode: no regs/mode-config/streamon/csiphy — CSID TPG is the source\n");

    /* 2. session */
    struct cam_req_mgr_session_info si;
    memset(&si, 0, sizeof(si));
    if (cam_ioctl(video_fd, CAM_REQ_MGR_CREATE_SESSION, &si,
                  CAM_HANDLE_USER_POINTER, sizeof(si)) < 0) {
        fprintf(stderr, "CREATE_SESSION: %s\n", strerror(errno));
        goto out;
    }
    session = (uint32_t)si.session_hdl;
    printf("session %u\n", session);

    /* 2a. --railhelper: hold the rear sensor's INIT so its rail set stays
     * up for the target slot's power-up (see g_railhelper note). */
    struct shot_bufs rail_sb = {0};
    if (g_railhelper && slot != 0) {
        rail_fd = find_subdev_by_type(CAM_SENSOR_DEVICE_TYPE, 0, NULL);
        struct cam_sensor_acquire_dev racq;
        memset(&racq, 0, sizeof(racq));
        if (rail_fd < 0) {
            fprintf(stderr, "railhelper: rear sensor node not found\n");
            goto out;
        }
        /* the kernel rejects ACQUIRE_DEV with EINVAL until a successful
         * PROBE marked is_probe_succeed=1 for that subdev (fresh boot) */
        double tr0 = kt;
        if (probe_once(video_fd, rail_fd, 0, slots[0].addr, 0x0016,
                       slot_id[0]) != 0) {
            fprintf(stderr, "railhelper: rear probe failed\n");
            kt = kmsg_drain(kmsg, tr0);
            goto out;
        }
        kt = kmsg_drain(kmsg, tr0);
        racq.session_handle = session;
        racq.handle_type = CAM_HANDLE_USER_POINTER;
        if (cam_ioctl(rail_fd, CAM_ACQUIRE_DEV, &racq,
                      CAM_HANDLE_USER_POINTER, sizeof(racq)) < 0) {
            fprintf(stderr, "railhelper ACQUIRE_DEV: %s\n", strerror(errno));
            goto out;
        }
        rail_hdl = racq.device_handle;
        if (shot_alloc(video_fd, &rail_sb, 16 * 80 + 256, 0) < 0)
            goto out;
        double tr = kt;
        if (sensor_config(video_fd, rail_fd, &rail_sb, session, rail_hdl,
                          2 /* INITIAL_CONFIG */, imx363_init,
                          sizeof(imx363_init) / sizeof(imx363_init[0])) < 0) {
            fprintf(stderr, "railhelper INIT: %s\n", strerror(errno));
            kt = stream_kmsg(kmsg, tr);
            goto out;
        }
        kt = kmsg_drain(kmsg, tr);
        printf("railhelper: rear @0 INIT held — SLG ldo1..7 + camera_ldo up\n");
    }

    /* 2b. occupy CSIDs above g_force_ife so the real acquire lands on it */
    if (g_force_ife >= 0 && g_force_ife <= 2) {
        int nhold = 2 - g_force_ife;
        if (hold_higher_csids(session, nhold,
                CAM_ISP_IFE_IN_RES_BASE + 1 + (uint32_t)phy_idx,
                (uint32_t)g_lanes, dt, width, height) < 0)
            goto out;
        printf("holding %d higher csid(s); real acquire should land IFE%d\n",
            nhold, g_force_ife);
    }

    /* 3a. sensor acquire */
    struct cam_sensor_acquire_dev acq;
    memset(&acq, 0, sizeof(acq));
    acq.session_handle = session;
    acq.handle_type = CAM_HANDLE_USER_POINTER;
    if (cam_ioctl(sensor_fd, CAM_ACQUIRE_DEV, &acq,
                  CAM_HANDLE_USER_POINTER, sizeof(acq)) < 0) {
        fprintf(stderr, "sensor ACQUIRE_DEV: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, kt);
        goto out;
    }
    sensor_hdl = acq.device_handle;
    printf("sensor dev hdl %u\n", sensor_hdl);

    /* 3b. csiphy acquire (combo_mode 0) */
    struct cam_csiphy_acquire_dev_info phy_ai;
    memset(&phy_ai, 0, sizeof(phy_ai));
    memset(&acq, 0, sizeof(acq));
    acq.session_handle = session;
    acq.handle_type = CAM_HANDLE_USER_POINTER;
    acq.info_handle = (uint64_t)(uintptr_t)&phy_ai;
    if (cam_ioctl(csiphy_fd, CAM_ACQUIRE_DEV, &acq,
                  CAM_HANDLE_USER_POINTER, sizeof(acq)) < 0) {
        fprintf(stderr, "csiphy ACQUIRE_DEV: %s\n", strerror(errno));
        goto out;
    }
    csiphy_hdl = acq.device_handle;
    printf("csiphy dev hdl %u\n", csiphy_hdl);

    /* 3c. isp acquire (compat constant: dev handle only, HW via ACQUIRE_HW) */
    struct cam_acquire_dev_cmd iacq;
    memset(&iacq, 0, sizeof(iacq));
    iacq.session_handle = (int32_t)session;
    iacq.handle_type = CAM_HANDLE_USER_POINTER;
    iacq.num_resources = CAM_API_COMPAT_CONSTANT;
    if (cam_ioctl(isp_fd, CAM_ACQUIRE_DEV, &iacq,
                  CAM_HANDLE_USER_POINTER, sizeof(iacq)) < 0) {
        fprintf(stderr, "isp ACQUIRE_DEV: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, kt);
        goto out;
    }
    isp_hdl = (uint32_t)iacq.dev_handle;
    printf("isp dev hdl %u\n", isp_hdl);

    /* 3d. isp ACQUIRE_HW: in_port PHY_2 -> out RDI_0, rdi-only context.
     * The kernel reads the in_port at &acquire_hw_info->data == blob+24;
     * nesting hdr+port in one struct puts the port 8 B late (observed:
     * "Invalid num output res 0"). Build it by pointer instead. */
    uint8_t acq_blob[256];
    memset(acq_blob, 0, sizeof(acq_blob));
    struct cam_isp_acquire_hw_info *ah = (void *)acq_blob;
    struct cam_isp_in_port_info *port = (void *)&ah->data;
    ah->common_info_version = 0x1000;
    ah->common_info_size = sizeof(*port);
    ah->num_inputs = 1;
    ah->input_info_version = 0x2000;
    ah->input_info_size = sizeof(*port);
    ah->input_info_offset = 0;
    /* in-port = the CSID PHY the sensor's csiphy routes to (PHY_0 for the
     * rear, PHY_2 for the front); claiming the wrong PHY leaves the IFE
     * listening on an RDI that never receives (observed: fence -110 with
     * every other step green). */
    port->res_type = g_tpg ? CAM_ISP_IFE_IN_RES_TPG
                           : (CAM_ISP_IFE_IN_RES_BASE + 1 + (uint32_t)phy_idx);
    port->lane_type = 0;              /* DPHY */
    port->lane_num = (uint32_t)g_lanes;
    port->lane_cfg = g_lanecfg;
    port->vc = g_vc;
    port->dt = dt;                    /* RAW10 */
    port->format = CAM_FORMAT_MIPI_RAW_10;
    port->test_pattern = 0;           /* PIX: Bayer phase RGRGRG = RGGB
                                       * (CAM_ISP_PATTERN_BAYER_RGRGRG;
                                       * feeds core_cfg's pixel_pattern) */
    port->usage_type = 0;             /* single IFE */
    port->left_start = 0;
    port->left_stop = width - 1;
    port->left_width = width;
    port->line_start = 0;
    port->line_stop = height - 1;
    port->height = height;
    port->pixel_clk = pixel_clk;      /* imx363 208M / imx355 177.6M */
    port->num_out_res = 1;
    port->data[0].res_type = g_pix
        ? (g_pixraw ? CAM_ISP_IFE_OUT_RES_RAW_DUMP
                    : CAM_ISP_IFE_OUT_RES_FULL)
        : CAM_ISP_IFE_OUT_RES_RDI_0;
    port->data[0].format = g_pix
        ? (g_pixraw ? CAM_FORMAT_PLAIN16_10 : CAM_FORMAT_NV12)
        : CAM_FORMAT_MIPI_RAW_10;
    port->data[0].width = width;
    port->data[0].height = height;

    struct cam_acquire_hw_cmd_v2 ahw;
    memset(&ahw, 0, sizeof(ahw));
    ahw.struct_version = 2;
    ahw.session_handle = (int32_t)session;
    ahw.dev_handle = (int32_t)isp_hdl;
    ahw.handle_type = CAM_HANDLE_USER_POINTER;
    ahw.data_size = 24 + (uint32_t)sizeof(*port);
    ahw.resource_hdl = (uint64_t)(uintptr_t)acq_blob;
    if (cam_ioctl(isp_fd, CAM_ACQUIRE_HW, &ahw,
                  CAM_HANDLE_USER_POINTER, sizeof(ahw)) < 0) {
        fprintf(stderr, "isp ACQUIRE_HW: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, kt);
        goto out;
    }
    printf("isp ACQUIRE_HW ok (hw id mask 0x%x, valid %u)\n",
        ahw.hw_info.acquired_hw_id[0], ahw.hw_info.valid_acquired_hw);

    /* 4. link sensor + ife */
    struct cam_req_mgr_link_info li;
    memset(&li, 0, sizeof(li));
    li.session_hdl = (int32_t)session;
    li.num_devices = 2;
    li.dev_hdls[0] = (int32_t)sensor_hdl;
    li.dev_hdls[1] = (int32_t)isp_hdl;
    if (cam_ioctl(video_fd, CAM_REQ_MGR_LINK, &li,
                  CAM_HANDLE_USER_POINTER, sizeof(li)) < 0) {
        fprintf(stderr, "LINK: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, kt);
        goto out;
    }
    link_hdl = (uint32_t)li.link_hdl;
    printf("link hdl %u\n", link_hdl);

    /* 5. sensor register lists: global (INIT), mode (CONFIG), MODE_SELECT
     * (STREAMON, applied at START_DEV).
     * --tpg still runs INIT+CONFIG: the kernel only advances the sensor to
     * CAM_SENSOR_CONFIG when a CONFIG packet carries real i2c settings,
     * and the NOP opcode handler silently drops our per-frame nop while
     * the state is INIT/ACQUIRE ("Rxed NOP packets without linking") —
     * the req mgr then skips every frame waiting on cam-sensor and the
     * IFE's req 1 (with our fence) never applies (observed 2026-09-01:
     * "Skip Frame: req: 1 not ready ... dev: cam-sensor" at every SOF).
     * Only the STREAMON packet (sensor emitting) stays sensor-mode-only. */
    /* cmd buffer sized for the biggest table: imx481_init is a single
     * 209-write group = 8 B header + 209 x 8 B = 1680 B (the old
     * 16*80+256 = 1536 would truncate it) */
    if (shot_alloc(video_fd, &sb, 4096, 0) < 0)
        goto out;
    const struct wreg *init_tbl = slot == 0 ? imx363_init
        : (slot == 1 ? imx481_init : imx355_vinit);
    size_t n_init = slot == 0
        ? sizeof(imx363_init) / sizeof(imx363_init[0])
        : (slot == 1 ? sizeof(imx481_init) / sizeof(imx481_init[0])
                     : sizeof(imx355_vinit) / sizeof(imx355_vinit[0]));
    if (slot == 0 && !g_keep0112) {
        /* 2026-09-01 exact register diff vs the vendor bin: our INIT = the
         * bin's 29-write initSettings plus this prepended 0x0112=0x0a — the
         * vendor NEVER writes 0x0112 (relies on POR default). That prepend
         * was the sole sensor-state delta, so drop it by default;
         * --keep0112 restores the old behavior for A/B. */
        init_tbl++;
        n_init--;
    }
    double t0 = kt;
    if (!g_noglobal &&
        sensor_config(video_fd, sensor_fd, &sb, session, sensor_hdl,
                      2 /* INITIAL_CONFIG */, init_tbl, n_init) < 0) {
        fprintf(stderr, "sensor INIT packet: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, t0);
        goto out;
    }
    printf("sensor INIT (global %zu regs) %s\n", n_init,
        g_noglobal ? "SKIPPED (--noglobal)" : "applied");
    t0 = kt;
    /* tp: force the sensor's built-in test pattern (reg 0x0600) — makes it
     * emit MIPI regardless of the array, separating "sensor transmits" from
     * "sensor exposes". */
    struct wreg cfg_regs[192];   /* imx481_mode3 alone is 138 writes */
    size_t n_cfg;
    if (slot == 0) {
        if (g_slowrear) {
            memcpy(cfg_regs, imx363_mode2610, sizeof(imx363_mode2610));
            n_cfg = sizeof(imx363_mode2610) / sizeof(imx363_mode2610[0]);
            printf("rear imx363: vendor-bin mode #2610 2016x1136 "
                   "(1128 Mbps/lane — true rate test; halfrate touched only "
                   "the pck mult 0x0307)\n");
        } else {
        memcpy(cfg_regs, imx363_cfg, sizeof(imx363_cfg));
        n_cfg = sizeof(imx363_cfg) / sizeof(imx363_cfg[0]);
        printf("rear imx363: vendor-bin mode 2016x1136 (verbatim, no PLL "
               "retune — 24 MHz MCLK matches vendor design)\n");
        }
        if (g_rear564) {
            /* half the #2610 lane rate: OP_MUL 188->94 keeps pck (0x0307
             * path) and 30 fps untouched, only the serializer slows. Line
             * payload needs >=~380 Mbps/lane, so 564 still has headroom. */
            for (size_t i = 0; i < n_cfg; i++)
                if (cfg_regs[i].addr == 0x030f)
                    cfg_regs[i].val = 0x5e;
            printf("rear564: OP_MUL 188->94 (564 Mbps/lane)\n");
        }
        /* exposure/gain (imx355-family formulas, mainline imx355.c):
         * CIT 0x0202 (<= FLL-10), analog gain 0x0204 multiplier =
         * 1024/(1024-reg) so reg = 1024-1024/g (max 960 = 16x), digital
         * gain 0x020e (256 = 1x) with 0x3070=1 selecting global. Mode
         * #2610 defaults CIT 2474 (FLL 2488) and gain 1x — correct for a
         * lit room; a dark scene needs --gain/--dgain, not more time. */
        if (g_cit) {
            cfg_override(cfg_regs, &n_cfg, 0x0202, (uint8_t)(g_cit >> 8));
            cfg_override(cfg_regs, &n_cfg, 0x0203, (uint8_t)(g_cit & 0xff));
            printf("exposure: CIT=%u lines\n", g_cit);
        }
        if (g_gain > 1.0) {
            double m = g_gain > 16.0 ? 16.0 : g_gain;
            unsigned gv = (unsigned)(1024.0 - 1024.0 / m + 0.5);
            if (gv > 960) gv = 960;
            cfg_override(cfg_regs, &n_cfg, 0x0204, (uint8_t)(gv >> 8));
            cfg_override(cfg_regs, &n_cfg, 0x0205, (uint8_t)(gv & 0xff));
            printf("analog gain: %.2fx (reg 0x%03x)\n",
                   1024.0 / (1024.0 - (double)gv), gv);
        }
        if (g_dgain > 1.0) {
            double m = g_dgain > 16.0 ? 16.0 : g_dgain;
            unsigned dv = (unsigned)(m * 256.0 + 0.5);
            if (dv > 4095) dv = 4095;
            cfg_override(cfg_regs, &n_cfg, 0x3070, 0x01);
            cfg_override(cfg_regs, &n_cfg, 0x020e, (uint8_t)(dv >> 8));
            cfg_override(cfg_regs, &n_cfg, 0x020f, (uint8_t)(dv & 0xff));
            printf("digital gain: %.2fx (reg 0x%03x)\n", dv / 256.0, dv);
        }
        if (g_halfrate) {
            for (size_t i = 0; i < n_cfg; i++)
                if (cfg_regs[i].addr == 0x0307)
                    cfg_regs[i].val = 104;
            printf("halfrate: PLL mult 207->104 (~260 Mbps/lane, ~15 fps)\n");
        }
        /* global regs the vendor INIT writes but mode table #544 lacks
         * (live readback 2026-09-01: defaults 0x0136=0x1a(26MHz!), 0x0820=0).
         * 0x0136 INCK freq 8.8 fixed point must equal the real MCLK (24M).
         * 0x0820 REQ_LINK_BIT_RATE = total Mbps = OP-PLL VCO x lanes, rule
         * proven on this device's imx355 vendor table (0x5a0=1440 = 24/2*30
         * *4); rear OP regs 0x030d=4 prediv, mpy16 0x030e:0x030f=0x0132=306
         * -> 24/4*306 = 1836/lane -> 7344 = 0x1cb0.
         * CAVEAT (2026-09-01): the vendor bin writes NEITHER of these for
         * imx363 — the rear tables above are verbatim and leave both at
         * defaults. --rawvendor drops this whole block to test whether our
         * guessed REQ_LINK (a live DPHY-rate knob on Sony parts) is what
         * corrupts every packet header (ERROR_ECC on all packets, settle-
         * and CSID-independent, observed all rear sessions). */
        if (!g_rawvendor) {
        cfg_regs[n_cfg].addr = 0x0136;
        cfg_regs[n_cfg].val = 0x18;
        cfg_regs[n_cfg].width = 8;
        n_cfg++;
        cfg_regs[n_cfg].addr = 0x0137;
        cfg_regs[n_cfg].val = 0x00;
        cfg_regs[n_cfg].width = 8;
        n_cfg++;
        cfg_regs[n_cfg].addr = 0x0820;
        cfg_regs[n_cfg].val = 0x1c;
        cfg_regs[n_cfg].width = 8;
        n_cfg++;
        cfg_regs[n_cfg].addr = 0x0821;
        cfg_regs[n_cfg].val = 0xb0;
        cfg_regs[n_cfg].width = 8;
        n_cfg++;
        } else {
            printf("rawvendor: 0x0136/0x0821 block skipped (bin verbatim)\n");
        }
        /* masterSettings (see imx363_master note) — the rear runs as the
         * sync master; without these the sensor gates frame readout. */
        memcpy(&cfg_regs[n_cfg], imx363_master, sizeof(imx363_master));
        n_cfg += sizeof(imx363_master) / sizeof(imx363_master[0]);
        printf("rear masterSettings appended (%zu regs)\n",
               sizeof(imx363_master) / sizeof(imx363_master[0]));
        if (tp) {
            /* vendor test-pattern regSettings write the 16-bit 0x0600 as
             * two bytes: 0x600<-0 (hi) 0x601<-N (lo). Values 0..4 match
             * mainline imx355: off/solid/bars/grey-bars/PN9. */
            cfg_regs[n_cfg].addr = 0x0600;
            cfg_regs[n_cfg].val = 0;
            cfg_regs[n_cfg].width = 8;
            n_cfg++;
            cfg_regs[n_cfg].addr = 0x0601;
            cfg_regs[n_cfg].val = (uint16_t)tp;
            cfg_regs[n_cfg].width = 8;
            n_cfg++;
            printf("test pattern 0x0600<-0x%04x appended\n", tp);
        }
    } else if (slot == 1) {
        memcpy(cfg_regs, imx481_mode3, sizeof(imx481_mode3));
        n_cfg = sizeof(imx481_mode3) / sizeof(imx481_mode3[0]);
        printf("uw imx481: vendor-bin mode 2328x1310 4-lane (verbatim, "
               "702 Mbps/lane, ~29 fps)\n");
        /* exposure/gain: same imx355-family register set as the rear */
        if (g_cit) {
            cfg_override(cfg_regs, &n_cfg, 0x0202, (uint8_t)(g_cit >> 8));
            cfg_override(cfg_regs, &n_cfg, 0x0203, (uint8_t)(g_cit & 0xff));
            printf("exposure: CIT=%u lines\n", g_cit);
        }
        if (g_gain > 1.0) {
            double m = g_gain > 16.0 ? 16.0 : g_gain;
            unsigned gv = (unsigned)(1024.0 - 1024.0 / m + 0.5);
            if (gv > 960) gv = 960;
            cfg_override(cfg_regs, &n_cfg, 0x0204, (uint8_t)(gv >> 8));
            cfg_override(cfg_regs, &n_cfg, 0x0205, (uint8_t)(gv & 0xff));
            printf("analog gain: %.2fx (reg 0x%03x)\n",
                   1024.0 / (1024.0 - (double)gv), gv);
        }
        if (g_dgain > 1.0) {
            double m = g_dgain > 16.0 ? 16.0 : g_dgain;
            unsigned dv = (unsigned)(m * 256.0 + 0.5);
            if (dv > 4095) dv = 4095;
            cfg_override(cfg_regs, &n_cfg, 0x3070, 0x01);
            cfg_override(cfg_regs, &n_cfg, 0x020e, (uint8_t)(dv >> 8));
            cfg_override(cfg_regs, &n_cfg, 0x020f, (uint8_t)(dv & 0xff));
            printf("digital gain: %.2fx (reg 0x%03x)\n", dv / 256.0, dv);
        }
        if (tp) {
            cfg_regs[n_cfg].addr = 0x0600;
            cfg_regs[n_cfg].val = 0;
            cfg_regs[n_cfg].width = 8;
            n_cfg++;
            cfg_regs[n_cfg].addr = 0x0601;
            cfg_regs[n_cfg].val = (uint16_t)tp;
            cfg_regs[n_cfg].width = 8;
            n_cfg++;
            printf("test pattern 0x0600<-0x%04x appended\n", tp);
        }
    } else {
        memcpy(cfg_regs, imx355_vcfg, sizeof(imx355_vcfg));
        n_cfg = sizeof(imx355_vcfg) / sizeof(imx355_vcfg[0]);
        printf("front imx355: vendor-bin mode 1640x925 4-lane (verbatim, "
               "360 Mbps/lane, 30 fps — mainline 2-lane table retired)\n");
        if (tp) {
            int has_tp = 0;
            for (size_t i = 0; i < n_cfg; i++)
                if (cfg_regs[i].addr == 0x0600) {
                    cfg_regs[i].val = (uint16_t)tp; has_tp = 1;
                }
            printf("test pattern 0x%04x %s\n", tp,
                has_tp ? "enabled" : "NOT SET (no 0x0600 in vendor table)");
        }
    }
    if (sensor_config(video_fd, sensor_fd, &sb, session, sensor_hdl,
                      4 /* CONFIG */, cfg_regs, n_cfg) < 0) {
        fprintf(stderr, "sensor CONFIG packet: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, t0);
        goto out;
    }
    printf("sensor CONFIG (mode/crop/pll %zu regs) applied\n", n_cfg);
    t0 = kt;
    if (!g_tpg) {
    if (sensor_config(video_fd, sensor_fd, &sb, session, sensor_hdl,
                      0 /* STREAMON, held for START_DEV */, g_nostarton
                          ? imx355_streamoff : imx355_streamon,
                      1) < 0) {
        fprintf(stderr, "sensor STREAMON packet: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, t0);
        goto out;
    }
    printf("sensor STREAMON packet queued%s\n",
        g_nostarton ? " (MODE_SELECT=0 — sensor held in standby)" : "");
    }
    /* 6. csiphy: 2-lane DPHY @ 888 Mbps/lane (link freq 444 MHz).
     * KMD does NOT derive settle from data_rate: cam_csiphy_config_dev
     * does plain settle_cnt = settle_time / 200000000, so 0 means the
     * HS-RX settle counter is programmed 0 and the receiver never syncs
     * (observed: no data, no CSID errors). settle_cnt from the DPHY
     * spec + mainline camss 2ph calc:
     *   ui = 1e12/data_rate ps = 1126
     *   t_hs_settle = (85+6ui + 145+10ui)/2 = 115000+8ui ps
     *   timer 200 MHz -> 5000 ps/count
     *   settle_cnt = t_hs_settle/5000 - 1 = 23
     * settle_time (ps-ish vendor units) = settle_cnt * 2e8. */
    struct cam_csiphy_info csi;
    memset(&csi, 0, sizeof(csi));
    if (!g_tpg) {
    /* lane_mask bitmap: bit0=DL0, bit1=CLK, bit2=DL1, bit3=DL2, bit4=DL3
     * (cam_csiphy_config_dev builds the lane-enable register from it).
     * 2-lane 0x7, 4-lane 0x1f — the vendor module runs 4 data lanes. */
    csi.lane_mask = g_lanes == 4 ? 0x1f : 0x7;
    csi.lane_assign = 0x0000;
    csi.csiphy_3phase = 0;
    csi.combo_mode = 0;
    csi.lane_cnt = (uint8_t)g_lanes;
    csi.secure_mode = 0;
    {
        /* per-lane rate: imx363 MIPI = OP-PLL VCO = INCK/0x030d*mpy16
         * = 24/4*306 = 1836 Mbps (rule proven on this device's imx355
         * vendor bin: 0x0820=1440 total = 24/2*30*4 lanes);
         * imx355 vendor = REQ_LINK 0x0820 1440 Mbps total / 4 = 360. */
        uint64_t dr = slot == 0
            ? (g_rear564 ? 564000000ULL :
               (g_slowrear ? 1128000000ULL :
               (g_halfrate ? 918000000ULL : 1836000000ULL)))
            : (slot == 1
               ? 702400000ULL   /* 24/15*439, see imx481_mode3 note */
               : (g_halfrate ? 180000000ULL : 360000000ULL));
        uint64_t ui = 1000000000000ULL / dr;      /* ps */
        uint32_t cnt = (uint32_t)((115000 + 8 * ui) / 5000) - 1;
        if (settle_cnt)
            cnt = settle_cnt;
        csi.settle_time = (uint64_t)cnt * 200000000ULL;
        printf("csiphy settle_cnt=%u (settle_time=%llu, dr=%llu Mbps)\n",
            cnt, (unsigned long long)csi.settle_time,
            (unsigned long long)(dr / 1000000));
        csi.data_rate = dr;
    }
    struct cam_config_dev_cmd pcfg = {
        .session_handle = (int32_t)session, .dev_handle = (int32_t)csiphy_hdl,
        .offset = 0, .packet_handle = (uint64_t)(uintptr_t)&csi };
    if (cam_ioctl(csiphy_fd, CAM_CONFIG_DEV_EXTERNAL, &pcfg,
                  CAM_HANDLE_USER_POINTER, sizeof(pcfg)) < 0) {
        fprintf(stderr, "csiphy CONFIG_DEV_EXTERNAL: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, kt);
        goto out;
    }
    printf("csiphy configured (%d lane, %llu Mbps/lane)\n",
        g_lanes, (unsigned long long)(csi.data_rate / 1000000));
    }

    /* 7. pixel buffer via cam_mem_mgr (PIXEL_BUF maps into IFE img iommu).
     * HW bufs must name the SMMU ctx bank: num_hdl=0 leaves map_hw_va's
     * loop empty and it returns its init -1 = EPERM (observed on device).
     * The IFE image iommu handle comes from QUERY_CAP on the isp node. */
    struct cam_query_cap_cmd qc;
    struct cam_isp_query_cap_cmd qisp;
    memset(&qisp, 0, sizeof(qisp));
    memset(&qc, 0, sizeof(qc));
    qc.size = sizeof(qisp);
    qc.handle_type = CAM_HANDLE_USER_POINTER;
    qc.caps_handle = (uint64_t)(uintptr_t)&qisp;
    if (cam_ioctl(isp_fd, CAM_QUERY_CAP, &qc, CAM_HANDLE_USER_POINTER,
                  sizeof(qc)) < 0) {
        fprintf(stderr, "isp QUERY_CAP: %s\n", strerror(errno));
        goto out;
    }
    printf("isp iommu: img %d (sec %d), cdm %d\n",
        qisp.device_iommu.non_secure, qisp.device_iommu.secure,
        qisp.cdm_iommu.non_secure);
    for (int fi = 0; fi < window; fi++) {
        pix[fi].len = pixbuf_len;
        pix[fi].align = 4096;
        pix[fi].mmu_hdls[0] = qisp.device_iommu.non_secure;
        pix[fi].num_hdl = 1;
        pix[fi].flags = 0x81;  /* CAM_MEM_FLAG_HW_READ_WRITE|CAM_MEM_FLAG_PIXEL_BUF_TYPE */
        if (cam_ioctl(video_fd, CAM_REQ_MGR_ALLOC_BUF, &pix[fi],
                      CAM_HANDLE_USER_POINTER, sizeof(pix[fi])) < 0) {
            fprintf(stderr, "pixel ALLOC_BUF[%d]: %s\n", fi, strerror(errno));
            goto out;
        }
        pix_mfd[fi] = pix[fi].out.fd;
        pix_map[fi] = map_fd(pix_mfd[fi], pixbuf_len);
        if (pix_map[fi] == MAP_FAILED)
            goto out;
        memset(pix_map[fi], 0, pixbuf_len);
        printf("pixel buf[%d/%d] hdl 0x%x iova 0x%llx (%llu B)\n", fi + 1,
            nframes, pix[fi].out.buf_handle,
            (unsigned long long)pix[fi].out.vaddr,
            (unsigned long long)pixbuf_len);
    }

    /* 8. per-frame sync fences (frame-done signals) — window of them in
     * ring mode; a fresh one is created on each slot recycle */
    struct cam_sync_info sinfo;
    struct cam_private_ioctl_arg sarg;
    for (int fi = 0; fi < window; fi++) {
        memset(&sinfo, 0, sizeof(sinfo));
        snprintf(sinfo.name, sizeof(sinfo.name), "cam-shot-rdi0-%d", fi + 1);
        memset(&sarg, 0, sizeof(sarg));
        sarg.id = CAM_SYNC_CREATE;
        sarg.size = sizeof(sinfo);
        sarg.ioctl_ptr = (uint64_t)(uintptr_t)&sinfo;
        if (sync_fd < 0 || ioctl(sync_fd, VIDIOC_CAM_CONTROL, &sarg) < 0) {
            fprintf(stderr, "CAM_SYNC_CREATE[%d]: %s\n", fi, strerror(errno));
            goto out;
        }
        sync_obj[fi] = (uint32_t)sinfo.sync_obj;
        printf("sync obj[%d/%d] %d\n", fi + 1, nframes, sync_obj[fi]);
    }

    /* 9. IFE INIT packet (op 0): clock + csid clock + hfr + sensor dim */
    if (shot_alloc(video_fd, &ib, 8192, qisp.cdm_iommu.non_secure) < 0)  /* blobs + kmd */
        goto out;
    /* our builder puts blob cmd at offset 0 and kmd at offset cmd_cap */
    uint8_t *blob = ib.p_cmd;
    size_t bl = 0;
    {
        struct cam_isp_clock_config cc;
        memset(&cc, 0, sizeof(cc));
        cc.usage_type = 0;
        cc.num_rdi = 1;
        /* RDI path / CSID core clocks. 400 MHz proved the pipeline with the
         * (slow, internal) TPG, but real sensor traffic ECC+UNDERFLOW'd at
         * it (observed 2026-09-01: CSID ERROR_ECC + ERROR_STREAM_UNDERFLOW
         * + UNBOUNDED_FRAME, no frame, ~line-rate error IRQs) — the CSID RX
         * fifo starves when the core clock can't keep up with the incoming
         * byte stream. Real-PHY modes run 800M csid / 600M rdi. */
        cc.rdi_hz[0] = g_tpg ? 400000000 : 600000000;
        if (g_pix) {
            /* PIX path clock vote: cam_isp_blob_clock_update applies
             * left_pix_hz to the CAMIF src resource; 0 would leave the
             * processing chain under-clocked (first guess 600M — the
             * sdm845-class vendor PIX vote; kmsg cam_ife_clock lines will
             * tell if the apply path disagrees). */
            cc.left_pix_hz = 600000000;
            cc.num_rdi = 0;
        }
        bl += blob_add(blob + bl, CAM_ISP_GENERIC_BLOB_TYPE_CLOCK_CONFIG,
                       &cc, sizeof(cc));
        struct cam_isp_csid_clock_config sc;
        sc.csid_clock = g_tpg ? 400000000 : 800000000;
        bl += blob_add(blob + bl, CAM_ISP_GENERIC_BLOB_TYPE_CSID_CLOCK_CONFIG,
                       &sc, sizeof(sc));
        /* BW_CONFIG_V2 (type 9), --bw only. Hypothesis 2026-09-01: a missing
         * AXI vote starves the RDI write master (NOC stall → ECC +
         * STREAM_UNDERFLOW storm, zero buffers). Observed: the blob IS
         * applied ("ISP_BLOB usage_type=0 [IFE_RDI0] [TRANSAC_WRITE]") but
         * does NOT clear the storm, and NEW failure lines appear (VFE Bus
         * Violation 0x10010000, "Apply failed in Substate[SOF]" — apply
         * failures were absent in every pre-blob storm). So the vote either
         * breaks config_hw or changes nothing useful; default off. */
        if (g_bw) {
            struct cam_isp_bw_config_v2 bw;
            memset(&bw, 0, sizeof(bw));
            bw.usage_type = 0;
            bw.num_paths = 1;
            bw.axi_path[0].transac_type = 1;  /* CAM_AXI_TRANSACTION_WRITE */
            bw.axi_path[0].path_data_type = 4; /* CAM_AXI_PATH_DATA_IFE_RDI0 */
            bw.axi_path[0].camnoc_bw = 2400000000ULL;
            bw.axi_path[0].mnoc_ab_bw = 2400000000ULL;
            bw.axi_path[0].mnoc_ib_bw = 2400000000ULL;
            bw.axi_path[0].ddr_ab_bw = 2400000000ULL;
            bw.axi_path[0].ddr_ib_bw = 2400000000ULL;
            bl += blob_add(blob + bl, 9 /*CAM_ISP_GENERIC_BLOB_TYPE_BW_CONFIG_V2*/,
                           &bw, sizeof(bw));
        }
        struct cam_isp_resource_hfr_config hfr;
        memset(&hfr, 0, sizeof(hfr));
        hfr.num_ports = 1;
        hfr.port[0].resource_type = g_pix
            ? (g_pixraw ? CAM_ISP_IFE_OUT_RES_RAW_DUMP
                        : CAM_ISP_IFE_OUT_RES_FULL)
            : CAM_ISP_IFE_OUT_RES_RDI_0;
        hfr.port[0].subsample_pattern = 1;
        /* subsample_period MUST be 0 for per-request buffers. RDI write
         * masters (wm index < 3) take loop_size = irq_subsample_period + 1
         * image_addr writes per request (cam_vfe_bus_ver2_update_wm) and
         * the WM hardware auto-advances by frame_inc (whole frame) within
         * that cycle: period 1 => frame N+1 lands at buf+size, past the
         * mapping — the burst2/3 overrun (faults at round_up(pix[0] end),
         * RDI Error STATUS_1=0x4, frame 2 zero). Observed/fixed 2026-09-01. */
        hfr.port[0].subsample_period = 0;
        hfr.port[0].framedrop_pattern = 1;
        hfr.port[0].framedrop_period = 1;
        bl += blob_add(blob + bl, CAM_ISP_GENERIC_BLOB_TYPE_HFR_CONFIG,
                       &hfr, sizeof(hfr));
        struct cam_isp_sensor_config_blob dim;
        memset(&dim, 0, sizeof(dim));
        dim.rdi_path[0].width = width;
        dim.rdi_path[0].height = height;
        if (g_pix) {
            /* PIX context reads the ipp path dims for the CSID timing model
             * (same blob, different member than the RDI path). */
            dim.ipp_path.width = width;
            dim.ipp_path.height = height;
        }
        dim.hbi = hbi;
        dim.vbi = vbi;
        bl += blob_add(blob + bl,
                       CAM_ISP_GENERIC_BLOB_TYPE_SENSOR_DIMENSION_CONFIG,
                       &dim, sizeof(dim));
    }
    /* PIX mode: userspace CDM payload filling what the techpack kernel never
     * programs. (a) VFE module CGC: camif start only un-gates STATS, so
     * LENS/COLOR/ZOOM/bus stay gated (demosaic/gamma/CCM/write masters).
     * (b) CAMIF geometry + io format: the techpack camif start writes only
     * core_cfg/epoch/RUP — msm_vfe47_cfg_camif (camera_v2/isp, same register
     * layout) additionally programs 0x484 pixels_per_line/lines_per_frame,
     * 0x488/0x48C first/last pixel/line, 0x494/0x498/0x49C subsample
     * period/patterns and 0x88 io_format (camif raw output pack = PLAIN16).
     * Without 0x484 the CAMIF has no frame geometry at reset 0: no SOF, no
     * data forward, zero IPP/CAMIF IRQs — the silent PIX death (observed
     * 2026-09-01). CAMX sends this same set as CDM IQ commands; this is our
     * equivalent through the META_COMMON BL channel. */
    uint32_t cdm_cgc[40];
    uint32_t ncdm = 0;
    if (g_pix) {
        cdm_cgc[ncdm++] = (8u << 24) | (g_ife_base & 0xFFFFFFu);
        cdm_cgc[ncdm++] = (4u << 24) | 16u; /* RegRandom, 16 pairs */
        cdm_cgc[ncdm++] = 0x2C; cdm_cgc[ncdm++] = 0xFFFFFFFF; /* LENS cgc  */
        cdm_cgc[ncdm++] = 0x30; cdm_cgc[ncdm++] = 0xFFFFFFFF; /* STATS cgc */
        cdm_cgc[ncdm++] = 0x34; cdm_cgc[ncdm++] = 0xFFFFFFFF; /* COLOR cgc */
        cdm_cgc[ncdm++] = 0x38; cdm_cgc[ncdm++] = 0xFFFFFFFF; /* ZOOM cgc  */
        cdm_cgc[ncdm++] = 0x3C; cdm_cgc[ncdm++] = 0xFFFFFFFF; /* bus cgc   */
        /* CAMIF program (msm_vfe47_cfg_camif reference) */
        cdm_cgc[ncdm++] = 0x088; cdm_cgc[ncdm++] = 0xA00;    /* io_format: PLAIN16(5)<<9 */
        cdm_cgc[ncdm++] = 0x484;
        cdm_cgc[ncdm++] = ((height - 1) << 16) | (width - 1);
        cdm_cgc[ncdm++] = 0x488; cdm_cgc[ncdm++] = width - 1;
        cdm_cgc[ncdm++] = 0x48C; cdm_cgc[ncdm++] = height - 1;
        cdm_cgc[ncdm++] = 0x494; cdm_cgc[ncdm++] = 0x1F1F;
        cdm_cgc[ncdm++] = 0x498; cdm_cgc[ncdm++] = 0xFFFFFFFF;
        cdm_cgc[ncdm++] = 0x49C; cdm_cgc[ncdm++] = 0xFFFFFFFF;
        /* input select + enable: msm_vfe47_update_camif_state(ENABLE) writes
         * 0x4 then 0x1 to camif_cmd; 0x46C selects the MIPI/CSID pixel input
         * (enum msm_vfe_camif_input CAMIF_MIPI_INPUT=3). Nobody in the
         * techpack writes either register — without input enable the CAMIF
         * counts lines (we saw one teardown EOF) but never raises SOF nor
         * feeds the downstream modules. */
        cdm_cgc[ncdm++] = 0x46C; cdm_cgc[ncdm++] = 3;
        cdm_cgc[ncdm++] = 0x478; cdm_cgc[ncdm++] = 0x4;
        cdm_cgc[ncdm++] = 0x478; cdm_cgc[ncdm++] = 0x1;
        /* COLOR group master enable (vfe170 top module_ctrl.color.enable):
         * reset default 0 keeps the whole color pipe off, so the frame dies
         * after CAMIF and the FULL/DS WMs never write. All-ones turns on
         * every submodule with their reset-default (unity/bypass) configs. */
        cdm_cgc[ncdm++] = 0x048; cdm_cgc[ncdm++] = 0xFFFFFFFF;
    }
    t0 = kt;
    if (isp_config(video_fd, isp_fd, &ib, session, isp_hdl,
                   0 /* INIT: ((op+1)&0xf)==1 */, 0, bl,
                   g_pix ? cdm_cgc : NULL, g_pix ? ncdm : 0,
                   0, NULL) < 0) {
        fprintf(stderr, "isp INIT packet: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, t0);
        goto out;
    }
    printf("isp INIT packet ok (blobs %zu B)\n", bl);

    /* 10/11. request queueing. SCHED_REQ first: it inserts the id into the
     * req mgr's in_q; each device's per-request packet then calls add_req,
     * which fails with ENOENT ("req not found in in_q") if the id was never
     * scheduled (observed on device). Then the IFE UPDATE packet (own
     * buffer + fence — one shot_bufs per request; the hw mgr keeps built
     * CDM commands in the packet's kmd scratch) and the sensor NOP.
     * DEFAULT (pre-queue): all N requests go in before START. Rolling
     * submission (queue req i+1 only after fence i signals) loses the race:
     * fence signal is EOF-ish, userspace wakeup + 3 ioctls + CRM apply take
     * ~17 ms while EOF->next SOF is only ~10-18 ms (vbi is 31-54% of the
     * frame), so no request is pending at SOF+1, the RDI write master keeps
     * its end-of-frame address register and writes the next frame past the
     * buffer end (SMMU PF at round_up(pix_end) + CAMNOC decode error +
     * RDI Error STATUS_1=0x4, observed 2026-09-01 burst3). --roll restores
     * the rolling variant for A/B. Ring mode pre-queues the window (all
     * MAXF slots); recycling happens at each fence below. */
    for (int rq = 1; rq <= window; rq++) {
        if (shot_alloc(video_fd, &ub[rq - 1], 8192,
                       qisp.cdm_iommu.non_secure) < 0)
            goto out;
        if (g_roll && rq > 1)
            continue;   /* rolling: queued later, at fence i-1 */
        struct cam_req_mgr_sched_request sr;
        memset(&sr, 0, sizeof(sr));
        sr.session_hdl = (int32_t)session;
        sr.link_hdl = (int32_t)link_hdl;
        sr.sync_mode = 0;
        sr.req_id = rq;
        printf("[t=%.3f] queueing req %d (SCHED_REQ)\n", mono(), rq);
        t0 = kt;
        if (cam_ioctl(video_fd, CAM_REQ_MGR_SCHED_REQ, &sr,
                      CAM_HANDLE_USER_POINTER, sizeof(sr)) < 0) {
            fprintf(stderr, "SCHED_REQ %d: %s\n", rq, strerror(errno));
            kt = stream_kmsg(kmsg, t0);
            goto out;
        }
        struct cam_buf_io_cfg io;
        fill_out_io(&io, pix[rq - 1].out.buf_handle, sync_obj[rq - 1],
                    width, height, stride);
        t0 = kt;
        if (isp_config(video_fd, isp_fd, &ub[rq - 1], session, isp_hdl,
                       1 /* UPDATE */, rq, 0, NULL, 0, 1, &io) < 0) {
            fprintf(stderr, "isp UPDATE packet %d: %s\n", rq, strerror(errno));
            kt = stream_kmsg(kmsg, t0);
            goto out;
        }
        dump_kmd(&ub[rq - 1], rq);
        /* sensor must also register req N -> NOP packet */
        if (sensor_nop(video_fd, sensor_fd, &sb, session, sensor_hdl, rq) < 0) {
            fprintf(stderr, "sensor NOP req%d: %s\n", rq, strerror(errno));
            kt = stream_kmsg(kmsg, kt);
            goto out;
        }
        printf("req %d/%d queued (buf 0x%x, fence %d)\n", rq, nframes,
               pix[rq - 1].out.buf_handle, sync_obj[rq - 1]);
    }

    /* 11. start: csiphy -> ife (arms CSID/IFE, no data yet) -> sensor
     * (applies MODE_SELECT=1; frames begin flowing). TPG mode starts the
     * IFE only — the generator lives inside the CSID the IFE manager armed. */
    struct cam_start_stop_dev_cmd ss;
    memset(&ss, 0, sizeof(ss));
    ss.session_handle = (int32_t)session;
    ss.dev_handle = (int32_t)csiphy_hdl;
    if (!g_tpg &&
        cam_ioctl(csiphy_fd, CAM_START_DEV, &ss, CAM_HANDLE_USER_POINTER,
                  sizeof(ss)) < 0) {
        fprintf(stderr, "csiphy START: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, kt);
        goto out;
    }
    if (!g_tpg)
        printf("csiphy started\n");
    memset(&ss, 0, sizeof(ss));
    ss.session_handle = (int32_t)session;
    ss.dev_handle = (int32_t)isp_hdl;
    t0 = kt;
    if (cam_ioctl(isp_fd, CAM_START_DEV, &ss, CAM_HANDLE_USER_POINTER,
                  sizeof(ss)) < 0) {
        fprintf(stderr, "isp START: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, t0);
        goto out;
    }
    printf("isp started\n");
    memset(&ss, 0, sizeof(ss));
    ss.session_handle = (int32_t)session;
    ss.dev_handle = (int32_t)sensor_hdl;
    t0 = kt;
    if (!g_tpg &&
        cam_ioctl(sensor_fd, CAM_START_DEV, &ss, CAM_HANDLE_USER_POINTER,
                  sizeof(ss)) < 0) {
        fprintf(stderr, "sensor START: %s\n", strerror(errno));
        kt = stream_kmsg(kmsg, t0);
        goto out;
    }
    printf(g_tpg ? "tpg: ife armed, generator runs from CSID\n"
                 : "sensor started (MODE_SELECT=1 written)\n");
    printf("[t=%.3f] streaming\n", mono());
    kt = stream_kmsg(kmsg, kt);

    if (rb && !g_tpg)
        sensor_readback(sensor_fd, &sb, session, sensor_hdl, "post-START");
    if (g_verify && !g_tpg)
        verify_cfg_table(sensor_fd, &sb, session, sensor_hdl, cfg_regs, n_cfg,
                         "post-START");

    /* 14. wait on each fence in order (frame i signals as it lands).
     * RING mode (nframes > MAXF): only `window` buffers exist; frame f
     * lives in slot f%window. The moment its fence signals, the slot is
     * recycled for request f+window (retire the fired fence, create a
     * fresh one, SCHED_REQ + UPDATE + NOP), then the heavy pass runs on
     * the landed frame IN PLACE — the slot is not rewritten until that
     * request executes at SOF, window frame periods away (0.17 s encode
     * vs >=1 s margin). Raw dumps are skipped: nframes x 2.86 MB would
     * fill tmpfs; JPEG/PNG stay per-frame-numbered. */
    int frames_ok = 0, frames_empty = 0;
    for (int fi = 0; fi < nframes; fi++) {
        int slot = fi % window;
        printf("[t=%.3f] waiting for frame %d/%d (fence %d, slot %d, %d ms)...\n",
               mono(), fi + 1, nframes, sync_obj[slot], slot, wait_ms);
        struct cam_sync_wait sw;
        memset(&sw, 0, sizeof(sw));
        sw.sync_obj = (int32_t)sync_obj[slot];
        sw.timeout_ms = (uint64_t)wait_ms;
        memset(&sarg, 0, sizeof(sarg));
        sarg.id = CAM_SYNC_WAIT;
        sarg.size = sizeof(sw);
        sarg.ioctl_ptr = (uint64_t)(uintptr_t)&sw;
        int wrc = ioctl(sync_fd, VIDIOC_CAM_CONTROL, &sarg);
        /* cam_sync_wait reports its own status through sarg.result even
         * when the ioctl returns 0 (observed: rc=0, result=-110 ETIMEDOUT
         * together with CAM_ERR "timed out for sync obj" in dmesg). */
        int32_t wres = (int32_t)sarg.result;
        printf("[t=%.3f] SYNC_WAIT[%d] rc=%d result=%d (%s)\n", mono(), fi + 1, wrc, wres,
            wrc ? strerror(errno)
                : (wres ? "timed out" : "signaled"));
        kt = stream_kmsg(kmsg, kt);
        if (wrc < 0 || wres < 0) {
            /* RX has gone silent by now (observed: CSID fatal-halts its
             * csi2 rx ~48 ms after stream start). Read the sensor again
             * AFTER the silence: MODE_SELECT still 1 = sensor alive and
             * (as far as it knows) transmitting -> the receiver side
             * died, not the sensor. */
            if (rb && !g_tpg)
                sensor_readback(sensor_fd, &sb, session, sensor_hdl,
                                "post-WAIT (after silence)");
            /* The RDI path may still have written partial bytes before
             * the fatal halt — the byte pattern of that partial data is
             * direct evidence of the scrambling mode (lane swap ->
             * structured repeats, analog noise -> spread corruption,
             * zero -> path never wrote). */
            inspect_buf(pix_map[slot], (size_t)pixbuf_len, out_path, "partial");
            rc = 2;
            goto out;
        }
        frames_ok++;

        /* rolling mode only: queue the next request NOW, while the sensor
         * is between frames (pre-queue mode already has every request in) */
        if (g_roll && fi + 1 < nframes) {
            int rq = fi + 2, rslot = (rq - 1) % window;
            printf("[t=%.3f] queueing req %d (SCHED_REQ)\n", mono(), rq);
            struct cam_req_mgr_sched_request sr;
            memset(&sr, 0, sizeof(sr));
            sr.session_hdl = (int32_t)session;
            sr.link_hdl = (int32_t)link_hdl;
            sr.sync_mode = 0;
            sr.req_id = rq;
            t0 = kt;
            if (cam_ioctl(video_fd, CAM_REQ_MGR_SCHED_REQ, &sr,
                          CAM_HANDLE_USER_POINTER, sizeof(sr)) < 0) {
                fprintf(stderr, "SCHED_REQ %d: %s\n", rq, strerror(errno));
                kt = stream_kmsg(kmsg, t0);
                rc = 2;
                goto out;
            }
            struct cam_buf_io_cfg io;
            fill_out_io(&io, pix[rslot].out.buf_handle, sync_obj[rslot],
                        width, height, stride);
            printf("[t=%.3f] req %d: UPDATE packet\n", mono(), rq);
            t0 = kt;
            if (isp_config(video_fd, isp_fd, &ub[rslot], session, isp_hdl,
                           1 /* UPDATE */, rq, 0, NULL, 0, 1, &io) < 0) {
                fprintf(stderr, "isp UPDATE packet %d: %s\n", rq,
                        strerror(errno));
                kt = stream_kmsg(kmsg, t0);
                rc = 2;
                goto out;
            }
            dump_kmd(&ub[rslot], rq);
            if (sensor_nop(video_fd, sensor_fd, &sb, session, sensor_hdl,
                           rq) < 0) {
                fprintf(stderr, "sensor NOP req%d: %s\n", rq, strerror(errno));
                kt = stream_kmsg(kmsg, kt);
                rc = 2;
                goto out;
            }
            printf("[t=%.3f] req %d/%d queued (buf 0x%x, fence %d)\n",
                   mono(), rq, nframes,
                   pix[rslot].out.buf_handle, sync_obj[rslot]);
        }

        /* ring: recycle this slot for request fi+window+1 (its fence just
         * fired — retire it, arm a fresh one), then process the landed
         * frame in place. Requeue BEFORE the encode so the pipeline
         * refills first; the buffer stays untouched until the request
         * executes window frames later. */
        if (ring && fi + window < nframes) {
            int rq = fi + window + 1;
            memset(&sinfo, 0, sizeof(sinfo));
            sinfo.sync_obj = (int32_t)sync_obj[slot];
            memset(&sarg, 0, sizeof(sarg));
            sarg.id = CAM_SYNC_DESTROY;
            sarg.size = sizeof(sinfo);
            sarg.ioctl_ptr = (uint64_t)(uintptr_t)&sinfo;
            if (ioctl(sync_fd, VIDIOC_CAM_CONTROL, &sarg) < 0)
                fprintf(stderr, "CAM_SYNC_DESTROY[%d]: %s\n", slot,
                        strerror(errno));
            memset(&sinfo, 0, sizeof(sinfo));
            snprintf(sinfo.name, sizeof(sinfo.name), "cam-shot-rdi0-%d", rq);
            memset(&sarg, 0, sizeof(sarg));
            sarg.id = CAM_SYNC_CREATE;
            sarg.size = sizeof(sinfo);
            sarg.ioctl_ptr = (uint64_t)(uintptr_t)&sinfo;
            if (sync_fd < 0 ||
                ioctl(sync_fd, VIDIOC_CAM_CONTROL, &sarg) < 0) {
                fprintf(stderr, "CAM_SYNC_CREATE[%d]: %s\n", slot,
                        strerror(errno));
                rc = 2;
                goto out;
            }
            sync_obj[slot] = (uint32_t)sinfo.sync_obj;
            struct cam_req_mgr_sched_request sr;
            memset(&sr, 0, sizeof(sr));
            sr.session_hdl = (int32_t)session;
            sr.link_hdl = (int32_t)link_hdl;
            sr.sync_mode = 0;
            sr.req_id = rq;
            t0 = kt;
            if (cam_ioctl(video_fd, CAM_REQ_MGR_SCHED_REQ, &sr,
                          CAM_HANDLE_USER_POINTER, sizeof(sr)) < 0) {
                fprintf(stderr, "SCHED_REQ %d: %s\n", rq, strerror(errno));
                kt = stream_kmsg(kmsg, t0);
                rc = 2;
                goto out;
            }
            struct cam_buf_io_cfg io;
            fill_out_io(&io, pix[slot].out.buf_handle, sync_obj[slot],
                        width, height, stride);
            t0 = kt;
            if (isp_config(video_fd, isp_fd, &ub[slot], session, isp_hdl,
                           1 /* UPDATE */, rq, 0, NULL, 0, 1, &io) < 0) {
                fprintf(stderr, "isp UPDATE packet %d: %s\n", rq,
                        strerror(errno));
                kt = stream_kmsg(kmsg, t0);
                rc = 2;
                goto out;
            }
            if (sensor_nop(video_fd, sensor_fd, &sb, session, sensor_hdl,
                           rq) < 0) {
                fprintf(stderr, "sensor NOP req%d: %s\n", rq, strerror(errno));
                kt = stream_kmsg(kmsg, kt);
                rc = 2;
                goto out;
            }
        }
        if (ring) {
            /* spill the frame to disk and move on — the encode pass runs
             * after the burst (0.17 s/frame would stall this loop past the
             * 67 ms frame period and drain the in-flight window) */
            char fpath[512];
            frame_path(fpath, sizeof fpath, out_path, fi + 1, nframes);
            int fd = open(fpath, O_WRONLY | O_CREAT | O_TRUNC, 0644);
            if (fd < 0) {
                fprintf(stderr, "spill %s: %s\n", fpath, strerror(errno));
                frames_empty++;
            } else {
                size_t off = 0;
                while (off < (size_t)pixbuf_len) {
                    ssize_t w = write(fd, (uint8_t *)pix_map[slot] + off,
                                      (size_t)pixbuf_len - off);
                    if (w <= 0)
                        break;
                    off += (size_t)w;
                }
                close(fd);
                if (off < (size_t)pixbuf_len) {
                    fprintf(stderr, "spill %s: short write\n", fpath);
                    frames_empty++;
                }
            }
        }
    }

    /* 15. heavy pass (non-ring: every frame sits in its own buffer; ring
     * spilled each frame to disk). Kept out of the wait loop so encode
     * time never throttles the burst. */
    if (!ring) {
        for (int fi = 0; fi < window; fi++) {
            size_t nz = process_frame(pix_map[fi], (size_t)pixbuf_len,
                                      out_path, fi + 1, nframes, width,
                                      height, stride, 1);
            if (!nz)
                frames_empty++;
        }
    } else {
        /* ring post-encode from the spilled raws: inspect + JPEG each,
         * then unlink the raw to hand tmpfs back */
        uint8_t *rb = malloc((size_t)pixbuf_len);
        if (!rb) {
            fprintf(stderr, "post-encode alloc: %s\n", strerror(errno));
            rc = 2;
            goto out;
        }
        printf("== post-encode %d spilled frames ==\n", nframes);
        for (int f = 1; f <= nframes; f++) {
            char fpath[512];
            frame_path(fpath, sizeof fpath, out_path, f, nframes);
            int fd = open(fpath, O_RDONLY);
            if (fd < 0) {
                fprintf(stderr, "post %s: %s\n", fpath, strerror(errno));
                frames_empty++;
                continue;
            }
            size_t off = 0;
            while (off < (size_t)pixbuf_len) {
                ssize_t r = read(fd, rb + off, (size_t)pixbuf_len - off);
                if (r <= 0)
                    break;
                off += (size_t)r;
            }
            close(fd);
            size_t nz = off < (size_t)pixbuf_len
                          ? 0
                          : inspect_buf(rb, (size_t)pixbuf_len, NULL, "frame");
            if (nz && g_png) {
                char ppath[512];
                snprintf(ppath, sizeof ppath, "/tmp/frame-%d.png", f);
                dump_png(rb, width, height, stride, ppath);
            }
            if (nz && g_jpeg_q > 0)
                dump_jpeg(rb, width, height, stride, fpath);
            if (!nz)
                frames_empty++;
            unlink(fpath);
        }
        free(rb);
    }
    rc = frames_empty == 0 ? 0 : (frames_empty == nframes ? 3 : 4);
    goto out;

out:
    /* teardown: streamoff -> stop (isp, csiphy, sensor) -> unlink -> release
     * -> destroy session. Best effort; the kernel also tears down on close. */
    printf("== teardown ==\n");
    if (sensor_fd >= 0 && sensor_hdl && sb.p_pkt)
        sensor_config(video_fd, sensor_fd, &sb, session, sensor_hdl,
                      5 /* STREAMOFF */, imx355_streamoff, 1);
    struct cam_start_stop_dev_cmd st;
    memset(&st, 0, sizeof(st));
    st.session_handle = (int32_t)session;
    if (isp_hdl) {
        st.dev_handle = (int32_t)isp_hdl;
        cam_ioctl(isp_fd, CAM_STOP_DEV, &st, CAM_HANDLE_USER_POINTER,
                  sizeof(st));
    }
    if (csiphy_hdl) {
        st.dev_handle = (int32_t)csiphy_hdl;
        cam_ioctl(csiphy_fd, CAM_STOP_DEV, &st, CAM_HANDLE_USER_POINTER,
                  sizeof(st));
    }
    if (sensor_hdl) {
        st.dev_handle = (int32_t)sensor_hdl;
        cam_ioctl(sensor_fd, CAM_STOP_DEV, &st, CAM_HANDLE_USER_POINTER,
                  sizeof(st));
    }
    if (link_hdl) {
        struct cam_req_mgr_unlink_info ul;
        memset(&ul, 0, sizeof(ul));
        ul.session_hdl = (int32_t)session;
        ul.link_hdl = (int32_t)link_hdl;
        cam_ioctl(video_fd, CAM_REQ_MGR_UNLINK, &ul, CAM_HANDLE_USER_POINTER,
                  sizeof(ul));
    }
    if (isp_hdl) {
        struct cam_release_dev_cmd rel;
        memset(&rel, 0, sizeof(rel));
        rel.session_handle = (int32_t)session;
        rel.dev_handle = (int32_t)isp_hdl;
        cam_ioctl(isp_fd, CAM_RELEASE_DEV, &rel, CAM_HANDLE_USER_POINTER,
                  sizeof(rel));
    }
    if (csiphy_hdl) {
        struct cam_release_dev_cmd rel;
        memset(&rel, 0, sizeof(rel));
        rel.session_handle = (int32_t)session;
        rel.dev_handle = (int32_t)csiphy_hdl;
        cam_ioctl(csiphy_fd, CAM_RELEASE_DEV, &rel, CAM_HANDLE_USER_POINTER,
                  sizeof(rel));
    }
    if (sensor_hdl) {
        struct cam_release_dev_cmd rel;
        memset(&rel, 0, sizeof(rel));
        rel.session_handle = (int32_t)session;
        rel.dev_handle = (int32_t)sensor_hdl;
        cam_ioctl(sensor_fd, CAM_RELEASE_DEV, &rel, CAM_HANDLE_USER_POINTER,
                  sizeof(rel));
    }
    if (rail_hdl) {
        struct cam_release_dev_cmd rel;
        memset(&rel, 0, sizeof(rel));
        rel.session_handle = (int32_t)session;
        rel.dev_handle = (int32_t)rail_hdl;
        cam_ioctl(rail_fd, CAM_RELEASE_DEV, &rel, CAM_HANDLE_USER_POINTER,
                  sizeof(rel));
    }
    if (session) {
        struct cam_req_mgr_session_info ds;
        memset(&ds, 0, sizeof(ds));
        ds.session_hdl = (int32_t)session;
        cam_ioctl(video_fd, CAM_REQ_MGR_DESTROY_SESSION, &ds,
                  CAM_HANDLE_USER_POINTER, sizeof(ds));
    }
    for (int fi = 0; fi < MAXF; fi++) {
        if (sync_obj[fi]) {
            memset(&sinfo, 0, sizeof(sinfo));
            sinfo.sync_obj = (int32_t)sync_obj[fi];
            memset(&sarg, 0, sizeof(sarg));
            sarg.id = CAM_SYNC_DESTROY;
            sarg.size = sizeof(sinfo);
            sarg.ioctl_ptr = (uint64_t)(uintptr_t)&sinfo;
            ioctl(sync_fd, VIDIOC_CAM_CONTROL, &sarg);
        }
    }
    shot_free(video_fd, &sb);
    shot_free(video_fd, &rail_sb);
    shot_free(video_fd, &ib);
    for (int fi = 0; fi < MAXF; fi++)
        shot_free(video_fd, &ub[fi]);
    for (int fi = 0; fi < MAXF; fi++) {
        if (pix[fi].out.buf_handle)
            release_buf(video_fd, pix[fi].out.buf_handle);
        if (pix_map[fi] != MAP_FAILED) munmap(pix_map[fi], pixbuf_len);
        if (pix_mfd[fi] > 0) close(pix_mfd[fi]);
    }
    if (out_fd >= 0) close(out_fd);
    if (sensor_fd >= 0) close(sensor_fd);
    if (rail_fd >= 0) close(rail_fd);
    if (csiphy_fd >= 0) close(csiphy_fd);
    if (isp_fd >= 0) close(isp_fd);
    if (kmsg >= 0) {
        stream_kmsg(kmsg, kt);
        close(kmsg);
    }
    if (video_fd >= 0) close(video_fd);
    if (sync_fd >= 0) close(sync_fd);
    return rc;
}

int main(int argc, char **argv)
{
    int only_slot = -1;
    int real = 0, sweep = 0, stream = 0;
    const char *out_path = "/tmp/frame.raw";
    int wait_ms = 3000;
    uint32_t settle_cnt = 0;   /* 0 = derive from data rate (23 @888 Mbps) */
    uint32_t tp = 0;           /* sensor test pattern (0x0600), 0=off */
    int rb = 0;                /* post-START sensor register readback */
    uint32_t reg = 0x0016;   /* Sony IMX3xx-family chip id register */
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--reg") == 0 && i + 1 < argc)
            reg = (uint32_t)strtoul(argv[++i], 0, 0);
        else if (strcmp(argv[i], "--stream") == 0)
            stream = 1;
        else if (strcmp(argv[i], "--out") == 0 && i + 1 < argc)
            out_path = argv[++i];
        else if (strcmp(argv[i], "--wait") == 0 && i + 1 < argc)
            wait_ms = atoi(argv[++i]);
        else if (strcmp(argv[i], "--settle") == 0 && i + 1 < argc)
            settle_cnt = (uint32_t)strtoul(argv[++i], 0, 0);
        else if (strcmp(argv[i], "--tp") == 0 && i + 1 < argc)
            tp = (uint32_t)strtoul(argv[++i], 0, 0);
        else if (strcmp(argv[i], "--mclk") == 0 && i + 1 < argc) {
            g_extclk_mhz = atof(argv[++i]);
            g_mclk24 = g_extclk_mhz == 24.0;
            if (g_extclk_mhz < 6.0 || g_extclk_mhz > 40.0) {
                fprintf(stderr, "--mclk: 6..40 MHz supported\n");
                return 1;
            }
        }
        else if (strcmp(argv[i], "--lanecfg") == 0 && i + 1 < argc)
            g_lanecfg = (uint32_t)strtoul(argv[++i], 0, 0);
        else if (strcmp(argv[i], "--force-ife") == 0 && i + 1 < argc) {
            g_force_ife = atoi(argv[++i]);
            if (g_force_ife < 0 || g_force_ife > 2) {
                fprintf(stderr, "--force-ife: 0..2\n");
                return 2;
            }
        }
        else if (strcmp(argv[i], "--ife-base") == 0 && i + 1 < argc)
            g_ife_base = (uint32_t)strtoul(argv[++i], 0, 0);
        else if (strcmp(argv[i], "--lanes") == 0 && i + 1 < argc) {
            g_lanes = atoi(argv[++i]);
            if (g_lanes != 2 && g_lanes != 4) {
                fprintf(stderr, "--lanes: only 2 or 4 supported\n");
                return 1;
            }
        }
        else if (strcmp(argv[i], "--rb") == 0)
            rb = 1;
        else if (strcmp(argv[i], "--pix") == 0)
            g_pix = 1;
        else if (strcmp(argv[i], "--pix-raw") == 0) {
            g_pix = 1;
            g_pixraw = 1;
        }
        else if (strcmp(argv[i], "--verify") == 0)
            g_verify = 1;
        else if (strcmp(argv[i], "--bw") == 0)
            g_bw = 1;
        else if (strcmp(argv[i], "--vc") == 0 && i + 1 < argc)
            g_vc = (uint32_t)strtoul(argv[++i], 0, 0);
        else if (strcmp(argv[i], "--dt") == 0 && i + 1 < argc)
            g_dt = (uint32_t)strtoul(argv[++i], 0, 0);
        else if (strcmp(argv[i], "--noglobal") == 0)
            g_noglobal = 1;
        else if (strcmp(argv[i], "--nostarton") == 0)
            g_nostarton = 1;
        else if (strcmp(argv[i], "--rawvendor") == 0)
            g_rawvendor = 1;
        else if (strcmp(argv[i], "--slowrear") == 0)
            g_slowrear = 1;
        else if (strcmp(argv[i], "--rear564") == 0) {
            g_slowrear = 1;
            g_rear564 = 1;
        }
        else if (strcmp(argv[i], "--keep0112") == 0)
            g_keep0112 = 1;
        else if (strcmp(argv[i], "--png") == 0)
            g_png = 1;
        else if (strcmp(argv[i], "--jpeg") == 0) {
            g_jpeg_q = 85;
            if (i + 1 < argc && argv[i + 1][0] != '-')
                g_jpeg_q = atoi(argv[++i]);
            if (g_jpeg_q < 1 || g_jpeg_q > 100) {
                fprintf(stderr, "--jpeg: q 1..100\n");
                return 1;
            }
        }
        else if (strcmp(argv[i], "--jpeg-color") == 0) {
            g_jpeg_color = 1;
            if (!g_jpeg_q) g_jpeg_q = 85;
        }
        else if (strcmp(argv[i], "--jpeg-gray") == 0) {
            g_jpeg_color = 0;
            if (!g_jpeg_q) g_jpeg_q = 85;
        }
        else if (strcmp(argv[i], "--jpeg-out") == 0 && i + 1 < argc)
            g_jpeg_out = argv[++i];
        else if (strcmp(argv[i], "--wb") == 0 && i + 1 < argc) {
            const char *s = argv[++i];
            if (!strcmp(s, "auto")) {
                g_wb_auto = 1;
            } else if (!strcmp(s, "off")) {
                g_wb_auto = 0;
                g_wb_r = g_wb_g = g_wb_b = 1.0f;
            } else {
                float r, gg, b;
                if (sscanf(s, "%f,%f,%f", &r, &gg, &b) != 3 ||
                    r <= 0 || gg <= 0 || b <= 0) {
                    fprintf(stderr, "--wb: auto | off | r,g,b\n");
                    return 2;
                }
                g_wb_auto = 0;
                g_wb_r = r; g_wb_g = gg; g_wb_b = b;
            }
        }
        else if (strcmp(argv[i], "--frames") == 0 && i + 1 < argc) {
            g_frames = atoi(argv[++i]);
            if (g_frames < 1 || g_frames > 999) {
                fprintf(stderr, "--frames: 1..999 (<=16 pre-queued, "
                                ">16 ring mode — see the wait loop note)\n");
                return 1;
            }
        }
        else if (strcmp(argv[i], "--roll") == 0)
            g_roll = 1;
        else if (strcmp(argv[i], "--cit") == 0 && i + 1 < argc)
            g_cit = (unsigned)strtoul(argv[++i], 0, 0);
        else if (strcmp(argv[i], "--gain") == 0 && i + 1 < argc)
            g_gain = atof(argv[++i]);
        else if (strcmp(argv[i], "--dgain") == 0 && i + 1 < argc)
            g_dgain = atof(argv[++i]);
        else if (strcmp(argv[i], "--halfrate") == 0)
            g_halfrate = 1;
        else if (strcmp(argv[i], "--real") == 0)
            real = 1;
        else if (strcmp(argv[i], "--rear") == 0)
            only_slot = 0;   /* rear imx363: vendor-bin tables, 4-lane */
        else if (strcmp(argv[i], "--uw") == 0)
            only_slot = 1;   /* rear ultra-wide imx481: vendor-bin mode3 */
        else if (strcmp(argv[i], "--tpg") == 0)
            g_tpg = 1;
        else if (strcmp(argv[i], "--railhelper") == 0)
            g_railhelper = 1;
        else if (strcmp(argv[i], "--sweep") == 0)
            sweep = 1;
        else
            only_slot = atoi(argv[i]);
    }
    if (stream)   /* default front camera (slot 2, imx355) */
        return run_stream(only_slot >= 0 ? only_slot : 2, out_path, wait_ms,
                          settle_cnt, tp, rb, g_frames);

    int video_fd = open("/dev/video3", O_RDWR);
    if (video_fd < 0) {
        fprintf(stderr, "open /dev/video3: %s\n", strerror(errno));
        return 1;
    }

    int kmsg = open("/dev/kmsg", O_RDONLY | O_NONBLOCK);
    double kmark = 0;
    if (kmsg >= 0)
        kmark = kmsg_drain(kmsg, 0);   /* flush ring, remember newest */

    int sd_fd[MAX_SUBDEV], sd_slot[MAX_SUBDEV];
    int n = find_sensor_nodes(sd_fd, sd_slot);
    if (n == 0) {
        fprintf(stderr, "no sensor subdevs found\n");
        return 1;
    }

    /* chip ids for the real probe (moved to file scope: probe + stream) */
    for (int i = 0; i < n; i++) {
        if (only_slot >= 0 && sd_slot[i] != only_slot) {
            close(sd_fd[i]);
            continue;
        }
        int slot = sd_slot[i];
        uint32_t expected = real ? slot_id[slot] : 0xFFFF;
        if (sweep) {
            /* walk every even 8-bit address on this slot's CCI bus to
             * answer "is anything alive on this bus at all?" — the rear
             * module bus (cci0/master0) carries sensor + eeprom +
             * actuator + ois, so any ACK proves bus+pins alive */
            printf("sweep slot %d (%s), all even addrs, reg 0x%04x\n",
                slot, slots[slot].name, reg);
            fflush(stdout);
            for (uint32_t a = 0x02; a <= 0xFE; a += 2) {
                double t0 = kmark;
                probe_once(video_fd, sd_fd[i], slot, a, reg, 0xFFFF);
                uint32_t id = 0;
                int st = kmsg_classify(kmsg, t0, &id, &kmark);
                if (st == KMSG_HIT)
                    printf("  0x%02x: ACK id=0x%04x\n", a, id);
                else if (st == KMSG_WEDGE) {
                    printf("  0x%02x: BUS WEDGE (timeout) — abort sweep\n", a);
                    fflush(stdout);
                    break;
                }
                if ((a & 0x1e) == 0) {
                    printf("  .. 0x%02x done\n", a);
                    fflush(stdout);
                }
            }
            close(sd_fd[i]);
            continue;
        }
        printf("probing slot %d (%s), id reg 0x%04x, expected 0x%04x\n",
            slot, slots[slot].name, reg, expected);
        if (real) {
            /* known sensor: single address, real expectation; the IMX363
             * (slg51000 rails, slow ramp) is cold-start flaky — retry */
            for (int attempt = 1; attempt <= 3; attempt++) {
                printf("  addr 0x%02x (try %d): ", slots[slot].addr,
                       attempt);
                fflush(stdout);
                double t0 = kmark;
                int rc = probe_once(video_fd, sd_fd[i], slot,
                                    slots[slot].addr, reg, expected);
                printf("rc=%d (%s)\n", rc,
                    rc == 0 ? "OK" : strerror(errno));
                kmark = kmsg_drain(kmsg, t0);
                if (rc == 0)
                    break;
                sleep(2);  /* let the module rails fully discharge */
            }
            close(sd_fd[i]);
            continue;
        }
        for (size_t a = 0; a < sizeof(try_addrs) / sizeof(try_addrs[0]);
             a++) {
            printf("  addr 0x%02x: ", try_addrs[a]);
            fflush(stdout);
            double t0 = kmark;
            int rc = probe_once(video_fd, sd_fd[i], slot,
                                try_addrs[a], reg, expected);
            if (rc == 0) {
                printf("probe rc=0 (unexpected match?)\n");
                kmark = kmsg_drain(kmsg, t0);
                break;
            }
            printf("rc=%d (%s)\n", rc, strerror(errno));
            kmark = kmsg_drain(kmsg, t0);
            /* the kmsg lines above carry the verdict: "read id: 0xNNN"
               = chip present at this addr; silence/cci errors = absent */
        }
        close(sd_fd[i]);
    }
    close(video_fd);
    if (kmsg >= 0)
        close(kmsg);
    return 0;
}
