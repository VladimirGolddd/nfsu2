/* Windows build: getrusage for the diagnostics lines (peak memory, CPU) */
#pragma once
#include <sys/time.h>
#define RUSAGE_SELF 0
#define RUSAGE_THREAD 1
struct rusage { struct timeval ru_utime, ru_stime; long ru_maxrss; };
int getrusage(int who, struct rusage *r);   /* runtime/win/plat_win.c */
