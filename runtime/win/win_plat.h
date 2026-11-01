/* Force-included (-include) in every runtime file of the Windows build
 * (win/build.sh).  The Windows build is the browser's build for another
 * machine: the shared runtime with the runtime/web platform layer, compiled
 * to x86-64 by clang for MinGW-w64, with win/host (a native program) in the
 * page's place.  This header gives that code what wasi-libc and the page
 * gave it: the few POSIX and macOS names MinGW lacks, and the wasm atomics'
 * wait/notify (memory.atomic.wait32 / notify) on WaitOnAddress. */
#ifndef NFSU2_WIN_PLAT_H
#define NFSU2_WIN_PLAT_H
#ifndef NFSU2_WIN
#define NFSU2_WIN 1
#endif
#include <stdint.h>
#include <time.h>
#include <pthread.h>

/* macOS clock ids: one monotonic clock */
#ifndef CLOCK_UPTIME_RAW
#define CLOCK_UPTIME_RAW CLOCK_MONOTONIC
#endif
#ifndef CLOCK_MONOTONIC_RAW
#define CLOCK_MONOTONIC_RAW CLOCK_MONOTONIC
#endif
static inline uint64_t clock_gettime_nsec_np(clockid_t id){
    struct timespec ts; clock_gettime(id, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}
/* macOS's one-argument pthread_setname_np (the calling thread) */
int nfsu2_setname(const char *name);
#define pthread_setname_np(n) nfsu2_setname(n)
#include <unistd.h>
static inline int pause(void){ for (;;) sleep(3600); return -1; }

/* wasm's memory.atomic.wait32 / notify on a 32-bit word of shared memory
 * (runtime/web/ files and the host use the same words).  wait: 0 woken, 1 the
 * word was not v, 2 timed out (ns < 0: no timeout). */
int nfsu2_wait32(volatile void *p, int32_t v, int64_t ns);
unsigned nfsu2_notify(volatile void *p, unsigned count);
#define __builtin_wasm_memory_atomic_wait32(p, v, ns) nfsu2_wait32((p), (int32_t)(v), (int64_t)(ns))
#define __builtin_wasm_memory_atomic_notify(p, n) nfsu2_notify((p), (unsigned)(n))

/* ---- POSIX names MinGW lacks (runtime/win/plat_win.c) ---- */
#include <stdlib.h>
#include <string.h>
#include <io.h>
#include <direct.h>
#include <sys/types.h>
#include <sys/stat.h>
#define mkdir(p, m) mkdir(p)
int setenv(const char *name, const char *value, int overwrite);
int unsetenv(const char *name);
ssize_t pread(int fd, void *buf, size_t n, off_t off);
ssize_t pwrite(int fd, const void *buf, size_t n, off_t off);
#define fsync(fd) _commit(fd)
void arc4random_buf(void *buf, size_t n);
uint32_t arc4random(void);
uint32_t arc4random_uniform(uint32_t n);
int gethostname(char *name, size_t len);
int ioctl(int fd, unsigned long req, ...);
static inline void *sbrk(intptr_t inc){ (void)inc; return 0; }
/* never freed in the runtime (the virtual LAN's mailboxes): blocks above
 * the guest span, as the GPU's */
#define aligned_alloc(a, n) mem_host_alloc((n), (a))

/* struct tm with the BSD fields the runtime reads (tm_gmtoff, tm_zone), and
 * local time from Windows' own zone (GetTimeZoneInformation), so the Win32
 * time-zone HLE (win32_ext.c) shows the game the PC's real zone */
struct nfsu2_tm { int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst;
                  long tm_gmtoff; const char *tm_zone; };
#define tm nfsu2_tm
extern const char *nfsu2_tzname[2];
struct tm *nfsu2_gmtime_r(const time_t *t, struct tm *out);
struct tm *nfsu2_localtime_r(const time_t *t, struct tm *out);
struct tm *nfsu2_localtime(const time_t *t);
struct tm *nfsu2_gmtime(const time_t *t);
time_t nfsu2_mktime(struct tm *tm);
time_t nfsu2_timegm(struct tm *tm);
#undef localtime_r
#undef gmtime_r
#define tzname nfsu2_tzname
#define gmtime_r nfsu2_gmtime_r
#define localtime_r nfsu2_localtime_r
#define localtime nfsu2_localtime
#define gmtime nfsu2_gmtime
#define mktime nfsu2_mktime
#define timegm nfsu2_timegm

/* A block above the guest span for the structures the host reads (mem.c),
 * and its 32-bit address as the host sees it (the page's "wasm address"). */
#include <stddef.h>
void *mem_host_alloc(size_t size, size_t align);
#endif
