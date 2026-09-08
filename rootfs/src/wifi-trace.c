/* wifi-trace — flip qcacld QDF trace levels at runtime via the QCA vendor
 * command SET_TRACE_LEVEL (152). The join-time SME/scan filter decisions
 * (why does the scan-cache lookup reject our CONNECT profile?) only log at
 * DEBUG level; this raises SME/HDD/SCAN to full mask before a connect
 * attempt. musl-static, no deps.
 *
 * usage: wifi-trace <ifname> [module-id mask]...
 *   (no pairs → defaults: SCAN=21 SME=52 HDD=51, mask 0xff)
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

#define BUF 8192
#define QCA_OUI 0x001374
#define QCA_SUBCMD_SET_TRACE_LEVEL 152
/* QCA_WLAN_VENDOR_ATTR_SET_TRACE_LEVEL_* */
#define ATTR_TL_PARAM 1		/* nested container */
#define ATTR_TL_MODULE_ID 2	/* u32 */
#define ATTR_TL_TRACE_MASK 3	/* u32 */

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
	n->nlmsg_len = NLMSG_HDRLEN + GENL_HDRLEN;
	n->nlmsg_flags = NLM_F_REQUEST | flags;
	n->nlmsg_seq = seq;
	struct genlmsghdr *g = NLMSG_DATA(n);
	g->cmd = cmd;
	g->version = 1;
	return n;
}

static int send_msg(struct nlmsghdr *n, __u16 type)
{
	n->nlmsg_type = type;
	struct sockaddr_nl sa = { .nl_family = AF_NETLINK };
	return sendto(nl_fd, n, n->nlmsg_len, 0, (void *)&sa, sizeof(sa));
}

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
	     rem -= NLA_ALIGN(a->nla_len), a = (void *)((char *)a + NLA_ALIGN(a->nla_len))) {
		void *p = (char *)a + NLA_HDRLEN;
		int plen = a->nla_len - NLA_HDRLEN;
		if (a->nla_type == CTRL_ATTR_FAMILY_ID && plen >= 2)
			id = *(__u16 *)p;
		else if (a->nla_type == CTRL_ATTR_FAMILY_NAME && plen > 0 && plen <= GENL_NAMSIZ) {
			memset(nm, 0, sizeof(nm));
			memcpy(nm, p, plen - 1);
		}
	}
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
	static char rbuf[BUF];
	for (;;) {
		int len = recv(nl_fd, rbuf, sizeof(rbuf), 0);
		if (len < 0) { perror("recv"); return -1; }
		int done = 0;
		for (struct nlmsghdr *r = (void *)rbuf; NLMSG_OK(r, (unsigned)len);
		     r = NLMSG_NEXT(r, len)) {
			if (r->nlmsg_type == NLMSG_DONE) { done = 1; break; }
			if (r->nlmsg_type == NLMSG_ERROR) {
				struct nlmsgerr *e = NLMSG_DATA(r);
				if (e->error) {
					fprintf(stderr, "nlmsg error %d\n", e->error);
					return -1;
				}
				continue;
			}
			family_cb(r);
		}
		if (done) break;
	}
	return want_id;
}

int main(int argc, char **argv)
{
	if (argc < 2) {
		fprintf(stderr, "usage: wifi-trace <ifname> [module-id mask]...\n");
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

	int fam = resolve_family("nl80211");
	if (fam < 0) { fprintf(stderr, "nl80211 family not found\n"); return 1; }

	/* (module, mask) pairs */
	int mods[16], masks[16], np = 0;
	if (argc >= 4) {
		for (int i = 2; i + 1 < argc && np < 16; i += 2) {
			mods[np] = atoi(argv[i]);
			masks[np] = (int)strtoul(argv[i + 1], NULL, 0);
			np++;
		}
	} else {
		/* QDF_MODULE_ID: SCAN=21, HDD=51, SME=52; 0xff = all levels */
		int def[3] = { 21, 51, 52 };
		for (int i = 0; i < 3; i++) { mods[i] = def[i]; masks[i] = 0xff; }
		np = 3;
	}

	/* Build a nested blob: one entry per (module, mask) pair. Each entry
	 * is an NLA_F_NESTED container whose payload holds u32 attrs
	 * MODULE_ID(2) and TRACE_MASK(3) — that is the layout the kernel's
	 * nla_for_each_nested(nla_parse(...)) walk in the handler expects. */
	static char blob[1024];
	int blen = 0;
	for (int i = 0; i < np; i++) {
		char one[64];
		struct nlattr *en = (void *)one;
		en->nla_type = 0x8000;	/* nested, id 0 */
		en->nla_len = NLA_HDRLEN;
		struct nlattr a2 = { .nla_type = ATTR_TL_MODULE_ID, .nla_len = NLA_HDRLEN + 4 };
		struct nlattr a3 = { .nla_type = ATTR_TL_TRACE_MASK, .nla_len = NLA_HDRLEN + 4 };
		memcpy(one + en->nla_len, &a2, NLA_HDRLEN);
		memcpy(one + en->nla_len + NLA_HDRLEN, &mods[i], 4);
		en->nla_len += NLA_HDRLEN + 4;
		memcpy(one + en->nla_len, &a3, NLA_HDRLEN);
		memcpy(one + en->nla_len + NLA_HDRLEN, &masks[i], 4);
		en->nla_len += NLA_HDRLEN + 4;
		memcpy(blob + blen, one, en->nla_len);
		blen += NLA_ALIGN(en->nla_len);
	}

	struct nlmsghdr *n = mkmsg(NL80211_CMD_VENDOR, NLM_F_ACK, 7);
	__u32 oui = QCA_OUI, sub = QCA_SUBCMD_SET_TRACE_LEVEL;
	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &ifindex, 4);
	nla_put(n, BUF, NL80211_ATTR_VENDOR_ID, &oui, 4);
	nla_put(n, BUF, NL80211_ATTR_VENDOR_SUBCMD, &sub, 4);
	/* VENDOR_DATA payload = PARAM(1) nested attr wrapping the entries blob */
	static char param[1024 + NLA_HDRLEN];
	struct nlattr pn = { .nla_type = 0x8000 | ATTR_TL_PARAM, .nla_len = NLA_HDRLEN + blen };
	memcpy(param, &pn, NLA_HDRLEN);
	memcpy(param + NLA_HDRLEN, blob, blen);
	nla_put(n, BUF, NL80211_ATTR_VENDOR_DATA, param, NLA_ALIGN(pn.nla_len));
	if (send_msg(n, fam) < 0) { perror("sendto"); return 1; }

	static char rbuf[BUF];
	int len = recv(nl_fd, rbuf, sizeof(rbuf), 0);
	if (len < 0) { perror("recv"); return 1; }
	for (struct nlmsghdr *r = (void *)rbuf; NLMSG_OK(r, (unsigned)len);
	     r = NLMSG_NEXT(r, len)) {
		if (r->nlmsg_type == NLMSG_ERROR) {
			struct nlmsgerr *e = NLMSG_DATA(r);
			if (e->error) {
				fprintf(stderr, "vendor cmd failed: %d (%s)\n",
					e->error, strerror(-e->error));
				return 1;
			}
			printf("trace levels set:");
			for (int i = 0; i < np; i++)
				printf(" mod %d mask %#x", mods[i], masks[i]);
			printf("\n");
			return 0;
		}
	}
	fprintf(stderr, "no reply\n");
	return 1;
}
