/* qmi-req: send one or more raw QMI requests to a QRTR node:port over a
 * SINGLE socket (one WDS/UIM client), printing each hex response.
 * usage: qmi-req node port msgid [hextlv|""] [msgid tlv ...]
 * Multiple messages matter when the modem ties state to the client, e.g.
 * WDS Bind Mux Data Port + Start Network must share one client (M7,
 * observed: bind on a fresh client then start on another = INVALID_OPERATION).
 * A msgid of the form "raw:<hex>" sends those exact bytes as the whole
 * frame — used to replay vendor libqmi_cci frames verbatim (they carry a
 * leading txn byte; netmgrd reply analysis, M7).
 * QR_DRAIN=<ms> keeps reading after each response and prints late frames
 * too: PDC (0x2a service) answers in QMI indications, which arrive as
 * separate frames after the bare ack (observed M7, redfin). */
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/time.h>
struct sockaddr_qrtr { unsigned short fam; unsigned int node, port; };
static int hexv(int c){ if(c>='0'&&c<='9')return c-'0'; if(c>='a'&&c<='f')return c-'a'+10; if(c>='A'&&c<='F')return c-'A'+10; return -1; }
/* WDS GET_CURRENT_SETTINGS (0x2D) carries the bearer's addressing as
 * little-endian u32 TLVs — flip for the dotted quad (M7: the modem hands
 * 0a 94 e0 3b for 10.148.224.59). Printing a cooked SETTINGS line here
 * keeps bring-up scripts out of awk (busybox awk segfaults on this
 * rootfs, observed 2026-08-29). */
static void dump_settings(const unsigned char *b, ssize_t n)
{
	unsigned msg = ((unsigned)b[2] << 8) | b[3];
	if (n < 7 || b[0] != 2 || msg != 0x2D) return;
	char ip[32] = "", gw[32] = "", mask[32] = "", d1[32] = "", d2[32] = "";
	unsigned pos = 7;
	while (pos + 3 <= (unsigned)n) {
		unsigned type = b[pos], len = b[pos+1] | (b[pos+2] << 8);
		const unsigned char *v = b + pos + 3;
		if (pos + 3 + len > (unsigned)n) break;
		if (len == 4) {
			char tmp[32];
			snprintf(tmp, sizeof tmp, "%u.%u.%u.%u", v[3], v[2], v[1], v[0]);
			if (type == 0x1E) strcpy(ip, tmp);
			else if (type == 0x20) strcpy(gw, tmp);
			else if (type == 0x21) strcpy(mask, tmp);
			else if (type == 0x15) strcpy(d1, tmp);
			else if (type == 0x16) strcpy(d2, tmp);
		}
		pos += 3 + len;
	}
	int plen = 0;
	if (mask[0]) {
		unsigned a, o[4];
		sscanf(mask, "%u.%u.%u.%u", &o[0], &o[1], &o[2], &o[3]);
		for (int i = 0; i < 4; i++)
			for (a = o[i]; a; a >>= 1) plen += a & 1;
	}
	if (ip[0]) printf("SETTINGS ip=%s gw=%s mask=%s plen=%d dns=%s,%s\n",
			  ip, gw, mask, plen, d1, d2);
}
int main(int argc, char **argv)
{
	int msleep_us;
	const char *e;
	/* unbuffered: callers parse this output live while the process is
	 * still holding a bearer open (QR_HOLD) — block buffering made the
	 * whole run invisible until exit (M7). */
	setvbuf(stdout, NULL, _IONBF, 0);
	if (argc < 4 || (argc != 4 && (argc - 3) % 2)) { fprintf(stderr,"usage: qmi-req node port msgid [hextlv|\"\"] [msgid tlv ...]\n"); return 1; }
	unsigned node=atoi(argv[1]), port=atoi(argv[2]);
	int fd=socket(42,SOCK_DGRAM,0);
	if (fd<0){perror("socket");return 1;}
	struct sockaddr_qrtr me={42,0,0};
	if (connect(fd,(void*)&me,sizeof me)<0){perror("connect");return 1;}
	struct timeval tv={6,0};
	/* long-running replies (NAS network scan ~30s) need more than the 6s
	 * default; QR_TIMEOUT sets the socket timeout in seconds (M7). */
	if ((e=getenv("QR_TIMEOUT"))) tv.tv_sec=atoi(e);
	setsockopt(fd,SOL_SOCKET,SO_RCVTIMEO,&tv,sizeof tv);
	struct sockaddr_qrtr dst={42,node,port};
	/* data calls need seconds to establish; QR_SLEEP ms between messages
	 * so bind + start + status polls run on one client (call state is
	 * per-qrtr-client, observed M7: status from a fresh client = err 15) */
	msleep_us = (e=getenv("QR_SLEEP")) ? atoi(e) * 1000 : 0;
	/* QR_DRAIN ms: after the response, read until this short timeout
	 * expires, so indication frames land on stdout as well (PDC, M7). */
	int drain_us = (e=getenv("QR_DRAIN")) ? atoi(e) * 1000 : 0;
	/* QR_HOLD ms: sleep AFTER the last response, keeping the QRTR
	 * client (and any per-client modem state — a WDS bearer — it owns)
	 * alive (M7: the data call drops with a PKT_SRVC_STATUS_IND the
	 * moment the client socket closes, observed 2026-08-29).
	 * long long: QR_HOLD=7200000 in an int overflows negative in the
	 * µs multiply and the hold silently never runs. */
	long long hold_us = (e=getenv("QR_HOLD")) ? atoll(e) * 1000 : 0;
	for (int a=3; a<argc; a+=2) {
		if (a > 3 && msleep_us) usleep(msleep_us);
		unsigned char req[512]; unsigned n=0;
		if (!strncmp(argv[a],"raw:",4)) {
			const char *h=argv[a]+4;
			for (unsigned i=0; h[i]&&h[i+1]&&n<sizeof req;i+=2) req[n++]=(hexv(h[i])<<4)|hexv(h[i+1]);
			printf("-> %u:%u raw %u bytes\n",node,port,n);
		} else {
			unsigned msg=strtoul(argv[a],0,0);
			/* txn byte must be unique per client: reusing txn 1 for a
			 * second request makes WDS answer MALFORMED_MESSAGE (M7:
			 * bind_mux + START_NET in one run — bind OK, start err 0x01). */
			req[0]=0; req[1]=1+(a-3)/2; req[2]=0; req[3]=msg&0xff; req[4]=msg>>8; n=5;
			unsigned char tlvs[512]; unsigned tn=0;
			const char *h=argv[a+1];
			for (unsigned i=0; h[i]&&h[i+1]&&tn<sizeof tlvs;i+=2) tlvs[tn++]=(hexv(h[i])<<4)|hexv(h[i+1]);
			if (tn) { req[5]=tn&0xff; req[6]=tn>>8; memcpy(req+7,tlvs,tn); n=7+tn; }
			else { req[5]=0; req[6]=0; n=7; }
			printf("-> %u:%u msg=0x%04x tlv=%u bytes\n",node,port,msg,tn);
		}
		if (sendto(fd,req,n,0,(void*)&dst,sizeof dst)<0){perror("sendto");return 1;}
		for (int first=1;;first=0) {
			unsigned char buf[2048];
			ssize_t r=recv(fd,buf,sizeof buf,0);
			if (r<0){ if(first) printf("<- timeout\n"); break; }
			printf("<- %zd bytes:",r);
			for (ssize_t i=0;i<r&&i<1024;i++) printf("%s%02x",(i%16)?" ":"\n   ",buf[i]);
			printf("\n");
			dump_settings(buf, r);
			if (!drain_us) break;
			struct timeval dtv={drain_us/1000000,drain_us%1000000};
			setsockopt(fd,SOL_SOCKET,SO_RCVTIMEO,&dtv,sizeof dtv);
		}
		if (drain_us) setsockopt(fd,SOL_SOCKET,SO_RCVTIMEO,&tv,sizeof tv);
	}
	if (hold_us > 0) {
		struct timeval htv = {1, 0};
		setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &htv, sizeof htv);
		while (hold_us > 0) {
			unsigned char buf[2048];
			ssize_t r = recv(fd, buf, sizeof buf, 0);
			if (r > 0) {
				printf("<- held %zd bytes:", r);
				for (ssize_t i = 0; i < r && i < 1024; i++) printf("%s%02x",(i%16)?" ":"\n   ",buf[i]);
				printf("\n");
				fflush(stdout);
			} else {
				hold_us -= 1000000; /* 1 s recv timeout elapsed */
			}
		}
	}
	return 0;
}
