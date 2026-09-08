/* httpget.c — minimal HTTP/1.0 fetch for the M5 boot internet check.
 *
 * This build's busybox wget applet segfaults (observed 2026-08-28: any URL,
 * raw-IP included — while nslookup/ping resolve fine through the udhcpc-
 * written resolv.conf), so the check gets its own ~100-line fetcher:
 * getaddrinfo -> TCP connect -> GET -> read.
 *
 * usage: httpget http://host[:port]/path [outfile]
 * Prints one line "HTTP <code> <n> bytes" and exits 0 on a 2xx/3xx response
 * with a non-empty body — the check proves DNS + TCP + HTTP round-trip,
 * which is what the boot card's INTERNET row claims, no more.
 */
#define _GNU_SOURCE
#include <netdb.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
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
  int s = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
  if (s < 0 || connect(s, res->ai_addr, res->ai_addrlen) < 0) {
    perror("connect");
    if (s >= 0) close(s);
    return 1;
  }
  freeaddrinfo(res);

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
