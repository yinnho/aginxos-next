/* httpget.c — minimal HTTP/1.0 fetch for the M5 boot internet check.
 *
 * This build's busybox wget applet segfaults (observed 2026-08-28: any URL,
 * raw-IP included — while nslookup/ping resolve fine through the udhcpc-
 * written resolv.conf), so the check gets its own ~100-line fetcher:
 * getaddrinfo -> TCP connect -> GET -> read.
 *
 * Address iteration + socket timeouts (2026-09-09, bake-#1 flash day):
 * DNS here rotates AAAA/A order while the AP carries no IPv6 default
 * route — dialing the v6 answer hung the whole boot internet stage
 * (sk_wait_data, no timeout; observed stuck 10+ min). Now every
 * addrinfo answer is tried in order (v6 connect fails in ms with
 * ENETUNREACH, the A record answers), and a wedged peer can't hold the
 * boot chain hostage: 15 s send/receive timeouts cap any single dial.
 *
 * usage: httpget http://host[:port]/path [outfile]
 * Prints one line "HTTP <code> <n> bytes" and exits 0 on a 2xx/3xx response
 * with a non-empty body — the check proves DNS + TCP + HTTP round-trip,
 * which is what the boot card's INTERNET row claims, no more.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <netdb.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

int main(int argc, char **argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: httpget http://host[:port]/path [outfile]\n");
    return 2;
  }
  const char *url = argv[1];
  if (strncmp(url, "http://", 7)) {
    fprintf(stderr, "only http:// URLs\n");
    return 2;
  }
  char host[256] = "", port[8] = "80", path[512] = "/";
  const char *p = url + 7;
  const char *slash = strchr(p, '/');
  size_t hlen = slash ? (size_t)(slash - p) : strlen(p);
  if (hlen >= sizeof host) hlen = sizeof host - 1;
  memcpy(host, p, hlen);
  host[hlen] = 0;
  char *colon = strchr(host, ':');
  if (colon) {
    snprintf(port, sizeof port, "%s", colon + 1);
    *colon = 0;
  }
  if (slash) {
    size_t plen = strlen(slash);
    if (plen >= sizeof path) plen = sizeof path - 1;
    memcpy(path, slash, plen);
    path[plen] = 0;
  }

  struct addrinfo hints;
  memset(&hints, 0, sizeof hints);
  hints.ai_family = AF_UNSPEC;
  hints.ai_socktype = SOCK_STREAM;
  struct addrinfo *res = NULL;
  int rc = getaddrinfo(host, port, &hints, &res);
  if (rc || !res) {
    fprintf(stderr, "resolve %s: %s\n", host, gai_strerror(rc));
    return 1;
  }
  int s = -1;
  for (struct addrinfo *ai = res; ai; ai = ai->ai_next) {
    s = socket(ai->ai_family, ai->ai_socktype, ai->ai_protocol);
    if (s < 0) continue;
    /* 15 s send/receive cap: a wedged peer or a black-holed v6 dial
     * must not hang the boot internet stage (observed 10+ min). */
    struct timeval tv = {.tv_sec = 15};
    setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
    setsockopt(s, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
    if (connect(s, ai->ai_addr, ai->ai_addrlen) == 0) break;
    fprintf(stderr, "connect %s: %s\n", host, strerror(errno));
    close(s);
    s = -1;
  }
  freeaddrinfo(res);
  if (s < 0) {
    fprintf(stderr, "connect %s: no reachable address\n", host);
    return 1;
  }

  char req[640];
  int rl = snprintf(req, sizeof req,
                    "GET %s HTTP/1.0\r\nHost: %s\r\n"
                    "User-Agent: aginxos-bootcheck\r\nAccept: */*\r\n\r\n",
                    path, host);
  if (write(s, req, rl) != rl) {
    perror("write");
    close(s);
    return 1;
  }

  /* whole response into memory (capped) — headers are parsed after the
   * read completes, so a split CRLFCRLF across reads is not an issue. */
  size_t cap = 1u << 20, len = 0;
  char *r = malloc(cap);
  if (!r) return 1;
  for (;;) {
    ssize_t n = read(s, r + len, cap - len);
    if (n <= 0) break;
    len += (size_t)n;
    if (len == cap) break;
  }
  close(s);

  int status = 0;
  if (len > 12 && !strncmp(r, "HTTP/", 5)) status = atoi(r + 9);
  char *body = memmem(r, len, "\r\n\r\n", 4);
  size_t blen = body ? len - (size_t)(body - r) - 4 : 0;
  if (argc > 2 && body) {
    FILE *out = fopen(argv[2], "wb");
    if (out) {
      fwrite(body + 4, 1, blen, out);
      fclose(out);
    }
  }
  free(r);
  printf("HTTP %d %zu bytes\n", status, blen);
  return (status >= 200 && status < 400 && blen > 0) ? 0 : 1;
}
