/*
 * qmi-ask — QMI query probe over QRTR (enchilada MPSS line).
 *
 * Wire format pinned from upstream rmtfs/qmi_tlv.c:
 *   frame = { u8 flags; u16 txn; u16 msg_id; u16 msg_len; u8 tlvs[] } packed
 *           -> 7-byte header, data at offset 7.  Host LE == wire LE on
 *           aarch64, so plain struct/memcpy assignment is correct.
 *   tlv   = { u8 key; u16 len; u8 data[] } packed (3-byte item header).
 *   flags = 0x00 request / 0x02 response / 0x04 indication.
 * Transport = libqrtr (the same static lib rmtfs/pd-mapper use here).
 * Lookup end-sentinel = NEW_SERVER with all-zero fields (kernel ns.c,
 * same contract qrtr-lookup's loop breaks on).
 *
 * Usage: qmi-ask imei|sim|sig|serving|all
 * Exit 0 only if a QMI response frame arrived.  Every response TLV is
 * dumped raw (hex + printable) — the hex IS the receipt; the labeled
 * hints below are interpretation only.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <stdint.h>
#include <libqrtr.h>

struct query {
	const char *cmd;
	uint32_t svc;
	uint16_t msg;
	const char *desc;
	/* Optional single TLV appended to the request (key 0 = none). */
	uint8_t tlv_key;
	uint8_t tlv_data[8];
	uint8_t tlv_len;
	/* Second optional TLV (e.g. CHANGE_PROVISIONING_SESSION's
	 * Application Info, which this firmware requires: without it the
	 * modem answers 17 MISSING_ARGUMENT). */
	uint8_t tlv2_key;
	uint8_t tlv2_data[20];
	uint8_t tlv2_len;
	/* Third/fourth optional TLVs (SET_SSP wants change duration +
	 * network selection alongside the service domain). */
	uint8_t tlv3_key;
	uint8_t tlv3_data[8];
	uint8_t tlv3_len;
	uint8_t tlv4_key;
	uint8_t tlv4_data[8];
	uint8_t tlv4_len;
	/* Fifth optional TLV (START_NETWORK carries five: apn, ip family,
	 * 3gpp profile, 3gpp2 profile, call type). */
	uint8_t tlv5_key;
	uint8_t tlv5_data[8];
	uint8_t tlv5_len;
	/* Sixth optional TLV (WDA SET_DATA_FORMAT carries six: link layer
	 * protocol, UL/DL aggregation, max datagrams, max size, endpoint). */
	uint8_t tlv6_key;
	uint8_t tlv6_data[8];
	uint8_t tlv6_len;
};

/* Not const: wdsstop patches its packet handle in at runtime. */
static struct query QUERIES[] = {
	{ "imei",    2,  0x002C, "DMS GET_IDS" },
	{ "mode",    2,  0x002D, "DMS GET_OPERATING_MODE" },
	{ "online",  2,  0x002E, "DMS SET_OPERATING_MODE online", 0x01, {0}, 1 },
	{ "offline", 2,  0x002E, "DMS SET_OPERATING_MODE offline", 0x01, {3}, 1 },
	{ "lpm",     2,  0x002E, "DMS SET_OPERATING_MODE lpm", 0x01, {1}, 1 },
	{ "uireset", 11, 0x0000, "UIM RESET" },
	{ "sim",     11, 0x002F, "UIM GET_CARD_STATUS" },
	{ "slots",   11, 0x0047, "UIM GET_SLOT_STATUS" },
	{ "simon",   11, 0x0031, "UIM POWER_ON_SIM slot 1", 0x01, {1}, 1 },
	{ "simoff",  11, 0x0030, "UIM POWER_OFF_SIM slot 1", 0x01, {1}, 1 },
	{ "simon2",  11, 0x0031, "UIM POWER_ON_SIM slot 2", 0x01, {2}, 1 },
	{ "simoff2", 11, 0x0030, "UIM POWER_OFF_SIM slot 2", 0x01, {2}, 1 },
	{ "provision", 11, 0x0038, "UIM CHANGE_PROVISIONING_SESSION activate gw slot1 usim", 0x01, {0, 1}, 2,
	  0x10, {1, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x02, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18 },
	{ "provision2", 11, 0x0038, "UIM CHANGE_PROVISIONING_SESSION activate gw slot2 usim", 0x01, {0, 1}, 2,
	  0x10, {2, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x02, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18 },
	{ "prov0",   11, 0x0038, "UIM CHANGE_PROVISIONING_SESSION activate gw slot1 empty-aid", 0x01, {0, 1}, 2,
	  0x10, {1, 0}, 2 },
	{ "provp",   11, 0x0038, "UIM CHANGE_PROVISIONING_SESSION activate gw slot1 short-aid", 0x01, {0, 1}, 2,
	  0x10, {1, 7, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x02}, 9 },
	{ "switchslot", 11, 0x0046, "UIM SWITCH_SLOT logical1->physical2", 0x01, {1}, 1,
	  0x02, {2, 0, 0, 0}, 4 },
	{ "switchback", 11, 0x0046, "UIM SWITCH_SLOT logical1->physical1 (restore)", 0x01, {1}, 1,
	  0x02, {1, 0, 0, 0}, 4 },
	{ "unprovision", 11, 0x0038, "UIM CHANGE_PROVISIONING_SESSION deactivate gw", 0x01, {0, 0}, 2 },
	{ "events",  11, 0x002E, "UIM REGISTER_EVENTS mask 3", 0x01, {3, 0, 0, 0}, 4 },
	{ "sig",     3,  0x0020, "NAS GET_SIGNAL_STRENGTH" },
	{ "serving", 3,  0x0024, "NAS GET_SERVING_SYSTEM" },
	{ "sysinfo", 3,  0x004D, "NAS GET_SYSTEM_INFO" },
	{ "ssp",     3,  0x0034, "NAS GET_SYSTEM_SELECTION_PREFERENCE" },
	/* SET_SSP: mode pref 0x003f preserved from the 2026-09-14 read;
	 * TLV 0x18 service domain: 3 = CS+PS (enables CSFB/SGs attach,
	 * prerequisite for SMS on this CT LTE-only SIM), 2 = PS only
	 * (the shipped default — restore point).  0x16 auto network
	 * selection + 0x17 change duration=permanent ride along (bare
	 * 0x11+0x18 got error 3 INTERNAL). */
	{ "sspcs",   3,  0x0033, "NAS SET_SYSTEM_SELECTION_PREFERENCE cs+ps", 0x11, {0x3f, 0}, 2,
	  0x16, {0, 0, 0, 0, 0}, 5,
	  0x17, {1}, 1,
	  0x18, {3, 0, 0, 0}, 4 },
	{ "sspps",   3,  0x0033, "NAS SET_SYSTEM_SELECTION_PREFERENCE ps-only (restore)", 0x11, {0x3f, 0}, 2,
	  0x16, {0, 0, 0, 0, 0}, 5,
	  0x17, {1}, 1,
	  0x18, {2, 0, 0, 0}, 4 },
	/* --- WDS (service 1) / IMSA (service 0x21): the IMS bring-up
	 * chain, byte-for-byte from the imsdatadaemon strace reverse
	 * engineered by Dylan Van Assche (imsd docs, IMS-QUALCOMM.md).
	 * Only the daemon's OUTBOUND modem-facing messages are
	 * replicated; the inbound msg 2/4/5/6/7/8/9 frames in that
	 * trace are qcrild talking to the daemon itself, not to the
	 * modem.  Everything here is volatile — STOP_NETWORK (or a
	 * modem reboot) undoes it; no NV/EFS writes, unlike SET_SSP. */
	{ "wdsmux",  1,  0x00A2, "WDS BIND_MUX_DATA_PORT (QMAP mux bind)", 0x10, {4, 0, 0, 0, 1, 0, 0, 0}, 8,
	  0x11, {1}, 1 },
	/* Legacy bind per the redfin M7 receipt (devices/redfin/bringup/
	 * cell-bringup:69): 0x2F sent BARE (no TLVs) when the WDS instance
	 * answers err3 on the 0xA2 mux bind — bearer lands directly on
	 * rmnet_ipa0.  Redfin needed the netmgrd shim's DPM port open
	 * first or this bind was refused all boot. */
	{ "wdsport", 1,  0x002F, "WDS BIND_DATA_PORT (legacy, bare)" },
	{ "wdsbind", 1,  0x00AF, "WDS BIND_SUBSCRIPTION primary", 0x01, {1, 0, 0, 0}, 4 },
	{ "wdsgetsub", 1, 0x00B0, "WDS GET_BIND_SUBSCRIPTION" },
	{ "wdsipfam", 1, 0x004D, "WDS SET_IP_FAMILY ipv4", 0x01, {4}, 1 },
	/* START_NETWORK terminal form: apn=ims, ipv4, 3gpp profile 2,
	 * 3gpp2 profile 0xFF (none), call type 1.  TLV ids/names per
	 * libqmi qmi-service-wds.json; values verbatim from the strace. */
	{ "wdsstart", 1, 0x0020, "WDS START_NETWORK apn=ims ipv4 profile2 calltype1",
	  0x14, {'i', 'm', 's'}, 3,
	  0x19, {4}, 1,
	  0x31, {2}, 1,
	  0x32, {0xFF}, 1,
	  0x35, {1}, 1 },
	{ "wdsstat", 1,  0x0022, "WDS GET_PACKET_SERVICE_STATUS" },
	{ "wdsget",  1,  0x002D, "WDS GET_CURRENT_SETTINGS (bare)" },
	/* Profile numbering is device-config dependent: this unit's 3GPP
	 * table holds only idx 0/100/101 (ctnet/ctwap), so wdschain takes
	 * pN to override 0x31, or "ims" to start by APN with no profile. */
	{ "wdsplist", 1, 0x002A, "WDS GET_PROFILE_LIST 3gpp", 0x10, {1}, 1 },
	/* wdsprof takes the profile index as argv[2] (see main). */
	{ "wdsprof",  1, 0x002B, "WDS GET_PROFILE_SETTINGS idx=<argv2>", 0x01, {0, 1}, 2 },
	/* wdsstop takes the packet handle printed by wdsstart as argv[2]
	 * (decimal or 0x-hex). */
	{ "wdsstop", 1,  0x0021, "WDS STOP_NETWORK handle=<argv2>", 0x01, {0, 0, 0, 0}, 4 },
	/* LTE attach profile state (blank-EFS suspect): the default EPS
	 * bearer is bound to a configured attach profile; if unset, EMM
	 * attach lives but every START dies InvalidOperation. */
	{ "wdsattp", 1,  0x0085, "WDS GET_LTE_ATTACH_PARAMETERS (bare)" },
	{ "wdsattn", 1,  0x0094, "WDS GET_LTE_ATTACH_PDN_LIST (bare)" },
	/* Byte counters on the default-bearer session the modem holds:
	 * nonzero rx/tx = live traffic, zeros = cached attach state. */
	{ "wdsstatx", 1, 0x0024, "WDS GET_PACKET_STATISTICS (bare)" },
	/* WDA data-format pair = netmgrd's boot job on stock Android.  On
	 * this bare L0 nobody ever configured the data EP: mux-bind INTERNAL
	 * and START error 70 (even with existing profile 100) both match a
	 * never-configured path.  Link-layer protocol per libqmi: 1=802.3,
	 * 2=raw-IP. */
	/* This firmware rejects endpoint-less Get Data Format (error 48
	 * INVALID_ARGUMENT) although libqmi marks the TLV optional —
	 * carry EP {type=4 embedded, iface=1} like the trace mux bind. */
	{ "wdfmt",   0x1A, 0x0021, "WDA GET_DATA_FORMAT ep(4,1)", 0x10, {4, 0, 0, 0, 1, 0, 0, 0}, 8 },
	{ "wdfmtraw", 0x1A, 0x0020, "WDA SET_DATA_FORMAT raw-ip", 0x11, {2, 0, 0, 0}, 4 },
	/* Endpoint-less GET: libqmi marks the TLV optional; run once to
	 * confirm the firmware's rejection is universal. */
	{ "wdfmtget", 0x1A, 0x0021, "WDA GET_DATA_FORMAT bare" },
	/* Full SET bundle copied from ModemManager sync_wda_data_format on
	 * qrtr+IPA (mm-port-qmi.c): raw-ip + UL/DL aggregation + max
	 * datagrams 32 + max size 32768 + endpoint.  NOTE: on SET the
	 * endpoint TLV is 0x17, not GET's 0x10 (libqmi
	 * qmi-service-wda.json).  DAP: QMAP=5, QMAPV4=8, QMAPV5=9.  MM
	 * tries V5, then V4, then plain QMAP.  This bundle is what netmgrd
	 * programs at boot; without it Bind Mux Data Port dies with
	 * error 3 INTERNAL. */
	{ "wdfmtqmap5", 0x1A, 0x0020, "WDA SET_DATA_FORMAT raw-ip+qmapv5",
	  0x11, {2, 0, 0, 0}, 4,
	  0x12, {9, 0, 0, 0}, 4,
	  0x13, {9, 0, 0, 0}, 4,
	  0x15, {32, 0, 0, 0}, 4,
	  0x16, {0, 128, 0, 0}, 4,
	  0x17, {4, 0, 0, 0, 1, 0, 0, 0}, 8 },
	{ "wdfmtqmap4", 0x1A, 0x0020, "WDA SET_DATA_FORMAT raw-ip+qmapv4",
	  0x11, {2, 0, 0, 0}, 4,
	  0x12, {8, 0, 0, 0}, 4,
	  0x13, {8, 0, 0, 0}, 4,
	  0x15, {32, 0, 0, 0}, 4,
	  0x16, {0, 128, 0, 0}, 4,
	  0x17, {4, 0, 0, 0, 1, 0, 0, 0}, 8 },
	{ "wdfmtqmap", 0x1A, 0x0020, "WDA SET_DATA_FORMAT raw-ip+qmap",
	  0x11, {2, 0, 0, 0}, 4,
	  0x12, {5, 0, 0, 0}, 4,
	  0x13, {5, 0, 0, 0}, 4,
	  0x15, {32, 0, 0, 0}, 4,
	  0x16, {0, 128, 0, 0}, 4,
	  0x17, {4, 0, 0, 0, 1, 0, 0, 0}, 8 },
	{ "imsareg", 0x21, 0x0020, "IMSA GET_IMS_REGISTRATION_STATUS" },
	{ "imsasvc", 0x21, 0x0021, "IMSA GET_IMS_SERVICES_STATUS" },
	/* IMS service (0x12): the on-modem IMS client's switches and
	 * APN/credential policy.  Stock Android's ims APK programs these
	 * (SET 0x008f / 0x0047); nobody does on bare L0.  0x0090 output
	 * TLVs are u8 booleans: 0x11 voice / 0x19 registration / 0x1b
	 * SMS enabled.  0x0048: 0x15..0x18 priority (u32, ACS/ISIM/NV/
	 * PCO per input json) + 0x1a APN list. */
	{ "imsget", 0x12, 0x0090, "IMS GET_SERVICES_ENABLED_SETTING (bare)" },
	{ "imspolicy", 0x12, 0x0048, "IMS GET_POLICY_MANAGER_SETTINGS (bare)" },
	/* DSD tells which data systems are available for calls — the IMS
	 * client consults it before attaching the ims PDN. */
	{ "dsd", 0x2A, 0x0024, "DSD GET_SYSTEM_STATUS (bare)" },
	{ "dsdapn0", 0x2A, 0x0033, "DSD GET_APN_INFO type=default", 0x01, {0, 0, 0, 0}, 4 },
	{ "dsdapn1", 0x2A, 0x0033, "DSD GET_APN_INFO type=ims", 0x01, {1, 0, 0, 0}, 4 },
	{ "dsdapn2", 0x2A, 0x0033, "DSD GET_APN_INFO type=mms", 0x01, {2, 0, 0, 0}, 4 },
	{ "dsdapn8", 0x2A, 0x0033, "DSD GET_APN_INFO type=emergency", 0x01, {8, 0, 0, 0}, 4 },
	/* OpenIMSd OP6T pcap #499: DSD 0x0034 TLV 0x12 = 4, SUCCESS,
	 * between IMS 0x8f and IMS DCM START. */
	{ "dsd34", 0x2A, 0x0034, "DSD 0x34 gold tlv 0x12=4",
	  0x12, {4, 0, 0, 0}, 4 },
	/* OpenIMSd pcap before 0x8f: VOICE 0x40 TLV 0x16=3; WMS 0x5c /
	 * 0x45 / 0x4a / 0x48. */
	{ "voice40", 9, 0x0040, "VOICE 0x40 gold tlv 0x16=3", 0x16, {3}, 1 },
	{ "wms5c", 5, 0x005C, "WMS gold 0x5c bare" },
	{ "wms45", 5, 0x0045, "WMS IND_REG gold tlv 0x01=1", 0x01, {1}, 1 },
	{ "wms4a", 5, 0x004A, "WMS GET_TRANSPORT_NW_REG" },
	{ "wms30", 5, 0x0030, "WMS GET_MESSAGE_PROTOCOL" },
	{ "wms32", 5, 0x0032, "WMS GET_ROUTES" },
	{ "wms48", 5, 0x0048, "WMS GET_TRANSPORT_LAYER" },
	{ "wmsbind", 5, 0x004F, "WMS BIND_SUBSCRIPTION", 0x01, {2, 0, 0, 0}, 4 },
	{ "wmsmsgs", 5, 0x001E, "WMS GET_SUPPORTED_MESSAGES" },
	{ "dsdmsgs", 0x2A, 0x001E, "DSD GET_SUPPORTED_MESSAGES" },
	{ "voicemsgs", 9, 0x001E, "VOICE GET_SUPPORTED_MESSAGES" },
	/* The boot trigger stock Android's ims APK owns: enable voice+SMS
	 * so the on-modem IMS client starts registering.  TLVs are u8
	 * booleans per json (0x10 voice-over-LTE, 0x1a sms). */
	{ "imsen", 0x12, 0x008f, "IMS SET_SERVICES_ENABLED voice+sms",
	  0x10, {1}, 1,
	  0x1A, {1}, 1 },
	/* libqmi 1.36 --ims-bind: msg 0x0098 TLV 0x01 u32 subscription.
	 * GET/SET 0x90/0x8f are InvalidOperation until this client is bound. */
	{ "imsbind", 0x12, 0x0098, "IMS BIND subscription", 0x01, {1, 0, 0, 0}, 4 },
	/* --imsa-bind: msg 0x0033 TLV 0x01 u32 subscription. */
	{ "imsabind", 0x21, 0x0033, "IMSA BIND subscription", 0x01, {1, 0, 0, 0}, 4 },
	/* B1 (READ_TRANSPARENT 0x0020): EF_IMPI (6F02) through a card
	 * session bound to the ISIM ADF.  Session TLV 0x01 = {u8 type
	 * CARD_SLOT_1=6, u8 aid_len, aid}; binding this session is the
	 * normal trigger that pulls an app from detected to ready.
	 * File TLV 0x02 = {u16 file_id LE, u8 path_len, path}; 7FFF is
	 * the QMI path notation for the session's ADF (MM reads USIM
	 * ADF EFs at 3F00+7FFF the same way).  Strictly read-only. */
	{ "isimread", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMPI 6f02 via ISIM card-session",
	  0x02, {0x02, 0x6f, 0x04, 0x3f, 0x00, 0x7f, 0xff}, 7,
	  0x01, {6, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 64, 0}, 4 },
	/* Shape 2: path relative to the ADF only (no 3F00 MF prefix). */
	{ "isimread2", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMPI path=7fff only",
	  0x02, {0x02, 0x6f, 0x02, 0x7f, 0xff}, 5,
	  0x01, {6, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 64, 0}, 4 },
	/* Shape 3: NONPROVISIONING_SLOT_1 session (no provisioning binding,
	 * AID selects the app). */
	{ "isimread3", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMPI session=nonprov-slot1",
	  0x02, {0x02, 0x6f, 0x04, 0x3f, 0x00, 0x7f, 0xff}, 7,
	  0x01, {4, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 64, 0}, 4 },
	/* Controls: same session machinery against the READY USIM app.
	 * If these succeed while the ISIM shapes die INTERNAL, the
	 * read path is fine and the wall is ISIM-select specific. */
	{ "usimread", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMSI 6f07 via USIM card-session (control)",
	  0x02, {0x07, 0x6f, 0x04, 0x3f, 0x00, 0x7f, 0xff}, 7,
	  0x01, {6, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x02, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 16, 0}, 4 },
	{ "usimread2", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMSI 6f07 via primary-gw session (control)",
	  0x02, {0x07, 0x6f, 0x04, 0x3f, 0x00, 0x7f, 0xff}, 7,
	  0x01, {0, 0}, 2,
	  0x03, {0, 0, 16, 0}, 4 },
	/* Old-format session hypothesis: this MPSS.AT generation may
	 * implement the pre-extended Session TLV = bare u8 provisioning
	 * session, so our AID-carrying TLV poisons every read.  Three
	 * discriminators: no session at all, ICCID at MF root (no ADF),
	 * and a len-1 bare-u8 primary-gw session. */
	{ "usimread3", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMSI 6f07 no session TLV",
	  0x02, {0x07, 0x6f, 0x04, 0x3f, 0x00, 0x7f, 0xff}, 7,
	  0, {0}, 0,
	  0x03, {0, 0, 16, 0}, 4 },
	{ "iccread", 11, 0x0020, "UIM READ_TRANSPARENT EF_ICCID 2fe2 at MF root no session",
	  0x02, {0xe2, 0x2f, 0x02, 0x3f, 0x00}, 5,
	  0x01, {0, 0}, 2,
	  0x03, {0, 0, 16, 0}, 4 },
	/* qmicli replicas (the decisive reference shapes from
	 * qmicli-uim.c read_transparent_build_input): path elements are
	 * u16 LITTLE-ENDIAN byte pairs ("3f00" -> 00 3f, "7fff" -> ff 7f,
	 * per get_sim_file_id_and_path_with_separator), Read Information
	 * is always (offset 0, length 0) = whole file, and the session is
	 * PRIMARY_GW with an empty ignored AID. */
	{ "iccread5", 11, 0x0020, "UIM READ_TRANSPARENT EF_ICCID qmicli-shape",
	  0x02, {0xe2, 0x2f, 0x02, 0x00, 0x3f}, 5,
	  0x01, {0, 0}, 2,
	  0x03, {0, 0, 0, 0}, 4 },
	{ "usimread5", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMSI qmicli-shape",
	  0x02, {0x07, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {0, 0}, 2,
	  0x03, {0, 0, 0, 0}, 4 },
	{ "isimread5", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMPI qmicli-shape isim card-session",
	  0x02, {0x02, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {6, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	/* ISIM wall follow-ups: the qmicli shape works for ICCID/IMSI on
	 * the provisioned USIM, so the remaining err3 is ISIM-session
	 * specific.  Discriminate: nonprovisioning session vs card
	 * session, ADF-relative path (no 3F00), and a second ISIM file
	 * (EF_DOMAIN 6F03) in case IMPI itself is special. */
	{ "isimread6", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMPI session=nonprov-slot1",
	  0x02, {0x02, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {4, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	{ "isimdom6", 11, 0x0020, "UIM READ_TRANSPARENT EF_DOMAIN 6f03 session=nonprov-slot1",
	  0x02, {0x03, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {4, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	{ "isimread7", 11, 0x0020, "UIM READ_TRANSPARENT EF_IMPI path=adf-relative",
	  0x02, {0x02, 0x6f, 0x02, 0xff, 0x7f}, 5,
	  0x01, {6, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	{ "isimdom", 11, 0x0020, "UIM READ_TRANSPARENT EF_DOMAIN 6f03 isim card-session",
	  0x02, {0x03, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {6, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	/* Physical slot 2 (this CT card): CARD_SLOT_2 = 7. */
	{ "isim2", 11, 0x0020, "UIM READ EF_IMPI ISIM CARD_SLOT_2",
	  0x02, {0x02, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {7, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	{ "isim2dom", 11, 0x0020, "UIM READ EF_DOMAIN ISIM CARD_SLOT_2",
	  0x02, {0x03, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {7, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	{ "imspen", 0x1F, 0x0020, "IMSP GET_ENABLER_STATE" },
	/* Gold qcrild before 0x8f: ISIM via NONPROVISIONING_SLOT_2 (5),
	 * not CARD_SLOT_2 (7). AID = this card's ISIM. */
	{ "isimnp2", 11, 0x0020, "UIM READ EF_IMPI nonprov-slot2 ISIM",
	  0x02, {0x02, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {5, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	{ "isimnp2dom", 11, 0x0020, "UIM READ EF_DOMAIN nonprov-slot2 ISIM",
	  0x02, {0x03, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {5, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {0, 0, 0, 0}, 4 },
	/* Gold #193: READ_RECORD 0x21 EF_IMPU 6F04 rec#1 on ISIM session. */
	{ "uim36", 11, 0x0036, "UIM GET_SERVICE_STATUS gold",
	  0x01, {0, 0}, 2,
	  0x02, {1, 0, 0, 0}, 4 },
	/* Gold first UIM: EVENT_REG 0x41 TLV 0x01=1; GET_CONFIGURATION 0x2A. */
	{ "uim41", 11, 0x0041, "UIM EVENT_REG gold tlv 0x01=1", 0x01, {1}, 1 },
	{ "uim2a", 11, 0x002A, "UIM REFRESH_REGISTER gold vote=0",
	  0x01, {7, 0}, 2,
	  0x02, {1, 0, 0, 0}, 4 },
	/* Same 0x2A with Vote For Init = 1 (NAA init). */
	{ "uim2avote", 11, 0x002A, "UIM REFRESH_REGISTER vote_for_init=1 slot2",
	  0x01, {7, 0}, 2,
	  0x02, {1, 1, 0, 0}, 4 },
	{ "uim2e", 11, 0x002E, "UIM REGISTER_EVENTS mask=card+ext",
	  0x01, {0x03, 0, 0, 0}, 4 },
	{ "uim24isim", 11, 0x0024, "UIM GET_FILE_ATTRIBUTES EF_IMPI nonprov-slot2",
	  0x02, {0x02, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {5, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18 },
	{ "isimnp2impu", 11, 0x0021, "UIM READ_RECORD EF_IMPU nonprov-slot2",
	  0x02, {0x04, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f}, 7,
	  0x01, {5, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18,
	  0x03, {1, 0, 0x64, 0}, 4 },
	/* MM combination row 6 (bam-dmux / any-driver shape): raw-ip, no
	 * aggregation.  If the kernel ipa3 IPA-QMI contract is up WITHOUT
	 * aggregation, this SET should MATCH the contract and succeed where
	 * the QMAP ladder got INVALID_OPERATION. */
	{ "wdfmtdis", 0x1A, 0x0020, "WDA SET_DATA_FORMAT raw-ip+noagg+ep",
	  0x11, {2, 0, 0, 0}, 4,
	  0x12, {0, 0, 0, 0}, 4,
	  0x13, {0, 0, 0, 0}, 4,
	  0x17, {4, 0, 0, 0, 1, 0, 0, 0}, 8 },
	/* Same minus the endpoint TLV, in case 0x17 itself is what the
	 * firmware chokes on. */
	{ "wdfmtdisn", 0x1A, 0x0020, "WDA SET_DATA_FORMAT raw-ip+noagg",
	  0x11, {2, 0, 0, 0}, 4,
	  0x12, {0, 0, 0, 0}, 4,
	  0x13, {0, 0, 0, 0}, 4 },
	/* Capability self-report: which message ids each service supports.
	 * If the port behind svc 0x1A answers 0x001E but its list omits
	 * 0x0020/0x0021, the "WDA" we found is a stub and every GET/SET
	 * rejection (48/70) is id-based, not state-based.  List TLV is
	 * 0x02, payload = array of u8 msg ids (raw hexdump readable). */
	{ "wdsmsgs",  1,    0x001E, "WDS GET_SUPPORTED_MESSAGES" },
	{ "wdamsgs",  0x1A, 0x001E, "WDA GET_SUPPORTED_MESSAGES" },
	{ "imsmsgs",  0x12, 0x001E, "IMS GET_SUPPORTED_MESSAGES" },
	{ "imsamsgs", 0x21, 0x001E, "IMSA GET_SUPPORTED_MESSAGES" },
	/* DPM (svc 0x2f): redfin's netmgrd shim does DPM port open before
	 * WDS bind is accepted; these probes let the modem enumerate its
	 * own DPM message ids before we craft OPEN_PORT by hand. */
	{ "dpmmsgs", 0x2F, 0x001E, "DPM GET_SUPPORTED_MESSAGES" },
	/* DPM OPEN_PORT (0x0020) per libqmi data/qmi-service-dpm.json: TLV
	 * 0x10 = array of control ports {string name, u32 ep_type, u32
	 * iface}.  One embedded (4) control port named after the IPA
	 * netdev; tlv2 alone carries it (tlv1 stays empty).  dpmopen =
	 * iface 0, dpmopen1 = iface 1 (redfin's mux bind used iface 1). */
	{ "dpmopen",  0x2F, 0x0020, "DPM OPEN_PORT ctl rmnet_ipa0 emb iface0", 0, {0}, 0,
	  0x10, {1, 10, 'r','m','n','e','t','_','i','p','a','0', 4,0,0,0, 0,0,0,0}, 20 },
	{ "dpmopen1", 0x2F, 0x0020, "DPM OPEN_PORT ctl rmnet_ipa0 emb iface1", 0, {0}, 0,
	  0x10, {1, 10, 'r','m','n','e','t','_','i','p','a','0', 4,0,0,0, 1,0,0,0}, 20 },
};

static void put16(uint8_t *p, uint16_t v) { memcpy(p, &v, 2); }
static uint16_t get16(const uint8_t *p) { uint16_t v; memcpy(&v, p, 2); return v; }
static void put32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }
static uint32_t get32(const uint8_t *p)
{
	return get16(p) | ((uint32_t)get16(p + 2) << 16);
}

static void dump_tlv(const uint8_t *d, unsigned n)
{
	unsigned i;

	for (i = 0; i < n; i++)
		printf("%02x", d[i]);
	putchar(' ');
	putchar('"');
	for (i = 0; i < n; i++)
		putchar((d[i] >= 0x20 && d[i] < 0x7f) ? d[i] : '.');
	putchar('"');
}

/* TLV 0x02 = struct qmi_response_type_v01 { u16 result; u16 error; } */
static void note_result(const uint8_t *d, unsigned n)
{
	uint16_t r, e;

	if (n < 4)
		return;
	r = get16(d);
	e = get16(d + 2);
	printf("    [qmi result=%u error=%u %s]\n", r, e,
	       r == QMI_RESULT_SUCCESS_V01 ? "SUCCESS" : "FAILURE");
}

static const char *reg_state_str(unsigned v)
{
	switch (v) {
	case 0: return "NOT_REGISTERED";
	case 1: return "REGISTERED (home)";
	case 2: return "SEARCHING";
	case 3: return "DENIED";
	case 4: return "UNKNOWN";
	case 5: return "REGISTERED (roaming)";
	default: return "?";
	}
}

static const char *attach_state_str(unsigned v)
{
	switch (v) {
	case 0: return "unknown";
	case 1: return "ATTACHED";
	case 2: return "detached";
	default: return "?";
	}
}

static const char *network_type_str(unsigned v)
{
	switch (v) {
	case 0: return "no service";
	case 1: return "CDMA2000 1X";
	case 2: return "CDMA2000 HRPD";
	case 3: return "GSM";
	case 4: return "UMTS";
	case 5: return "LTE";
	case 8: return "TD-SCDMA";
	case 9: return "5G NR";
	default: return "?";
	}
}

/*
 * Labeled hints for the known TLVs of each query.  Deliberately minimal:
 * only decode what the standard tables pin unambiguously; the raw hex
 * above each hint stays the authoritative record.
 */
static void hint(const struct query *q, uint8_t key,
		 const uint8_t *d, unsigned n)
{
	if (key == 0x02) {
		note_result(d, n);
		return;
	}
	/* GET_SUPPORTED_MESSAGES: TLV 0x10 = u16 byte-count + bitmap.
	 * Opcode N is bit (N%8) of byte (N/8). HARDWARE 2026-09-16
	 * IMS list prefix `9b 00` is this count. */
	if (q->msg == 0x001E && key == 0x10 && n >= 2) {
		uint16_t nbytes = get16(d);
		unsigned lim = nbytes;
		unsigned i, b, shown = 0;

		if (lim > n - 2)
			lim = n - 2;
		printf("    [opcodes");
		for (i = 0; i < lim; i++) {
			for (b = 0; b < 8; b++) {
				if (d[2 + i] & (1u << b)) {
					printf(" 0x%02x", i * 8 + b);
					shown++;
				}
			}
		}
		printf(" (%u)]\n", shown);
		return;
	}
	if (!strcmp(q->cmd, "sig") && key == 0x01 && n >= 2) {
		int rssi = (int8_t)d[0];

		printf("    [rssi %d dBm  radio_if=%u (%s)]\n", rssi, d[1],
		       d[1] == 8 ? "LTE" : d[1] == 4 ? "UMTS" : d[1] == 5 ? "GSM" : "?");
		return;
	}
	if (!strcmp(q->cmd, "serving") && key == 0x01 && n >= 5) {
		printf("    [reg=%u %s  cs=%u %s  ps=%u %s  selected_net=%u  radios:",
		       d[0], reg_state_str(d[0]),
		       d[1], attach_state_str(d[1]),
		       d[2], attach_state_str(d[2]), d[3]);
		for (unsigned i = 5; i < n && i < 5u + d[4]; i++)
			printf(" %u", d[i]);
		printf("]\n");
		return;
	}
	if (!strcmp(q->cmd, "serving") && key == 0x10 && n >= 1) {
		printf("    [roaming indicator %u]\n", d[0]);
		return;
	}
	if (!strcmp(q->cmd, "sim") && key == 0x10 && n >= 10) {
		unsigned ncards = d[8], off = 9, c, a, j;
		static const char *ty[] = {"unknown", "sim", "usim", "ruim",
					   "csim", "isim"};
		static const char *st[] = {"unknown", "detected", "pin-req",
					   "puk-req", "check-pers",
					   "pin1-blocked", "illegal", "ready"};

		printf("    [gw_primary_idx=%u 1x_primary_idx=%u ncards=%u]\n",
		       get16(d), get16(d + 2), ncards);
		for (c = 0; c < ncards && off + 6 <= n; c++) {
			unsigned napps = d[off + 5];

			printf("    [card%u state=%u (%s) upin=%u err=%u napps=%u]\n",
			       c, d[off],
			       d[off] == 0 ? "ABSENT" : d[off] == 1 ? "PRESENT" :
			       d[off] == 2 ? "ERROR" : "?",
			       d[off + 1], d[off + 4], napps);
			off += 6;
			for (a = 0; a < napps && off + 7 <= n; a++) {
				unsigned alen = d[off + 6];

				if (off + 7 + alen + 7 > n)
					break;
				printf("    [app%u type=%u (%s) state=%u (%s) pers=%u feat=%#x aid=",
				       a, d[off], d[off] < 6 ? ty[d[off]] : "?",
				       d[off + 1],
				       d[off + 1] < 8 ? st[d[off + 1]] : "?",
				       d[off + 2], d[off + 3]);
				for (j = 0; j < alen; j++)
					printf("%02x", d[off + 7 + j]);
				printf("]\n");
				off += 7 + alen + 7;
			}
		}
		return;
	}
	/* slots: TLV 0x10 = array of {u32 card_state, u32 slot_state,
	 * u8 logical_slot, u8 iccid_len, iccid[]}.  Print per-slot state. */
	if (!strcmp(q->cmd, "slots") && key == 0x10 && n >= 1) {
		unsigned i, nslots = d[0], off = 1;

		for (i = 0; i < nslots && off + 9 <= n; i++) {
			uint32_t cs = get16(d + off) | ((uint32_t)get16(d + off + 2) << 16);
			uint32_t ss = get16(d + off + 4) | ((uint32_t)get16(d + off + 6) << 16);

			printf("    [phys slot %u: card_state=%u (%s) slot_state=%u (%s) logical=%u]\n",
			       i + 1, cs, cs == 0 ? "absent" : cs == 1 ? "present" : "?",
			       ss, ss == 0 ? "inactive" : ss == 1 ? "active" : "?",
			       d[off + 8]);
			off += 9 + 1 + d[off + 9];
		}
		return;
	}
	if (!strcmp(q->cmd, "mode") && key == 0x01 && n >= 1) {
		const char *m[] = {"online","LPM (airplane)","factory-test","offline",
				   "resetting","shutting-down","persistent LPM","mode-only LPM"};

		printf("    [operating mode %u = %s]\n", d[0],
		       d[0] < 8 ? m[d[0]] : "?");
		return;
	}
	/* ssp: TLV 0x11 RAT mode bitmask (bit2 gsm, bit3 wcdma, bit4 lte,
	 * bit5 tdscdma); TLV 0x16 network selection (0=auto, 1=manual);
	 * TLV 0x18 service domain preference (0 auto, 1 cs, 2 ps, 3 cs+ps) */
	if (!strcmp(q->cmd, "ssp") && key == 0x11 && n >= 2) {
		uint16_t m = get16(d);

		printf("    [mode pref 0x%04x: gsm=%s wcdma=%s lte=%s tdscdma=%s]\n", m,
		       (m & 0x0004) ? "y" : "-", (m & 0x0008) ? "y" : "-",
		       (m & 0x0010) ? "y" : "-", (m & 0x0020) ? "y" : "-");
		return;
	}
	if (!strcmp(q->cmd, "ssp") && key == 0x16 && n >= 1) {
		printf("    [network selection %u (%s)]\n", d[0],
		       d[0] == 0 ? "auto" : "manual");
		return;
	}
	if (!strcmp(q->cmd, "ssp") && key == 0x18 && n >= 4) {
		uint32_t v = get16(d) | ((uint32_t)get16(d + 2) << 16);
		const char *sd[] = {"automatic", "cs only", "ps only", "cs+ps"};

		printf("    [service domain pref %u = %s]\n", v,
		       v < 4 ? sd[v] : "?");
		return;
	}
	/* ssp: TLV 0x20 voice domain preference (0 cs-only, 1 ps-only,
	 * 2 cs-preferred/csfb, 3 ps-preferred/volte) */
	if (!strcmp(q->cmd, "ssp") && key == 0x20 && n >= 4) {
		uint32_t v = get16(d) | ((uint32_t)get16(d + 2) << 16);
		const char *vd[] = {"cs only", "ps only (ims)",
				    "cs preferred (csfb)", "ps preferred (volte)"};

		printf("    [voice domain pref %u = %s]\n", v,
		       v < 4 ? vd[v] : "?");
		return;
	}
	/* dsd: TLV 0x10 = count u8, then {tech u32, rat u32, so_mask u64}
	 * per available system.  tech: 0=3gpp 1=3gpp2 2=wlan; rat:
	 * 1=wcdma 2=geran 3=lte 4=tds 6=5g 101=1x 102=hrpd 103=ehrpd */
	if (!strcmp(q->cmd, "dsd") && key == 0x10 && n >= 1) {
		unsigned i, cnt = d[0], off = 1;
		const char *tt[] = {"3gpp", "3gpp2", "wlan"};

		printf("    [%u available system(s)]\n", cnt);
		for (i = 0; i < cnt && off + 16 <= n; i++) {
			uint32_t tech = get16(d + off) |
					((uint32_t)get16(d + off + 2) << 16);
			uint32_t rat = get16(d + off + 4) |
					((uint32_t)get16(d + off + 6) << 16);

			printf("    [sys%u: tech=%u (%s) rat=%u (%s)]\n", i,
			       tech, tech < 3 ? tt[tech] : "?", rat,
			       rat == 1 ? "wcdma" : rat == 2 ? "geran" :
			       rat == 3 ? "LTE" : rat == 4 ? "tdscdma" :
			       rat == 6 ? "5g" : rat == 101 ? "1x" :
			       rat == 102 ? "hrpd" : rat == 103 ? "ehrpd" : "?");
			off += 16;
		}
		return;
	}
	/* imei: the printable digits inside TLV 0x10/0x11 are already
	 * visible in the raw dump; no extra hint needed. */
	/* wdfmt* GET_DATA_FORMAT replies: TLV 0x11 = link layer
	 * protocol u32 (1=802.3, 2=raw-ip); TLV 0x13 = downlink
	 * aggregation u32 (0=disabled, 5=QMAP, 8=QMAPv4, 9=QMAPv5). */
	if (!strncmp(q->cmd, "wdfmt", 5) && key == 0x11 && n >= 4) {
		uint32_t v = get16(d) | ((uint32_t)get16(d + 2) << 16);

		printf("    [link layer protocol %u = %s]\n", v,
		       v == 1 ? "802.3" : v == 2 ? "raw-ip" : "?");
		return;
	}
	if (!strncmp(q->cmd, "wdfmt", 5) && key == 0x13 && n >= 4) {
		uint32_t v = get16(d) | ((uint32_t)get16(d + 2) << 16);
		const char *ap[] = {"disabled", "tlp", "qc-ncm", "mbim", "rndis",
				    "qmap", "qmapv2", "qmapv3", "qmapv4", "qmapv5"};

		printf("    [dl aggregation %u = %s]\n", v,
		       v < 10 ? ap[v] : "?");
		return;
	}
	/* wdsstart: TLV 0x01 = packet data handle (u32 LE) — feed this
	 * back to wdsstop.  TLV 0x10 = call end reason (u16) on failure. */
	if (!strcmp(q->cmd, "wdsstart") && key == 0x01 && n >= 4) {
		uint32_t h = get16(d) | ((uint32_t)get16(d + 2) << 16);

		printf("    [packet handle 0x%08x  (wdsstop 0x%x)]\n", h, h);
		return;
	}
	if (!strcmp(q->cmd, "wdsstart") && key == 0x10 && n >= 2) {
		printf("    [call end reason %u]\n", get16(d));
		return;
	}
	/* wdsstat: TLV 0x01 connection status (u8).  2 = CONNECTED is
	 * pinned by the strace (indication arrives right at connect);
	 * the neighbours follow libqmi's table, provisional. */
	if (!strcmp(q->cmd, "wdsstat") && key == 0x01 && n >= 1) {
		const char *cs[] = {"unknown", "disconnected", "CONNECTED",
				    "suspended", "authenticating"};

		printf("    [connection status %u = %s]\n", d[0],
		       d[0] < 5 ? cs[d[0]] : "?");
		return;
	}
	/* wdsplist: TLV 0x01 = u8 count + elements {type u8, index u8,
	 * NUL-terminated name}. */
	if (!strcmp(q->cmd, "wdsplist") && key == 0x01 && n >= 1) {
		unsigned cnt = d[0], off = 1, k;

		printf("    [%u 3gpp profile(s)]\n", cnt);
		for (k = 0; k < cnt && off + 2 <= n; k++) {
			unsigned nl = strnlen((const char *)&d[off + 2],
					      n - off - 2);

			printf("    [profile type=%u idx=%u name=\"%.*s\"]\n",
			       d[off], d[off + 1], (int)nl, &d[off + 2]);
			off += 2 + nl + 1;
		}
		return;
	}
	/* wdsprof: TLV 0x10 = profile name string, 0x14 = APN string. */
	if (!strcmp(q->cmd, "wdsprof") && key == 0x10 && n >= 1) {
		printf("    [profile name \"%.*s\"]\n", (int)n, d);
		return;
	}
	if (!strcmp(q->cmd, "wdsprof") && key == 0x14 && n >= 1) {
		printf("    [apn \"%.*s\"]\n", (int)n, d);
		return;
	}
	/* wdfmt: TLV 0x11 link-layer protocol (u32), 0x12/0x13 uplink/
	 * downlink aggregation protocol (u32). */
	if (!strcmp(q->cmd, "wdfmt") &&
	    (key == 0x11 || key == 0x12 || key == 0x13) && n >= 4) {
		uint32_t v = get16(d) | ((uint32_t)get16(d + 2) << 16);

		printf("    [%s=%u%s]\n",
		       key == 0x11 ? "link-protocol" :
		       key == 0x12 ? "uplink-agg-proto" : "downlink-agg-proto",
		       v, key == 0x11 ? " (1=802.3 2=raw-ip)" : "");
		return;
	}
	/* imsareg: TLV 0x12 registration status (u32), 0x11 error code
	 * (u16), 0x13 error message (ascii — raw dump shows it),
	 * 0x14 registration technology (u32). */
	if (!strcmp(q->cmd, "imsareg") && key == 0x12 && n >= 4) {
		uint32_t v = get16(d) | ((uint32_t)get16(d + 2) << 16);

		printf("    [ims registration status %u = %s]\n", v,
		       v == 0 ? "not registered" : v == 1 ? "REGISTERED" : "(see libqmi QmiImsaImsRegistrationStatus)");
		return;
	}
	if (!strcmp(q->cmd, "imsareg") && key == 0x11 && n >= 2) {
		printf("    [ims registration error code %u]\n", get16(d));
		return;
	}
	if (!strcmp(q->cmd, "imsareg") && key == 0x14 && n >= 4) {
		uint32_t v = get16(d) | ((uint32_t)get16(d + 2) << 16);

		printf("    [ims registration technology %u]\n", v);
		return;
	}
	/* imsasvc: TLV 0x10 sms / 0x11 voice service status (u32). */
	if (!strcmp(q->cmd, "imsasvc") && (key == 0x10 || key == 0x11) && n >= 4) {
		uint32_t v = get16(d) | ((uint32_t)get16(d + 2) << 16);

		printf("    [ims %s service status %u]\n",
		       key == 0x10 ? "SMS" : "voice", v);
		return;
	}
}

/*
 * Resolve the (node, port) of the modem's service via NEW_LOOKUP with
 * wildcard version/instance.  Ends on the kernel's all-zero NEW_SERVER
 * sentinel, a matching hit, or timeout — whichever comes first.
 */
static int lookup_service(int sock, uint32_t svc, uint32_t ins,
			  uint32_t *node, uint32_t *port, uint32_t *got_ins)
{
	uint8_t buf[512];
	struct qrtr_packet pkt;
	struct sockaddr_qrtr sq;
	uint32_t n, p;
	int len, rc, i, found = 0;

	rc = qrtr_new_lookup(sock, svc, 0, ins);
	if (rc < 0) {
		fprintf(stderr, "lookup send failed\n");
		return -1;
	}

	for (i = 0; i < 10; i++) {
		len = qrtr_recvfrom(sock, buf, sizeof(buf), &n, &p);
		if (len < 0)
			continue;			/* 1s timeout each */
		if (p != QRTR_PORT_CTRL)
			continue;			/* stray data frame */

		memset(&sq, 0, sizeof(sq));
		sq.sq_family = AF_QIPCRTR;
		sq.sq_node = n;
		sq.sq_port = p;
		memset(&pkt, 0, sizeof(pkt));
		if (qrtr_decode(&pkt, buf, len, &sq) != 0)
			continue;
		if (pkt.type != QRTR_TYPE_NEW_SERVER)
			continue;
		if (!pkt.service && !pkt.instance && !pkt.node && !pkt.port)
			break;				/* kernel end-sentinel */
		if (pkt.service != svc || found)
			continue;

		*node = pkt.node;
		*port = pkt.port;
		*got_ins = pkt.instance;
		found = 1;
		/* keep reading: the sentinel must be consumed here or the
		 * next lookup on this socket reads ours as its own and
		 * reports ABSENT. */
	}

	qrtr_remove_lookup(sock, svc, 0, ins);
	return found ? 0 : -1;
}

/* svcls: which QMI services are registered on qrtr right now.  Probe
 * only (lookup, no message sent) — presence/absence of the IPA host
 * (0x31 ins 1) and IPA modem (0x31 ins 2) instances tells us whether
 * the kernel↔modem IPA handshake can even run. */
/* enumsvc: dump EVERY server registration visible on qrtr — one
 * service (hex arg) or all services (no arg).  Unlike svcls this
 * prints every matching NEW_SERVER, so multi-instance services show
 * all their ports: the WDS err-70 hunt needs to know whether the
 * first-announced server (what lookup_service's wildcard grabs) is a
 * different instance from the one cell-bringup targets (svc 1 ins 1).
 */
static void enumsvc(uint32_t svc)
{
	uint8_t buf[512];
	struct qrtr_packet pkt;
	struct sockaddr_qrtr sq;
	uint32_t n, p;
	int len, timeouts = 0, hits = 0;
	int sock = qrtr_open(0);

	if (sock < 0) {
		printf("  qrtr_open failed\n");
		return;
	}
	if (qrtr_new_lookup(sock, svc, 0, 0) < 0) {
		printf("  lookup send failed\n");
		close(sock);
		return;
	}
	for (;;) {
		len = qrtr_recvfrom(sock, buf, sizeof(buf), &n, &p);
		if (len < 0) {
			if (++timeouts >= 3)
				break;
			continue;
		}
		timeouts = 0;
		if (p != QRTR_PORT_CTRL)
			continue;
		memset(&sq, 0, sizeof(sq));
		sq.sq_family = AF_QIPCRTR;
		sq.sq_node = n;
		sq.sq_port = p;
		memset(&pkt, 0, sizeof(pkt));
		if (qrtr_decode(&pkt, buf, len, &sq) != 0)
			continue;
		if (pkt.type != QRTR_TYPE_NEW_SERVER)
			continue;
		if (!pkt.service && !pkt.instance && !pkt.node && !pkt.port)
			break;
		printf("  svc %-4u (0x%02x) ins %-4u (0x%x) ver %u "
		       "-> node %u port %u\n",
		       pkt.service, pkt.service, pkt.instance,
		       pkt.instance, pkt.version, pkt.node, pkt.port);
		hits++;
	}
	qrtr_remove_lookup(sock, svc, 0, 0);
	if (!hits)
		printf("  (no servers%s)\n", svc ? "" : " at all");
	close(sock);
}

static void svcls(void)
{
	static const struct {
		uint32_t svc, ins;
		const char *name;
	} tbl[] = {
		{ 0x01, 0, "WDS" },   { 0x02, 0, "DMS" },
		{ 0x03, 0, "NAS" },   { 0x05, 0, "WMS" },
		{ 0x09, 0, "VOICE" }, { 0x0b, 0, "UIM" },
		{ 0x10, 0, "LOC" },   { 0x12, 0, "IMS" },
		{ 0x13, 0, "ADS" },   { 0x15, 0, "MFS" },
		{ 0x24, 0, "PDC" },
		{ 0x1a, 0, "WDA" },   { 0x1e, 0, "IMSP" },
		{ 0x21, 0, "IMSA" },  { 0x31, 0, "IPA-any" },
		{ 0x31, 1, "IPA-host" },
		{ 0x31, 2, "IPA-modem" },
	};
	uint32_t node, port, ins;
	int sock, i;

	sock = qrtr_open(0);
	if (sock < 0) {
		printf("  qrtr_open failed\n");
		return;
	}
	for (i = 0; i < (int)(sizeof(tbl) / sizeof(tbl[0])); i++) {
		if (lookup_service(sock, tbl[i].svc, tbl[i].ins,
				   &node, &port, &ins) == 0)
			printf("  %-10s svc 0x%02x ins %u -> node %u port %u\n",
			       tbl[i].name, tbl[i].svc, ins, node, port);
		else
			printf("  %-10s svc 0x%02x ins %u -> ABSENT\n",
			       tbl[i].name, tbl[i].svc, tbl[i].ins);
	}
	close(sock);
}

static int ask_on(int sock, uint32_t node, uint32_t port,
		  const struct query *q, uint16_t txn, uint32_t *tlv1_u32,
		  uint8_t *tlv1_u8)
{
	uint8_t buf[2048];
	uint8_t req[64];
	uint16_t req_len = 0, tlv_bytes = 0;
	uint32_t rn, rp;
	unsigned off, end;
	int len, i, rc = 1;

	printf("== %s: %s (service %u, msg 0x%04x) ==\n",
	       q->cmd, q->desc, q->svc, q->msg);

	/* 7-byte frame header; TLVs appended when the query asks for them. */
	req[0] = QMI_REQUEST;
	put16(req + 1, txn);
	put16(req + 3, q->msg);
	req_len = 7;
	if (q->tlv_key) {
		req[req_len] = q->tlv_key;
		put16(req + req_len + 1, q->tlv_len);
		memcpy(req + req_len + 3, q->tlv_data, q->tlv_len);
		req_len += 3 + q->tlv_len;
	}
	if (q->tlv2_key) {
		req[req_len] = q->tlv2_key;
		put16(req + req_len + 1, q->tlv2_len);
		memcpy(req + req_len + 3, q->tlv2_data, q->tlv2_len);
		req_len += 3 + q->tlv2_len;
	}
	if (q->tlv3_key) {
		req[req_len] = q->tlv3_key;
		put16(req + req_len + 1, q->tlv3_len);
		memcpy(req + req_len + 3, q->tlv3_data, q->tlv3_len);
		req_len += 3 + q->tlv3_len;
	}
	if (q->tlv4_key) {
		req[req_len] = q->tlv4_key;
		put16(req + req_len + 1, q->tlv4_len);
		memcpy(req + req_len + 3, q->tlv4_data, q->tlv4_len);
		req_len += 3 + q->tlv4_len;
	}
	if (q->tlv5_key) {
		req[req_len] = q->tlv5_key;
		put16(req + req_len + 1, q->tlv5_len);
		memcpy(req + req_len + 3, q->tlv5_data, q->tlv5_len);
		req_len += 3 + q->tlv5_len;
	}
	if (q->tlv6_key) {
		req[req_len] = q->tlv6_key;
		put16(req + req_len + 1, q->tlv6_len);
		memcpy(req + req_len + 3, q->tlv6_data, q->tlv6_len);
		req_len += 3 + q->tlv6_len;
	}
	tlv_bytes = req_len - 7;
	put16(req + 5, tlv_bytes);
	if (qrtr_sendto(sock, node, port, req, req_len) < 0) {
		printf("  sendto failed\n");
		close(sock);
		return 1;
	}

	for (i = 0; i < 6; i++) {
		uint8_t flags;
		uint16_t rtxn, mid, mlen;

		len = qrtr_recvfrom(sock, buf, sizeof(buf), &rn, &rp);
		if (len < 0) {
			printf("  ...waiting (%d/6)\n", i + 1);
			continue;
		}
		if (rp == QRTR_PORT_CTRL)
			continue;			/* lookup tail packets */

		if (len < 7) {
			printf("  short frame (%d bytes) from node %u port %u\n",
			       len, rn, rp);
			continue;
		}
		flags = buf[0];
		rtxn = get16(buf + 1);
		mid = get16(buf + 3);
		mlen = get16(buf + 5);
		printf("  resp: flags 0x%02x txn %u msg 0x%04x msg_len %u "
		       "(from node %u port %u)\n", flags, rtxn, mid, mlen, rn, rp);

		if (flags != QMI_RESPONSE || rtxn != txn || mid != q->msg) {
			printf("  (not our response, ignored)\n");
			continue;
		}

		end = 7 + mlen;
		if (end > (unsigned)len)
			end = len;			/* clamp to what arrived */
		off = 7;
		while (off + 3 <= end) {
			uint8_t key = buf[off];
			uint16_t tl = get16(buf + off + 1);

			off += 3;
			if (off + tl > end) {
				printf("  TLV 0x%02x len %u TRUNCATED "
				       "(frame %d bytes)\n", key, tl, len);
				break;
			}
			printf("  TLV 0x%02x len %u: ", key, tl);
			dump_tlv(buf + off, tl);
			putchar('\n');
			hint(q, key, buf + off, tl);
			if (key == 0x01 && tl >= 4 && tlv1_u32)
				*tlv1_u32 = get16(buf + off) |
					    ((uint32_t)get16(buf + off + 2) << 16);
			if (key == 0x01 && tl >= 1 && tlv1_u8)
				*tlv1_u8 = buf[off];
			off += tl;
		}
		rc = 0;				/* a response arrived */
		break;
	}

	if (rc)
		printf("  TIMEOUT: no response\n");
	return rc;
}

static int ask(const struct query *q)
{
	uint32_t node, port, ins;
	int sock, rc;

	sock = qrtr_open(0);
	if (sock < 0) {
		printf("  qrtr_open failed\n");
		return 1;
	}
	if (lookup_service(sock, q->svc, 0, &node, &port, &ins) < 0) {
		printf("  no NEW_SERVER for service %u\n", q->svc);
		close(sock);
		return 1;
	}
	printf("  server: node %u port %u\n", node, port);

	rc = ask_on(sock, node, port, q, 1, NULL, NULL);
	close(sock);
	return rc;
}

/*
 * wdschain <hold-seconds> — the whole IMS PDN bring-up on ONE QMI
 * client, then hold the client open.  BIND_MUX/BIND_SUB/SET_IP_FAMILY
 * are per-client WDS state and the strace shows imsdatadaemon running
 * mux bind → sub bind → ipfam → start on a single client; scattering
 * them across qmi-ask processes (a fresh client each) got START_NETWORK
 * error 70 INVALID_OPERATION with handle 0.  Holding the client open
 * keeps the call up while IMSA registration is polled from elsewhere;
 * exiting (or a modem reboot) is the cleanup path.
 *
 * argv[3] "nomux" skips BIND_MUX_DATA_PORT: on this unit the byte-exact
 * trace bind answers error 3 INTERNAL (no QMAP/IPA data path up), and
 * a failed bind is a candidate poison for START_NETWORK error 70.
 *
 * Token "ims" drops TLVs 0x31/0x32 (profile indices) from START_NETWORK
 * and relies on the apn="ims" TLV 0x14 instead: this unit's 3GPP profile
 * table has only {0,100,101} = ctnet/ctwap data profiles (hidden idx 1-4
 * answer error 81), so no profile index can point at IMS; some firmwares
 * match or provision the PDN from the APN directly.
 */
static int chain(const char *hold_arg, int use_mux, int profile_idx,
		 int apn_only, int no_call_type, uint32_t wds_ins,
		 int sub_first, int prof2, int drop_apn)
{
	/* profile_idx/prof2: -1 = not set; 0 is a valid profile index */
	struct query *mux = NULL, *portb = NULL, *bind = NULL,
		     *ipfam = NULL, *start = NULL, *stat = NULL;
	uint32_t node, port, ins, handle = 0;
	uint16_t txn = 1;
	int sock, i, rc = 1;
	unsigned hold, t;

	hold = (unsigned)strtoul(hold_arg, NULL, 0);
	for (i = 0; i < (int)(sizeof(QUERIES) / sizeof(QUERIES[0])); i++) {
		const char *c = QUERIES[i].cmd;

		if (!strcmp(c, "wdsmux")) mux = &QUERIES[i];
		else if (!strcmp(c, "wdsport")) portb = &QUERIES[i];
		else if (!strcmp(c, "wdsbind")) bind = &QUERIES[i];
		else if (!strcmp(c, "wdsipfam")) ipfam = &QUERIES[i];
		else if (!strcmp(c, "wdsstart")) start = &QUERIES[i];
		else if (!strcmp(c, "wdsstat")) stat = &QUERIES[i];
	}
	if (profile_idx >= 0 && start) {
		start->tlv3_data[0] = (uint8_t)profile_idx;
		printf("  [chain start: 3gpp profile idx %u]\n", profile_idx);
	}
	if (apn_only && start) {
		start->tlv3_key = 0;
		start->tlv3_len = 0;
		start->tlv4_key = 0;
		start->tlv4_len = 0;
		printf("  [chain start: no profile TLVs, apn=ims only]\n");
	}
	/* qN: start on the OTHER profile family (TLV 0x32) — this unit's
	 * readable table {0,100,101}=ctnet/ctwap answers selector type=1,
	 * so its indices ride the 3gpp2-family TLV per libqmi naming.
	 * With an explicit profile the apn=ims TLV must go (it would
	 * override the profile's APN). */
	if (prof2 >= 0 && start) {
		start->tlv4_data[0] = (uint8_t)prof2;
		start->tlv3_key = 0;
		start->tlv3_len = 0;
		start->tlv_key = 0;
		start->tlv_len = 0;
		printf("  [chain start: family-1 profile idx %u, 3gpp+apn "
		       "TLVs dropped]\n", prof2);
	}
	if (drop_apn && start) {
		start->tlv_key = 0;
		start->tlv_len = 0;
		printf("  [chain start: apn TLV dropped]\n");
	}
	if (no_call_type && start) {
		start->tlv5_key = 0;
		start->tlv5_len = 0;
		printf("  [chain start: no call type TLV (MM sends none)]\n");
	}

	sock = qrtr_open(0);
	if (sock < 0) {
		printf("  qrtr_open failed\n");
		return 1;
	}
	if (lookup_service(sock, 1, wds_ins, &node, &port, &ins) < 0) {
		printf("  no NEW_SERVER for WDS ins %u\n", wds_ins);
		close(sock);
		return 1;
	}
	printf("  server: node %u port %u ins %u (single client, txn 1..)\n",
	       node, port, ins);

	/* QMI-level errors don't abort the chain — the printed receipts
	 * carry the diagnosis; only transport failures do (ask_on rc=1).
	 * sub_first = MM order: BIND_SUBSCRIPTION before the data-port
	 * bind (mm-broadband-modem-qmi binds the subscription when the WDS
	 * client is set up, bind_mux only at connect). */
	if (sub_first)
		ask_on(sock, node, port, bind, txn++, NULL, NULL);
	if (use_mux == 1)
		ask_on(sock, node, port, mux, txn++, NULL, NULL);
	else if (use_mux == 2)
		ask_on(sock, node, port, portb, txn++, NULL, NULL);
	if (!sub_first)
		ask_on(sock, node, port, bind, txn++, NULL, NULL);
	ask_on(sock, node, port, ipfam, txn++, NULL, NULL);
	if (ask_on(sock, node, port, start, txn++, &handle, NULL)) {
		printf("  start: no response\n");
		goto out;
	}
	printf("  [chain handle 0x%08x]\n", handle);
	if (!handle) {
		printf("  start failed (see result above); nothing to hold\n");
		goto out;
	}

	/* Poll packet service status on the same client until connected
	 * (2, pinned by the strace) or ~30s elapse. */
	for (t = 0; t < 15; t++) {
		uint8_t st = 0;

		sleep(2);
		if (ask_on(sock, node, port, stat, txn++, NULL, &st) == 0 &&
		    st == 2) {
			printf("  [packet service CONNECTED after %us]\n",
			       (t + 1) * 2);
			break;
		}
	}
	printf("  [holding client open %us — poll imsareg/imsasvc now]\n",
	       hold);
	for (t = 0; t < hold; t++)
		sleep(1);
	rc = 0;
out:
	close(sock);
	return rc;
}

/*
 * ipa <main|hwstats|nossr|android> [R] — IPA_QMI_INIT_DRIVER (svc 0x31,
 * msg 0x21) probe.  The B1 source audit (Android ipa_qmi_service.c vs
 * mainline v6.11 ipa_qmi.c/ipa_data_v3.5.1) left exactly three wire
 * deltas, one per variant; R = smem_restricted_bytes, which Android
 * adds to every IPA SRAM offset (readl(0x01e00054)>>16<<3 on this
 * SoC) and mainline does not.
 *
 *   main      mainline shape verbatim (no hw_stats, TLV 0x18 present=0)
 *   hwstats   main + TLV 0x1F..0x22 {base=R, size=0}   (Delta A)
 *   nossr     main minus TLV 0x18                       (Delta B)
 *   android   nossr + hwstats — Android first-boot shape  (V3)
 *   R arg     adds R to every address TLV               (V4)
 *
 * Mem offsets are the v3.5.1 layout, byte-identical on both sides
 * (DT qcom,ipa-ram-mmap == mainline ipa_data_v3_5_1); route TLVs
 * carry end=count-1=7, which equals Android's num_indices=hi — the
 * old off-by-one suspicion died at the wire level.  Timeout matches
 * mainline's QMI_INIT_DRIVER_TIMEOUT (60s).
 */
static int ipa_init(int argc, char **argv)
{
	uint8_t req[256], buf[2048];
	uint32_t R = 0, v[2], node, port, ins, rn, rp;
	uint16_t len = 7, txn = 1;
	const char *variant;
	int sock, i, rc = 1, hw = 0, ssr = 0;

	if (argc < 3 || argc > 4) {
		fprintf(stderr, "usage: %s ipa <main|hwstats|nossr|android> [R]\n",
			argv[0]);
		return 2;
	}
	variant = argv[2];
	if (argc == 4)
		R = (uint32_t)strtoul(argv[3], NULL, 0);
	if (!strcmp(variant, "main")) {
		ssr = 1;
	} else if (!strcmp(variant, "hwstats")) {
		hw = 1; ssr = 1;
	} else if (!strcmp(variant, "nossr")) {
		/* bare mainline minus 0x18 */
	} else if (!strcmp(variant, "android")) {
		hw = 1;
	} else {
		fprintf(stderr, "unknown ipa variant '%s'\n", variant);
		return 2;
	}
	printf("== ipa INIT_DRIVER variant=%s R=%u (0x%x) ==\n",
	       variant, R, R);

	req[0] = QMI_REQUEST;
	put16(req + 1, txn);
	put16(req + 3, 0x21);

	/* 0x10 platform_type = MSM_ANDROID (both sides) */
	v[0] = 3;
	len = 7;
	req[7] = 0x10; put16(req + 8, 4); put32(req + 11, v[0]);
	len += 3 + 4;
	/* 0x11 modem hdr table {start, end} */
	req[len] = 0x11; put16(req + len + 1, 8);
	put32(req + len + 3, 0x688 + R); put32(req + len + 7, 0x7c7 + R);
	len += 3 + 8;
	/* 0x12/0x13 route tables {start, max index} */
	req[len] = 0x12; put16(req + len + 1, 8);
	put32(req + len + 3, 0x508 + R); put32(req + len + 7, 7);
	len += 3 + 8;
	req[len] = 0x13; put16(req + len + 1, 8);
	put32(req + len + 3, 0x608 + R); put32(req + len + 7, 7);
	len += 3 + 8;
	/* 0x14/0x15 filter table starts */
	req[len] = 0x14; put16(req + len + 1, 4); put32(req + len + 3, 0x308 + R);
	len += 3 + 4;
	req[len] = 0x15; put16(req + len + 1, 4); put32(req + len + 3, 0x408 + R);
	len += 3 + 4;
	/* 0x16 modem mem {start, size} */
	req[len] = 0x16; put16(req + len + 1, 8);
	put32(req + len + 3, 0xbd8 + R); put32(req + len + 7, 0x1024);
	len += 3 + 8;
	/* 0x17 ctrl_comm_dest_end_pt = APPS_WAN_CONS (10 both sides) */
	req[len] = 0x17; put16(req + len + 1, 4); put32(req + len + 3, 10);
	len += 3 + 4;
	/* 0x18 is_ssr_bootup / skip_uc_load (same slot): Android omits
	 * on first boot, mainline sends valid=1 value=0 */
	if (ssr) {
		req[len] = 0x18; put16(req + len + 1, 1); req[len + 3] = 0;
		len += 3 + 1;
	}
	/* 0x19 modem proc ctx {start, end} */
	req[len] = 0x19; put16(req + len + 1, 8);
	put32(req + len + 3, 0x7d0 + R); put32(req + len + 7, 0x9cf + R);
	len += 3 + 8;
	/* 0x1A zip: absent on both sides for v3.5.1 (comp_decomp size 0) */
	/* 0x1B..0x1E hashed route/filter tables */
	req[len] = 0x1B; put16(req + len + 1, 8);
	put32(req + len + 3, 0x488 + R); put32(req + len + 7, 7);
	len += 3 + 8;
	req[len] = 0x1C; put16(req + len + 1, 8);
	put32(req + len + 3, 0x588 + R); put32(req + len + 7, 7);
	len += 3 + 8;
	req[len] = 0x1D; put16(req + len + 1, 4); put32(req + len + 3, 0x288 + R);
	len += 3 + 4;
	req[len] = 0x1E; put16(req + len + 1, 4); put32(req + len + 3, 0x388 + R);
	len += 3 + 4;
	/* 0x1F..0x22 hw stats: Android sends unconditionally with the
	 * OP6 DT's short array (base=0+R, size=0); mainline <4.0 never
	 * does.  Four separate single-u32 TLVs. */
	if (hw) {
		req[len] = 0x1F; put16(req + len + 1, 4); put32(req + len + 3, R);
		len += 3 + 4;
		req[len] = 0x20; put16(req + len + 1, 4); put32(req + len + 3, 0);
		len += 3 + 4;
		req[len] = 0x21; put16(req + len + 1, 4); put32(req + len + 3, R);
		len += 3 + 4;
		req[len] = 0x22; put16(req + len + 1, 4); put32(req + len + 3, 0);
		len += 3 + 4;
	}
	put16(req + 5, len - 7);
	printf("  request %u bytes, %u TLV bytes\n", len, len - 7);

	sock = qrtr_open(0);
	if (sock < 0) {
		printf("  qrtr_open failed\n");
		return 1;
	}
	if (lookup_service(sock, 0x31, 0, &node, &port, &ins) < 0) {
		printf("  no NEW_SERVER for service 0x31 (IPA) — modem "
		       "IPA task not up; run modem-up sequence first\n");
		close(sock);
		return 1;
	}
	printf("  server: node %u port %u (instance 0x%x)\n", node, port, ins);
	if (qrtr_sendto(sock, node, port, req, len) < 0) {
		printf("  sendto failed\n");
		close(sock);
		return 1;
	}

	/* 60s window (mainline's timeout), heartbeat every 10. */
	for (i = 0; i < 60; i++) {
		unsigned off, end;
		uint8_t flags;
		uint16_t rtxn, mid, mlen;
		int l;

		l = qrtr_recvfrom(sock, buf, sizeof(buf), &rn, &rp);
		if (l < 0) {
			if (i % 10 == 9)
				printf("  ...waiting (%ds)\n", i + 1);
			continue;
		}
		if (rp == QRTR_PORT_CTRL || l < 7)
			continue;
		flags = buf[0];
		rtxn = get16(buf + 1);
		mid = get16(buf + 3);
		mlen = get16(buf + 5);
		printf("  resp: flags 0x%02x txn %u msg 0x%04x msg_len %u "
		       "(node %u port %u) after %ds\n",
		       flags, rtxn, mid, mlen, rn, rp, i + 1);
		if (flags != QMI_RESPONSE || rtxn != txn || mid != 0x21) {
			printf("  (not our response, ignored)\n");
			continue;
		}
		end = 7 + mlen;
		if (end > (unsigned)l)
			end = (unsigned)l;
		off = 7;
		while (off + 3 <= end) {
			uint8_t key = buf[off];
			uint16_t tl = get16(buf + off + 1);

			off += 3;
			if (off + tl > end) {
				printf("  TLV 0x%02x len %u TRUNCATED\n",
				       key, tl);
				break;
			}
			printf("  TLV 0x%02x len %u: ", key, tl);
			dump_tlv(buf + off, tl);
			putchar('\n');
			if (key == 0x02)
				note_result(buf + off, tl);
			/* resp 0x10/0x11 = modem's ctrl comm dest EP and
			 * default EP (modem tells us where to talk) */
			if ((key == 0x10 || key == 0x11) && tl >= 4)
				printf("    [%s EP %u]\n",
				       key == 0x10 ? "ctrl-comm-dest" : "default",
				       get32(buf + off));
			off += tl;
		}
		rc = 0;
		break;
	}
	if (rc)
		printf("  TIMEOUT: no response in 60s\n");
	close(sock);
	return rc;
}

/*
 * pdc — read-only PDC (svc 0x24) probes: GET_SELECTED_CONFIG (0x22)
 * and LIST_CONFIGS (0x24), for both config types (0=platform/hw,
 * 1=software/sw, libqmi qmi-enums-pdc.h).  Both messages answer with
 * a bare result frame; the payload rides a separate indication
 * (flags 0x04) keyed by token — so each request opens an 8x1s frame
 * window.  READ ONLY: no Load/Set/Delete/Activate is sent here.
 */
static int pdc_run(void)
{
	static const struct { uint16_t msg; const char *name; } msgs[] = {
		{ 0x22, "GET_SELECTED_CONFIG" },
		{ 0x24, "LIST_CONFIGS" },
	};
	static const uint32_t ctypes[] = { 0, 1 };
	static const char *ctnames[] = { "platform(hw)", "software(sw)" };
	uint8_t req[64], buf[2048], active_id[24];
	uint32_t node, port, ins, rn, rp;
	uint16_t txn = 1;
	unsigned active_id_len = 0;
	int sock, len, m, c, any = 0;

	sock = qrtr_open(0);
	if (sock < 0) {
		printf("  qrtr_open failed\n");
		return 1;
	}
	if (lookup_service(sock, 0x24, 0, &node, &port, &ins) < 0) {
		printf("  no NEW_SERVER for service 0x24 (PDC) — modem "
		       "PDC task absent\n");
		close(sock);
		return 1;
	}
	printf("  PDC server: node %u port %u (ins 0x%x)\n", node, port, ins);

	for (m = 0; m < 2; m++) {
		for (c = 0; c < 2; c++) {
			unsigned off, end, got = 0;
			int i;

			printf("== PDC %s ctype=%u (%s) token=%u ==\n",
			       msgs[m].name, ctypes[c], ctnames[c], txn);
			req[0] = QMI_REQUEST;
			put16(req + 1, txn);
			put16(req + 3, msgs[m].msg);
			if (msgs[m].msg == 0x22) {
				/* TLV 0x01 config type, TLV 0x10 token */
				req[7] = 0x01; put16(req + 8, 4);
				put32(req + 10, ctypes[c]);
				req[14] = 0x10; put16(req + 15, 4);
				put32(req + 17, txn);
			} else {
				/* TLV 0x10 token, TLV 0x11 config type */
				req[7] = 0x10; put16(req + 8, 4);
				put32(req + 10, txn);
				req[14] = 0x11; put16(req + 15, 4);
				put32(req + 17, ctypes[c]);
			}
			put16(req + 5, 14);
			if (qrtr_sendto(sock, node, port, req, 21) < 0) {
				printf("  sendto failed\n");
				close(sock);
				return 1;
			}
			for (i = 0; i < 8; i++) {
				uint8_t flags;
				uint16_t rtxn, mid, mlen;

				len = qrtr_recvfrom(sock, buf, sizeof(buf),
						    &rn, &rp);
				if (len < 0)
					continue;
				if (rp == QRTR_PORT_CTRL || len < 7)
					continue;
				flags = buf[0];
				rtxn = get16(buf + 1);
				mid = get16(buf + 3);
				mlen = get16(buf + 5);
				if (rn != node ||
				    (flags != QMI_RESPONSE &&
				     flags != QMI_INDICATION) ||
				    mid != msgs[m].msg || rtxn != txn)
					continue;
				printf("  %s: flags 0x%02x txn %u msg "
				       "0x%04x msg_len %u\n",
				       flags == QMI_RESPONSE ? "resp" : "ind",
				       flags, rtxn, mid, mlen);
				end = 7 + mlen;
				if (end > (unsigned)len)
					end = (unsigned)len;
				off = 7;
				while (off + 3 <= end) {
					uint8_t key = buf[off];
					uint16_t tl = get16(buf + off + 1);

					off += 3;
					if (off + tl > end) {
						printf("  TLV 0x%02x len %u "
						       "TRUNCATED\n", key, tl);
						break;
					}
					printf("  TLV 0x%02x len %u: ", key, tl);
					dump_tlv(buf + off, tl);
					putchar('\n');
					if (key == 0x02)
						note_result(buf + off, tl);
					/* 0x11 active id / 0x12 pending id:
					 * u8 length + raw id bytes */
					if ((key == 0x11 || key == 0x12) &&
					    tl >= 1 && flags == QMI_INDICATION &&
					    msgs[m].msg == 0x22) {
						unsigned idl = buf[off];

						printf("    [%s id len %u:",
						       key == 0x11 ? "active"
								   : "pending", idl);
						if (off + 1 + idl <= off + tl) {
							printf(" ");
							dump_tlv(buf + off + 1,
								 idl);
							if (key == 0x11 &&
							    msgs[m].msg == 0x22 &&
							    ctypes[c] == 1 &&
							    idl <= sizeof(active_id)) {
								memcpy(active_id,
								       buf + off + 1,
								       idl);
								active_id_len = idl;
							}
						}
						printf("]\n");
					}
					/* LIST_CONFIGS ind 0x11 = u8 count +
					 * {u32 type, u8-len id[]} elements */
					if (key == 0x11 && tl >= 1 &&
					    flags == QMI_INDICATION &&
					    msgs[m].msg == 0x24) {
						unsigned cnt = buf[off], o2 = off + 1, k;

						printf("    [%u config(s)]\n", cnt);
						for (k = 0; k < cnt &&
							     o2 + 5 <= off + tl;
						     k++) {
							uint32_t t =
								get16(buf + o2) |
								((uint32_t)get16(buf + o2 + 2) << 16);
							uint8_t idl = buf[o2 + 4];

							printf("    [cfg type %u "
							       "id_len %u id ",
							       t, idl);
							if (o2 + 5 + idl <=
							    off + tl)
								dump_tlv(buf + o2 + 5, idl);
							printf("]\n");
							o2 += 5 + idl;
						}
					}
					off += tl;
				}
				if (flags == QMI_RESPONSE) {
					got = 1;
					any = 1;
				}
			}
			if (!got)
				printf("  TIMEOUT: no response frame\n");
			txn++;
		}
	}

	/* GET_CONFIG_INFO (0x28) on the active software config — read-only;
	 * description string tells us WHICH mcfg the modem picked. */
	if (active_id_len) {
		unsigned off, end;
		int i;

		printf("== PDC GET_CONFIG_INFO ctype=1 id=<active> token=%u "
		       "==\n", txn);
		req[0] = QMI_REQUEST;
		put16(req + 1, txn);
		put16(req + 3, 0x28);
		req[7] = 0x01; put16(req + 8, 5 + active_id_len);
		put32(req + 10, 1);
		req[14] = active_id_len;
		memcpy(req + 15, active_id, active_id_len);
		req[15 + active_id_len] = 0x10;
		put16(req + 16 + active_id_len, 4);
		put32(req + 18 + active_id_len, txn);
		put16(req + 5, 15 + active_id_len);
		if (qrtr_sendto(sock, node, port, req,
				22 + active_id_len) == 0) {
			for (i = 0; i < 8; i++) {
				uint8_t flags;
				uint16_t rtxn, mid, mlen;

				len = qrtr_recvfrom(sock, buf, sizeof(buf),
						    &rn, &rp);
				if (len < 0)
					continue;
				if (rp == QRTR_PORT_CTRL || len < 7)
					continue;
				flags = buf[0];
				rtxn = get16(buf + 1);
				mid = get16(buf + 3);
				mlen = get16(buf + 5);
				if (rn != node ||
				    (flags != QMI_RESPONSE &&
				     flags != QMI_INDICATION) ||
				    mid != 0x28 || rtxn != txn)
					continue;
				printf("  %s: flags 0x%02x txn %u msg "
				       "0x%04x msg_len %u\n",
				       flags == QMI_RESPONSE ? "resp" : "ind",
				       flags, rtxn, mid, mlen);
				end = 7 + mlen;
				if (end > (unsigned)len)
					end = (unsigned)len;
				off = 7;
				while (off + 3 <= end) {
					uint8_t key = buf[off];
					uint16_t tl = get16(buf + off + 1);

					off += 3;
					if (off + tl > end)
						break;
					printf("  TLV 0x%02x len %u: ", key, tl);
					dump_tlv(buf + off, tl);
					putchar('\n');
					if (key == 0x02)
						note_result(buf + off, tl);
					/* 0x12 description: u8-len string */
					if (key == 0x12 && tl >= 1 &&
					    buf[off] <= tl - 1)
						printf("    [description: "
						       "%.*s]\n", buf[off],
						       (char *)buf + off + 1);
					off += tl;
				}
				if (flags == QMI_RESPONSE)
					any = 1;
			}
		}
		txn++;
	}

	close(sock);
	return any ? 0 : 1;
}

/* One IMS/IMSA request on an already-open client. Prints the matching
 * response and any indications that arrive first. */
static int ims_send(int sock, uint32_t node, uint32_t port, uint16_t txn,
		    uint16_t msg, const uint8_t *tlvs, uint16_t tlen,
		    const char *tag)
{
	uint8_t req[280], buf[512];
	uint32_t rn, rp;
	int n, i;

	req[0] = QMI_REQUEST;
	put16(req + 1, txn);
	put16(req + 3, msg);
	put16(req + 5, tlen);
	if (tlen)
		memcpy(req + 7, tlvs, tlen);
	printf("== %s msg 0x%04x txn %u (%u tlv bytes) ==\n",
	       tag, msg, txn, tlen);
	if (qrtr_sendto(sock, node, port, req, 7 + tlen) < 0) {
		printf("  sendto failed\n");
		return 1;
	}
	for (i = 0; i < 8; i++) {
		n = qrtr_recvfrom(sock, buf, sizeof(buf), &rn, &rp);
		if (n < 7)
			continue;
		if (rp == QRTR_PORT_CTRL)
			continue;
		if (buf[0] == QMI_INDICATION) {
			printf("  ind  msg 0x%04x len %u\n",
			       get16(buf + 3), get16(buf + 5));
			continue;
		}
		if (buf[0] == QMI_RESPONSE && get16(buf + 1) == txn &&
		    get16(buf + 3) == msg) {
			printf("  resp msg_len %u  ", get16(buf + 5));
			if (get16(buf + 5) >= 7)
				note_result(buf + 10, 4);
			else
				putchar('\n');
			return 0;
		}
	}
	printf("  no response\n");
	return 1;
}

int main(int argc, char **argv)
{
	unsigned i;
	int rc = 0, matched = 0;

	if (argc != 2 &&
	    !(argc >= 3 && !strcmp(argv[1], "wdschain") && argc <= 8) &&
	    !(argc <= 3 && !strcmp(argv[1], "enumsvc")) &&
	    !(argc == 3 && (!strcmp(argv[1], "wdsstop") ||
			    !strcmp(argv[1], "wdsprof") ||
			    !strcmp(argv[1], "wdsplist") ||
			    !strcmp(argv[1], "iccread") ||
			    !strcmp(argv[1], "wdfmt") ||
			    !strcmp(argv[1], "wdfmtqmap4") ||
			    !strcmp(argv[1], "wdfmtqmap5") ||
			    !strcmp(argv[1], "wdfmtqmap"))) &&
	    !(argc == 4 && !strcmp(argv[1], "wdsprof")) &&
	    !((argc >= 3 && argc <= 5) && !strcmp(argv[1], "wdsmux")) &&
	    !(argc >= 3 && argc <= 4 && !strcmp(argv[1], "ipa"))) {
		fprintf(stderr, "usage: %s imei|mode|online|offline|lpm|uireset|sim|slots|simon|simoff|simon2|simoff2|provision|provision2|prov0|provp|switchslot|switchback|unprovision|events|sig|serving|sysinfo|ssp|sspcs|sspps|wdsmux|wdsbind|wdsipfam|wdsstart|wdsstat|wdsget|wdsstop <handle>|wdsprof <idx> [type]|wdsplist <type>|wdsattp|wdsattn|wdsstatx|wdsmux <mux> [iface]|wdschain <hold-seconds> [nomux|port|sub1|ims|nocall|noapn|pN|qN|insN]|wdfmt|wdfmtget|wdfmtraw|wdfmtqmap5|wdfmtqmap4|wdfmtqmap|wdfmtdis|wdfmtdisn|imsareg|imsasvc|imsget|imspolicy|imsen|isimread|isimread2|isimread3|usimread|usimread2|usimread3|iccread|iccread5|usimread5|isimread5|isimread6|isimread7|isimdom|dsd|dpmmsgs|dpmopen|dpmopen1|ipa <main|hwstats|nossr|android> [R]|pdc|svcls|enumsvc [svc]|all\n", argv[0]);
		return 2;
	}

	if (!strcmp(argv[1], "uimpre")) {
		struct query q41 = { .cmd = "uim41",
			.desc = "UIM EVENT_REG", .svc = 11, .msg = 0x0041,
			.tlv_key = 0x01, .tlv_data = {1}, .tlv_len = 1 };
		struct query q2a = { .cmd = "uim2a",
			.desc = "UIM GET_CONFIGURATION", .svc = 11, .msg = 0x002A,
			.tlv_key = 0x01, .tlv_data = {7, 0}, .tlv_len = 2,
			.tlv2_key = 0x02, .tlv2_data = {1, 0, 0, 0}, .tlv2_len = 4 };
		struct query q24 = { .cmd = "uim24isim",
			.desc = "GET_FILE_ATTRIBUTES IMPI nonprov-slot2",
			.svc = 11, .msg = 0x0024,
			.tlv_key = 0x02,
			.tlv_data = {0x02, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f},
			.tlv_len = 7,
			.tlv2_key = 0x01,
			.tlv2_data = {5, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff},
			.tlv2_len = 18 };
		struct query q20 = { .cmd = "isimnp2",
			.desc = "READ EF_IMPI after EVENT_REG",
			.svc = 11, .msg = 0x0020,
			.tlv_key = 0x02,
			.tlv_data = {0x02, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f},
			.tlv_len = 7,
			.tlv2_key = 0x01,
			.tlv2_data = {5, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff},
			.tlv2_len = 18,
			.tlv3_key = 0x03, .tlv3_data = {0, 0, 0, 0}, .tlv3_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 11, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("uimpre: EVENT_REG + GET_CONFIG + GET_ATTR + READ IMPI\n");
		ask_on(sock, node, port, &q41, txn++, NULL, NULL);
		ask_on(sock, node, port, &q2a, txn++, NULL, NULL);
		ask_on(sock, node, port, &q24, txn++, NULL, NULL);
		ask_on(sock, node, port, &q20, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "wms45a")) {
		struct query q45 = { .cmd = "wms45",
			.desc = "WMS IND_REG then 0x4a", .svc = 5, .msg = 0x0045,
			.tlv_key = 0x01, .tlv_data = {1}, .tlv_len = 1 };
		struct query q4a = { .cmd = "wms4a",
			.desc = "WMS 0x4a same client after 0x45",
			.svc = 5, .msg = 0x004A };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 5, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		ask_on(sock, node, port, &q45, txn++, NULL, NULL);
		ask_on(sock, node, port, &q4a, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "wmsreg")) {
		struct query qbind = { .cmd = "wmsbind2",
			.desc = "WMS BIND sub=2", .svc = 5, .msg = 0x004F,
			.tlv_key = 0x01, .tlv_data = {2, 0, 0, 0}, .tlv_len = 4 };
		struct query q4a = { .cmd = "wms4a",
			.desc = "WMS GET_TRANSPORT_NW_REG after bind",
			.svc = 5, .msg = 0x004A };
		struct query q48 = { .cmd = "wms48",
			.desc = "WMS GET_TRANSPORT_LAYER after bind",
			.svc = 5, .msg = 0x0048 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 5, 0, &node, &port, &ins) < 0) {
			printf("  no WMS\n");
			close(sock);
			return 1;
		}
		printf("  WMS %u:%u bind2 then 0x4a/0x48\n", node, port);
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &q4a, txn++, NULL, NULL);
		ask_on(sock, node, port, &q48, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsarich")) {
		/* pmaports#1878 Richard Acayan: IMSA BIND 0x33 TLV 0x10
		 * then GET_REG on the same client. We previously used
		 * TLV 0x01. */
		struct query qbind = { .cmd = "imsabind10",
			.desc = "IMSA BIND tlv 0x10=2", .svc = 0x21, .msg = 0x0033,
			.tlv_key = 0x10, .tlv_data = {2, 0, 0, 0}, .tlv_len = 4 };
		struct query qbind0 = { .cmd = "imsabind10z",
			.desc = "IMSA BIND tlv 0x10=0 (Richard hex)",
			.svc = 0x21, .msg = 0x0033,
			.tlv_key = 0x10, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query qareg = { .cmd = "imsareg",
			.desc = "IMSA GET_REG after tlv0x10 bind",
			.svc = 0x21, .msg = 0x0020 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x21, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsarich: BIND 0x33 tlv 0x10 then GET_REG\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qareg, txn++, NULL, NULL);
		close(sock);
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x21, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		txn = 1;
		ask_on(sock, node, port, &qbind0, txn++, NULL, NULL);
		ask_on(sock, node, port, &qareg, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsa0")) {
		/* Read-only: IMSA on Binding 0 (tlv 0x10), after IMS
		 * Settings bind 0 works. IND_REG then GET_REG/GET_SVC/
		 * GET_BIND, then wait for 0x23/0x24. Not NV. */
		static const uint8_t imsa_ind[] = {
			0x10, 0x01, 0x00, 0x01,
			0x11, 0x01, 0x00, 0x01,
			0x12, 0x01, 0x00, 0x01,
			0x18, 0x01, 0x00, 0x01,
			0x19, 0x01, 0x00, 0x01,
			0x1b, 0x01, 0x00, 0x01,
		};
		struct query qbind = { .cmd = "imsabind0",
			.desc = "IMSA BIND tlv 0x10=0", .svc = 0x21, .msg = 0x0033,
			.tlv_key = 0x10, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query qgetb = { .cmd = "imsagetbind",
			.desc = "IMSA GET_BIND 0x34", .svc = 0x21, .msg = 0x0034 };
		struct query qareg = { .cmd = "imsareg",
			.desc = "IMSA GET_REG", .svc = 0x21, .msg = 0x0020 };
		struct query qasvc = { .cmd = "imsasvc",
			.desc = "IMSA GET_SVC", .svc = 0x21, .msg = 0x0021 };
		uint32_t node, port, ins, rn, rp;
		uint16_t txn = 1;
		uint8_t buf[512];
		int sock, n, i, inds = 0;

		printf("imsa0: BIND 0 + IND_REG + GET_REG/SVC/BIND, wait 8s\n");
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x21, 0, &node, &port, &ins) < 0) {
			printf("  no IMSA\n");
			close(sock);
			return 1;
		}
		printf("  IMSA %u:%u ins %u\n", node, port, ins);
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qgetb, txn++, NULL, NULL);
		ims_send(sock, node, port, txn++, 0x0022, imsa_ind,
			 (uint16_t)sizeof(imsa_ind), "IMSA IND_REG 0x22 gold");
		ask_on(sock, node, port, &qareg, txn++, NULL, NULL);
		ask_on(sock, node, port, &qasvc, txn++, NULL, NULL);
		printf("imsa0: waiting 8s for 0x23/0x24 indications\n");
		for (i = 0; i < 8; i++) {
			n = qrtr_recvfrom(sock, buf, sizeof(buf), &rn, &rp);
			if (n < 7)
				continue;
			if (rp == QRTR_PORT_CTRL)
				continue;
			if (buf[0] == QMI_INDICATION) {
				inds++;
				printf("  IND flags=0x%02x msg=0x%04x len=%u\n",
				       buf[0], get16(buf + 3), get16(buf + 5));
			}
		}
		printf("imsa0: indications during wait: %d\n", inds);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsscan")) {
		/* 358880 IMS GET_SUPPORTED_MESSAGES bits; skip SET 0x8f / missing 0x2C. */
		static const uint16_t gets[] = {
			0x0023, 0x0056, 0x005d, 0x005e, 0x0063, 0x0064, 0x0066, 0x0067,
			0x0068, 0x0069, 0x006a, 0x006c, 0x006d, 0x006f, 0x0070, 0x0072,
			0x0073, 0x0074, 0x0075, 0x0077, 0x0078, 0x007a, 0x007b, 0x007d,
			0x007e, 0x0080, 0x0081, 0x0083, 0x0084, 0x0086, 0x0087, 0x0089,
			0x008a, 0x008c, 0x008d, 0x0090, 0x0096, 0x009a
		};
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;
		unsigned i;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsscan: BIND 0 then GET every bitmap opcode except 0x8f/0x2C\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		for (i = 0; i < sizeof(gets) / sizeof(gets[0]); i++) {
			struct query q = { .cmd = "imsget",
				.desc = "IMS GET (bitmap)",
				.svc = 0x12, .msg = gets[i] };
			printf("-- GET 0x%02x --\n", gets[i]);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims72a")) {
		/* SET 0x72 variants after err1 on full GET-mirror.
		 * A: only 0x12=ims  B: only 0x10=ims  C: 0x11=u32:1 + 0x12=ims
		 * D: 0x12 with u16 length prefix  E: same A on 0x73 */
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query qa = { .cmd = "ims72a",
			.desc = "IMS 0x72 TLV 0x12=ims",
			.svc = 0x12, .msg = 0x0072,
			.tlv_key = 0x12, .tlv_data = { 'i','m','s' }, .tlv_len = 3 };
		struct query qb = { .cmd = "ims72b",
			.desc = "IMS 0x72 TLV 0x10=ims",
			.svc = 0x12, .msg = 0x0072,
			.tlv_key = 0x10, .tlv_data = { 'i','m','s' }, .tlv_len = 3 };
		struct query qc = { .cmd = "ims72c",
			.desc = "IMS 0x72 0x11=1 0x12=ims",
			.svc = 0x12, .msg = 0x0072,
			.tlv_key = 0x11, .tlv_data = {1, 0, 0, 0}, .tlv_len = 4,
			.tlv2_key = 0x12, .tlv2_data = { 'i','m','s' }, .tlv2_len = 3 };
		struct query qd = { .cmd = "ims72d",
			.desc = "IMS 0x72 0x12 u16len+ims",
			.svc = 0x12, .msg = 0x0072,
			.tlv_key = 0x12, .tlv_data = { 3, 0, 'i','m','s' }, .tlv_len = 5 };
		struct query qe = { .cmd = "ims73e",
			.desc = "IMS 0x73 TLV 0x12=ims",
			.svc = 0x12, .msg = 0x0073,
			.tlv_key = 0x12, .tlv_data = { 'i','m','s' }, .tlv_len = 3 };
		struct query qget = { .cmd = "ims73get",
			.desc = "IMS GET 0x73",
			.svc = 0x12, .msg = 0x0073 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("ims72a: BIND 0 then 0x72/0x73 SET variants, GET 0x73\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qa, txn++, NULL, NULL);
		ask_on(sock, node, port, &qb, txn++, NULL, NULL);
		ask_on(sock, node, port, &qc, txn++, NULL, NULL);
		ask_on(sock, node, port, &qd, txn++, NULL, NULL);
		ask_on(sock, node, port, &qe, txn++, NULL, NULL);
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imspriv")) {
		/* IMS-related QRTR services besides 0x12/0x21: libqmi
		 * IMSVT=0x20 IMSRTP=0x28 IMSP=0x1F; cnss2 IMSPRIVATE=0x4D
		 * (WFC 0x3E/0x40). DSD/VOICE 0x1E check OpenIMSd extras
		 * still in bitmap. GET 0x1E only — no SET. */
		static const uint32_t svcs[] = { 0x20, 0x28, 0x4d, 0x1f, 0x2a, 9 };
		static const char *names[] = {
			"IMSVT", "IMSRTP", "IMSPRIV", "IMSP", "DSD", "VOICE"
		};
		uint16_t txn = 1;
		int sock;
		unsigned i;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		printf("imspriv: GET_SUPPORTED_MESSAGES 0x1E on IMSVT/RTP/PRIV/IMSP/DSD/VOICE\n");
		for (i = 0; i < sizeof(svcs) / sizeof(svcs[0]); i++) {
			struct query q = { .cmd = "ims1e",
				.desc = "GET_SUPPORTED_MESSAGES",
				.svc = svcs[i], .msg = 0x001E };
			uint32_t node, port, ins;

			if (lookup_service(sock, svcs[i], 0, &node, &port, &ins) < 0) {
				printf("  %s svc 0x%02x ABSENT\n", names[i], svcs[i]);
				continue;
			}
			printf("  %s svc 0x%02x %u:%u ins %u\n",
			       names[i], svcs[i], node, port, ins);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imspcscf")) {
		/* CafeTele gate: no P-CSCF => no SIP. libqmi mask with
		 * PCSCF bits = 0x4FF30 (HARDWARE 2026-09-15). Same client
		 * BIND_MUX then GET 0x2D. mux 2 = qmapmux0.0, 3 = 0.1. */
		static const uint8_t mask[4] = { 0x30, 0xff, 0x04, 0x00 };
		struct query qbindsub = { .cmd = "wdsbind",
			.desc = "WDS BIND_SUB primary", .svc = 1, .msg = 0x00AF,
			.tlv_key = 0x01, .tlv_data = {1, 0, 0, 0}, .tlv_len = 4 };
		struct query qget = { .cmd = "wdsget",
			.desc = "WDS GET_CURRENT_SETTINGS pcscf mask",
			.svc = 1, .msg = 0x002D,
			.tlv_key = 0x10, .tlv_data = { 0x30, 0xff, 0x04, 0x00 },
			.tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock, mux;

		(void)mask;
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 1, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imspcscf: BIND_SUB + BIND_MUX 2/3 + GET 0x2D mask 0x4FF30\n");
		ask_on(sock, node, port, &qbindsub, txn++, NULL, NULL);
		for (mux = 2; mux <= 3; mux++) {
			struct query qmux = { .cmd = "wdsmux",
				.desc = "WDS BIND_MUX",
				.svc = 1, .msg = 0x00A2,
				.tlv_key = 0x10,
				.tlv_data = {4, 0, 0, 0, 1, 0, 0, 0},
				.tlv_len = 8,
				.tlv2_key = 0x11,
				.tlv2_data = { (uint8_t)mux },
				.tlv2_len = 1 };
			printf("-- mux %d --\n", mux);
			ask_on(sock, node, port, &qmux, txn++, NULL, NULL);
			ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsarg6")) {
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsarg6: 0x96 0x01=0 plus 0x10; 0x10 empty; 0x01=0xff\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		{
			struct query a = { .cmd = "a", .desc = "0x96 0x01 u8=0 + 0x10 u32=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x01, .tlv_data = {0}, .tlv_len = 1,
				.tlv2_key = 0x10, .tlv2_data = {0, 0, 0, 0}, .tlv2_len = 4 };
			struct query b = { .cmd = "b", .desc = "0x96 0x01 u8=0 + 0x10 u8=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x01, .tlv_data = {0}, .tlv_len = 1,
				.tlv2_key = 0x10, .tlv2_data = {0}, .tlv2_len = 1 };
			struct query c = { .cmd = "c", .desc = "0x96 0x10 empty",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x10, .tlv_len = 0 };
			struct query d = { .cmd = "d", .desc = "0x96 0x01 u8=0xff",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x01, .tlv_data = {0xff}, .tlv_len = 1 };
			struct query e = { .cmd = "e", .desc = "0x96 0x01 empty",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x01, .tlv_len = 0 };
			printf("-- 0x01=0 + 0x10 u32=0 --\n");
			ask_on(sock, node, port, &a, txn++, NULL, NULL);
			printf("-- 0x01=0 + 0x10 u8=0 --\n");
			ask_on(sock, node, port, &b, txn++, NULL, NULL);
			printf("-- 0x10 empty --\n");
			ask_on(sock, node, port, &c, txn++, NULL, NULL);
			printf("-- 0x01=0xff --\n");
			ask_on(sock, node, port, &d, txn++, NULL, NULL);
			printf("-- 0x01 empty --\n");
			ask_on(sock, node, port, &e, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsarg5")) {
		/* 0x96: tags 0x10/0x11 are known (err1 on u8) — try u16/u32/u64.
		 * 0x10-as-u32 is the WDS GET requested-settings pattern. */
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsarg5: BIND 0, 0x96 0x10/0x11 width sweep\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		{
			struct query a = { .cmd = "a", .desc = "0x96 0x10 u16=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x10, .tlv_data = {0, 0}, .tlv_len = 2 };
			struct query b = { .cmd = "b", .desc = "0x96 0x10 u32=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x10, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
			struct query c = { .cmd = "c", .desc = "0x96 0x10 u32=1",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x10, .tlv_data = {1, 0, 0, 0}, .tlv_len = 4 };
			struct query d = { .cmd = "d", .desc = "0x96 0x10 u32=all1",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x10,
				.tlv_data = {0xff, 0xff, 0xff, 0xff}, .tlv_len = 4 };
			struct query e = { .cmd = "e", .desc = "0x96 0x10 u64=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x10,
				.tlv_data = {0, 0, 0, 0, 0, 0, 0, 0}, .tlv_len = 8 };
			struct query f = { .cmd = "f", .desc = "0x96 0x11 u16=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x11, .tlv_data = {0, 0}, .tlv_len = 2 };
			struct query g = { .cmd = "g", .desc = "0x96 0x11 u32=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x11, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
			struct query h = { .cmd = "h", .desc = "0x96 0x11 u8=1",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x11, .tlv_data = {1}, .tlv_len = 1 };
			struct query i = { .cmd = "i", .desc = "0x96 0x10 u32=0 + 0x11 u8=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x10, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4,
				.tlv2_key = 0x11, .tlv2_data = {0}, .tlv2_len = 1 };
			printf("-- 0x96 0x10 u16=0 --\n");
			ask_on(sock, node, port, &a, txn++, NULL, NULL);
			printf("-- 0x96 0x10 u32=0 --\n");
			ask_on(sock, node, port, &b, txn++, NULL, NULL);
			printf("-- 0x96 0x10 u32=1 --\n");
			ask_on(sock, node, port, &c, txn++, NULL, NULL);
			printf("-- 0x96 0x10 u32=ffffffff --\n");
			ask_on(sock, node, port, &d, txn++, NULL, NULL);
			printf("-- 0x96 0x10 u64=0 --\n");
			ask_on(sock, node, port, &e, txn++, NULL, NULL);
			printf("-- 0x96 0x11 u16=0 --\n");
			ask_on(sock, node, port, &f, txn++, NULL, NULL);
			printf("-- 0x96 0x11 u32=0 --\n");
			ask_on(sock, node, port, &g, txn++, NULL, NULL);
			printf("-- 0x96 0x11 u8=1 --\n");
			ask_on(sock, node, port, &h, txn++, NULL, NULL);
			printf("-- 0x96 0x10 u32=0 + 0x11 u8=0 --\n");
			ask_on(sock, node, port, &i, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsarg4")) {
		/* GET-side selectors: 0x67/0x8a already SUCCESS empty.
		 * 0x96: scan TLV tags 0x02-0x16 as u8=0. No value SET on 0x66/0x89. */
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;
		unsigned v, t;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsarg4: BIND 0, GET 0x67/0x8a + 0x01, 0x96 tag scan\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		for (v = 0; v <= 8; v++) {
			struct query q = { .cmd = "ims67s",
				.desc = "IMS GET 0x67 0x01 u8",
				.svc = 0x12, .msg = 0x0067,
				.tlv_key = 0x01,
				.tlv_data = { (uint8_t)v }, .tlv_len = 1 };
			printf("-- GET 0x67 tlv01-u8=%u --\n", v);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		for (v = 0; v <= 2; v++) {
			struct query q = { .cmd = "ims8as",
				.desc = "IMS GET 0x8a 0x01 u8",
				.svc = 0x12, .msg = 0x008a,
				.tlv_key = 0x01,
				.tlv_data = { (uint8_t)v }, .tlv_len = 1 };
			printf("-- GET 0x8a tlv01-u8=%u --\n", v);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		{
			struct query q9 = { .cmd = "ims9as",
				.desc = "IMS GET 0x9a 0x01 u8=0",
				.svc = 0x12, .msg = 0x009a,
				.tlv_key = 0x01, .tlv_data = {0}, .tlv_len = 1 };
			printf("-- GET 0x9a tlv01-u8=0 --\n");
			ask_on(sock, node, port, &q9, txn++, NULL, NULL);
		}
		for (t = 0x02; t <= 0x16; t++) {
			struct query q = { .cmd = "ims96t",
				.desc = "IMS 0x96 tag scan u8=0",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = (uint8_t)t,
				.tlv_data = {0}, .tlv_len = 1 };
			printf("-- 0x96 tlv%02x-u8=0 --\n", t);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsarg3")) {
		/* Which TLV tag is required? 0x11 mirrors GET 0x8a/0x9a. */
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;
		unsigned i;
		static const uint16_t miss[] = { 0x0066, 0x0089, 0x0096 };

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsarg3: BIND 0, 0x11-only and 0x01-len0 on miss opcodes\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		for (i = 0; i < sizeof(miss) / sizeof(miss[0]); i++) {
			struct query q11 = { .cmd = "ims11",
				.desc = "IMS tlv 0x11 u8=0",
				.svc = 0x12, .msg = miss[i],
				.tlv_key = 0x11, .tlv_data = {0}, .tlv_len = 1 };
			struct query q11z = { .cmd = "ims11z",
				.desc = "IMS tlv 0x11 empty",
				.svc = 0x12, .msg = miss[i],
				.tlv_key = 0x11, .tlv_len = 0 };
			struct query q12 = { .cmd = "ims12",
				.desc = "IMS tlv 0x12 u8=0",
				.svc = 0x12, .msg = miss[i],
				.tlv_key = 0x12, .tlv_data = {0}, .tlv_len = 1 };
			printf("-- 0x%02x tlv11-u8=0 --\n", miss[i]);
			ask_on(sock, node, port, &q11, txn++, NULL, NULL);
			printf("-- 0x%02x tlv11-empty --\n", miss[i]);
			ask_on(sock, node, port, &q11z, txn++, NULL, NULL);
			printf("-- 0x%02x tlv12-u8=0 --\n", miss[i]);
			ask_on(sock, node, port, &q12, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsarg2")) {
		/* Follow-up: re-GET pair after 0x66/0x89 0x01-u8 SUCCESS;
		 * widen 0x66/0x96 u8 range. No value TLVs. */
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		static const uint16_t reget[] = {
			0x0066, 0x0067, 0x0068, 0x0089, 0x008a, 0x008d, 0x0096, 0x009a
		};
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;
		unsigned i, v;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsarg2: BIND 0, re-GET pairs, 0x66/0x96 u8 0-8\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		for (i = 0; i < sizeof(reget) / sizeof(reget[0]); i++) {
			struct query q = { .cmd = "imsreget",
				.desc = "IMS re-GET after selectors",
				.svc = 0x12, .msg = reget[i] };
			printf("-- reget 0x%02x --\n", reget[i]);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		for (v = 0; v <= 8; v++) {
			struct query q = { .cmd = "ims66c",
				.desc = "IMS 0x66 0x01 u8 codec",
				.svc = 0x12, .msg = 0x0066,
				.tlv_key = 0x01,
				.tlv_data = { (uint8_t)v }, .tlv_len = 1 };
			printf("-- 0x66 tlv01-u8=%u --\n", v);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		for (v = 0; v <= 8; v++) {
			struct query q = { .cmd = "ims96c",
				.desc = "IMS 0x96 0x01 u8",
				.svc = 0x12, .msg = 0x0096,
				.tlv_key = 0x01,
				.tlv_data = { (uint8_t)v }, .tlv_len = 1 };
			printf("-- 0x96 tlv01-u8=%u --\n", v);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsargp")) {
		/* 0x66/0x89/0x96 were err17 empty. Probe GET_SUPPORTED_FIELDS
		 * (0x1F + msg id) then selector-only TLVs. No enable bits. */
		static const uint16_t miss[] = { 0x0066, 0x0089, 0x0096 };
		static const uint16_t neigh[] = {
			0x0065, 0x0067, 0x0068, 0x0088, 0x008a, 0x008c, 0x008d,
			0x0095, 0x0097, 0x009a
		};
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;
		unsigned i, v;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsargp: BIND 0, empty+0x1F+selector for 0x66/0x89/0x96\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		for (i = 0; i < sizeof(miss) / sizeof(miss[0]); i++) {
			struct query q = { .cmd = "imsempty",
				.desc = "IMS empty (err17 confirm)",
				.svc = 0x12, .msg = miss[i] };
			printf("-- empty 0x%02x --\n", miss[i]);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		for (i = 0; i < sizeof(miss) / sizeof(miss[0]); i++) {
			struct query q = { .cmd = "ims1f",
				.desc = "IMS GET_SUPPORTED_FIELDS",
				.svc = 0x12, .msg = 0x001F,
				.tlv_key = 0x01,
				.tlv_data = { (uint8_t)miss[i],
					      (uint8_t)(miss[i] >> 8) },
				.tlv_len = 2 };
			printf("-- 0x1F fields for 0x%02x --\n", miss[i]);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		for (i = 0; i < sizeof(neigh) / sizeof(neigh[0]); i++) {
			struct query q = { .cmd = "imsneigh",
				.desc = "IMS neighbor empty",
				.svc = 0x12, .msg = neigh[i] };
			printf("-- neigh empty 0x%02x --\n", neigh[i]);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		for (i = 0; i < sizeof(miss) / sizeof(miss[0]); i++) {
			for (v = 0; v <= 2; v++) {
				struct query q = { .cmd = "imssel",
					.desc = "IMS 0x01 u8 selector",
					.svc = 0x12, .msg = miss[i],
					.tlv_key = 0x01,
					.tlv_data = { (uint8_t)v },
					.tlv_len = 1 };
				printf("-- 0x%02x tlv01-u8=%u --\n",
				       miss[i], v);
				ask_on(sock, node, port, &q, txn++, NULL, NULL);
			}
			{
				struct query q16 = { .cmd = "imssel16",
					.desc = "IMS 0x01 u16=0",
					.svc = 0x12, .msg = miss[i],
					.tlv_key = 0x01,
					.tlv_data = {0, 0}, .tlv_len = 2 };
				struct query q32 = { .cmd = "imssel32",
					.desc = "IMS 0x01 u32=0",
					.svc = 0x12, .msg = miss[i],
					.tlv_key = 0x01,
					.tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
				struct query q10 = { .cmd = "imssel10",
					.desc = "IMS 0x10 u8=0",
					.svc = 0x12, .msg = miss[i],
					.tlv_key = 0x10,
					.tlv_data = {0}, .tlv_len = 1 };
				printf("-- 0x%02x tlv01-u16=0 --\n", miss[i]);
				ask_on(sock, node, port, &q16, txn++, NULL, NULL);
				printf("-- 0x%02x tlv01-u32=0 --\n", miss[i]);
				ask_on(sock, node, port, &q32, txn++, NULL, NULL);
				printf("-- 0x%02x tlv10-u8=0 --\n", miss[i]);
				ask_on(sock, node, port, &q10, txn++, NULL, NULL);
			}
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims72s")) {
		/* SET 0x72 mirroring GET 0x73 TLVs, APN CTNET -> ims.
		 * Bitmap has 0x72; empty 0x72 is SUCCESS (SET no-op). */
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query qset = { .cmd = "ims72set",
			.desc = "IMS SET 0x72 APN=ims (mirror 0x73)",
			.svc = 0x12, .msg = 0x0072,
			.tlv_key = 0x11, .tlv_data = {1, 0, 0, 0}, .tlv_len = 4,
			.tlv2_key = 0x12, .tlv2_data = { 'i','m','s' }, .tlv2_len = 3,
			.tlv3_key = 0x13, .tlv3_data = {0, 0, 0, 0}, .tlv3_len = 4,
			.tlv4_key = 0x14, .tlv4_len = 0 };
		struct query qget = { .cmd = "ims73get",
			.desc = "IMS GET 0x73 after SET 0x72",
			.svc = 0x12, .msg = 0x0073 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("ims72s: BIND 0, SET 0x72 APN=ims, GET 0x73\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qset, txn++, NULL, NULL);
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims73p")) {
		/* Pairing probe for GET 0x73 CTNET. Empty requests only:
		 * odd/even SET vs GET is told by err17 (missing args) vs a
		 * payload. Does not SET APN. */
		static const uint16_t gets[] = {
			0x0023, 0x006f, 0x0070, 0x0072, 0x0073, 0x0074, 0x0075
		};
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;
		unsigned i;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("ims73p: BIND 0 then empty GET 0x23/0x6f/0x70/0x72-0x75\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		for (i = 0; i < sizeof(gets) / sizeof(gets[0]); i++) {
			struct query q = { .cmd = "imsget",
				.desc = "IMS empty (pairing)",
				.svc = 0x12, .msg = gets[i] };
			printf("-- empty 0x%02x --\n", gets[i]);
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims28p")) {
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query q28 = { .cmd = "ims28",
			.desc = "IMS GET 0x28 after bind0",
			.svc = 0x12, .msg = 0x0028 };
		struct query q2a = { .cmd = "ims2a",
			.desc = "IMS GET 0x2A after bind0",
			.svc = 0x12, .msg = 0x002A };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("ims28p: BIND 0 then GET 0x28 and 0x2A (bitmap bits, not 0x2C)\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &q28, txn++, NULL, NULL);
		ask_on(sock, node, port, &q2a, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims48p")) {
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND sub=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query qpolg = { .cmd = "imspolicy",
			.desc = "IMS GET_POLICY 0x48 after bind0",
			.svc = 0x12, .msg = 0x0048 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("ims48p: BIND 0 then GET 0x48\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qpolg, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imsgetpol")) {
		struct query qbind = { .cmd = "imsbind2",
			.desc = "IMS BIND sub=2", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {2, 0, 0, 0}, .tlv_len = 4 };
		struct query qpolg = { .cmd = "imspolicy",
			.desc = "IMS GET_POLICY 0x48 after bind2",
			.svc = 0x12, .msg = 0x0048 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("imsgetpol: BIND 2 then GET 0x48\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qpolg, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims8f1")) {
		static const uint8_t tlv10[] = { 0x10, 0x01, 0x00, 0x01 };
		struct query qbind = { .cmd = "imsbind1",
			.desc = "IMS BIND sub=1", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {1, 0, 0, 0}, .tlv_len = 4 };
		struct query qget = { .cmd = "imsget",
			.desc = "IMS GET after bind1", .svc = 0x12, .msg = 0x0090 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock;

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		printf("ims8f1: BIND sub=1 then SET 0x8f tlv 0x10=1\n");
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ims_send(sock, node, port, txn++, 0x008f, tlv10, 4, "SET 0x8f voice bind1");
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims8f")) {
		/* Replay qcrild IMS Settings 0x8f from the OpenIMSd
		 * OnePlus 6T VoLTE registration pcap: one TLV per SET,
		 * same client, BIND subscription 2 first (this SIM is
		 * physical slot 2). Then IMSA bind+ind-register+GET. */
		static const struct {
			const char *tag;
			uint8_t tlv[8];
			uint16_t n;
		} steps[] = {
			{ "SET 0x8f tlv 0x15=2 (call-mode)", { 0x15, 0x04, 0x00, 0x02, 0x00, 0x00, 0x00 }, 7 },
			{ "SET 0x8f tlv 0x23=0",             { 0x23, 0x01, 0x00, 0x00 }, 4 },
			{ "SET 0x8f tlv 0x10=1 (voice)",     { 0x10, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x14=1",             { 0x14, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x11=1",             { 0x11, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x19=1",             { 0x19, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x18=1",             { 0x18, 0x01, 0x00, 0x01 }, 4 },
		};
		static const uint8_t imsa_ind[] = {
			0x10, 0x01, 0x00, 0x01,
			0x11, 0x01, 0x00, 0x01,
			0x12, 0x01, 0x00, 0x01,
			0x18, 0x01, 0x00, 0x01,
			0x19, 0x01, 0x00, 0x01,
			0x1b, 0x01, 0x00, 0x01,
		};
		struct query qbind = { .cmd = "imsbind2",
			.desc = "IMS BIND sub=2", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {2, 0, 0, 0}, .tlv_len = 4 };
		struct query qget = { .cmd = "imsget",
			.desc = "IMS GET_SERVICES_ENABLED after 0x8f",
			.svc = 0x12, .msg = 0x0090 };
		struct query qabind = { .cmd = "imsabind2",
			.desc = "IMSA BIND sub=2", .svc = 0x21, .msg = 0x0033,
			.tlv_key = 0x01, .tlv_data = {2, 0, 0, 0}, .tlv_len = 4 };
		struct query qareg = { .cmd = "imsareg",
			.desc = "IMSA GET_REG after 0x8f", .svc = 0x21,
			.msg = 0x0020 };
		struct query qasvc = { .cmd = "imsasvc",
			.desc = "IMSA GET_SVC after 0x8f", .svc = 0x21,
			.msg = 0x0021 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock, s;

		printf("ims8f: qcrild gold 0x8f sequence, bind sub=2\n");
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			printf("  no IMS server\n");
			close(sock);
			return 1;
		}
		printf("  IMS %u:%u\n", node, port);
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		for (s = 0; s < (int)(sizeof(steps) / sizeof(steps[0])); s++)
			ims_send(sock, node, port, txn++, 0x008f,
				 steps[s].tlv, steps[s].n, steps[s].tag);
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		close(sock);

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x21, 0, &node, &port, &ins) < 0) {
			printf("  no IMSA\n");
			close(sock);
			return 1;
		}
		printf("  IMSA %u:%u\n", node, port);
		txn = 1;
		ask_on(sock, node, port, &qabind, txn++, NULL, NULL);
		ims_send(sock, node, port, txn++, 0x0022, imsa_ind,
			 (uint16_t)sizeof(imsa_ind), "IMSA IND_REG 0x22");
		ask_on(sock, node, port, &qareg, txn++, NULL, NULL);
		ask_on(sock, node, port, &qasvc, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims98")) {
		/* Richard pmaports#1878: IMSA BIND is 0x33 TLV 0x10=0;
		 * "corresponding" IMS Settings BIND is 0x98 with the same
		 * TLV. libqmi 0x98 uses TLV 0x01 subscription (already
		 * SUCCESS + 0x90 still 70). This command tries TLV 0x10
		 * then gold one-TLV 0x8f on that client. Not NV. */
		static const struct {
			const char *tag;
			uint8_t tlv[8];
			uint16_t n;
		} steps[] = {
			{ "SET 0x8f tlv 0x15=2 (call-mode)", { 0x15, 0x04, 0x00, 0x02, 0x00, 0x00, 0x00 }, 7 },
			{ "SET 0x8f tlv 0x23=0",             { 0x23, 0x01, 0x00, 0x00 }, 4 },
			{ "SET 0x8f tlv 0x10=1 (voice)",     { 0x10, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x14=1",             { 0x14, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x11=1",             { 0x11, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x19=1",             { 0x19, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x18=1",             { 0x18, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x1A=1 (sms)",       { 0x1A, 0x01, 0x00, 0x01 }, 4 },
		};
		struct query qbind10 = { .cmd = "ims98t10",
			.desc = "IMS BIND 0x98 tlv 0x10=0 (Richard corresponding)",
			.svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x10, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query qbind10b = { .cmd = "ims98t10b",
			.desc = "IMS BIND 0x98 tlv 0x10=2",
			.svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x10, .tlv_data = {2, 0, 0, 0}, .tlv_len = 4 };
		struct query qget = { .cmd = "imsget",
			.desc = "IMS GET 0x90", .svc = 0x12, .msg = 0x0090 };
		struct query qabind = { .cmd = "imsabind10z",
			.desc = "IMSA BIND tlv 0x10=0", .svc = 0x21, .msg = 0x0033,
			.tlv_key = 0x10, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query qareg = { .cmd = "imsareg",
			.desc = "IMSA GET_REG", .svc = 0x21, .msg = 0x0020 };
		struct query qasvc = { .cmd = "imsasvc",
			.desc = "IMSA GET_SVC", .svc = 0x21, .msg = 0x0021 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock, s;

		printf("ims98: 0x98 tlv 0x10=0 then 0x90 then gold 0x8f\n");
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			printf("  no IMS server\n");
			close(sock);
			return 1;
		}
		printf("  IMS %u:%u\n", node, port);
		ask_on(sock, node, port, &qbind10, txn++, NULL, NULL);
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		for (s = 0; s < (int)(sizeof(steps) / sizeof(steps[0])); s++)
			ims_send(sock, node, port, txn++, 0x008f,
				 steps[s].tlv, steps[s].n, steps[s].tag);
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		close(sock);

		printf("ims98: 0x98 tlv 0x10=2 then 0x90 (separate client)\n");
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		txn = 1;
		ask_on(sock, node, port, &qbind10b, txn++, NULL, NULL);
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		close(sock);

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x21, 0, &node, &port, &ins) < 0) {
			printf("  no IMSA\n");
			close(sock);
			return 1;
		}
		printf("  IMSA %u:%u after 0x98/0x8f\n", node, port);
		txn = 1;
		ask_on(sock, node, port, &qabind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qareg, txn++, NULL, NULL);
		ask_on(sock, node, port, &qasvc, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims90p")) {
		/* Read-only: why 0x90 stays 70 after 0x98. No NV.
		 * 1) GET_SUPPORTED_MESSAGES
		 * 2) BIND 0x98 tlv 0x01 = 0/1/2 then GET 0x90 (new client each)
		 * 3) IMSA 0x33 tlv 0x10=0 + 0x22 gold, then IMS BIND 2 + 0x90
		 */
		static const uint8_t imsa_ind[] = {
			0x10, 0x01, 0x00, 0x01,
			0x11, 0x01, 0x00, 0x01,
			0x12, 0x01, 0x00, 0x01,
			0x18, 0x01, 0x00, 0x01,
			0x19, 0x01, 0x00, 0x01,
			0x1b, 0x01, 0x00, 0x01,
		};
		struct query qmsgs = { .cmd = "imsmsgs",
			.desc = "IMS GET_SUPPORTED_MESSAGES", .svc = 0x12, .msg = 0x001E };
		struct query qget = { .cmd = "imsget",
			.desc = "IMS GET 0x90", .svc = 0x12, .msg = 0x0090 };
		struct query qpol = { .cmd = "imspolicy",
			.desc = "IMS GET 0x48", .svc = 0x12, .msg = 0x0048 };
		struct query qabind = { .cmd = "imsabind10z",
			.desc = "IMSA BIND tlv 0x10=0", .svc = 0x21, .msg = 0x0033,
			.tlv_key = 0x10, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn;
		int sock, b;

		printf("ims90p: supported-msgs then BIND 0/1/2 + GET 0x90\n");
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			printf("  no IMS\n");
			close(sock);
			return 1;
		}
		printf("  IMS %u:%u ins %u\n", node, port, ins);
		txn = 1;
		ask_on(sock, node, port, &qmsgs, txn++, NULL, NULL);
		close(sock);

		for (b = 0; b <= 2; b++) {
			struct query qbind = { .cmd = "imsbindn",
				.desc = "IMS BIND 0x98", .svc = 0x12, .msg = 0x0098,
				.tlv_key = 0x01,
				.tlv_data = { (uint8_t)b, 0, 0, 0 }, .tlv_len = 4 };

			qbind.desc = (b == 0) ? "IMS BIND tlv0x01=0" :
				     (b == 1) ? "IMS BIND tlv0x01=1" :
						"IMS BIND tlv0x01=2";
			sock = qrtr_open(0);
			if (sock < 0)
				return 1;
			if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
				close(sock);
				return 1;
			}
			txn = 1;
			printf("ims90p: bind=%u then 0x90/0x48\n", b);
			ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
			ask_on(sock, node, port, &qget, txn++, NULL, NULL);
			ask_on(sock, node, port, &qpol, txn++, NULL, NULL);
			close(sock);
		}

		printf("ims90p: IMSA 0x33+0x22 then IMS bind2+0x90\n");
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x21, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		txn = 1;
		ask_on(sock, node, port, &qabind, txn++, NULL, NULL);
		ims_send(sock, node, port, txn++, 0x0022, imsa_ind,
			 (uint16_t)sizeof(imsa_ind), "IMSA IND_REG 0x22 gold");
		close(sock);

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			close(sock);
			return 1;
		}
		{
			struct query qbind2 = { .cmd = "imsbind2",
				.desc = "IMS BIND tlv0x01=2 after IMSA init",
				.svc = 0x12, .msg = 0x0098,
				.tlv_key = 0x01, .tlv_data = {2, 0, 0, 0},
				.tlv_len = 4 };
			txn = 1;
			ask_on(sock, node, port, &qbind2, txn++, NULL, NULL);
			ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ims8f0")) {
		/* Read-only-ish: BIND 0x98 tlv 0x01=0 (the only bind
		 * that makes GET 0x90 succeed), then SET 0x8f of bits
		 * GET already showed as 1. Not NV. */
		static const struct {
			const char *tag;
			uint8_t tlv[8];
			uint16_t n;
		} steps[] = {
			{ "SET 0x8f tlv 0x10=1 (voice, already on)", { 0x10, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x1A=1 (sms, already on)",   { 0x1A, 0x01, 0x00, 0x01 }, 4 },
			{ "SET 0x8f tlv 0x18=1 (ims, already on)",   { 0x18, 0x01, 0x00, 0x01 }, 4 },
		};
		struct query qbind = { .cmd = "imsbind0",
			.desc = "IMS BIND tlv0x01=0", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {0, 0, 0, 0}, .tlv_len = 4 };
		struct query qget = { .cmd = "imsget",
			.desc = "IMS GET 0x90", .svc = 0x12, .msg = 0x0090 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock, s;

		printf("ims8f0: BIND 0 then SET already-1 bits, no NV\n");
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			printf("  no IMS\n");
			close(sock);
			return 1;
		}
		printf("  IMS %u:%u\n", node, port);
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		for (s = 0; s < (int)(sizeof(steps) / sizeof(steps[0])); s++)
			ims_send(sock, node, port, txn++, 0x008f,
				 steps[s].tlv, steps[s].n, steps[s].tag);
		ask_on(sock, node, port, &qget, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imscfg")) {
		/* One-shot: bind 2, GET/SET policy 0x48/0x47, SET_USER_CONFIG
		 * 0x2C with IMSI-derived IMPI, IMSA bind 2 + GET_REG.
		 * Stop after this regardless of err70. */
		static const char domain[] = "ims.mnc011.mcc460.3gppnetwork.org";
		static const char impi[] =
			"460110440364089@ims.mnc011.mcc460.3gppnetwork.org";
		static const char impu[] =
			"sip:460110440364089@ims.mnc011.mcc460.3gppnetwork.org";
		uint8_t tlvs[256], req[280], buf[512];
		uint16_t tlen, txn = 1;
		uint32_t node, port, ins, rn, rp, prio;
		int sock, i, n;
		struct query qbind = { .cmd = "imsbind2",
			.desc = "IMS BIND sub=2", .svc = 0x12, .msg = 0x0098,
			.tlv_key = 0x01, .tlv_data = {2, 0, 0, 0}, .tlv_len = 4 };
		struct query qpolg = { .cmd = "imspolicy",
			.desc = "IMS GET_POLICY after bind2", .svc = 0x12,
			.msg = 0x0048 };
		struct query qabind = { .cmd = "imsabind2",
			.desc = "IMSA BIND sub=2", .svc = 0x21, .msg = 0x0033,
			.tlv_key = 0x01, .tlv_data = {2, 0, 0, 0}, .tlv_len = 4 };
		struct query qareg = { .cmd = "imsareg",
			.desc = "IMSA GET_REG after cfg", .svc = 0x21,
			.msg = 0x0020 };
		struct query qasvc = { .cmd = "imsasvc",
			.desc = "IMSA GET_SVC after cfg", .svc = 0x21,
			.msg = 0x0021 };

		printf("imscfg: IMPI=%s\n", impi);
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			printf("  no IMS server\n");
			close(sock);
			return 1;
		}
		printf("  IMS %u:%u\n", node, port);
		ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qpolg, txn++, NULL, NULL);

		/* SET 0x0047: ACS/ISIM/NV/PCO priorities + APN ims */
		tlen = 0;
		prio = 2; memcpy(tlvs + tlen, "\x15\x04\x00", 3); tlen += 3;
		put32(tlvs + tlen, prio); tlen += 4;		/* ACS=2 */
		prio = 0; memcpy(tlvs + tlen, "\x16\x04\x00", 3); tlen += 3;
		put32(tlvs + tlen, prio); tlen += 4;		/* ISIM=0 */
		prio = 1; memcpy(tlvs + tlen, "\x17\x04\x00", 3); tlen += 3;
		put32(tlvs + tlen, prio); tlen += 4;		/* NV=1 */
		prio = 3; memcpy(tlvs + tlen, "\x18\x04\x00", 3); tlen += 3;
		put32(tlvs + tlen, prio); tlen += 4;		/* PCO=3 */
		tlvs[tlen] = 0x1A; put16(tlvs + tlen + 1, 3);
		memcpy(tlvs + tlen + 3, "ims", 3); tlen += 6;
		req[0] = QMI_REQUEST;
		put16(req + 1, txn);
		put16(req + 3, 0x0047);
		put16(req + 5, tlen);
		memcpy(req + 7, tlvs, tlen);
		printf("== SET_POL_MGR 0x0047 (%u tlv bytes) ==\n", tlen);
		qrtr_sendto(sock, node, port, req, 7 + tlen);
		for (i = 0; i < 6; i++) {
			n = qrtr_recvfrom(sock, buf, sizeof(buf), &rn, &rp);
			if (n >= 7 && buf[0] == QMI_RESPONSE &&
			    get16(buf + 1) == txn && get16(buf + 3) == 0x0047) {
				printf("  resp msg_len %u  ", get16(buf + 5));
				note_result(buf + 10, 4);
				break;
			}
		}
		if (i == 6)
			printf("  no SET_POL_MGR response\n");
		txn++;

		/* SET_USER_CONFIG 0x002C: domain, IMPI, IMPU */
		tlen = 0;
		tlvs[tlen] = 0x10; put16(tlvs + tlen + 1, (uint16_t)strlen(domain));
		memcpy(tlvs + tlen + 3, domain, strlen(domain));
		tlen += 3 + (uint16_t)strlen(domain);
		tlvs[tlen] = 0x11; put16(tlvs + tlen + 1, (uint16_t)strlen(impi));
		memcpy(tlvs + tlen + 3, impi, strlen(impi));
		tlen += 3 + (uint16_t)strlen(impi);
		tlvs[tlen] = 0x12; put16(tlvs + tlen + 1, (uint16_t)strlen(impu));
		memcpy(tlvs + tlen + 3, impu, strlen(impu));
		tlen += 3 + (uint16_t)strlen(impu);
		req[0] = QMI_REQUEST;
		put16(req + 1, txn);
		put16(req + 3, 0x002C);
		put16(req + 5, tlen);
		memcpy(req + 7, tlvs, tlen);
		printf("== SET_USER_CONFIG 0x002C domain+impi+impu ==\n");
		qrtr_sendto(sock, node, port, req, 7 + tlen);
		for (i = 0; i < 6; i++) {
			uint32_t rn, rp;
			n = qrtr_recvfrom(sock, buf, sizeof(buf), &rn, &rp);
			if (n >= 7 && buf[0] == QMI_RESPONSE &&
			    get16(buf + 1) == txn && get16(buf + 3) == 0x002C) {
				printf("  resp msg_len %u  ", get16(buf + 5));
				if (get16(buf + 5) >= 7)
					note_result(buf + 10, 4);
				break;
			}
		}
		close(sock);

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x21, 0, &node, &port, &ins) < 0) {
			printf("  no IMSA\n");
			close(sock);
			return 1;
		}
		txn = 1;
		ask_on(sock, node, port, &qabind, txn++, NULL, NULL);
		ask_on(sock, node, port, &qareg, txn++, NULL, NULL);
		ask_on(sock, node, port, &qasvc, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "submap")) {
		/* Read-only numbering: WDS 0xAF/0xB0 and WMS 0x4F for
		 * sub 0/1/2. Throwaway clients. Not NV. */
		uint32_t node, port, ins;
		uint16_t txn;
		int sock, s;
		struct query qget = { .cmd = "wdsgetsub",
			.desc = "WDS GET_BIND_SUB 0xB0", .svc = 1, .msg = 0x00B0 };

		printf("submap: WDS GET then BIND 0/1/2 + GET; WMS BIND 0/1/2\n");
		for (s = 0; s <= 2; s++) {
			struct query qbind = { .cmd = "wdsbindn",
				.desc = "WDS BIND_SUB", .svc = 1, .msg = 0x00AF,
				.tlv_key = 0x01,
				.tlv_data = { (uint8_t)s, 0, 0, 0 }, .tlv_len = 4 };
			sock = qrtr_open(0);
			if (sock < 0)
				return 1;
			if (lookup_service(sock, 1, 0, &node, &port, &ins) < 0) {
				close(sock);
				return 1;
			}
			txn = 1;
			printf("submap: WDS bind=%u\n", s);
			if (s == 0)
				ask_on(sock, node, port, &qget, txn++, NULL, NULL);
			ask_on(sock, node, port, &qbind, txn++, NULL, NULL);
			ask_on(sock, node, port, &qget, txn++, NULL, NULL);
			close(sock);
		}
		for (s = 0; s <= 2; s++) {
			struct query qwb = { .cmd = "wmsbindn",
				.desc = "WMS BIND_SUB 0x4F", .svc = 5, .msg = 0x004F,
				.tlv_key = 0x01,
				.tlv_data = { (uint8_t)s, 0, 0, 0 }, .tlv_len = 4 };
			struct query q4a = { .cmd = "wms4a",
				.desc = "WMS GET_TRANSPORT_NW_REG", .svc = 5, .msg = 0x004A };
			sock = qrtr_open(0);
			if (sock < 0)
				return 1;
			if (lookup_service(sock, 5, 0, &node, &port, &ins) < 0) {
				close(sock);
				return 1;
			}
			txn = 1;
			printf("submap: WMS bind=%u then 0x4A\n", s);
			ask_on(sock, node, port, &qwb, txn++, NULL, NULL);
			ask_on(sock, node, port, &q4a, txn++, NULL, NULL);
			close(sock);
		}
		return 0;
	}

	if (!strcmp(argv[1], "isimdump")) {
		/* Read-only ISIM ADF on NONPROV_SLOT_2. Not NV, not SIM write. */
		static const uint8_t aid[16] = {
			0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04, 0xff,
			0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff
		};
		struct query qsim = { .cmd = "sim",
			.desc = "UIM GET_CARD_STATUS", .svc = 11, .msg = 0x002F };
		struct query qimsi = { .cmd = "usimread5",
			.desc = "USIM EF_IMSI primary-gw", .svc = 11, .msg = 0x0020,
			.tlv_key = 0x02,
			.tlv_data = {0x07, 0x6f, 0x04, 0x00, 0x3f, 0xff, 0x7f},
			.tlv_len = 7,
			.tlv2_key = 0x01, .tlv2_data = {0, 0}, .tlv2_len = 2,
			.tlv3_key = 0x03, .tlv3_data = {0, 0, 0, 0}, .tlv3_len = 4 };
		uint32_t node, port, ins;
		uint16_t txn = 1;
		int sock, i;
		static const struct {
			const char *tag;
			uint16_t fid;
			int record; /* 0 = transparent */
		} files[] = {
			{ "GET_ATTR EF_IMPI 6F02",  0x6F02, -1 },
			{ "READ EF_IMPI 6F02",      0x6F02, 0 },
			{ "GET_ATTR EF_DOMAIN 6F03", 0x6F03, -1 },
			{ "READ EF_DOMAIN 6F03",    0x6F03, 0 },
			{ "GET_ATTR EF_IMPU 6F04",  0x6F04, -1 },
			{ "READ_REC EF_IMPU 6F04#1", 0x6F04, 1 },
			{ "READ_REC EF_IMPU 6F04#2", 0x6F04, 2 },
			{ "GET_ATTR EF_IST 6F07",   0x6F07, -1 },
			{ "READ EF_IST 6F07",       0x6F07, 0 },
			{ "GET_ATTR EF_PCSCF 6F09", 0x6F09, -1 },
			{ "READ_REC EF_PCSCF 6F09#1", 0x6F09, 1 },
			{ "GET_ATTR EF_AD 6FAD",    0x6FAD, -1 },
			{ "READ EF_AD 6FAD",        0x6FAD, 0 },
		};

		printf("isimdump: nonprov-slot2 ISIM, read-only\n");
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 11, 0, &node, &port, &ins) < 0) {
			printf("  no UIM\n");
			close(sock);
			return 1;
		}
		printf("  UIM %u:%u\n", node, port);
		ask_on(sock, node, port, &qsim, txn++, NULL, NULL);
		ask_on(sock, node, port, &qimsi, txn++, NULL, NULL);
		for (i = 0; i < (int)(sizeof(files) / sizeof(files[0])); i++) {
			struct query q = { 0 };
			uint16_t fid = files[i].fid;

			q.cmd = "isimf";
			q.desc = files[i].tag;
			q.svc = 11;
			q.tlv_key = 0x02;
			q.tlv_data[0] = (uint8_t)(fid & 0xff);
			q.tlv_data[1] = (uint8_t)(fid >> 8);
			q.tlv_data[2] = 0x04;
			q.tlv_data[3] = 0x00;
			q.tlv_data[4] = 0x3f;
			q.tlv_data[5] = 0xff;
			q.tlv_data[6] = 0x7f;
			q.tlv_len = 7;
			q.tlv2_key = 0x01;
			q.tlv2_data[0] = 5;
			q.tlv2_data[1] = 16;
			memcpy(q.tlv2_data + 2, aid, 16);
			q.tlv2_len = 18;
			if (files[i].record < 0) {
				q.msg = 0x0024;
			} else if (files[i].record == 0) {
				q.msg = 0x0020;
				q.tlv3_key = 0x03;
				q.tlv3_data[0] = 0;
				q.tlv3_data[1] = 0;
				q.tlv3_data[2] = 0;
				q.tlv3_data[3] = 0;
				q.tlv3_len = 4;
			} else {
				q.msg = 0x0021;
				q.tlv3_key = 0x03;
				q.tlv3_data[0] = (uint8_t)files[i].record;
				q.tlv3_data[1] = 0;
				q.tlv3_data[2] = 0x64;
				q.tlv3_data[3] = 0;
				q.tlv3_len = 4;
			}
			ask_on(sock, node, port, &q, txn++, NULL, NULL);
		}
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "imschain")) {
		struct query *bind = NULL, *set = NULL, *get = NULL;
		struct query *abind = NULL, *areg = NULL, *asvc = NULL;
		uint32_t node, port, ins;
		int sock, i;
		uint16_t txn = 1;

		for (i = 0; i < (int)(sizeof(QUERIES) / sizeof(QUERIES[0])); i++) {
			if (!strcmp(QUERIES[i].cmd, "imsbind")) bind = &QUERIES[i];
			else if (!strcmp(QUERIES[i].cmd, "imsen")) set = &QUERIES[i];
			else if (!strcmp(QUERIES[i].cmd, "imsget")) get = &QUERIES[i];
			else if (!strcmp(QUERIES[i].cmd, "imsabind")) abind = &QUERIES[i];
			else if (!strcmp(QUERIES[i].cmd, "imsareg")) areg = &QUERIES[i];
			else if (!strcmp(QUERIES[i].cmd, "imsasvc")) asvc = &QUERIES[i];
		}
		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x12, 0, &node, &port, &ins) < 0) {
			printf("  no IMS server\n");
			close(sock);
			return 1;
		}
		printf("  IMS server %u:%u — BIND then SET then GET\n", node, port);
		if (bind) ask_on(sock, node, port, bind, txn++, NULL, NULL);
		if (set) ask_on(sock, node, port, set, txn++, NULL, NULL);
		if (get) ask_on(sock, node, port, get, txn++, NULL, NULL);
		close(sock);

		sock = qrtr_open(0);
		if (sock < 0)
			return 1;
		if (lookup_service(sock, 0x21, 0, &node, &port, &ins) < 0) {
			printf("  no IMSA server\n");
			close(sock);
			return 1;
		}
		printf("  IMSA server %u:%u — BIND then REG/SVC\n", node, port);
		txn = 1;
		if (abind) ask_on(sock, node, port, abind, txn++, NULL, NULL);
		if (areg) ask_on(sock, node, port, areg, txn++, NULL, NULL);
		if (asvc) ask_on(sock, node, port, asvc, txn++, NULL, NULL);
		close(sock);
		return 0;
	}

	if (!strcmp(argv[1], "ipa"))
		return ipa_init(argc, argv);

	if (!strcmp(argv[1], "pdc"))
		return pdc_run();

	if (!strcmp(argv[1], "svcls")) {
		svcls();
		return 0;
	}

	if (!strcmp(argv[1], "enumsvc")) {
		enumsvc(argc == 3 ? (uint32_t)strtoul(argv[2], NULL, 0) : 0);
		return 0;
	}

	/* wdschain: whole IMS PDN bring-up on one client, then hold.
	 * Extra tokens: "port" swaps the 0xA2 mux bind for the bare
	 * legacy 0x2F BIND_DATA_PORT (redfin M7 recipe for WDS
	 * instances that answer err3 on 0xA2), "nomux" sends no
	 * data-port bind at all, "pN" overrides the START_NETWORK 3GPP
	 * profile index, "ims" drops profile TLVs and starts by APN
	 * alone, "insN" pins the WDS server instance (default 0 =
	 * first announced). */
	if (!strcmp(argv[1], "wdschain") && argc >= 3) {
		int use_mux = 1, apn_only = 0, no_call_type = 0, sub_first = 0;
		int prof = -1, prof2 = -1, drop_apn = 0;
		unsigned a;
		uint32_t wds_ins = 0;

		for (a = 3; a < (unsigned)argc; a++) {
			if (!strcmp(argv[a], "nomux"))
				use_mux = 0;
			else if (!strcmp(argv[a], "port"))
				use_mux = 2;
			else if (!strcmp(argv[a], "ims"))
				apn_only = 1;
			else if (!strcmp(argv[a], "sub1"))
				sub_first = 1;
			else if (!strcmp(argv[a], "nocall"))
				no_call_type = 1;
			else if (!strcmp(argv[a], "noapn"))
				drop_apn = 1;
			else if (!strncmp(argv[a], "ins", 3))
				wds_ins = (uint32_t)strtoul(argv[a] + 3,
							     NULL, 0);
			else if (argv[a][0] == 'q' && argv[a][1])
				prof2 = (int)strtoul(argv[a] + 1, NULL, 0);
			else if (argv[a][0] == 'p' && argv[a][1])
				prof = (int)strtoul(argv[a] + 1, NULL, 0);
			else if (!strncmp(argv[a], "if", 2) && argv[a][2] >= '0') {
				unsigned long iface = strtoul(argv[a] + 2, NULL, 0);
				unsigned qi;

				for (qi = 0; qi < sizeof(QUERIES) / sizeof(QUERIES[0]); qi++) {
					if (strcmp(QUERIES[qi].cmd, "wdsmux"))
						continue;
					put32(QUERIES[qi].tlv_data + 4, (uint32_t)iface);
					printf("  [chain mux iface %lu]\n", iface);
				}
			} else if (!strcmp(argv[a], "slot2")) {
				unsigned qi;

				for (qi = 0; qi < sizeof(QUERIES) / sizeof(QUERIES[0]); qi++) {
					if (strcmp(QUERIES[qi].cmd, "wdsbind"))
						continue;
					put32(QUERIES[qi].tlv_data, 2);
					printf("  [chain BIND_SUB 2]\n");
				}
			} else if (!strcmp(argv[a], "ctnet")) {
				unsigned qi;

				for (qi = 0; qi < sizeof(QUERIES) / sizeof(QUERIES[0]); qi++) {
					if (strcmp(QUERIES[qi].cmd, "wdsstart"))
						continue;
					memcpy(QUERIES[qi].tlv_data, "ctnet", 5);
					QUERIES[qi].tlv_len = 5;
					QUERIES[qi].tlv2_data[0] = 4;
					printf("  [chain START apn=ctnet ipv4]\n");
				}
			}
		}
		return chain(argv[2], use_mux, prof, apn_only, no_call_type,
			     wds_ins, sub_first, prof2, drop_apn);
	}

	/* argv[2] patches: wdsstop gets the packet handle (from
	 * wdsstart's reply); wdsprof gets the profile index;
	 * wdsplist gets the profile table type (0 = 3GPP). */
	if (argc == 3 && !strcmp(argv[1], "wdsprof")) {
		unsigned long idx = strtoul(argv[2], NULL, 0);

		for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++) {
			if (strcmp(QUERIES[i].cmd, "wdsprof"))
				continue;
			QUERIES[i].tlv_data[1] = (uint8_t)idx;
		}
	}
	if (argc == 4 && !strcmp(argv[1], "wdsprof")) {
		unsigned long idx = strtoul(argv[2], NULL, 0);
		unsigned long t = strtoul(argv[3], NULL, 0);

		for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++) {
			if (strcmp(QUERIES[i].cmd, "wdsprof"))
				continue;
			QUERIES[i].tlv_data[0] = (uint8_t)t;
			QUERIES[i].tlv_data[1] = (uint8_t)idx;
		}
	}
	if (argc == 3 && !strcmp(argv[1], "wdsstop")) {
		unsigned long h = strtoul(argv[2], NULL, 0);

		for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++) {
			if (strcmp(QUERIES[i].cmd, "wdsstop"))
				continue;
			QUERIES[i].tlv_data[0] = h & 0xff;
			QUERIES[i].tlv_data[1] = (h >> 8) & 0xff;
			QUERIES[i].tlv_data[2] = (h >> 16) & 0xff;
			QUERIES[i].tlv_data[3] = (h >> 24) & 0xff;
		}
	}
	if (argc == 3 && !strcmp(argv[1], "wdsplist")) {
		unsigned long t = strtoul(argv[2], NULL, 0);

		for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++) {
			if (strcmp(QUERIES[i].cmd, "wdsplist"))
				continue;
			QUERIES[i].tlv_data[0] = (uint8_t)t;
		}
	}
	/* iccread <len>: over-read probe — vary the Read Information
	 * length (argv[2]) to test whether strict-length reads pass
	 * where over-reads die INTERNAL. */
	if (argc == 3 && !strcmp(argv[1], "iccread")) {
		unsigned long l = strtoul(argv[2], NULL, 0);

		for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++) {
			if (strcmp(QUERIES[i].cmd, "iccread"))
				continue;
			QUERIES[i].tlv3_data[2] = l & 0xff;
			QUERIES[i].tlv3_data[3] = (l >> 8) & 0xff;
		}
	}
	/* wdfmt / wdfmtqmap4 / wdfmtqmap5 <iface>: IPA sysfs says
	 * modem_rx=10 modem_tx=2, not the hardcoded iface 1. */
	if (argc == 3 && (!strcmp(argv[1], "wdfmt") ||
			  !strcmp(argv[1], "wdfmtqmap4") ||
			  !strcmp(argv[1], "wdfmtqmap5") ||
			  !strcmp(argv[1], "wdfmtqmap"))) {
		unsigned long iface = strtoul(argv[2], NULL, 0);

		for (i = 0; i < (int)(sizeof(QUERIES) / sizeof(QUERIES[0])); i++) {
			if (strcmp(QUERIES[i].cmd, argv[1]))
				continue;
			if (!strcmp(argv[1], "wdfmt"))
				put32(QUERIES[i].tlv_data + 4, (uint32_t)iface);
			else
				put32(QUERIES[i].tlv6_data + 4, (uint32_t)iface);
			printf("  [wda ep iface %lu]\n", iface);
		}
	}

	/* wdsmux <mux_id> [iface] [client_type]: probe bind-mux shapes
	 * beyond {4,1}+1; client_type emits optional TLV 0x13 (u32). */
	if ((argc == 3 || argc == 4 || argc == 5) &&
	    !strcmp(argv[1], "wdsmux")) {
		unsigned long muxid = strtoul(argv[2], NULL, 0);
		unsigned long iface = argc >= 4 ? strtoul(argv[3], NULL, 0) : 1;
		unsigned long ctype = argc == 5 ? strtoul(argv[4], NULL, 0) : 0;

		for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++) {
			if (strcmp(QUERIES[i].cmd, "wdsmux"))
				continue;
			QUERIES[i].tlv2_data[0] = (uint8_t)muxid;
			QUERIES[i].tlv_data[4] = (uint8_t)iface;
			if (argc == 5) {
				QUERIES[i].tlv3_key = 0x13;
				QUERIES[i].tlv3_data[0] = (uint8_t)ctype;
				QUERIES[i].tlv3_len = 4;
			}
		}
	}

	if (!strcmp(argv[1], "all")) {
		for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++)
			if (ask(&QUERIES[i]))
				rc = 1;
		return rc;
	}

	for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++) {
		if (!strcmp(argv[1], QUERIES[i].cmd)) {
			matched = 1;
			rc = ask(&QUERIES[i]);
			break;
		}
	}
	if (!matched) {
		fprintf(stderr, "unknown query '%s'\n", argv[1]);
		return 2;
	}
	return rc;
}
