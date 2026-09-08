/* nlscan — minimal nl80211 scan client (trigger + dump) for the AginxOS
 * initramfs. Proof-of-life for wlan0: if the firmware/RF chain works, this
 * prints visible APs. musl-static, no deps.
 *
 * usage: nlscan <ifname>
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <net/if.h>
#include <sys/socket.h>
#include <linux/netlink.h>
#include <linux/genetlink.h>
#include <linux/nl80211.h>

#define BUF 32768

static int nl_fd;

static struct nlattr *nla_put(struct nlmsghdr *n, int maxlen, __u16 type,
			      const void *data, int len)
{
	if ((int)NLMSG_ALIGN(n->nlmsg_len) + (int)NLA_HDRLEN + NLA_ALIGN(len) > maxlen)
		return NULL;
	struct nlattr *a = (void *)((char *)n + NLMSG_ALIGN(n->nlmsg_len));
	a->nla_type = type;
	a->nla_len = NLA_HDRLEN + len;
	memcpy((char *)a + NLA_HDRLEN, data, len);
	n->nlmsg_len = NLMSG_ALIGN(n->nlmsg_len) + NLA_ALIGN(a->nla_len);
	return a;
}

static struct nlmsghdr *mkmsg(__u8 cmd, __u16 flags, int seq)
{
	static char buf[BUF];
	struct nlmsghdr *n = (void *)buf;
	memset(buf, 0, sizeof(buf));
	n->nlmsg_len = NLMSG_HDRLEN;
	n->nlmsg_type = 0; /* filled by send_msg */
	n->nlmsg_flags = NLM_F_REQUEST | flags;
	n->nlmsg_seq = seq;
	n->nlmsg_pid = 0;
	struct genlmsghdr *g = NLMSG_DATA(n);
	g->cmd = cmd;
	g->version = 1;
	/* payload starts AFTER the 4-byte genlmsghdr — nlmsg_len must include
	 * it or the kernel rejects the message / attrs overwrite the header */
	n->nlmsg_len = NLMSG_HDRLEN + GENL_HDRLEN;
	return n;
}

static int send_msg(struct nlmsghdr *n, __u16 type)
{
	n->nlmsg_type = type;
	struct sockaddr_nl sa = { .nl_family = AF_NETLINK };
	return sendto(nl_fd, n, n->nlmsg_len, 0, (void *)&sa, sizeof(sa));
}

/* Iterate netlink reply messages; returns 0 when dump finished OK, -1 on
 * NLMSG_ERROR (printed), 1 if more messages may follow. */
static int recv_one(void (*cb)(struct nlmsghdr *))
{
	static char rbuf[BUF];
	int len = recv(nl_fd, rbuf, sizeof(rbuf), 0);
	if (len < 0) { perror("recv"); return -1; }
	int done = 0;
	for (struct nlmsghdr *r = (void *)rbuf; NLMSG_OK(r, (unsigned)len);
	     r = NLMSG_NEXT(r, len)) {
		if (r->nlmsg_type == NLMSG_ERROR) {
			struct nlmsgerr *e = NLMSG_DATA(r);
			if (e->error == 0)	/* plain ACK */
				continue;
			fprintf(stderr, "nlmsg error %d (%s)\n", e->error,
				strerror(-e->error));
			return -1;
		}
		if (r->nlmsg_type == NLMSG_DONE) { done = 1; break; }
		if (cb)
			cb(r);
	}
	return done ? 0 : 1;
}

/* Resolve a genl family id via CTRL_CMD_GETFAMILY *dump* — this kernel
 * rejects a plain doit GETFAMILY with -EOPNOTSUPP, so resolve the way
 * libnl does: dump all families, match by name client-side. Prints the
 * family list as a side effect. Returns family id or -1. */
static const char *want_name;
static int want_id = -1;

static void family_cb(struct nlmsghdr *r)
{
	if (r->nlmsg_type != GENL_ID_CTRL)
		return;
	int id = -1;
	char nm[GENL_NAMSIZ] = "?";
	struct nlattr *a = (void *)((char *)NLMSG_DATA(r) + GENL_HDRLEN);
	int rem = r->nlmsg_len - NLMSG_HDRLEN - GENL_HDRLEN;
	for (; rem >= (int)NLA_HDRLEN && a->nla_len >= NLA_HDRLEN && a->nla_len <= rem;
	     rem -= NLA_ALIGN(a->nla_len),
	     a = (void *)((char *)a + NLA_ALIGN(a->nla_len))) {
		void *p = (char *)a + NLA_HDRLEN;
		int plen = a->nla_len - NLA_HDRLEN;
		if (a->nla_type == CTRL_ATTR_FAMILY_ID && plen >= 2)
			id = *(__u16 *)p;
		else if (a->nla_type == CTRL_ATTR_FAMILY_NAME &&
			 plen > 0 && plen <= GENL_NAMSIZ) {
			memset(nm, 0, sizeof(nm));
			memcpy(nm, p, plen - 1);
		}
	}
	fprintf(stderr, "genl family %3d: %s\n", id, nm);
	if (id > 0 && want_name && !strcmp(nm, want_name))
		want_id = id;
}

static int resolve_family(const char *name)
{
	static int seq = 100;
	want_name = name;
	want_id = -1;
	struct nlmsghdr *n = mkmsg(CTRL_CMD_GETFAMILY, NLM_F_DUMP, ++seq);
	if (send_msg(n, GENL_ID_CTRL) < 0) { perror("sendto"); return -1; }
	for (;;) {
		int rc = recv_one(family_cb);
		if (rc <= 0) break;
	}
	return want_id;
}

/* trigger scan; returns 0 on ACK, -1 on NLMSG_ERROR */
static int trigger_scan(int fam, unsigned ifindex)
{
	static int seq = 200;
	/* NLM_F_ACK: without it a successful trigger gets no reply at all */
	struct nlmsghdr *n = mkmsg(NL80211_CMD_TRIGGER_SCAN, NLM_F_ACK, ++seq);
	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &ifindex, 4);

	if (send_msg(n, fam) < 0) { perror("sendto"); return -1; }
	return recv_one(NULL) < 0 ? -1 : 0;
}

static void parse_bss(struct nlattr *bss)
{
	unsigned char bssid[6] = {0};
	__s32 sig = 0;
	__u32 freq = 0;
	const char *ssid = NULL;
	int ssid_len = 0;

	int rem = NLA_ALIGN(bss->nla_len) - NLA_HDRLEN;
	struct nlattr *a = (void *)((char *)bss + NLA_HDRLEN);
	for (; rem >= (int)NLA_HDRLEN && a->nla_len >= NLA_HDRLEN && a->nla_len <= rem;
	     rem -= NLA_ALIGN(a->nla_len),
	     a = (void *)((char *)a + NLA_ALIGN(a->nla_len))) {
		void *p = (char *)a + NLA_HDRLEN;
		switch (a->nla_type) {
		case NL80211_BSS_BSSID:
			memcpy(bssid, p, 6);
			break;
		case NL80211_BSS_FREQUENCY:
			freq = *(__u32 *)p;
			break;
		case NL80211_BSS_SIGNAL_MBM:
			sig = *(__s32 *)p;
			break;
		case NL80211_BSS_INFORMATION_ELEMENTS: {
			int ielen = a->nla_len - NLA_HDRLEN;
			unsigned char *ie = p;
			while (ielen >= 2 && ielen >= 2 + ie[1]) {
				if (ie[0] == 0) { /* SSID */
					ssid = (char *)ie + 2;
					ssid_len = ie[1];
				}
				ielen -= 2 + ie[1];
				ie += 2 + ie[1];
			}
			break;
		}
		}
	}

	printf("%02x:%02x:%02x:%02x:%02x:%02x  ch=%-2d  %4d.%02d dBm  ",
	       bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5],
	       freq >= 5000 ? (freq - 5000) / 5 : freq > 2412 ? (freq - 2407) / 5 : 0,
	       sig / 100, abs(sig % 100));
	if (ssid_len <= 0) {
		printf("<hidden>");
	} else {
		/* SSIDs are arbitrary bytes, not text — hex-escape the
		 * non-printables so a CJK name stays joinable (wifi-join
		 * needs the exact bytes; '?' destroyed them). */
		for (int i = 0; i < ssid_len; i++) {
			unsigned char c = ssid[i];
			if (c >= 32 && c < 127)
				putchar(c);
			else
				printf("\\x%02x", c);
		}
	}
	putchar('\n');
}

static int bss_count;

static void scan_cb(struct nlmsghdr *r)
{
	if (r->nlmsg_type == NLMSG_ERROR || r->nlmsg_type == NLMSG_DONE)
		return;
	struct nlattr *a = (void *)((char *)NLMSG_DATA(r) + GENL_HDRLEN);
	int rem = r->nlmsg_len - NLMSG_HDRLEN - GENL_HDRLEN;
	for (; rem >= (int)NLA_HDRLEN && a->nla_len >= NLA_HDRLEN && a->nla_len <= rem;
	     rem -= NLA_ALIGN(a->nla_len),
	     a = (void *)((char *)a + NLA_ALIGN(a->nla_len))) {
		if (a->nla_type == NL80211_ATTR_BSS) {
			parse_bss(a);
			bss_count++;
		}
	}
}

static int dump_scan(int fam, unsigned ifindex)
{
	static int seq = 300;
	bss_count = 0;
	struct nlmsghdr *n = mkmsg(NL80211_CMD_GET_SCAN, NLM_F_DUMP, ++seq);
	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &ifindex, 4);
	if (send_msg(n, fam) < 0) { perror("sendto"); return -1; }
	for (;;) {
		int rc = recv_one(scan_cb);
		if (rc < 0) return -1;
		if (rc == 0) break;
	}
	return bss_count;
}

int main(int argc, char **argv)
{
	if (argc < 2) {
		fprintf(stderr, "usage: nlscan <ifname>\n");
		return 2;
	}
	unsigned ifindex = if_nametoindex(argv[1]);
	if (!ifindex) {
		fprintf(stderr, "if_nametoindex(%s): %s\n", argv[1], strerror(errno));
		return 2;
	}

	nl_fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_GENERIC);
	if (nl_fd < 0) { perror("netlink socket"); return 1; }
	struct sockaddr_nl la = { .nl_family = AF_NETLINK };
	if (bind(nl_fd, (void *)&la, sizeof(la)) < 0) { perror("bind"); return 1; }
	int rcvbuf = BUF * 4;
	setsockopt(nl_fd, SOL_SOCKET, SO_RCVBUF, &rcvbuf, sizeof(rcvbuf));

	int fam = resolve_family("nl80211");
	if (fam < 0) {
		fprintf(stderr, "nl80211 genl family not found\n");
		return 1;
	}
	printf("nl80211 family id %d, ifindex %u — triggering scan\n", fam, ifindex);

	if (trigger_scan(fam, ifindex) < 0)
		return 1;
	printf("scan triggered, waiting...\n");

	/* qcacld needs a few seconds; poll the results table */
	for (int try = 0; try < 8; try++) {
		sleep(try ? 2 : 3);
		int n = dump_scan(fam, ifindex);
		if (n > 0) {
			printf("--- %d BSS ---\n", n);
			return 0;
		}
		if (n < 0) return 1;
	}
	printf("scan results table still empty\n");
	return 1;
}
