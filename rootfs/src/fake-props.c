/* fake-props.c — LD_PRELOAD for stock vendor binaries on AginxOS.
 *
 * No init/property service exists here, so every bionic property lookup
 * returns nothing. Two groups need real answers:
 *   - pm-service/cnss-daemon block in libbinder waiting for
 *     "servicemanager.ready" (find+read_callback+serial loop) — fake it by
 *     name so the wait passes.
 *   - cnss-daemon builds the BDF filename from ro.hardware ("bdwlan-%s.bin"
 *     fallback chain). Without it the string defaults to "default" and it
 *     searches for bdwlan-default-*.bin instead of /vendor/firmware/
 *     bdwlan-redfin.bin (observed 2026-08-28). ro.boot.hardware.radio.subtype
 *     feeds the SKU decision on Pixels — stock redfin has 2.
 *
 * Build (NDK):
 *   clang -shared -fPIC -O2 -o fake-props.so fake-props.c -ldl
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

struct prop_info; /* opaque bionic handle; we hand out sentinel pointers */

/* name → value table; find() returns the entry's address as the sentinel. */
static const struct {
	const char *name, *val;
} FK[] = {
	{"servicemanager.ready", "true"},
	{"vendor.servicemanager.ready", "true"},
	{"hwservicemanager.ready", "true"},
	{"servicemanager.ping", "true"},
	{"ro.hardware", "redfin"},
	{"ro.boot.hardware", "redfin"},
	{"ro.boot.hardware.radio.subtype", "2"},
	/* vendor data-stack log mask (libdsutils msg module) — full verbosity
	 * for netmgrd/dsi bring-up debugging (M7). */
	{"persist.vendor.net.logmask", "0xffffffff"},
};
#define FK_N (sizeof(FK) / sizeof(FK[0]))

static const char *fk_lookup(const char *n, const void **pi)
{
	if (!n)
		return NULL;
	for (unsigned i = 0; i < FK_N; i++)
		if (!strcmp(n, FK[i].name)) {
			if (pi)
				*pi = &FK[i];
			return FK[i].val;
		}
	return NULL;
}

static const char *fk_value_of(const struct prop_info *pi)
{
	if ((const void *)pi >= (const void *)FK &&
	    (const void *)pi < (const void *)(FK + FK_N))
		return ((const struct { const char *name, *val; } *)pi)->val;
	return NULL;
}

static void log2(const char *fn, const char *a, const char *b)
{
	static FILE *f;
	if (!f) f = fopen("/tmp/fake-props.log", "a");
	if (f) {
		fprintf(f, "%s(%s)%s%s\n", fn, a ? a : "?",
			b ? " = " : "", b ? b : "");
		fflush(f);
	}
}

/* libbase's WaitForProperty polls find + read_callback + serial. */
const struct prop_info *__system_property_find(const char *name)
{
	static const struct prop_info *(*real)(const char *);
	if (!real) real = dlsym(RTLD_NEXT, "__system_property_find");
	const void *pi = NULL;
	const char *v = fk_lookup(name, &pi);
	log2("find", name, v);
	if (v)
		return pi;
	return real(name);
}

int __system_property_read_callback(const struct prop_info *pi,
	void (*cb)(void *, const char *, const char *, uint32_t), void *cookie)
{
	const char *v = fk_value_of(pi);
	if (v) {
		cb(cookie, "", v, (uint32_t)strlen(v));
		return 0;
	}
	static int (*real)(const struct prop_info *, void (*)(void *, const char *,
		const char *, uint32_t), void *);
	if (!real) real = dlsym(RTLD_NEXT, "__system_property_read_callback");
	return real(pi, cb, cookie);
}

uint32_t __system_property_serial(const struct prop_info *pi)
{
	if (fk_value_of(pi))
		return 1;
	static uint32_t (*real)(const struct prop_info *);
	if (!real) real = dlsym(RTLD_NEXT, "__system_property_serial");
	return real(pi);
}

/* Direct string getter used by some callers. */
int __system_property_get(const char *name, char *buf)
{
	const char *v = fk_lookup(name, NULL);
	log2("get", name, v);
	if (v) {
		if (buf)
			strcpy(buf, v);
		return (int)strlen(v);
	}
	static int (*real)(const char *, char *);
	if (!real) real = dlsym(RTLD_NEXT, "__system_property_get");
	return real(name, buf);
}

/* libbase's CachedProperty / waiter may call wait_any in a loop. */
uint32_t __system_property_wait_any(uint32_t old_serial)
{
	/* never block: report a change so the caller re-reads */
	return old_serial == 0 ? 1 : old_serial;
}
