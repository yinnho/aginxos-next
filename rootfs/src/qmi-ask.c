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
};

static const struct query QUERIES[] = {
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
	{ "provision", 11, 0x0038, "UIM CHANGE_PROVISIONING_SESSION activate gw slot1 usim", 0x01, {0, 1}, 2,
	  0x10, {1, 16, 0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x02, 0xff, 0x86, 0xff, 0x03, 0x89, 0xff, 0xff, 0xff, 0xff}, 18 },
	{ "unprovision", 11, 0x0038, "UIM CHANGE_PROVISIONING_SESSION deactivate gw", 0x01, {0, 0}, 2 },
	{ "events",  11, 0x002E, "UIM REGISTER_EVENTS mask 3", 0x01, {3, 0, 0, 0}, 4 },
	{ "sig",     3,  0x0020, "NAS GET_SIGNAL_STRENGTH" },
	{ "serving", 3,  0x0024, "NAS GET_SERVING_SYSTEM" },
	{ "sysinfo", 3,  0x004D, "NAS GET_SYSTEM_INFO" },
};

static void put16(uint8_t *p, uint16_t v) { memcpy(p, &v, 2); }
static uint16_t get16(const uint8_t *p) { uint16_t v; memcpy(&v, p, 2); return v; }

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
	/* imei: the printable digits inside TLV 0x10/0x11 are already
	 * visible in the raw dump; no extra hint needed. */
}

/*
 * Resolve the (node, port) of the modem's service via NEW_LOOKUP with
 * wildcard version/instance.  Ends on the kernel's all-zero NEW_SERVER
 * sentinel, a matching hit, or timeout — whichever comes first.
 */
static int lookup_service(int sock, uint32_t svc, uint32_t *node, uint32_t *port)
{
	uint8_t buf[512];
	struct qrtr_packet pkt;
	struct sockaddr_qrtr sq;
	uint32_t n, p;
	int len, rc, i;

	rc = qrtr_new_lookup(sock, svc, 0, 0);
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
		if (pkt.service != svc)
			continue;

		*node = pkt.node;
		*port = pkt.port;
		qrtr_remove_lookup(sock, svc, 0, 0);
		return 0;
	}

	qrtr_remove_lookup(sock, svc, 0, 0);
	return -1;
}

static int ask(const struct query *q)
{
	uint8_t buf[2048];
	uint8_t req[64];
	uint16_t req_len = 0, tlv_bytes = 0;
	uint32_t node, port, rn, rp;
	uint16_t txn = 1;
	unsigned off, end;
	int sock, len, i, rc = 1;

	printf("== %s: %s (service %u, msg 0x%04x) ==\n",
	       q->cmd, q->desc, q->svc, q->msg);

	sock = qrtr_open(0);
	if (sock < 0) {
		printf("  qrtr_open failed\n");
		return 1;
	}

	if (lookup_service(sock, q->svc, &node, &port) < 0) {
		printf("  no NEW_SERVER for service %u\n", q->svc);
		close(sock);
		return 1;
	}
	printf("  server: node %u port %u\n", node, port);

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
			off += tl;
		}
		rc = 0;				/* a response arrived */
		break;
	}

	if (rc)
		printf("  TIMEOUT: no response\n");
	close(sock);
	return rc;
}

int main(int argc, char **argv)
{
	unsigned i;
	int rc = 0, matched = 0;

	if (argc != 2) {
		fprintf(stderr, "usage: %s imei|mode|online|offline|lpm|uireset|sim|slots|simon|simoff|provision|unprovision|events|sig|serving|sysinfo|all\n", argv[0]);
		return 2;
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
