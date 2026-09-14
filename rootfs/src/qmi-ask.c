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
	{ "wdsbind", 1,  0x00AF, "WDS BIND_SUBSCRIPTION primary", 0x01, {1, 0, 0, 0}, 4 },
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
	/* Profile numbering is device-config dependent: this unit's 3GPP
	 * table holds only idx 0/100/101 (ctnet/ctwap), so wdschain takes
	 * pN to override 0x31, or "ims" to start by APN with no profile. */
	{ "wdsplist", 1, 0x002A, "WDS GET_PROFILE_LIST 3gpp", 0x10, {1}, 1 },
	/* wdsprof takes the profile index as argv[2] (see main). */
	{ "wdsprof",  1, 0x002B, "WDS GET_PROFILE_SETTINGS idx=<argv2>", 0x01, {1, 0}, 2 },
	/* wdsstop takes the packet handle printed by wdsstart as argv[2]
	 * (decimal or 0x-hex). */
	{ "wdsstop", 1,  0x0021, "WDS STOP_NETWORK handle=<argv2>", 0x01, {0, 0, 0, 0}, 4 },
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
		unsigned ncards = d[8];

		printf("    [%u card(s); card0 state=%u (%s) error=%u]\n", ncards,
		       d[9], d[9] == 0 ? "ABSENT" : d[9] == 1 ? "PRESENT" : d[9] == 2 ? "ERROR" : "?",
		       d[13]);
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
		{ 0x13, 0, "ADS" },   { 0x15, 0, "PDC" },
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
static int chain(const char *hold_arg, int use_mux, unsigned profile_idx,
		 int apn_only, int no_call_type)
{
	struct query *mux = NULL, *bind = NULL, *ipfam = NULL,
		     *start = NULL, *stat = NULL;
	uint32_t node, port, ins, handle = 0;
	uint16_t txn = 1;
	int sock, i, rc = 1;
	unsigned hold, t;

	hold = (unsigned)strtoul(hold_arg, NULL, 0);
	for (i = 0; i < (int)(sizeof(QUERIES) / sizeof(QUERIES[0])); i++) {
		const char *c = QUERIES[i].cmd;

		if (!strcmp(c, "wdsmux")) mux = &QUERIES[i];
		else if (!strcmp(c, "wdsbind")) bind = &QUERIES[i];
		else if (!strcmp(c, "wdsipfam")) ipfam = &QUERIES[i];
		else if (!strcmp(c, "wdsstart")) start = &QUERIES[i];
		else if (!strcmp(c, "wdsstat")) stat = &QUERIES[i];
	}
	if (profile_idx && start) {
		start->tlv3_data[0] = (uint8_t)profile_idx;
		printf("  [chain start profile idx overridden to %u]\n",
		       profile_idx);
	}
	if (apn_only && start) {
		start->tlv3_key = 0;
		start->tlv3_len = 0;
		start->tlv4_key = 0;
		start->tlv4_len = 0;
		printf("  [chain start: no profile TLVs, apn=ims only]\n");
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
	if (lookup_service(sock, 1, 0, &node, &port, &ins) < 0) {
		printf("  no NEW_SERVER for WDS\n");
		close(sock);
		return 1;
	}
	printf("  server: node %u port %u (single client, txn 1..)\n",
	       node, port);

	/* QMI-level errors don't abort the chain — the printed receipts
	 * carry the diagnosis; only transport failures do (ask_on rc=1). */
	if (use_mux)
		ask_on(sock, node, port, mux, txn++, NULL, NULL);
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

int main(int argc, char **argv)
{
	unsigned i;
	int rc = 0, matched = 0;

	if (argc != 2 &&
	    !(argc >= 3 && !strcmp(argv[1], "wdschain") && argc <= 6) &&
	    !(argc == 3 && (!strcmp(argv[1], "wdsstop") ||
			    !strcmp(argv[1], "wdsprof"))) &&
	    !(argc >= 3 && argc <= 4 && !strcmp(argv[1], "ipa"))) {
		fprintf(stderr, "usage: %s imei|mode|online|offline|lpm|uireset|sim|slots|simon|simoff|simon2|simoff2|provision|provision2|prov0|provp|switchslot|switchback|unprovision|events|sig|serving|sysinfo|ssp|sspcs|sspps|wdsmux|wdsbind|wdsipfam|wdsstart|wdsstat|wdsstop <handle>|wdschain <hold-seconds> [nomux|ims|nocall|pN]|wdfmt|wdfmtget|wdfmtraw|wdfmtqmap5|wdfmtqmap4|wdfmtqmap|wdfmtdis|wdfmtdisn|imsareg|imsasvc|ipa <main|hwstats|nossr|android> [R]|svcls|all\n", argv[0]);
		return 2;
	}

	if (!strcmp(argv[1], "ipa"))
		return ipa_init(argc, argv);

	if (!strcmp(argv[1], "svcls")) {
		svcls();
		return 0;
	}

	/* wdschain: whole IMS PDN bring-up on one client, then hold.
	 * Extra tokens: "nomux" skips BIND_MUX_DATA_PORT, "pN" overrides
	 * the START_NETWORK 3GPP profile index, "ims" drops profile TLVs
	 * and starts by APN alone. */
	if (!strcmp(argv[1], "wdschain") && argc >= 3) {
		int use_mux = 1, apn_only = 0, no_call_type = 0;
		unsigned prof = 0, a;

		for (a = 3; a < (unsigned)argc; a++) {
			if (!strcmp(argv[a], "nomux"))
				use_mux = 0;
			else if (!strcmp(argv[a], "ims"))
				apn_only = 1;
			else if (!strcmp(argv[a], "nocall"))
				no_call_type = 1;
			else if (argv[a][0] == 'p')
				prof = (unsigned)strtoul(argv[a] + 1, NULL, 0);
		}
		return chain(argv[2], use_mux, prof, apn_only, no_call_type);
	}

	/* argv[2] patches: wdsstop gets the packet handle (from
	 * wdsstart's reply); wdsprof gets the profile index. */
	if (argc == 3 && !strcmp(argv[1], "wdsprof")) {
		unsigned long idx = strtoul(argv[2], NULL, 0);

		for (i = 0; i < sizeof(QUERIES) / sizeof(QUERIES[0]); i++) {
			if (strcmp(QUERIES[i].cmd, "wdsprof"))
				continue;
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
