/* wifi-join — minimal nl80211 STA connect client with WPA2-PSK
 * 4-way handshake, for the AginxOS initramfs. No libnl, no wpa_supplicant:
 * CONNECT (auth+assoc in fw) → EAPOL M1..M4 over AF_PACKET → NEW_KEY install.
 * CCMP pairwise; group cipher mirrored from the AP (TKIP on WPA2-mixed).
 * Crypto (SHA-1/HMAC/PBKDF2/AES-128/RFC 3394 unwrap) is embedded below.
 *
 * usage: wifi-join <ifname> <ssid> <passphrase>
 * exit: 0 associated+keys installed, 2 ssid not in scan, 3 handshake timeout,
 *       4 assoc refused, 5 MIC/unwrap failure, 1 other error.
 * IP provisioning is udhcpc's job (see usr/share/udhcpc/default.script).
 */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <arpa/inet.h>
#include <time.h>
#include <net/if.h>
#include <sys/socket.h>
#include <sys/ioctl.h>
#include <linux/netlink.h>
#include <linux/genetlink.h>
#include <linux/nl80211.h>
#include <linux/if_packet.h>
#include <linux/if_ether.h>

#define BUF 32768
#define debug(...) do { if (vflag) fprintf(stderr, __VA_ARGS__); } while (0)
static int vflag = 1;

/* ------------------------------------------------------------------ */
/* SHA-1 / HMAC-SHA1 / PBKDF2 / 802.11 PRF                            */
/* ------------------------------------------------------------------ */

typedef struct {
	uint32_t h[5];
	uint64_t len;
	unsigned char buf[64];
	size_t blen;
} sha1_t;

static uint32_t rol(uint32_t x, int n) { return (x << n) | (x >> (32 - n)); }

static void sha1_init(sha1_t *c)
{
	c->h[0] = 0x67452301; c->h[1] = 0xefcdab89; c->h[2] = 0x98badcfe;
	c->h[3] = 0x10325476; c->h[4] = 0xc3d2e1f0;
	c->len = 0; c->blen = 0;
}

static void sha1_block(sha1_t *c, const unsigned char *p)
{
	uint32_t w[80], a, b, cc, d, e, t;
	for (int i = 0; i < 16; i++)
		w[i] = (p[i*4] << 24) | (p[i*4+1] << 16) | (p[i*4+2] << 8) | p[i*4+3];
	for (int i = 16; i < 80; i++)
		w[i] = rol(w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16], 1);
	a = c->h[0]; b = c->h[1]; cc = c->h[2]; d = c->h[3]; e = c->h[4];
	for (int i = 0; i < 80; i++) {
		if (i < 20)      t = (b & cc) | ((~b) & d);
		else if (i < 40) t = b ^ cc ^ d;
		else if (i < 60) t = (b & cc) | (b & d) | (cc & d);
		else             t = b ^ cc ^ d;
		t += e + rol(a, 5) + w[i] +
		     (i < 20 ? 0x5a827999 : i < 40 ? 0x6ed9eba1 :
		      i < 60 ? 0x8f1bbcdc : 0xca62c1d6);
		e = d; d = cc; cc = rol(b, 30); b = a; a = t;
	}
	c->h[0] += a; c->h[1] += b; c->h[2] += cc; c->h[3] += d; c->h[4] += e;
}

static void sha1_update(sha1_t *c, const void *data, size_t n)
{
	const unsigned char *p = data;
	c->len += n;
	while (n) {
		size_t k = 64 - c->blen;
		if (k > n) k = n;
		memcpy(c->buf + c->blen, p, k);
		c->blen += k; p += k; n -= k;
		if (c->blen == 64) { sha1_block(c, c->buf); c->blen = 0; }
	}
}

static void sha1_final(sha1_t *c, unsigned char out[20])
{
	uint64_t bits = c->len * 8;
	unsigned char pad = 0x80;
	sha1_update(c, &pad, 1);
	pad = 0;
	while (c->blen != 56)
		sha1_update(c, &pad, 1);
	unsigned char lb[8];
	for (int i = 0; i < 8; i++)
		lb[i] = (bits >> (56 - 8 * i)) & 0xff;
	sha1_update(c, lb, 8);
	for (int i = 0; i < 5; i++) {
		out[i*4]   = c->h[i] >> 24;
		out[i*4+1] = c->h[i] >> 16;
		out[i*4+2] = c->h[i] >> 8;
		out[i*4+3] = c->h[i];
	}
}

static void sha1(const void *d, size_t n, unsigned char out[20])
{
	sha1_t c; sha1_init(&c); sha1_update(&c, d, n); sha1_final(&c, out);
}

static void hmac_sha1(const unsigned char *key, size_t klen,
		      const unsigned char *msg, size_t mlen,
		      unsigned char out[20])
{
	unsigned char k[64], ipad[64], opad[64], kd[20];
	sha1_t c;
	memset(k, 0, 64);
	if (klen > 64) sha1(key, klen, k);
	else memcpy(k, key, klen);
	for (int i = 0; i < 64; i++) {
		ipad[i] = k[i] ^ 0x36;
		opad[i] = k[i] ^ 0x5c;
	}
	sha1_init(&c); sha1_update(&c, ipad, 64); sha1_update(&c, msg, mlen);
	sha1_final(&c, kd);
	sha1_init(&c); sha1_update(&c, opad, 64); sha1_update(&c, kd, 20);
	sha1_final(&c, out);
}

/* WPA PMK: PBKDF2-HMAC-SHA1(pass, ssid, 4096, 32) */
static void pbkdf2_sha1(const char *pass, size_t plen,
			const char *salt, size_t slen,
			int iters, size_t dklen, unsigned char *out)
{
	unsigned char u[20], t[20];
	for (size_t blk = 1; dklen; blk++, out += (dklen < 20 ? dklen : 20),
					   dklen -= (dklen < 20 ? dklen : 20)) {
		unsigned char sb[slen + 4];
		memcpy(sb, salt, slen);
		sb[slen]   = blk >> 24; sb[slen+1] = blk >> 16;
		sb[slen+2] = blk >> 8;  sb[slen+3] = blk;
		hmac_sha1((const unsigned char *)pass, plen, sb, slen + 4, t);
		memcpy(u, t, 20);
		for (int i = 1; i < iters; i++) {
			hmac_sha1((const unsigned char *)pass, plen, u, 20, u);
			for (int j = 0; j < 20; j++) t[j] ^= u[j];
		}
		memcpy(out, t, dklen < 20 ? dklen : 20);
	}
}

/* 802.11 PRF (FIPS 186-2 style): T(i) = HMAC-SHA1(K, label || 0 || data || i),
 * i one byte — the NUL after the label is part of the construction
 * (wpa_supplicant's sha1_prf hashes strlen(label)+1 bytes); omitting it
 * derives a wrong PTK and the AP rejects every M2 MIC. */
static void sha1_prf(const unsigned char *key, size_t klen, const char *label,
		     const unsigned char *data, size_t dlen,
		     unsigned char *out, size_t olen)
{
	size_t ll = strlen(label) + 1;	/* include NUL: PRF hashes label||0||data||i */
	unsigned char buf[256 + 1];
	memcpy(buf, label, ll);
	memcpy(buf + ll, data, dlen);
	for (size_t i = 0; i < (olen + 19) / 20; i++) {
		buf[ll + dlen] = (unsigned char)i;
		hmac_sha1(key, klen, buf, ll + dlen + 1, out + i * 20);
	}
}

/* ------------------------------------------------------------------ */
/* AES-128 decrypt + RFC 3394 key unwrap                               */
/* ------------------------------------------------------------------ */

static const unsigned char aes_sbox[256] = {
0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16 };

static const unsigned char aes_inv_sbox[256] = {
0x52,0x09,0x6a,0xd5,0x30,0x36,0xa5,0x38,0xbf,0x40,0xa3,0x9e,0x81,0xf3,0xd7,0xfb,
0x7c,0xe3,0x39,0x82,0x9b,0x2f,0xff,0x87,0x34,0x8e,0x43,0x44,0xc4,0xde,0xe9,0xcb,
0x54,0x7b,0x94,0x32,0xa6,0xc2,0x23,0x3d,0xee,0x4c,0x95,0x0b,0x42,0xfa,0xc3,0x4e,
0x08,0x2e,0xa1,0x66,0x28,0xd9,0x24,0xb2,0x76,0x5b,0xa2,0x49,0x6d,0x8b,0xd1,0x25,
0x72,0xf8,0xf6,0x64,0x86,0x68,0x98,0x16,0xd4,0xa4,0x5c,0xcc,0x5d,0x65,0xb6,0x92,
0x6c,0x70,0x48,0x50,0xfd,0xed,0xb9,0xda,0x5e,0x15,0x46,0x57,0xa7,0x8d,0x9d,0x84,
0x90,0xd8,0xab,0x00,0x8c,0xbc,0xd3,0x0a,0xf7,0xe4,0x58,0x05,0xb8,0xb3,0x45,0x06,
0xd0,0x2c,0x1e,0x8f,0xca,0x3f,0x0f,0x02,0xc1,0xaf,0xbd,0x03,0x01,0x13,0x8a,0x6b,
0x3a,0x91,0x11,0x41,0x4f,0x67,0xdc,0xea,0x97,0xf2,0xcf,0xce,0xf0,0xb4,0xe6,0x73,
0x96,0xac,0x74,0x22,0xe7,0xad,0x35,0x85,0xe2,0xf9,0x37,0xe8,0x1c,0x75,0xdf,0x6e,
0x47,0xf1,0x1a,0x71,0x1d,0x29,0xc5,0x89,0x6f,0xb7,0x62,0x0e,0xaa,0x18,0xbe,0x1b,
0xfc,0x56,0x3e,0x4b,0xc6,0xd2,0x79,0x20,0x9a,0xdb,0xc0,0xfe,0x78,0xcd,0x5a,0xf4,
0x1f,0xdd,0xa8,0x33,0x88,0x07,0xc7,0x31,0xb1,0x12,0x10,0x59,0x27,0x80,0xec,0x5f,
0x60,0x51,0x7f,0xa9,0x19,0xb5,0x4a,0x0d,0x2d,0xe5,0x7a,0x9f,0x93,0xc9,0x9c,0xef,
0xa0,0xe0,0x3b,0x4d,0xae,0x2a,0xf5,0xb0,0xc8,0xeb,0xbb,0x3c,0x83,0x53,0x99,0x61,
0x17,0x2b,0x04,0x7e,0xba,0x77,0xd6,0x26,0xe1,0x69,0x14,0x63,0x55,0x21,0x0c,0x7d };

static uint8_t xt(uint8_t a)	/* GF(2^8) xtime */
{
	return (uint8_t)((a << 1) ^ ((a & 0x80) ? 0x1b : 0));
}

static uint8_t gmul(uint8_t a, uint8_t b)
{
	uint8_t r = 0;
	while (b) {
		if (b & 1) r ^= a;
		a = xt(a);
		b >>= 1;
	}
	return r;
}

typedef struct { uint32_t rk[44]; } aes128_t;

static void aes128_key(aes128_t *a, const unsigned char key[16])
{
	static const uint8_t rc[10] = {0x01,0x02,0x04,0x08,0x10,0x20,0x40,0x80,0x1b,0x36};
	for (int i = 0; i < 4; i++)
		a->rk[i] = (key[i*4] << 24) | (key[i*4+1] << 16) |
			   (key[i*4+2] << 8) | key[i*4+3];
	for (int i = 4; i < 44; i++) {
		uint32_t t = a->rk[i-1];
		if (i % 4 == 0) {
			t = (t << 8) | (t >> 24);			/* rotword */
			t = (aes_sbox[(t >> 24) & 0xff] << 24) |
			    (aes_sbox[(t >> 16) & 0xff] << 16) |
			    (aes_sbox[(t >> 8) & 0xff] << 8) |
			    aes_sbox[t & 0xff];
			t ^= (uint32_t)rc[i/4 - 1] << 24;
		}
		a->rk[i] = a->rk[i-4] ^ t;
	}
}

static void aes128_dec(const aes128_t *a, const unsigned char in[16],
		       unsigned char out[16])
{
	uint8_t s[16];
	memcpy(s, in, 16);
	/* addroundkey(10) then 9 rounds of inv */
	for (int i = 0; i < 16; i++)
		s[i] ^= (a->rk[40 + i/4] >> (24 - 8*(i%4))) & 0xff;
	for (int r = 9; r >= 1; r--) {
		/* inv shiftrows */
		uint8_t t;
		t = s[13]; s[13] = s[9]; s[9] = s[5]; s[5] = s[1]; s[1] = t;
		t = s[2];  s[2]  = s[10]; s[10] = t; t = s[6]; s[6] = s[14]; s[14] = t;
		t = s[3];  s[3]  = s[7];  s[7]  = s[11]; s[11] = s[15]; s[15] = t;
		for (int i = 0; i < 16; i++) s[i] = aes_inv_sbox[s[i]];
		for (int i = 0; i < 16; i++)
			s[i] ^= (a->rk[r*4 + i/4] >> (24 - 8*(i%4))) & 0xff;
		/* inv mixcolumns in every round r=9..1 (final round has none) */
		for (int c = 0; c < 4; c++) {
			uint8_t *p = s + c*4;
			uint8_t a0=p[0],a1=p[1],a2=p[2],a3=p[3];
			p[0] = gmul(a0,14) ^ gmul(a1,11) ^ gmul(a2,13) ^ gmul(a3,9);
			p[1] = gmul(a0,9)  ^ gmul(a1,14) ^ gmul(a2,11) ^ gmul(a3,13);
			p[2] = gmul(a0,13) ^ gmul(a1,9)  ^ gmul(a2,14) ^ gmul(a3,11);
			p[3] = gmul(a0,11) ^ gmul(a1,13) ^ gmul(a2,9)  ^ gmul(a3,14);
		}
	}
	/* final round (r=0 style): inv shiftrows, inv subbytes, addroundkey(0) */
	uint8_t t;
	t = s[13]; s[13] = s[9]; s[9] = s[5]; s[5] = s[1]; s[1] = t;
	t = s[2];  s[2]  = s[10]; s[10] = t; t = s[6]; s[6] = s[14]; s[14] = t;
	t = s[3];  s[3]  = s[7];  s[7]  = s[11]; s[11] = s[15]; s[15] = t;
	for (int i = 0; i < 16; i++) s[i] = aes_inv_sbox[s[i]];
	for (int i = 0; i < 16; i++)
		s[i] ^= (a->rk[i/4] >> (24 - 8*(i%4))) & 0xff;
	memcpy(out, s, 16);
}

/* RFC 3394 unwrap; returns 0 ok, -1 integrity */
static int aes_unwrap(const unsigned char kek[16], const unsigned char *in,
		      size_t inlen, unsigned char *out)
{
	if (inlen < 16 || inlen % 8) return -1;
	size_t n = inlen / 8 - 1;
	unsigned char a[8], r[64][8];
	aes128_t k;
	aes128_key(&k, kek);
	memcpy(a, in, 8);
	for (size_t i = 0; i < n; i++) memcpy(r[i], in + 8 + i*8, 8);
	for (int j = 5; j >= 0; j--) {
		for (size_t i = n; i >= 1; i--) {
			unsigned char blk[16], b[16];
			uint64_t t = (uint64_t)n * j + i;
			for (int x = 0; x < 8; x++) blk[x] = a[x] ^ ((t >> (56 - 8*x)) & 0xff);
			memcpy(blk + 8, r[i-1], 8);
			aes128_dec(&k, blk, b);
			memcpy(a, b, 8);
			memcpy(r[i-1], b + 8, 8);
			if (i == 1) break;	/* size_t underflow guard */
		}
	}
	for (int x = 0; x < 8; x++)
		if (a[x] != 0xa6) return -1;
	for (size_t i = 0; i < n; i++) memcpy(out + i*8, r[i], 8);
	return 0;
}

/* ------------------------------------------------------------------ */
/* generic netlink                                                     */
/* ------------------------------------------------------------------ */

static int nl_fd, nl80211_fam = -1;

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

static int nl_send(struct nlmsghdr *n, __u16 type)
{
	n->nlmsg_type = type;	/* 0 = NLMSG_NOOP — kernel drops silently */
	struct sockaddr_nl sa = { .nl_family = AF_NETLINK };
	return sendto(nl_fd, n, n->nlmsg_len, 0, (void *)&sa, sizeof(sa));
}

/* per-message callback returns 0 to keep reading, 1 to stop */
static int nl_read_loop(int (*cb)(struct nlmsghdr *, void *), void *arg,
			int timeout_ms)
{
	struct pollfd pf = { .fd = nl_fd, .events = POLLIN };
	int waited = 0;
	for (;;) {
		int rc = poll(&pf, 1, 200);
		if (rc < 0) { perror("poll"); return -1; }
		if (rc == 0) {
			waited += 200;
			if (timeout_ms > 0 && waited >= timeout_ms) return 0;
			continue;
		}
		static char rbuf[BUF];
		int len = recv(nl_fd, rbuf, sizeof(rbuf), 0);
		if (len < 0) { perror("recv"); return -1; }
		for (struct nlmsghdr *r = (void *)rbuf; NLMSG_OK(r, (unsigned)len);
		     r = NLMSG_NEXT(r, len)) {
			if (r->nlmsg_type == NLMSG_DONE) continue;
			if (r->nlmsg_type == NLMSG_ERROR) {
				struct nlmsgerr *e = NLMSG_DATA(r);
				if (e->error) {
					fprintf(stderr, "nlmsg error %d (%s)\n",
						e->error, strerror(-e->error));
					return -1;
				}
				continue;
			}
			if (cb && cb(r, arg) == 1) return 1;
		}
	}
}

static int fam_cb(struct nlmsghdr *r, void *arg)
{
	if (r->nlmsg_type != GENL_ID_CTRL) return 0;
	int id = -1;
	char nm[GENL_NAMSIZ] = "?";
	struct nlattr *a = (void *)((char *)NLMSG_DATA(r) + GENL_HDRLEN);
	int rem = r->nlmsg_len - NLMSG_HDRLEN - GENL_HDRLEN;
	int *mlme_grp = arg;
	for (; rem >= (int)NLA_HDRLEN && a->nla_len >= NLA_HDRLEN && a->nla_len <= rem;
	     rem -= NLA_ALIGN(a->nla_len), a = (void *)((char *)a + NLA_ALIGN(a->nla_len))) {
		void *p = (char *)a + NLA_HDRLEN;
		int plen = a->nla_len - NLA_HDRLEN;
		if (a->nla_type == CTRL_ATTR_FAMILY_ID && plen >= 2)
			id = *(__u16 *)p;
		else if (a->nla_type == CTRL_ATTR_FAMILY_NAME && plen > 0 && plen <= GENL_NAMSIZ) {
			memset(nm, 0, sizeof(nm));
			memcpy(nm, p, plen - 1);
		} else if (a->nla_type == CTRL_ATTR_MCAST_GROUPS && *mlme_grp < 0) {
			struct nlattr *g = p;
			int grem = plen;
			for (; grem >= (int)NLA_HDRLEN && g->nla_len >= (int)NLA_HDRLEN && g->nla_len <= grem;
			     grem -= NLA_ALIGN(g->nla_len), g = (void *)((char *)g + NLA_ALIGN(g->nla_len))) {
				/* each group: nested attrs NAME / GRP_ID */
				struct nlattr *e = (void *)((char *)g + NLA_HDRLEN);
				int erem = NLA_ALIGN(g->nla_len) - NLA_HDRLEN;
				char gname[64] = "";
				int gid = -1;
				for (; erem >= (int)NLA_HDRLEN && e->nla_len >= (int)NLA_HDRLEN && e->nla_len <= erem;
				     erem -= NLA_ALIGN(e->nla_len), e = (void *)((char *)e + NLA_ALIGN(e->nla_len))) {
					void *ep = (char *)e + NLA_HDRLEN;
					int epl = e->nla_len - NLA_HDRLEN;
					if (e->nla_type == CTRL_ATTR_MCAST_GRP_NAME && epl > 0 && epl < 64) {
						memcpy(gname, ep, epl);
						gname[epl] = 0;
					} else if (e->nla_type == CTRL_ATTR_MCAST_GRP_ID && epl >= 4)
						gid = *(__u32 *)ep;
				}
				debug("mcast grp %d %s\n", gid, gname);
				if (!strcmp(gname, "mlme") && gid >= 0)
					*mlme_grp = gid;
			}
		}
	}
	debug("genl family %3d: %s\n", id, nm);
	if (id > 0 && !strcmp(nm, "nl80211")) {
		nl80211_fam = id;
		return 1;
	}
	return 0;
}

static int join_group(const char *name)
{
	struct nlmsghdr *n = mkmsg(CTRL_CMD_GETFAMILY, NLM_F_DUMP, 9990);
	nl_send(n, GENL_ID_CTRL);
	int grp = -1;
	nl_read_loop(fam_cb, &grp, 3000);
	if (grp >= 0)
		setsockopt(nl_fd, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, &grp, 4);
	return grp;
}

/* ------------------------------------------------------------------ */
/* scan: find BSSID+freq for ssid                                      */
/* ------------------------------------------------------------------ */

struct target { const char *ssid; unsigned ifindex; unsigned char bssid[6];
		__u32 freq; int found; __s32 sig;
		__u32 grpcipher; int grp_known; int bss_sae; };

/* RSN IE: CCMP/CCMP/PSK, MFPC. Body is exactly 20 bytes:
 * ver(2)+grp(4)+pcnt(2)+pair(4)+acnt(2)+akm(4)+caps(2) */
static const unsigned char rsne[] = {
	0x30, 0x14,
	0x01, 0x00,			/* version 1 */
	0x00, 0x0f, 0xac, 0x04,		/* group CCMP */
	0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, /* 1 x pairwise CCMP */
	0x01, 0x00, 0x00, 0x0f, 0xac, 0x02, /* 1 x AKM PSK */
	0x00, 0x00			/* rsn caps — plain WPA2: Apple hotspots
					 * MIC-fail an M2 whose RSNE carries
					 * PMF caps they never offered */
};

static void parse_bss(struct nlattr *bss, struct target *t)
{
	unsigned char bssid[6] = {0};
	__u32 freq = 0;
	__s32 sig = -100000;
	const char *ssid = NULL;
	int ssid_len = 0;
	__u16 caps = 0;
	int mdie = 0, ctry = 0, capp = 0;
	int rem = NLA_ALIGN(bss->nla_len) - NLA_HDRLEN;
	unsigned char *rsn = NULL; int rsn_len = 0;
	struct nlattr *a = (void *)((char *)bss + NLA_HDRLEN);
	for (; rem >= (int)NLA_HDRLEN && a->nla_len >= (int)NLA_HDRLEN && a->nla_len <= rem;
	     rem -= NLA_ALIGN(a->nla_len), a = (void *)((char *)a + NLA_ALIGN(a->nla_len))) {
		void *p = (char *)a + NLA_HDRLEN;
		switch (a->nla_type) {
		case NL80211_BSS_BSSID: memcpy(bssid, p, 6); break;
		case NL80211_BSS_FREQUENCY: freq = *(__u32 *)p; break;
		case NL80211_BSS_SIGNAL_MBM: sig = *(__s32 *)p; break;
		case NL80211_BSS_CAPABILITY: caps = *(__u16 *)p; capp = 1; break;
		case NL80211_BSS_INFORMATION_ELEMENTS: {
			int ielen = a->nla_len - NLA_HDRLEN;
			unsigned char *ie = p;
			while (ielen >= 2 && ielen >= 2 + ie[1]) {
				if (ie[0] == 0) {		/* SSID */
					ssid = (char *)ie + 2;
					ssid_len = ie[1];
				} else if (ie[0] == 0x30) {	/* RSNE */
					rsn = ie + 2;
					rsn_len = ie[1];
				} else if (ie[0] == 0x54)	/* MDIE (FT) */
					mdie = 1;
				else if (ie[0] == 0x07)	/* country */
					ctry = 1;
				ielen -= 2 + ie[1];
				ie += 2 + ie[1];
			}
			break;
		}
		}
	}
	/* Hidden APs mask the beacon SSID (length kept, bytes zeroed —
	 * iOS hotspots do this, 2026-08-31); a wildcard scan never unmasks
	 * it. An all-zero SSID of the right length is accepted as a match:
	 * the associate request below carries the real SSID from argv,
	 * which the AP validates. */
	int hidden = 1;
	int sae = 0;
	for (int i = 0; i < ssid_len; i++)
		if (((const unsigned char *)ssid)[i]) { hidden = 0; break; }
	if (ssid_len == (int)strlen(t->ssid) &&
	    (!memcmp(ssid, t->ssid, ssid_len) || hidden)) {
		/* one diagnostic line per BSSID carrying the SSID — the SME scan
		 * filter matches against exactly this info. RSNE layout after the
		 * 2-byte header: ver(2) grp(2..5) pcnt(6..7) pairwise(8..)
		 * akm-cnt akm-list rsn-caps(last 2). */
		char rc[8];
		if (rsn && rsn_len >= 20)
			snprintf(rc, sizeof rc, "%04x", rsn[rsn_len - 2] | rsn[rsn_len - 1] << 8);
		else
			snprintf(rc, sizeof rc, "----");
		fprintf(stderr, "  BSS %02x:%02x:%02x:%02x:%02x:%02x freq %u sig %.2f"
			" bss-caps %04x(p%d ess%d ibss%d) rsn-caps %s mdie%d ctry%d",
			bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5],
			freq, sig / 100.0, caps, capp, !!(caps & 0x0001), !!(caps & 0x0002),
			rc, mdie, ctry);
		if (rsn && rsn_len >= 12) {
			int pc = rsn[6] | rsn[7] << 8;
			int ao = 8 + 4 * pc;
			int ac = ao + 2 <= rsn_len ? rsn[ao] | rsn[ao + 1] << 8 : 0;
			fprintf(stderr, " grp");
			for (int k = 0; k < 4; k++) fprintf(stderr, "%02x", rsn[2 + k]);
			for (int k = 0; k < pc && 8 + 4 * k + 3 < rsn_len; k++)
				fprintf(stderr, " uc%02x%02x%02x%02x",
					rsn[8 + 4*k], rsn[9 + 4*k], rsn[10 + 4*k], rsn[11 + 4*k]);
			for (int k = 0; k < ac && ao + 2 + 4 * k + 3 < rsn_len; k++) {
				if (rsn[ao + 2 + 4*k] == 0x00 && rsn[ao + 3 + 4*k] == 0x0f &&
				    rsn[ao + 4 + 4*k] == 0xac && rsn[ao + 5 + 4*k] == 0x08)
					sae = 1;	/* SAE (WPA3) in the AKM list */
				fprintf(stderr, " akm%02x%02x%02x%02x",
					rsn[ao + 2 + 4*k], rsn[ao + 3 + 4*k],
					rsn[ao + 4 + 4*k], rsn[ao + 5 + 4*k]);
			}
		} else if (!rsn) {
			fprintf(stderr, " rsn:none");
		}
		fprintf(stderr, "\n");
		/* We speak PSK only. A WPA3-transition AP (e.g. an iPhone
		 * hotspot in 5 GHz mode, 2026-08-31) associates fine over the
		 * PSK AKM but its WPA policy never starts the 4WHS without
		 * PMF/SAE — the join times out with zero EAPOL. When the same
		 * SSID has several BSSIDs (transition 5 GHz + plain-PSK 2.4 GHz
		 * "Maximize Compatibility"), pick a PSK-only BSS over any
		 * SAE-advertising one, signal breaks ties. */
		if (!t->found || (!sae && t->bss_sae) ||
		    (sae == t->bss_sae && sig > t->sig)) {
			memcpy(t->bssid, bssid, 6);
			t->freq = freq;
			t->sig = sig;
			t->found = 1;
			t->bss_sae = sae;
			if (rsn && rsn_len >= 6) {
				t->grpcipher = (__u32)rsn[2] << 24 | rsn[3] << 16 |
					       rsn[4] << 8 | rsn[5];
				t->grp_known = 1;
			}
		}
	}
}

static int scan_cb(struct nlmsghdr *r, void *arg)
{
	if (r->nlmsg_type == NLMSG_DONE) return 1;	/* dump complete */
	if (r->nlmsg_type == NLMSG_ERROR) return 0;
	struct nlattr *a = (void *)((char *)NLMSG_DATA(r) + GENL_HDRLEN);
	int rem = r->nlmsg_len - NLMSG_HDRLEN - GENL_HDRLEN;
	for (; rem >= (int)NLA_HDRLEN && a->nla_len >= (int)NLA_HDRLEN && a->nla_len <= rem;
	     rem -= NLA_ALIGN(a->nla_len), a = (void *)((char *)a + NLA_ALIGN(a->nla_len)))
		if (a->nla_type == NL80211_ATTR_BSS)
			parse_bss(a, arg);
	return 0;
}

static int scan_and_find(struct target *t)
{
	struct nlmsghdr *n = mkmsg(NL80211_CMD_TRIGGER_SCAN, NLM_F_ACK, 100);
	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &t->ifindex, 4);
	/* Carry an IE in TRIGGER_SCAN: HDD saves NL80211_ATTR_IE into
	 * scan_add_ie and mirrors it to roam_profile->pAddIEScan (comment in
	 * wlan_hdd_scan.c: "save this for future association (join requires
	 * this)"). csr_scan_for_ssid later does qdf_mem_malloc(nAddIEScanLength);
	 * with no IE on record that is malloc(0) → NULL → NOMEM and the whole
	 * join path aborts before the filter ever runs. */
	nla_put(n, BUF, NL80211_ATTR_IE, rsne, sizeof(rsne));
	/* Directed probe for hidden APs: SSID in TRIGGER_SCAN makes the scan
	 * unicast a probe request carrying the real name, which a hidden AP
	 * answers (wildcard probes get nothing). */
	nla_put(n, BUF, NL80211_ATTR_SSID, t->ssid, strlen(t->ssid));
	nl_send(n, nl80211_fam);
	nl_read_loop(NULL, NULL, 1500);
	sleep(3);
	n = mkmsg(NL80211_CMD_GET_SCAN, NLM_F_DUMP, 101);
	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &t->ifindex, 4);
	nl_send(n, nl80211_fam);
	nl_read_loop(scan_cb, t, 5000);
	return t->found;
}

/* ------------------------------------------------------------------ */
/* events                                                              */
/* ------------------------------------------------------------------ */

struct wait_ev { __u8 want; int status; };

static int ev_cb(struct nlmsghdr *r, void *arg)
{
	struct wait_ev *w = arg;
	if (r->nlmsg_type != (__u16)nl80211_fam) return 0;
	struct genlmsghdr *g = NLMSG_DATA(r);
	if (g->cmd == NL80211_CMD_DISCONNECT) {
		fprintf(stderr, "event: DISCONNECT\n");
		w->status = -1;
		return 1;
	}
	if (g->cmd != w->want) {
		debug("event cmd %d (want %d)\n", g->cmd, w->want);
		return 0;
	}
	struct nlattr *a = (void *)((char *)NLMSG_DATA(r) + GENL_HDRLEN);
	int rem = r->nlmsg_len - NLMSG_HDRLEN - GENL_HDRLEN;
	w->status = 0;
	for (; rem >= (int)NLA_HDRLEN && a->nla_len >= (int)NLA_HDRLEN && a->nla_len <= rem;
	     rem -= NLA_ALIGN(a->nla_len), a = (void *)((char *)a + NLA_ALIGN(a->nla_len))) {
		void *p = (char *)a + NLA_HDRLEN;
		if (a->nla_len - NLA_HDRLEN < 2) continue;
		if (a->nla_type == NL80211_ATTR_STATUS_CODE)
			w->status = *(__u16 *)p;
		if (a->nla_type == NL80211_ATTR_TIMED_OUT)
			w->status = -1;
	}
	return 1;
}

static int wait_event(__u8 cmd, int timeout_ms, int *status)
{
	struct wait_ev w = { .want = cmd, .status = 0 };
	int rc = nl_read_loop(ev_cb, &w, timeout_ms);
	*status = w.status;
	return rc;	/* 1 = event seen, 0 = timeout, -1 = error */
}

/* ------------------------------------------------------------------ */
/* EAPOL / WPA2 handshake                                              */
/* ------------------------------------------------------------------ */

#define EAPOL_HDR	4		/* ver, type, len16 */
#define KEY_BODY_FIXED	95		/* desc..kdlen inclusive */

struct eapol_key {
	unsigned char ver, type;
	/* body: */
	unsigned char desc;
	unsigned short ki, kl;
	unsigned char replay[8], nonce[32], iv[16], rsc[8], kid[8], mic[16];
	unsigned short kdlen;
	/* kd follows in the frame */
};

static int pk_fd;			/* AF_PACKET */
static struct sockaddr_ll pk_sa;

static int send_frame(const unsigned char *dst, const unsigned char *src,
		      const unsigned char *body, size_t blen)
{
	unsigned char pkt[14 + 512];
	memcpy(pkt, dst, 6);
	memcpy(pkt + 6, src, 6);
	pkt[12] = 0x88; pkt[13] = 0x8e;
	memcpy(pkt + 14, body, blen);
	return sendto(pk_fd, pkt, 14 + blen, 0, (void *)&pk_sa, sizeof(pk_sa));
}

/* build an EAPOL-Key frame; mic computed if kck given.
 * eapver: EAPOL protocol version byte (echo the AP's M1 version — some
 * firmwares reject a higher version than their own). */
static size_t build_key(unsigned char *out, const struct eapol_key *k,
			const unsigned char *kd, size_t kdlen,
			const unsigned char *kck, unsigned char eapver)
{
	size_t tlen = EAPOL_HDR + KEY_BODY_FIXED + kdlen;
	out[0] = eapver; out[1] = 3;		/* EAPOL, Key */
	out[2] = (KEY_BODY_FIXED + kdlen) >> 8;
	out[3] = (KEY_BODY_FIXED + kdlen) & 0xff;
	unsigned char *b = out + EAPOL_HDR;
	b[0] = k->desc;
	b[1] = k->ki >> 8; b[2] = k->ki & 0xff;
	b[3] = k->kl >> 8; b[4] = k->kl & 0xff;
	memcpy(b + 5, k->replay, 8);
	memcpy(b + 13, k->nonce, 32);
	memcpy(b + 45, k->iv, 16);
	memcpy(b + 61, k->rsc, 8);
	memcpy(b + 69, k->kid, 8);
	memset(b + 77, 0, 16);			/* MIC zeroed for computation */
	b[93] = kdlen >> 8; b[94] = kdlen & 0xff;
	memcpy(b + 95, kd, kdlen);
	if (kck) {
		unsigned char mic[20];
		/* MIC covers the FULL EAPOL PDU from the version byte
		 * (see hostapd wpa_verify_key_mic: it hashes the
		 * ieee802_1x_hdr onward), not just the key body */
		hmac_sha1(kck, 16, out, tlen, mic);
		memcpy(b + 77, mic, 16);
	}
	return tlen;
}

static int parse_eapol(const unsigned char *frame, size_t flen, struct eapol_key *k,
		       const unsigned char **kd)
{
	if (flen < 14 + 4 + KEY_BODY_FIXED) return -1;
	if (frame[12] != 0x88 || frame[13] != 0x8e) return -1;
	const unsigned char *p = frame + 14;
	k->ver = p[0]; k->type = p[1];
	if (k->type != 3) return -1;
	const unsigned char *b = p + 4;
	k->desc = b[0];
	k->ki = (b[1] << 8) | b[2];
	k->kl = (b[3] << 8) | b[4];
	memcpy(k->replay, b + 5, 8);
	memcpy(k->nonce, b + 13, 32);
	memcpy(k->iv, b + 45, 16);
	memcpy(k->rsc, b + 61, 8);
	memcpy(k->kid, b + 69, 8);
	memcpy(k->mic, b + 77, 16);
	k->kdlen = (b[93] << 8) | b[94];
	*kd = b + 95;
	return 0;
}

/* ------------------------------------------------------------------ */

int main(int argc, char **argv)
{
	if (argc != 4) {
		fprintf(stderr, "usage: wifi-join <ifname> <ssid> <passphrase>\n");
		return 2;
	}
	const char *ifname = argv[1];
	struct target t = { .ssid = argv[2] };
	t.ifindex = if_nametoindex(ifname);
	if (!t.ifindex) { fprintf(stderr, "no ifindex for %s\n", ifname); return 1; }

	unsigned char mymac[6];
	struct ifreq ifr;
	int s = socket(AF_INET, SOCK_DGRAM, 0);
	strcpy(ifr.ifr_name, ifname);
	ioctl(s, SIOCGIFHWADDR, &ifr);
	memcpy(mymac, ifr.ifr_hwaddr.sa_data, 6);
	/* ensure the netdev is UP: auth/assoc is control-path and succeeds
	 * regardless, but a down interface can drop the data path (EAPOL) */
	strcpy(ifr.ifr_name, ifname);
	ioctl(s, SIOCGIFFLAGS, &ifr);
	fprintf(stderr, "%s flags %#06x", ifname, (unsigned)ifr.ifr_flags);
	if (!(ifr.ifr_flags & IFF_UP)) {
		ifr.ifr_flags |= IFF_UP;
		if (ioctl(s, SIOCSIFFLAGS, &ifr) == 0)
			fprintf(stderr, " -> brought UP");
	}
	fprintf(stderr, "\n");
	close(s);
	fprintf(stderr, "our MAC   %02x:%02x:%02x:%02x:%02x:%02x\n",
		mymac[0], mymac[1], mymac[2], mymac[3], mymac[4], mymac[5]);

	nl_fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_GENERIC);
	struct sockaddr_nl la = { .nl_family = AF_NETLINK };
	bind(nl_fd, (void *)&la, sizeof(la));
	int rb = BUF * 4;
	setsockopt(nl_fd, SOL_SOCKET, SO_RCVBUF, &rb, sizeof(rb));
	if (join_group("mlme") < 0) fprintf(stderr, "WARN: mlme group not joined\n");
	if (nl80211_fam < 0) { fprintf(stderr, "nl80211 family missing\n"); return 1; }
	debug("nl80211 fam %d\n", nl80211_fam);

	if (!scan_and_find(&t)) {
		fprintf(stderr, "ssid '%s' not in scan results\n", t.ssid);
		return 2;
	}
	printf("AP %02x:%02x:%02x:%02x:%02x:%02x freq %u\n",
	       t.bssid[0], t.bssid[1], t.bssid[2], t.bssid[3], t.bssid[4], t.bssid[5],
	       t.freq);

	/* --- packet socket for EAPOL: open+bind BEFORE connecting so no
	 * M1 can slip past (the AP starts the 4-way within ~100 ms of assoc
	 * and gives up after 4 retries).
	 * MUST bind ETH_P_PAE, not ETH_P_ALL: the kernel stamps skb->protocol
	 * from sll_protocol, and HDD's hdd_is_tx_allowed() only exempts
	 * EAPOL from the pre-keys CONN peer state by checking that field —
	 * with ETH_P_ALL every TX frame is dropped ("Tx not allowed"). --- */
	pk_fd = socket(AF_PACKET, SOCK_RAW, htons(ETH_P_PAE));
	pk_sa.sll_family = AF_PACKET;
	pk_sa.sll_protocol = htons(ETH_P_PAE);
	pk_sa.sll_ifindex = t.ifindex;
	pk_sa.sll_halen = 6;
	memcpy(pk_sa.sll_addr, t.bssid, 6);
	bind(pk_fd, (void *)&pk_sa, sizeof(pk_sa));

	/* --- connect (qcacld has no split .auth/.assoc — -EOPNOTSUPP;
	 * NL80211_CMD_CONNECT does auth+assoc in firmware and emits a
	 * CONNECT event) --- */
	__u32 authtype = NL80211_AUTHTYPE_OPEN_SYSTEM;
	__u32 ccmp = 0x000fac04, psk = 0x000fac02, wpaver = 2 /* WPA2 */;
	/* mirror the AP's group cipher: WPA2-mixed APs run a TKIP group
	 * cipher with CCMP pairwise, and the SME scan filter rejects the
	 * cache entry if our profile's multicast cipher differs. */
	__u32 grpc = t.grp_known ? t.grpcipher : ccmp;
	unsigned char rsne_tx[22];
	memcpy(rsne_tx, rsne, sizeof(rsne_tx));
	rsne_tx[4] = grpc >> 24; rsne_tx[5] = grpc >> 16;
	rsne_tx[6] = grpc >> 8;  rsne_tx[7] = grpc;
	fprintf(stderr, "group cipher %08x (%s)\n", grpc,
		grpc == 0x000fac04 ? "CCMP" : grpc == 0x000fac02 ? "TKIP" : "?");

	/* tear down any existing association first — CONNECT returns
	 * EALREADY on qcacld while associated (observed on device) */
	struct nlmsghdr *n;
	n = mkmsg(NL80211_CMD_DISCONNECT, 0, 199);
	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &t.ifindex, 4);
	nl_send(n, nl80211_fam);
	{
		int dst;
		wait_event(NL80211_CMD_DISCONNECT, 3000, &dst);
	}

	n = mkmsg(NL80211_CMD_CONNECT, 0, 200);

	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &t.ifindex, 4);
	nla_put(n, BUF, NL80211_ATTR_MAC, t.bssid, 6);
	nla_put(n, BUF, NL80211_ATTR_SSID, t.ssid, strlen(t.ssid));
	nla_put(n, BUF, NL80211_ATTR_AUTH_TYPE, &authtype, 4);
	nla_put(n, BUF, NL80211_ATTR_WIPHY_FREQ, &t.freq, 4);
	nla_put(n, BUF, NL80211_ATTR_IE, rsne_tx, sizeof(rsne_tx));
	/* crypto profile: without these the roam profile stays "open" and
	 * hdd_set_csr_auth_type / hdd_set_genie_to_csr never run (they are
	 * gated on wpa_versions). NB: 4.19 wants FLAT u32 arrays here, not
	 * nested lists. */
	nla_put(n, BUF, NL80211_ATTR_CIPHER_SUITES_PAIRWISE, &ccmp, 4);
	nla_put(n, BUF, NL80211_ATTR_CIPHER_SUITE_GROUP, &grpc, 4);
	nla_put(n, BUF, NL80211_ATTR_AKM_SUITES, &psk, 4);
	/* WPA_VERSIONS=2 flips HDD's sta_ctx->wpa_versions so that
	 * hdd_set_csr_auth_type derives RSN_PSK from the OPEN_SYSTEM
	 * 802.11 auth type. */
	nla_put(n, BUF, NL80211_ATTR_WPA_VERSIONS, &wpaver, 4);
	if (nl_send(n, nl80211_fam) < 0) { perror("connect send"); return 1; }
	int st;
	if (wait_event(NL80211_CMD_CONNECT, 15000, &st) != 1) {
		fprintf(stderr, "connect: no event\n");
		return 3;
	}
	if (st) { fprintf(stderr, "connect status %d\n", st); return 4; }
	printf("connected\n");

	/* --- PMK --- */
	unsigned char pmk[32];
	pbkdf2_sha1(argv[3], strlen(argv[3]), t.ssid, strlen(t.ssid), 4096, 32, pmk);

	/* --- 4WHS driver loop: answer every M1 (retransmission) with a fresh
	 * M2, grab M3 when it comes. One-shot M2 is not enough: the first TX
	 * can get dropped by the fw datapath right after assoc. --- */
	unsigned char buf[2048];
	struct eapol_key k;
	const unsigned char *kd;
	struct pollfd pf = { .fd = pk_fd, .events = POLLIN };

	int fd = open("/dev/urandom", O_RDONLY);
	unsigned char snonce[32];
	read(fd, snonce, 32);
	close(fd);

	unsigned char ptk[64];
	const unsigned char *kck = NULL, *kek = NULL, *tk = NULL;
	unsigned char frame[512];
	size_t fl;
	int have_ptk = 0, got_m3 = 0, m2_count = 0;
	for (int to = 0; to < 200 && !got_m3; to++) {
		if (poll(&pf, 1, 100) != 1) continue;
		int flen = recv(pk_fd, buf, sizeof(buf), 0);
		if (flen < 14) continue;
		if (buf[12] == 0x88 && buf[13] == 0x8e && flen >= 23)
			debug("eapol: ver %d type %d len %d desc %d ki %04x kl %d\n",
			      buf[14], buf[15], (buf[16] << 8) | buf[17], buf[18],
			      (buf[19] << 8) | buf[20], (buf[21] << 8) | buf[22]);
		if (parse_eapol(buf, flen, &k, &kd) != 0) continue;
		int pairwise = k.ki & 0x0008, secure = k.ki & 0x0100;
		if (pairwise && !secure && k.kdlen == 0) {
			/* M1: ki 0x008a = v2|pairwise|ACK — the standard value
			 * (Key ACK is B7/0x80, Key MIC B8/0x100; M1 never
			 * carries a MIC). */
			if (!have_ptk) {
				unsigned char b[76];	/* 6+6 MACs, 32+32 nonces */
				const unsigned char *macmin, *macmax, *nmin, *nmax;
				if (memcmp(t.bssid, mymac, 6) < 0) { macmin = t.bssid; macmax = mymac; }
				else { macmin = mymac; macmax = t.bssid; }
				if (memcmp(k.nonce, snonce, 32) < 0) { nmin = k.nonce; nmax = snonce; }
				else { nmin = snonce; nmax = k.nonce; }
				memcpy(b, macmin, 6); memcpy(b + 6, macmax, 6);
				memcpy(b + 12, nmin, 32); memcpy(b + 44, nmax, 32);
				/* b is 76 bytes: 12 MAC + 64 nonce bytes —
				 * passing 64 here derives a WRONG PTK (the
				 * AP then rejects every M2's MIC) */
				sha1_prf(pmk, 32, "Pairwise key expansion",
					 b, 76, ptk, 48);
				kck = ptk; kek = ptk + 16; tk = ptk + 32;
				have_ptk = 1;
			}
			struct eapol_key m2;
			memset(&m2, 0, sizeof(m2));
			m2.desc = 2;
			m2.ki = 0x010a;		/* v2|pairwise|MIC — ACK must be CLEAR or hostapd drops the frame ("Key Ack set") */
			m2.kl = 16;
			memcpy(m2.replay, k.replay, 8);
			memcpy(m2.nonce, snonce, 32);
			/* 802.11i: M2 key data carries our RSNE — hostapd
			 * compares it against the assoc-request RSNE and
			 * deauths on mismatch. */
			fl = build_key(frame, &m2, rsne_tx,
				       sizeof(rsne_tx), kck, buf[14]);
			int rc = send_frame(t.bssid, mymac, frame, fl);
			fprintf(stderr, "M2 sent #%d (rc %d)\n", ++m2_count, rc);
		} else if (pairwise && (k.ki & 0x0080) && secure && k.kdlen > 0) {
			got_m3 = 1;	/* pairwise | ACK | secure | encrypted kd (M3) */
		}
	}
	if (!have_ptk) { fprintf(stderr, "no EAPOL M1\n"); return 3; }
	if (!got_m3) { fprintf(stderr, "no EAPOL M3 (M2 sent %dx)\n", m2_count); return 3; }

	/* verify MIC: zero the mic field in place, recompute, restore.
	 * Range = full EAPOL PDU (version byte onward), as in TX */
	unsigned char *raw = buf + 14 + EAPOL_HDR;
	unsigned char mic_save[16];
	memcpy(mic_save, raw + 77, 16);
	memset(raw + 77, 0, 16);
	unsigned char mic[20];
	size_t bodylen = EAPOL_HDR + KEY_BODY_FIXED + k.kdlen;
	hmac_sha1(kck, 16, buf + 14, bodylen, mic);
	memcpy(raw + 77, mic_save, 16);
	if (memcmp(mic, mic_save, 16)) {
		fprintf(stderr, "M3 MIC mismatch — wrong passphrase?\n");
		return 5;
	}
	printf("M3 MIC verified — passphrase correct\n");

	/* unwrap key data → GTK KDE */
	unsigned char plain[512];
	if (aes_unwrap(kek, kd, k.kdlen, plain)) {
		fprintf(stderr, "GTK unwrap failed\n");
		return 5;
	}
	size_t plen = k.kdlen - 8;
	unsigned char gtk[32] = {0};
	int gtklen = 16, gtkidx = 1;
	for (size_t i = 0; i + 2 <= plen; ) {
		unsigned char id = plain[i], l = plain[i+1];
		if (i + 2 + l > plen) break;
		if (id == 0xdd && l >= 6 && !memcmp(plain + i + 2, "\x00\x0f\xac\x01", 4)) {
			gtkidx = (plain[i+6] & 0x03);
			gtklen = l - 6;
			if (gtklen > 32) gtklen = 32;
			memcpy(gtk, plain + i + 8, gtklen);
			printf("GTK kde idx %d len %d\n", gtkidx, gtklen);
		}
		i += 2 + l;
	}

	/* send M4 */
	struct eapol_key m4;
	memset(&m4, 0, sizeof(m4));
	m4.desc = 2;
	m4.ki = 0x030a;			/* v2|pairwise|MIC|Secure */
	memcpy(m4.replay, k.replay, 8);
	fl = build_key(frame, &m4, NULL, 0, kck, 1);
	send_frame(t.bssid, mymac, frame, fl);
	printf("M4 sent\n");

	/* install pairwise key (TK, idx 0, MAC=AP) */
	__u32 cipher = 0x000fac04;	/* CCMP */
	__u8 kidx = 0;
	n = mkmsg(NL80211_CMD_NEW_KEY, 0, 202);
	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &t.ifindex, 4);
	nla_put(n, BUF, NL80211_ATTR_KEY_IDX, &kidx, 1);
	nla_put(n, BUF, NL80211_ATTR_KEY_DATA, tk, 16);
	nla_put(n, BUF, NL80211_ATTR_KEY_CIPHER, &cipher, 4);
	nla_put(n, BUF, NL80211_ATTR_MAC, t.bssid, 6);
	if (nl_send(n, nl80211_fam) < 0) { perror("new_key ptk"); return 1; }
	nl_read_loop(NULL, NULL, 300);	/* catch async errors only */

	/* install group key (GTK, idx from KDE, seq=rsc) — the GTK's cipher
	 * is the AP's *group* cipher (TKIP on WPA2-mixed APs, 32-byte key) */
	__u32 gtkcipher = t.grp_known ? t.grpcipher : cipher;
	kidx = gtkidx;
	n = mkmsg(NL80211_CMD_NEW_KEY, 0, 203);
	nla_put(n, BUF, NL80211_ATTR_IFINDEX, &t.ifindex, 4);
	nla_put(n, BUF, NL80211_ATTR_KEY_IDX, &kidx, 1);
	nla_put(n, BUF, NL80211_ATTR_KEY_DATA, gtk, gtklen);
	nla_put(n, BUF, NL80211_ATTR_KEY_CIPHER, &gtkcipher, 4);
	nla_put(n, BUF, NL80211_ATTR_KEY_SEQ, k.rsc, 6);
	if (nl_send(n, nl80211_fam) < 0) { perror("new_key gtk"); return 1; }
	nl_read_loop(NULL, NULL, 300);
	debug("keys installed\n");

	printf("keys installed — run udhcpc -i %s\n", ifname);
	return 0;
}
