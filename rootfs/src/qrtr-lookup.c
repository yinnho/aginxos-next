/* qrtr-lookup: enumerate QRTR services via the name service (NEW_LOOKUP +
 * NEW_SERVER/DEL_SERVER), timestamped. Run with no args for a 4 s snapshot,
 * or `qrtr-lookup 0 0 <secs>` to watch live for that long — radio-bringup
 * starts one BEFORE the modem boot trigger to capture the fresh-boot
 * registration order (does WLFW svc 0x45 ever appear?). */
#define _GNU_SOURCE
#include <stdio.h>
#include <time.h>
#include <string.h>
#include <unistd.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/time.h>
#define CTRL_PORT 0xfffffffeu
struct sockaddr_qrtr { unsigned short fam; unsigned int node, port; };
struct pkt { unsigned cmd, a, b, c, d; };
static void stamp(void)
{
	struct timeval tv; gettimeofday(&tv, 0);
	struct tm tm; localtime_r(&tv.tv_sec, &tm);
	printf("[%02d:%02d:%02d.%03d] ", tm.tm_hour, tm.tm_min, tm.tm_sec, tv.tv_usec / 1000);
}
int main(int argc, char **argv)
{
	unsigned fsvc = argc > 1 ? strtoul(argv[1], 0, 0) : 0;
	unsigned finst = argc > 2 ? strtoul(argv[2], 0, 0) : 0;
	int secs = argc > 3 ? atoi(argv[3]) : 4;
	setvbuf(stdout, 0, _IOLBF, 0);
	int fd = socket(42, SOCK_DGRAM, 0);
	if (fd < 0) { perror("socket"); return 1; }
	struct sockaddr_qrtr me = { 42, 0, 0 };
	if (connect(fd, (void *)&me, sizeof me) < 0) { perror("connect"); return 1; }
	struct sockaddr_qrtr ns; socklen_t nlen = sizeof ns;
	getsockname(fd, (void *)&ns, &nlen);
	stamp(); printf("# me=%u:%u filter svc=0x%x inst=0x%x\n", ns.node, ns.port, fsvc, finst);
	struct sockaddr_qrtr to = { 42, ns.node, CTRL_PORT };
	struct pkt lp; memset(&lp, 0, sizeof lp);
	lp.cmd = 10; lp.a = fsvc; lp.b = finst;
	if (sendto(fd, &lp, 20, 0, (void *)&to, sizeof to) < 0) { perror("lookup"); return 1; }
	struct timeval tv = { secs, 0 };
	setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
	for (;;) {
		unsigned char buf[2048];
		ssize_t n = recv(fd, buf, sizeof buf, 0);
		if (n < 0) break;
		if (n >= 20) {
			struct pkt p; memcpy(&p, buf, sizeof p);
			stamp();
			if (p.cmd == 4) printf("svc=0x%-6x inst=0x%-8x at %u:%u\n", p.a, p.b, p.c, p.d);
			else if (p.cmd == 5) printf("DEL  svc=0x%-6x inst=0x%-8x\n", p.a, p.b);
			fflush(stdout);
		}
	}
	stamp(); printf("# done\n");
	return 0;
}
