// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Copyright (c) 2024, Richard Acayan. All rights reserved.
 * Copyright (c) 2026, The AginxOS Authors.
 *
 * libqrtr port of 81voltd's IMS Data QMI server (qvd-server.c) without
 * glib/ModemManager. Publishes QRTR service 770 (version 1, instance 0).
 * START_CONNECTION attempts a WDS bring-up on a second socket using the
 * APN the modem asked for; CONNECTION_CHANGED carries the IP or err 13.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <unistd.h>
#include <errno.h>
#include <stdint.h>
#include <stdbool.h>
#include <time.h>
#include <arpa/inet.h>
#include <sys/socket.h>
#include <sys/ioctl.h>
#include <net/if.h>
#include <linux/netlink.h>
#include <libqrtr.h>
#include "qmi_imsd.h"

#define IFLA_IFNAME_		3
#define IFLA_LINK_		5
#define IFLA_LINKINFO_		18
#define IFLA_INFO_KIND_		1
#define IFLA_INFO_DATA_		2
#define IFLA_RMNET_MUX_ID_	1
#define IFLA_RMNET_FLAGS_	2
#define RTM_NEWLINK_		16
#define RTM_DELLINK_		17
#define RMNET_F_INGRESS_DEAGG		(1u << 0)
#define RMNET_F_INGRESS_MAP_CMDS	(1u << 1)
#define RMNET_F_INGRESS_CKSUMV5		(1u << 4)
#define RMNET_F_EGRESS_CKSUMV5		(1u << 5)
#define WDA_GET_FMT			0x0021

#define IMSD_SVC			770
#define IMSD_START_CONNECTION		0x20
#define IMSD_STOP_CONNECTION		0x21
#define IMSD_CONNECTION_CHANGED		0x20

#define IMSD_AF_IPV4			0
#define IMSD_AF_IPV6			1

#define WDS_SVC				1
#define WDS_START			0x0020
#define WDS_BIND_MUX			0x00A2
#define WDS_BIND_SUB			0x00AF
#define WDS_IPFAM			0x004D
#define WDS_GET_SETTINGS		0x002D
#define WDA_SVC				0x1A
#define WDA_SET_FMT			0x0020
#define DPM_SVC				0x2F
#define DPM_OPEN			0x0020

#define MAX_CLIENTS			8
#define MAX_CONNS			8

struct client {
	uint32_t node, port;
	int used;
};

struct conn {
	int used;
	uint8_t local_id;
	uint32_t orig_conn_id;
	uint32_t subscription;
	int ip_family;		/* IMSD_AF_* */
	char apn[256];
	int wds_fd;		/* held open for the life of the bearer */
};

static int srv_fd = -1;
static uint16_t ind_txn = 1;
static struct client clients[MAX_CLIENTS];
static struct conn conns[MAX_CONNS];
static char last_lladdr[64];	/* IPv6 from modem msg 0x23 */
static int wda_fd = -1, dpm_fd = -1;
static uint32_t wda_node, wda_port, dpm_node, dpm_port;
static int wda_ok;

static void stamp(void)
{
	struct timespec ts;
	struct tm tm;
	clock_gettime(CLOCK_REALTIME, &ts);
	localtime_r(&ts.tv_sec, &tm);
	printf("[%02d:%02d:%02d.%03ld] ", tm.tm_hour, tm.tm_min, tm.tm_sec,
	       ts.tv_nsec / 1000000);
}

static void hexdump(const char *tag, const uint8_t *p, unsigned n)
{
	unsigned i;

	stamp();
	printf("%s %u bytes:", tag, n);
	for (i = 0; i < n && i < 256; i++)
		printf(" %02x", p[i]);
	if (n > 256)
		printf(" ...");
	putchar('\n');
}

static void remember_client(uint32_t node, uint32_t port)
{
	int i, free_i = -1;

	for (i = 0; i < MAX_CLIENTS; i++) {
		if (clients[i].used && clients[i].node == node &&
		    clients[i].port == port)
			return;
		if (!clients[i].used && free_i < 0)
			free_i = i;
	}
	if (free_i < 0)
		free_i = 0;
	clients[free_i].used = 1;
	clients[free_i].node = node;
	clients[free_i].port = port;
}

static void forget_client(uint32_t node, uint32_t port)
{
	int i;

	for (i = 0; i < MAX_CLIENTS; i++) {
		if (clients[i].used && clients[i].node == node &&
		    clients[i].port == port)
			clients[i].used = 0;
	}
}

static int alloc_conn(uint32_t orig, uint32_t sub, int ip_family, const char *apn)
{
	int i;

	for (i = 0; i < MAX_CONNS; i++) {
		if (!conns[i].used) {
			conns[i].used = 1;
			/* Stock imsdatadaemon uses 0x14/0x15, not 0. */
			conns[i].local_id = (uint8_t)(0x14 + i);
			conns[i].orig_conn_id = orig;
			conns[i].subscription = sub;
			conns[i].ip_family = ip_family;
			conns[i].wds_fd = -1;
			snprintf(conns[i].apn, sizeof(conns[i].apn), "%s",
				 apn ? apn : "");
			return i;
		}
	}
	return -1;
}

static void send_pkt(uint32_t node, uint32_t port, const void *data, size_t n)
{
	if (qrtr_sendto(srv_fd, node, port, data, (unsigned)n) < 0)
		fprintf(stderr, "sendto %u:%u failed: %s\n", node, port,
			strerror(errno));
}

static int encode_send(uint32_t node, uint32_t port, int type, int msg_id,
		       int txn, const void *c_struct, struct qmi_elem_info *ei)
{
	DEFINE_QRTR_PACKET(buf, 1024);
	ssize_t len;

	len = qmi_encode_message(&buf, type, msg_id, txn, c_struct, ei);
	if (len < 0) {
		fprintf(stderr, "encode msg 0x%x failed: %s\n", msg_id,
			strerror(-len));
		return -1;
	}
	hexdump(type == QMI_INDICATION ? "ind>" : "resp>", buf.data,
		(unsigned)buf.data_len);
	send_pkt(node, port, buf.data, buf.data_len);
	return 0;
}

static void send_error(const struct qrtr_packet *pkt, unsigned txn, uint16_t code)
{
	struct imsd_no_op_resp resp = {
		.res = { .err_status = 1, .err_code = code },
	};

	stamp();
	printf("error resp code=%u to %u:%u\n", code, pkt->node, pkt->port);
	encode_send(pkt->node, pkt->port, QMI_RESPONSE, IMSD_START_CONNECTION,
		    (int)txn, &resp, imsd_no_op_resp_ei);
}

static void send_changed(const struct conn *c, const char *addr, uint16_t err)
{
	struct imsd_connection_changed_ind ind;
	int i;

	memset(&ind, 0, sizeof(ind));
	ind.res.err_status = err ? 1 : 0;
	ind.res.err_code = err;
	ind.connection = c->local_id;
	ind.orig_conn_id = c->orig_conn_id;
	ind.subscription = c->subscription;
	if (addr && !err) {
		ind.ip_addr_valid = true;
		ind.ip_addr.family = (uint32_t)c->ip_family;
		snprintf(ind.ip_addr.addr, sizeof(ind.ip_addr.addr), "%s", addr);
		ind.ip_addr.addr_len = (uint32_t)strlen(ind.ip_addr.addr);
	}

	stamp();
	printf("CONNECTION_CHANGED local=%u orig=%u sub=%u addr=%s err=%u\n",
	       c->local_id, c->orig_conn_id, c->subscription,
	       addr ? addr : "-", err);

	for (i = 0; i < MAX_CLIENTS; i++) {
		if (!clients[i].used)
			continue;
		encode_send(clients[i].node, clients[i].port, QMI_INDICATION,
			    IMSD_CONNECTION_CHANGED, ind_txn++, &ind,
			    imsd_connection_changed_ind_ei);
	}
}

/* ---- WDS client (second socket; 81voltd's MM bearer, in QMI) ---- */

static void put16(uint8_t *p, uint16_t v) { memcpy(p, &v, 2); }
static void put32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }
static uint16_t get16(const uint8_t *p)
{
	uint16_t v;
	memcpy(&v, p, 2);
	return v;
}
static uint32_t get32(const uint8_t *p)
{
	return get16(p) | ((uint32_t)get16(p + 2) << 16);
}

static int qmi_lookup(int sock, uint32_t svc, uint32_t *node, uint32_t *port)
{
	uint8_t buf[512];
	struct qrtr_packet pkt;
	struct sockaddr_qrtr sq;
	uint32_t n, p;
	int len, i;

	if (qrtr_new_lookup(sock, svc, 0, 0) < 0)
		return -1;
	for (i = 0; i < 12; i++) {
		len = qrtr_recvfrom(sock, buf, sizeof(buf), &n, &p);
		if (len < 0)
			continue;
		if (p != QRTR_PORT_CTRL)
			continue;
		memset(&sq, 0, sizeof(sq));
		sq.sq_family = AF_QIPCRTR;
		sq.sq_node = n;
		sq.sq_port = p;
		memset(&pkt, 0, sizeof(pkt));
		if (qrtr_decode(&pkt, buf, (size_t)len, &sq) != 0)
			continue;
		if (pkt.type != QRTR_TYPE_NEW_SERVER)
			continue;
		if (!pkt.service && !pkt.node && !pkt.port)
			break;
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

static uint16_t tlv_put(uint8_t *p, uint8_t key, const void *data, uint16_t n)
{
	p[0] = key;
	put16(p + 1, n);
	if (n && data)
		memcpy(p + 3, data, n);
	return (uint16_t)(3 + n);
}

static int qmi_txn(int sock, uint32_t node, uint32_t port, uint16_t txn,
		   uint16_t msg, const uint8_t *tlvs, uint16_t tlv_len,
		   uint8_t *resp, size_t cap, uint16_t *out_mlen)
{
	uint8_t req[256];
	uint32_t rn, rp;
	int len, i;

	req[0] = QMI_REQUEST;
	put16(req + 1, txn);
	put16(req + 3, msg);
	put16(req + 5, tlv_len);
	if (tlv_len)
		memcpy(req + 7, tlvs, tlv_len);
	if (qrtr_sendto(sock, node, port, req, 7 + tlv_len) < 0)
		return -1;
	for (i = 0; i < 8; i++) {
		len = qrtr_recvfrom(sock, resp, (unsigned)cap, &rn, &rp);
		if (len < 7)
			continue;
		if (rp == QRTR_PORT_CTRL)
			continue;
		if (resp[0] != QMI_RESPONSE)
			continue;
		if (get16(resp + 1) != txn || get16(resp + 3) != msg)
			continue;
		*out_mlen = get16(resp + 5);
		hexdump("qmi<", resp, (unsigned)len);
		return 0;
	}
	return -1;
}

static int wds_ok(const uint8_t *resp, uint16_t mlen)
{
	unsigned off = 7, end = 7 + (unsigned)mlen;

	while (off + 3 <= end) {
		uint8_t k = resp[off];
		uint16_t n = get16(resp + off + 1);

		off += 3;
		if (off + n > end)
			break;
		if (k == 0x02 && n >= 4)
			return get16(resp + off) == 0;
		off += n;
	}
	return 0;
}

static int wds_pick_ip(const uint8_t *resp, uint16_t mlen, char *out, size_t cap)
{
	unsigned off = 7, end = 7 + (unsigned)mlen;

	while (off + 3 <= end) {
		uint8_t k = resp[off];
		uint16_t n = get16(resp + off + 1);
		struct in_addr a4;

		off += 3;
		if (off + n > end)
			break;
		/* 0x1E IPv4; 0x20 was the 81voltd strace IPv4; 0x27 IPv6. */
		if ((k == 0x1E || k == 0x20) && n >= 4) {
			a4.s_addr = get32(resp + off);
			snprintf(out, cap, "%s", inet_ntoa(a4));
			return 0;
		}
		if ((k == 0x27 || k == 0x19) && n >= 16) {
			if (inet_ntop(AF_INET6, resp + off, out, (socklen_t)cap))
				return 0;
		}
		off += n;
	}
	return -1;
}

static char *nla_put(char *p, unsigned short type, const void *val, unsigned short len)
{
	struct nlattr *a = (struct nlattr *)p;

	a->nla_len = NLMSG_ALIGN(sizeof(*a)) + len;
	a->nla_type = type;
	memcpy(p + NLMSG_ALIGN(sizeof(*a)), val, len);
	return p + NLMSG_ALIGN(a->nla_len);
}

/* Create rmnet child over realdev with mux id (LOS gold uses mux 4 for IMS). */
static int rmnet_mux_add(const char *realdev, const char *name, unsigned mux)
{
	char path[128], data[64], info[128], buf[512], rbuf[256];
	FILE *f;
	int link, fd;
	unsigned short mux16;
	struct {
		unsigned char family, pad1;
		unsigned short pad2;
		int ifindex, flags, change;
	} ifi;
	struct nlmsghdr *nh;
	struct sockaddr_nl sa, kern;
	char *e, *i, *p;
	ssize_t r;
	struct nlmsgerr *err;

	snprintf(path, sizeof(path), "/sys/class/net/%s/ifindex", realdev);
	f = fopen(path, "r");
	if (!f) {
		stamp();
		printf("ipa: %s ifindex: %s\n", realdev, strerror(errno));
		return -1;
	}
	if (fscanf(f, "%d", &link) != 1) {
		fclose(f);
		return -1;
	}
	fclose(f);

	mux16 = (unsigned short)mux;
	e = nla_put(data, IFLA_RMNET_MUX_ID_, &mux16, 2);
	{
		struct {
			uint32_t flags, mask;
		} rf = {
			.flags = RMNET_F_INGRESS_DEAGG | RMNET_F_INGRESS_MAP_CMDS |
				 RMNET_F_INGRESS_CKSUMV5 | RMNET_F_EGRESS_CKSUMV5,
			.mask = 0xffffffffu,
		};

		e = nla_put(e, IFLA_RMNET_FLAGS_, &rf, 8);
	}
	i = nla_put(info, IFLA_INFO_KIND_, "rmnet", 6);
	i = nla_put(i, IFLA_INFO_DATA_, data, (unsigned short)(e - data));

	memset(buf, 0, sizeof(buf));
	memset(&ifi, 0, sizeof(ifi));
	nh = (struct nlmsghdr *)buf;
	nh->nlmsg_len = NLMSG_ALIGN(sizeof(*nh)) + sizeof(ifi);
	nh->nlmsg_type = RTM_NEWLINK_;
	nh->nlmsg_flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
	nh->nlmsg_seq = 1;
	memcpy(buf + NLMSG_ALIGN(sizeof(*nh)), &ifi, sizeof(ifi));
	p = buf + NLMSG_ALIGN(sizeof(*nh)) + sizeof(ifi);
	p = nla_put(p, IFLA_IFNAME_, name, (unsigned short)(strlen(name) + 1));
	p = nla_put(p, IFLA_LINK_, &link, 4);
	p = nla_put(p, IFLA_LINKINFO_, info, (unsigned short)(i - info));
	nh->nlmsg_len = (uint32_t)(p - buf);

	fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_ROUTE);
	if (fd < 0)
		return -1;
	memset(&sa, 0, sizeof(sa));
	sa.nl_family = AF_NETLINK;
	if (bind(fd, (void *)&sa, sizeof(sa)) < 0) {
		close(fd);
		return -1;
	}
	memset(&kern, 0, sizeof(kern));
	kern.nl_family = AF_NETLINK;
	if (sendto(fd, buf, nh->nlmsg_len, 0, (void *)&kern, sizeof(kern)) < 0) {
		stamp();
		printf("ipa: rmnet %s mux %u send: %s\n", name, mux, strerror(errno));
		close(fd);
		return -1;
	}
	r = recv(fd, rbuf, sizeof(rbuf), 0);
	close(fd);
	if (r < 0)
		return -1;
	nh = (struct nlmsghdr *)rbuf;
	if (nh->nlmsg_type == NLMSG_ERROR) {
		err = NLMSG_DATA(nh);
		if (err->error == 0 || err->error == -EEXIST) {
			stamp();
			printf("ipa: %s mux %u over %s %s\n", name, mux, realdev,
			       err->error ? "exists" : "created");
			return 0;
		}
		stamp();
		printf("ipa: rmnet %s mux %u netlink %d (%s)\n", name, mux,
		       err->error, strerror(-err->error));
		return -1;
	}
	return -1;
}

static void netdev_up(const char *name)
{
	struct ifreq ifr;
	int s, before, after;

	s = socket(AF_INET, SOCK_DGRAM, 0);
	if (s < 0) {
		stamp();
		printf("ipa: socket: %s\n", strerror(errno));
		return;
	}
	memset(&ifr, 0, sizeof(ifr));
	snprintf(ifr.ifr_name, sizeof(ifr.ifr_name), "%s", name);
	if (ioctl(s, SIOCGIFFLAGS, &ifr) < 0) {
		stamp();
		printf("ipa: %s get flags: %s\n", name, strerror(errno));
		close(s);
		return;
	}
	before = ifr.ifr_flags;
	ifr.ifr_flags |= IFF_UP;
	if (ioctl(s, SIOCSIFFLAGS, &ifr) < 0) {
		stamp();
		printf("ipa: %s set UP: %s (was 0x%x)\n", name, strerror(errno),
		       before);
		close(s);
		return;
	}
	ioctl(s, SIOCGIFFLAGS, &ifr);
	after = ifr.ifr_flags;
	close(s);
	stamp();
	printf("ipa: %s flags 0x%x -> 0x%x\n", name, before, after);
}

static int open_svc(uint32_t svc, uint32_t *node, uint32_t *port)
{
	int sock = qrtr_open(0);

	if (sock < 0)
		return -1;
	if (qmi_lookup(sock, svc, node, port) < 0) {
		close(sock);
		return -1;
	}
	return sock;
}

/* MM qrtr+IPA SET_DATA_FORMAT ladder (mm-port-qmi.c): V5, V4, QMAP, then
 * raw-ip/no-agg. Endpoint TLV 0x17 = embedded iface 1. Client is held. */
static int wda_set(const char *tag, const uint8_t *tlvs, uint16_t tlen,
		   uint16_t *txn)
{
	uint8_t resp[256];
	uint16_t mlen;
	int ok;

	if (qmi_txn(wda_fd, wda_node, wda_port, (*txn)++, WDA_SET_FMT,
		    tlvs, tlen, resp, sizeof(resp), &mlen) < 0) {
		stamp();
		printf("wda SET %s: no response\n", tag);
		return 0;
	}
	ok = wds_ok(resp, mlen);
	stamp();
	printf("wda SET %s %s\n", tag, ok ? "ok" : "fail");
	return ok;
}

static int dpm_open(uint32_t iface)
{
	uint8_t tlv[32], resp[128];
	uint16_t tlen, mlen;
	static uint16_t txn = 1;
	uint8_t body[20];
	int ok;

	/* count=1, namelen=10, "rmnet_ipa0", ep_type=4, iface. */
	memset(body, 0, sizeof(body));
	body[0] = 1;
	body[1] = 10;
	memcpy(body + 2, "rmnet_ipa0", 10);
	body[12] = 4;
	put32(body + 16, iface);
	tlen = tlv_put(tlv, 0x10, body, 20);
	if (qmi_txn(dpm_fd, dpm_node, dpm_port, txn++, DPM_OPEN,
		    tlv, tlen, resp, sizeof(resp), &mlen) < 0) {
		stamp();
		printf("dpm OPEN iface %u: no response\n", iface);
		return 0;
	}
	ok = wds_ok(resp, mlen);
	stamp();
	printf("dpm OPEN rmnet_ipa0 iface %u %s\n", iface, ok ? "ok" : "fail");
	return ok;
}

static void datapath_prepare(void)
{
	uint8_t t[64];
	uint16_t n, txn = 1;
	uint8_t ep[8] = { 4, 0, 0, 0, 1, 0, 0, 0 };
	uint8_t raw[4] = { 2, 0, 0, 0 };
	uint8_t z[4] = { 0, 0, 0, 0 };
	uint8_t n32[4] = { 32, 0, 0, 0 };
	uint8_t sz[4] = { 0, 128, 0, 0 };	/* 32768 */
	uint8_t v5[4] = { 9, 0, 0, 0 };
	uint8_t v4[4] = { 8, 0, 0, 0 };
	uint8_t qmap[4] = { 5, 0, 0, 0 };

	netdev_up("rmnet_ipa0");
	/* netmgrd lays rmnet_data0..10 with mux 1..11 before any BIND_MUX.
	 * LOS gold IMS uses mux_id=4 (= rmnet_data3). QMAP v5 flags on each. */
	{
		unsigned mux;
		char name[16];

		for (mux = 1; mux <= 11; mux++) {
			snprintf(name, sizeof(name), "rmnet_data%u", mux - 1);
			rmnet_mux_add("rmnet_ipa0", name, mux);
			netdev_up(name);
		}
	}

	if (wda_fd < 0) {
		wda_fd = open_svc(WDA_SVC, &wda_node, &wda_port);
		stamp();
		if (wda_fd < 0)
			printf("wda: no server\n");
		else
			printf("wda: server %u:%u\n", wda_node, wda_port);
	}
	if (wda_fd >= 0) {
		uint8_t resp[256];
		uint16_t mlen, gtxn = 200;
		uint8_t g[16];
		uint16_t gn = tlv_put(g, 0x10, ep, 8);

		if (qmi_txn(wda_fd, wda_node, wda_port, gtxn, WDA_GET_FMT,
			    g, gn, resp, sizeof(resp), &mlen) == 0)
			stamp(), printf("wda GET %s\n",
					wds_ok(resp, mlen) ? "ok" : "fail");
	}
	if (wda_fd >= 0 && !wda_ok) {
		n = tlv_put(t, 0x11, raw, 4);
		n = (uint16_t)(n + tlv_put(t + n, 0x12, v5, 4));
		n = (uint16_t)(n + tlv_put(t + n, 0x13, v5, 4));
		n = (uint16_t)(n + tlv_put(t + n, 0x15, n32, 4));
		n = (uint16_t)(n + tlv_put(t + n, 0x16, sz, 4));
		n = (uint16_t)(n + tlv_put(t + n, 0x17, ep, 8));
		if (wda_set("qmapv5", t, n, &txn))
			wda_ok = 1;
		if (!wda_ok) {
			n = tlv_put(t, 0x11, raw, 4);
			n = (uint16_t)(n + tlv_put(t + n, 0x12, v4, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x13, v4, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x15, n32, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x16, sz, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x17, ep, 8));
			if (wda_set("qmapv4", t, n, &txn))
				wda_ok = 1;
		}
		if (!wda_ok) {
			n = tlv_put(t, 0x11, raw, 4);
			n = (uint16_t)(n + tlv_put(t + n, 0x12, qmap, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x13, qmap, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x15, n32, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x16, sz, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x17, ep, 8));
			if (wda_set("qmap", t, n, &txn))
				wda_ok = 1;
		}
		if (!wda_ok) {
			n = tlv_put(t, 0x11, raw, 4);
			n = (uint16_t)(n + tlv_put(t + n, 0x12, z, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x13, z, 4));
			n = (uint16_t)(n + tlv_put(t + n, 0x17, ep, 8));
			if (wda_set("raw-noagg", t, n, &txn))
				wda_ok = 1;
		}
	}

	if (dpm_fd < 0) {
		dpm_fd = open_svc(DPM_SVC, &dpm_node, &dpm_port);
		stamp();
		if (dpm_fd < 0)
			printf("dpm: no server\n");
		else
			printf("dpm: server %u:%u\n", dpm_node, dpm_port);
	}
	if (dpm_fd >= 0) {
		dpm_open(1);
		dpm_open(0);
	}
}

/* If ModemManager already has an IMS bearer, reuse its IPv6. Do not
 * hand-roll WDS — that fights MM's mux. */
static int mm_ims_addr(char *out, size_t cap)
{
	char cmd[64], line[256], apn[64], addr[128];
	FILE *f;
	int b, is_ims;

	out[0] = '\0';
	setenv("DBUS_SYSTEM_BUS_ADDRESS",
	       "unix:path=/var/run/dbus/system_bus_socket", 0);
	for (b = 0; b < 8; b++) {
		snprintf(cmd, sizeof(cmd), "mmcli -b %d 2>/dev/null", b);
		f = popen(cmd, "r");
		if (!f)
			continue;
		is_ims = 0;
		apn[0] = addr[0] = '\0';
		while (fgets(line, sizeof(line), f)) {
			char *p = strstr(line, "apn:");
			if (p) {
				sscanf(p + 4, " %63s", apn);
				if (!strcasecmp(apn, "ims"))
					is_ims = 1;
			}
			p = strstr(line, "address:");
			if (p && strchr(p, ':'))
				sscanf(p + 8, " %127s", addr);
		}
		pclose(f);
		if (is_ims && addr[0]) {
			snprintf(out, cap, "%s", addr);
			stamp();
			printf("mm: IMS bearer %d addr %s\n", b, out);
			return 0;
		}
	}
	return -1;
}

/* Upstream 81voltd asks ModemManager to bring up the IMS APN in
 * response to START, not beforehand. Gold WMS 0x004A stays 0 until
 * CONNECTION_CHANGED. */
static void mm_ims_connect(void)
{
	char line[256];
	FILE *f;

	setenv("DBUS_SYSTEM_BUS_ADDRESS",
	       "unix:path=/var/run/dbus/system_bus_socket", 0);
	stamp();
	printf("mm: simple-connect apn=ims ip-type=ipv6\n");
	f = popen("mmcli -m 0 --simple-connect=apn=ims,ip-type=ipv6 2>&1", "r");
	if (!f)
		return;
	while (fgets(line, sizeof(line), f))
		printf("mm: %s", line);
	pclose(f);
}

/*
 * Prefer MM IMS bearer. Fallback: WDA SET + DPM + WDS START.
 */
static int wds_try_start(const char *apn, int imsd_af,
			 uint16_t p3gpp, uint16_t p3gpp2,
			 char *ip_out, size_t ip_cap, int *wds_fd_out)
{
	uint8_t tlvs[128], resp[512], start_req[128], mux_ep[8], mux_id;
	uint16_t tlen, mlen, txn = 1;
	uint32_t node, port;
	uint8_t fam, sub[4], p8;
	int sock;
	size_t apn_n;

	ip_out[0] = '\0';
	*wds_fd_out = -1;
	if (mm_ims_addr(ip_out, ip_cap) == 0)
		return 0;
	{
		const char *env = getenv("IMS_ADDR");

		if (env && env[0]) {
			snprintf(ip_out, ip_cap, "%s", env);
			stamp();
			printf("ims addr from IMS_ADDR %s\n", ip_out);
			return 0;
		}
	}
	/* MM's second bearer wants mux-id 2; this boot netlink fails
	 * mux 2 (mux 3 add_link works). Skip MM IMS connect. */
	datapath_prepare();

	sock = qrtr_open(0);
	if (sock < 0)
		return -1;
	if (qmi_lookup(sock, WDS_SVC, &node, &port) < 0) {
		stamp();
		printf("wds: no server\n");
		close(sock);
		return -1;
	}
	fam = (imsd_af == IMSD_AF_IPV6) ? 6 : 4;
	stamp();
	printf("wds: server %u:%u apn=%s ipfam=%u 3gpp=%u 3gpp2=%u wda_ok=%d\n",
	       node, port, apn, fam, p3gpp, p3gpp2, wda_ok);

	/* This boot: qmicli link-add mux 3 ok, mux 2/4 fail. */
	memset(mux_ep, 0, sizeof(mux_ep));
	mux_ep[0] = 4;
	mux_ep[4] = 1;
	mux_id = 3;
	tlen = tlv_put(tlvs, 0x10, mux_ep, 8);
	tlen = (uint16_t)(tlen + tlv_put(tlvs + tlen, 0x11, &mux_id, 1));
	if (qmi_txn(sock, node, port, txn++, WDS_BIND_MUX, tlvs, tlen, resp,
		    sizeof(resp), &mlen) == 0)
		stamp(), printf("wds BIND_MUX mux=3 %s\n",
				wds_ok(resp, mlen) ? "ok" : "fail");

	put32(sub, 2);
	tlen = tlv_put(tlvs, 0x01, sub, 4);
	if (qmi_txn(sock, node, port, txn++, WDS_BIND_SUB, tlvs, tlen, resp,
		    sizeof(resp), &mlen) == 0) {
		int ok = wds_ok(resp, mlen);

		stamp(), printf("wds BIND_SUB 2 %s\n", ok ? "ok" : "fail");
		if (!ok) {
			put32(sub, 1);
			tlen = tlv_put(tlvs, 0x01, sub, 4);
			if (qmi_txn(sock, node, port, txn++, WDS_BIND_SUB, tlvs,
				    tlen, resp, sizeof(resp), &mlen) == 0)
				stamp(), printf("wds BIND_SUB 1 %s\n",
						wds_ok(resp, mlen) ? "ok" : "fail");
		}
	}

	tlen = tlv_put(tlvs, 0x01, &fam, 1);
	if (qmi_txn(sock, node, port, txn++, WDS_IPFAM, tlvs, tlen, resp,
		    sizeof(resp), &mlen) == 0)
		stamp(), printf("wds IPFAM %s\n", wds_ok(resp, mlen) ? "ok" : "fail");

	apn_n = strlen(apn);
	if (apn_n > 64)
		apn_n = 64;
	tlen = tlv_put(tlvs, 0x14, apn, (uint16_t)apn_n);
	tlen = (uint16_t)(tlen + tlv_put(tlvs + tlen, 0x19, &fam, 1));
	if (p3gpp != 0xffff) {
		p8 = (uint8_t)p3gpp;
		tlen = (uint16_t)(tlen + tlv_put(tlvs + tlen, 0x31, &p8, 1));
	}
	p8 = (p3gpp2 == 0xffff) ? 0xff : (uint8_t)p3gpp2;
	tlen = (uint16_t)(tlen + tlv_put(tlvs + tlen, 0x32, &p8, 1));
	{
		uint8_t call = 1;

		tlen = (uint16_t)(tlen + tlv_put(tlvs + tlen, 0x35, &call, 1));
	}

	start_req[0] = QMI_REQUEST;
	put16(start_req + 1, txn);
	put16(start_req + 3, WDS_START);
	put16(start_req + 5, tlen);
	memcpy(start_req + 7, tlvs, tlen);
	hexdump("wds START>", start_req, (unsigned)(7 + tlen));

	if (qmi_txn(sock, node, port, txn++, WDS_START, tlvs, tlen, resp,
		    sizeof(resp), &mlen) < 0) {
		stamp();
		printf("wds START: no response\n");
		close(sock);
		return -1;
	}
	if (!wds_ok(resp, mlen)) {
		stamp();
		printf("wds START failed\n");
		close(sock);
		return -1;
	}

	if (qmi_txn(sock, node, port, txn++, WDS_GET_SETTINGS, NULL, 0, resp,
		    sizeof(resp), &mlen) == 0 &&
	    wds_pick_ip(resp, mlen, ip_out, ip_cap) == 0) {
		stamp();
		printf("wds IP %s\n", ip_out);
	} else {
		stamp();
		printf("wds START ok, no address TLV yet\n");
	}
	*wds_fd_out = sock;
	return 0;
}

/* Dylan IMS-QUALCOMM.md QMI msg 5 (svc 770): modem 0x2E REQ
 * TLV 0x10=1 0x11=0; gold RESP is result + TLV 0x10=0x5f (21 B).
 * A result-only 14 B no-op is the one 770 reply that does not match
 * gold; stack init is documented as happening after this exchange. */
static void send_gold_0x2e(const struct qrtr_packet *pkt, uint16_t txn)
{
	uint8_t resp[24];

	resp[0] = QMI_RESPONSE;
	put16(resp + 1, txn);
	put16(resp + 3, 0x002E);
	put16(resp + 5, 14);
	resp[7] = 0x02;
	put16(resp + 8, 4);
	put32(resp + 10, 0);
	resp[14] = 0x10;
	put16(resp + 15, 4);
	put32(resp + 17, 0x5f);
	stamp();
	printf("0x2e gold TLV 0x10=0x5f to %u:%u\n", pkt->node, pkt->port);
	hexdump("0x2e>", resp, 21);
	send_pkt(pkt->node, pkt->port, resp, 21);
}

/* Stock imsdatadaemon, after CONNECTION_CHANGED, sends IMS DCM 0x34
 * as a QMI request to the modem client (pcap #793:
 * TLV 0x01 = 03 00 00 00 00 00 00 00). */
static void send_gold_0x34(const struct qrtr_packet *pkt)
{
	uint8_t req[20];
	uint16_t txn = ind_txn++;

	req[0] = QMI_REQUEST;
	put16(req + 1, txn);
	put16(req + 3, 0x0034);
	put16(req + 5, 11);
	req[7] = 0x01;
	put16(req + 8, 8);
	memset(req + 10, 0, 8);
	req[10] = 3;
	stamp();
	printf("0x34 gold to modem client %u:%u\n", pkt->node, pkt->port);
	hexdump("0x34>", req, 18);
	send_pkt(pkt->node, pkt->port, req, 18);
}

/* ---- QMI handlers ---- */

static void handle_start(const struct qrtr_packet *pkt)
{
	struct imsd_start_connection_req req;
	struct imsd_start_connection_resp resp;
	unsigned txn = 0;
	char ip[64];
	const char *apn;
	int idx, af;

	memset(&req, 0, sizeof(req));
	if (qmi_decode_message(&req, &txn, pkt, QMI_REQUEST,
			       IMSD_START_CONNECTION,
			       imsd_start_connection_req_ei) < 0) {
		stamp();
		printf("START decode failed\n");
		send_error(pkt, txn, 1);
		return;
	}
	req.conn_params.apn[sizeof(req.conn_params.apn) - 1] = '\0';
	apn = req.conn_params.apn[0] ? req.conn_params.apn : "ims";
	af = (int)req.conn_params.ip_family;
	/* Dual-SIM START may carry TLV 0x12 and 0x13. imsd.qmi / stock
	 * IDL echo 0x13 into START resp and CONNECTION_CHANGED. This CT
	 * card on L0: 0x13=1 PRIMARY (IMS Settings Binding 0), 0x12=2.
	 * Do not overwrite 0x13 with 0x12. */
	stamp();
	printf("START conn=%u sub13=%u echo12=%u af=%u apn=%s profiles=%u 3gpp=%u 3gpp2=%u\n",
	       req.connection, req.subscription,
	       req.echo_sub_valid ? req.echo_sub : 0,
	       req.conn_params.ip_family, apn,
	       req.conn_params.profiles_select, req.conn_params.profile_idx_3gpp,
	       req.conn_params.profile_idx_3gpp2);

	remember_client(pkt->node, pkt->port);
	idx = alloc_conn(req.connection, req.subscription, af, apn);
	if (idx < 0) {
		send_error(pkt, txn, 3);
		return;
	}

	memset(&resp, 0, sizeof(resp));
	resp.res.err_status = 0;
	resp.res.err_code = 0;
	resp.connection = conns[idx].local_id;
	resp.orig_conn_id = req.connection;
	resp.subscription = req.subscription;
	encode_send(pkt->node, pkt->port, QMI_RESPONSE, IMSD_START_CONNECTION,
		    (int)txn, &resp, imsd_start_connection_resp_ei);

	if (wds_try_start(apn, af, req.conn_params.profile_idx_3gpp,
			  req.conn_params.profile_idx_3gpp2,
			  ip, sizeof(ip), &conns[idx].wds_fd) == 0) {
		/* Stock imsdatadaemon sends one CONNECTION_CHANGED with
		 * the global IMS IPv6 (OpenIMSd OP6T pcap). A second
		 * fe80 indication is not in that gold. */
		send_changed(&conns[idx], ip[0] ? ip : NULL, 0);
		/* Do not send AP→modem 0x34 here: we already no-op'd the
		 * modem's 0x34 REQ, and a second 0x34 is followed by
		 * DEL_CLIENT. Gold 0x34 is before START, not after
		 * CONNECTION_CHANGED. */
	} else
		send_changed(&conns[idx], NULL, 13);
}

static void handle_stop(const struct qrtr_packet *pkt)
{
	struct imsd_stop_connection_req req;
	struct imsd_stop_connection_resp resp;
	unsigned txn = 0;

	memset(&req, 0, sizeof(req));
	if (qmi_decode_message(&req, &txn, pkt, QMI_REQUEST,
			       IMSD_STOP_CONNECTION,
			       imsd_stop_connection_req_ei) < 0) {
		send_error(pkt, txn, 1);
		return;
	}
	stamp();
	printf("STOP local=%u echo=%u\n", req.connection, req.unkecho_01);
	remember_client(pkt->node, pkt->port);
	if (req.connection < MAX_CONNS) {
		if (conns[req.connection].wds_fd >= 0)
			close(conns[req.connection].wds_fd);
		conns[req.connection].wds_fd = -1;
		conns[req.connection].used = 0;
	}

	memset(&resp, 0, sizeof(resp));
	resp.res.err_status = 0;
	resp.res.err_code = 0;
	resp.unkfield_FA = 0xFA;
	resp.unkecho_01 = req.unkecho_01;
	encode_send(pkt->node, pkt->port, QMI_RESPONSE, IMSD_STOP_CONNECTION,
		    (int)txn, &resp, imsd_stop_connection_resp_ei);
}

static void handle_noop(const struct qrtr_packet *pkt, unsigned msg_id)
{
	struct imsd_no_op_resp resp = {
		.res = { .err_status = 0, .err_code = 0 },
	};
	/* msg 0x23 carries ASCII fe80:: link-local (IMS-QUALCOMM strace). */
	if (msg_id == 0x23 && pkt->data && pkt->data_len > 16) {
		unsigned i;
		const uint8_t *p = pkt->data;
		unsigned n = (unsigned)pkt->data_len;

		for (i = 0; i + 4 < n; i++) {
			if (p[i] == 'f' && p[i + 1] == 'e' && p[i + 2] == '8' &&
			    p[i + 3] == '0') {
				unsigned k = 0;
				while (i + k < n && k < sizeof(last_lladdr) - 1 &&
				       (p[i + k] == ':' ||
					(p[i + k] >= '0' && p[i + k] <= '9') ||
					(p[i + k] >= 'a' && p[i + k] <= 'f')))
					k++;
				memcpy(last_lladdr, p + i, k);
				last_lladdr[k] = '\0';
				stamp();
				printf("0x23 link-local %s\n", last_lladdr);
				break;
			}
		}
	}
	const struct qmi_header {
		uint8_t type;
		uint16_t txn_id;
		uint16_t msg_id;
		uint16_t msg_len;
	} __attribute__((packed)) *hdr = pkt->data;

	stamp();
	printf("no-op msg 0x%x txn %u\n", msg_id, hdr->txn_id);
	remember_client(pkt->node, pkt->port);
	if (msg_id == 0x2e) {
		send_gold_0x2e(pkt, hdr->txn_id);
		return;
	}
	encode_send(pkt->node, pkt->port, QMI_RESPONSE, (int)msg_id,
		    hdr->txn_id, &resp, imsd_no_op_resp_ei);
	/* Gold: modem 0x33 then AP 0x34 (pcap #783/#793). */
	if (msg_id == 0x33)
		send_gold_0x34(pkt);
}

int main(void)
{
	char buf[4096];
	struct sockaddr_qrtr sq;
	struct qrtr_packet pkt;
	socklen_t sl;
	unsigned msg_id;
	int ret;

	setvbuf(stdout, NULL, _IOLBF, 0);
	setvbuf(stderr, NULL, _IOLBF, 0);

	srv_fd = qrtr_open(0);
	if (srv_fd < 0) {
		fprintf(stderr, "qrtr_open failed\n");
		return 1;
	}
	if (qrtr_publish(srv_fd, IMSD_SVC, 1, 0) < 0) {
		fprintf(stderr, "publish service 770 failed\n");
		return 1;
	}
	stamp();
	printf("81voltd up — QRTR service 770 v1 inst 0\n");
	/* When MM owns rmnet/qmapmux, skip WDA/DPM/rmnet bring-up. */

	for (;;) {
		ret = qrtr_poll(srv_fd, (unsigned)-1);
		if (ret < 0) {
			if (errno == EINTR)
				continue;
			fprintf(stderr, "poll: %s\n", strerror(errno));
			break;
		}
		sl = sizeof(sq);
		ret = (int)recvfrom(srv_fd, buf, sizeof(buf), 0, (void *)&sq, &sl);
		if (ret < 0) {
			if (errno == EAGAIN || errno == EINTR)
				continue;
			fprintf(stderr, "recvfrom: %s\n", strerror(errno));
			break;
		}
		if (qrtr_decode(&pkt, buf, (size_t)ret, &sq) < 0)
			continue;
		if (pkt.type == QRTR_TYPE_DEL_CLIENT) {
			stamp();
			printf("DEL_CLIENT %u:%u\n", pkt.node, pkt.port);
			forget_client(pkt.node, pkt.port);
			continue;
		}
		if (pkt.type != QRTR_TYPE_DATA)
			continue;
		hexdump("req<", pkt.data, (unsigned)pkt.data_len);
		if (qmi_decode_header(&pkt, &msg_id) < 0)
			continue;
		if (msg_id == IMSD_START_CONNECTION)
			handle_start(&pkt);
		else if (msg_id == IMSD_STOP_CONNECTION)
			handle_stop(&pkt);
		else
			handle_noop(&pkt, msg_id);
	}
	return 1;
}
