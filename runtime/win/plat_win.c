/* Windows: what runtime/win/win_plat.h declares. */
#define WIN32_LEAN_AND_MEAN   /* windows.h would include <winsock.h>: runtime/winsock.h on this path */
#include <windows.h>
#include <stdint.h>
#include <pthread.h>

/* WaitOnAddress / WakeByAddressAll (Windows 8+, synchronization.lib) stand in
 * for wasm's memory.atomic.wait32 / notify; the host waits and wakes on the
 * same words.  0 woken (or a spurious wake: callers re-check), 1 not equal,
 * 2 timed out. */
int nfsu2_wait32(volatile void *p, int32_t v, int64_t ns){
    if (*(volatile int32_t *)p != v) return 1;
    DWORD ms = ns < 0 ? INFINITE : (DWORD)((ns + 999999) / 1000000);
    if (WaitOnAddress(p, &v, sizeof v, ms)) return 0;
    return GetLastError() == ERROR_TIMEOUT ? 2 : 0;
}
unsigned nfsu2_notify(volatile void *p, unsigned count){
    if (count == 1) WakeByAddressSingle((void *)p); else WakeByAddressAll((void *)p);
    return 0;
}

/* pthread_setname_np(name) as on macOS: the calling thread */
int nfsu2_setname(const char *name){
    wchar_t w[64]; int n = MultiByteToWideChar(CP_UTF8, 0, name, -1, w, 64);
    if (n <= 0) return 0;
    typedef HRESULT (WINAPI *SetDesc)(HANDLE, PCWSTR);
    static SetDesc f; static int looked;
    if (!looked){ looked = 1; f = (SetDesc)(void *)GetProcAddress(GetModuleHandleA("kernel32.dll"), "SetThreadDescription"); }
    if (f) f(GetCurrentThread(), w);
    return 0;
}

/* ---- POSIX names MinGW lacks (win_plat.h) ---- */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <io.h>
#include <bcrypt.h>
#include <sys/resource.h>

int setenv(const char *name, const char *value, int overwrite){
    if (!name || !*name || strchr(name, '=')) return -1;
    if (!overwrite && getenv(name)) return 0;
    return _putenv_s(name, value ? value : "") ? -1 : 0;
}
int unsetenv(const char *name){ return _putenv_s(name, "") ? -1 : 0; }

/* positioned I/O on the file's handle: an OVERLAPPED offset on a synchronous
 * handle reads there (and moves the file pointer, which the runtime's pread
 * callers never use) */
ssize_t pread(int fd, void *buf, size_t n, off_t off){
    HANDLE h = (HANDLE)_get_osfhandle(fd); if (h == INVALID_HANDLE_VALUE) return -1;
    OVERLAPPED o; memset(&o, 0, sizeof o); uint64_t q = (uint64_t)off; o.Offset = (DWORD)q; o.OffsetHigh = (DWORD)(q >> 32);
    DWORD got = 0;
    if (!ReadFile(h, buf, (DWORD)(n > 0x7FFFFFFFu ? 0x7FFFFFFFu : n), &got, &o))
        return GetLastError() == ERROR_HANDLE_EOF ? 0 : -1;
    return (ssize_t)got;
}
ssize_t pwrite(int fd, const void *buf, size_t n, off_t off){
    HANDLE h = (HANDLE)_get_osfhandle(fd); if (h == INVALID_HANDLE_VALUE) return -1;
    OVERLAPPED o; memset(&o, 0, sizeof o); uint64_t q = (uint64_t)off; o.Offset = (DWORD)q; o.OffsetHigh = (DWORD)(q >> 32);
    DWORD put = 0;
    if (!WriteFile(h, buf, (DWORD)(n > 0x7FFFFFFFu ? 0x7FFFFFFFu : n), &put, &o)) return -1;
    return (ssize_t)put;
}
void arc4random_buf(void *buf, size_t n){
    if (BCryptGenRandom(NULL, buf, (ULONG)n, BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0){
        static uint64_t x = 0x9E3779B97F4A7C15ull; uint8_t *b = buf;
        x ^= GetTickCount64() ^ (uint64_t)GetCurrentThreadId() << 32;
        for (size_t i = 0; i < n; i++){ x ^= x << 13; x ^= x >> 7; x ^= x << 17; b[i] = (uint8_t)x; }
    }
}
uint32_t arc4random(void){ uint32_t v; arc4random_buf(&v, sizeof v); return v; }
uint32_t arc4random_uniform(uint32_t n){
    if (n < 2) return 0;
    uint32_t min = -n % n, r;
    do r = arc4random(); while (r < min);
    return r % n;
}
int gethostname(char *name, size_t len){
    DWORD n = (DWORD)len;
    if (!len || !GetComputerNameA(name, &n)){ if (len) snprintf(name, len, "windows-pc"); }
    return 0;
}
int ioctl(int fd, unsigned long req, ...){ (void)fd; (void)req; return -1; }   /* only virtual sockets exist */
int getrusage(int who, struct rusage *r){
    memset(r, 0, sizeof *r);
    FILETIME c, e, k, u;
    BOOL ok = who == RUSAGE_THREAD ? GetThreadTimes(GetCurrentThread(), &c, &e, &k, &u)
                                   : GetProcessTimes(GetCurrentProcess(), &c, &e, &k, &u);
    if (ok){
        uint64_t uu = ((uint64_t)u.dwHighDateTime << 32 | u.dwLowDateTime) / 10, kk = ((uint64_t)k.dwHighDateTime << 32 | k.dwLowDateTime) / 10;
        r->ru_utime.tv_sec = (long)(uu / 1000000); r->ru_utime.tv_usec = (long)(uu % 1000000);
        r->ru_stime.tv_sec = (long)(kk / 1000000); r->ru_stime.tv_usec = (long)(kk % 1000000);
    }
    /* peak working set, in bytes as on macOS (the callers shift by 20) */
    typedef struct { DWORD cb, PageFaultCount; SIZE_T PeakWorkingSetSize, WorkingSetSize, a, b, c, d, e, f; } PMC;
    typedef BOOL (WINAPI *GetPMI)(HANDLE, PMC *, DWORD);
    static GetPMI f; static int looked;
    if (!looked){ looked = 1; f = (GetPMI)(void *)GetProcAddress(GetModuleHandleA("kernel32.dll"), "K32GetProcessMemoryInfo"); }
    PMC m; memset(&m, 0, sizeof m); m.cb = sizeof m;
    if (f && f(GetCurrentProcess(), &m, sizeof m)) r->ru_maxrss = (long)(m.PeakWorkingSetSize > 0x7FFFFFFF ? 0x7FFFFFFF : m.PeakWorkingSetSize);
    return 0;
}

/* ---- time: UTC by arithmetic, local time by Windows' zone ---- */
static int64_t days_from_civil(int64_t y, unsigned m, unsigned d){
    y -= m <= 2;
    int64_t era = (y >= 0 ? y : y - 399) / 400;
    unsigned yoe = (unsigned)(y - era * 400);
    unsigned doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1;
    unsigned doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return era * 146097 + (int64_t)doe - 719468;
}
static void civil_from_days(int64_t z, int *y, int *m, int *d){
    z += 719468;
    int64_t era = (z >= 0 ? z : z - 146096) / 146097;
    unsigned doe = (unsigned)(z - era * 146097);
    unsigned yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    int64_t yy = (int64_t)yoe + era * 400;
    unsigned doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    unsigned mp = (5 * doy + 2) / 153;
    *d = (int)(doy - (153 * mp + 2) / 5 + 1);
    *m = (int)(mp < 10 ? mp + 3 : mp - 9);
    *y = (int)(yy + (*m <= 2));
}
static int is_leap(int y){ return (y % 4 == 0 && y % 100 != 0) || y % 400 == 0; }
static void fill_tm(int64_t t, struct tm *o){
    int64_t days = t >= 0 ? t / 86400 : -((-t + 86399) / 86400);
    int64_t s = t - days * 86400;
    int y, m, d; civil_from_days(days, &y, &m, &d);
    o->tm_year = y - 1900; o->tm_mon = m - 1; o->tm_mday = d;
    o->tm_hour = (int)(s / 3600); o->tm_min = (int)(s / 60 % 60); o->tm_sec = (int)(s % 60);
    o->tm_wday = (int)((days % 7 + 11) % 7);                /* 1970-01-01 was a Thursday */
    o->tm_yday = (int)(days - days_from_civil(y, 1, 1));
}
struct tm *nfsu2_gmtime_r(const time_t *t, struct tm *o){
    fill_tm((int64_t)*t, o); o->tm_isdst = 0; o->tm_gmtoff = 0; o->tm_zone = "UTC"; return o;
}
struct tm *nfsu2_gmtime(const time_t *t){ static __thread struct tm r; return nfsu2_gmtime_r(t, &r); }
time_t nfsu2_timegm(struct tm *tm){
    int64_t y = tm->tm_year + 1900LL, mo = tm->tm_mon;
    y += mo >= 0 ? mo / 12 : -((11 - mo) / 12); mo = ((mo % 12) + 12) % 12;
    int64_t t = days_from_civil(y, (unsigned)mo + 1, 1) * 86400 + (int64_t)(tm->tm_mday - 1) * 86400
              + (int64_t)tm->tm_hour * 3600 + (int64_t)tm->tm_min * 60 + tm->tm_sec;
    fill_tm(t, tm); tm->tm_isdst = 0; tm->tm_gmtoff = 0; tm->tm_zone = "UTC";
    return (time_t)t;
}
static char g_tzn[2][64];
const char *nfsu2_tzname[2] = { g_tzn[0], g_tzn[1] };
static INIT_ONCE g_tz_once = INIT_ONCE_STATIC_INIT;
static BOOL CALLBACK tz_names(PINIT_ONCE o, PVOID p, PVOID *c){
    (void)o; (void)p; (void)c;
    TIME_ZONE_INFORMATION z;
    if (GetTimeZoneInformation(&z) != TIME_ZONE_ID_INVALID){
        WideCharToMultiByte(CP_ACP, 0, z.StandardName, -1, g_tzn[0], sizeof g_tzn[0], 0, 0);
        WideCharToMultiByte(CP_ACP, 0, z.DaylightName, -1, g_tzn[1], sizeof g_tzn[1], 0, 0);
    }
    return TRUE;
}
static SYSTEMTIME st_of(int64_t t){
    struct tm g; fill_tm(t, &g); SYSTEMTIME s;
    s.wYear = (WORD)(g.tm_year + 1900); s.wMonth = (WORD)(g.tm_mon + 1); s.wDayOfWeek = (WORD)g.tm_wday;
    s.wDay = (WORD)g.tm_mday; s.wHour = (WORD)g.tm_hour; s.wMinute = (WORD)g.tm_min; s.wSecond = (WORD)g.tm_sec; s.wMilliseconds = 0;
    return s;
}
static int64_t t_of(const SYSTEMTIME *s){
    return days_from_civil(s->wYear, s->wMonth, s->wDay) * 86400 + s->wHour * 3600 + s->wMinute * 60 + s->wSecond;
}
/* the UTC offset (seconds east) at t, and whether it is the zone's daylight one */
static long tz_off(int64_t t, int *dst){
    SYSTEMTIME u = st_of(t), l;
    long off = 0;
    if (SystemTimeToTzSpecificLocalTime(NULL, &u, &l)) off = (long)(t_of(&l) - t);
    *dst = 0;
    TIME_ZONE_INFORMATION z;
    if (GetTimeZoneInformationForYear(u.wYear, NULL, &z) && z.DaylightDate.wMonth)
        *dst = off == -(z.Bias + z.DaylightBias) * 60L && z.DaylightBias != 0;
    return off;
}
struct tm *nfsu2_localtime_r(const time_t *t, struct tm *o){
    InitOnceExecuteOnce(&g_tz_once, tz_names, 0, 0);
    int dst; long off = tz_off((int64_t)*t, &dst);
    fill_tm((int64_t)*t + off, o);
    o->tm_isdst = dst; o->tm_gmtoff = off; o->tm_zone = nfsu2_tzname[dst];
    return o;
}
struct tm *nfsu2_localtime(const time_t *t){ static __thread struct tm r; return nfsu2_localtime_r(t, &r); }
time_t nfsu2_mktime(struct tm *tm){
    InitOnceExecuteOnce(&g_tz_once, tz_names, 0, 0);
    int isdst = tm->tm_isdst;
    struct tm n = *tm;
    int64_t u = (int64_t)nfsu2_timegm(&n);            /* the fields as if UTC, normalised */
    int64_t t;
    if (isdst < 0){
        SYSTEMTIME l = st_of(u), s;
        t = TzSpecificLocalTimeToSystemTime(NULL, &l, &s) ? t_of(&s) : u;
    } else {
        TIME_ZONE_INFORMATION z; long bias = 0;
        if (GetTimeZoneInformation(&z) != TIME_ZONE_ID_INVALID)
            bias = (z.Bias + (isdst > 0 ? z.DaylightBias : z.StandardBias)) * 60L;
        t = u + bias;
    }
    nfsu2_localtime_r((time_t *)&t, tm);
    return (time_t)t;
}
