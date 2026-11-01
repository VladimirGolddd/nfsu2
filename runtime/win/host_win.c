/* Windows: the page's side of runtime/web/host_abi.h, for win/host.
 *
 * In the browser the runtime publishes its shared structures to the page with
 * nfsu2_register(kind, ptr, size) and reads the page's clock.  Here the host
 * is linked into the same program: the structures are kept in a table the
 * host reads (nfsu2_host_lookup), and the clock is QueryPerformanceCounter,
 * which the host uses too (GwVsync.ts_ms is on this clock). */
#define WIN32_LEAN_AND_MEAN   /* windows.h would include <winsock.h>: runtime/winsock.h on this path */
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include "web/host_abi.h"

#define NKIND 16
static struct { void *volatile ptr; volatile uint32_t size; } g_reg[NKIND];
static volatile LONG g_reg_gen;

int nfsu2_register(int kind, void *ptr, uint32_t size){
    if (kind <= 0 || kind >= NKIND) return -1;
    g_reg[kind].size = size;
    MemoryBarrier();
    g_reg[kind].ptr = ptr;
    InterlockedIncrement(&g_reg_gen);
    WakeByAddressAll((void *)&g_reg_gen);
    return 0;
}
/* the host: the structure of a kind, waiting at most ms for it (0: none yet) */
void *nfsu2_host_lookup(int kind, uint32_t *size, uint32_t ms){
    if (kind <= 0 || kind >= NKIND) return 0;
    ULONGLONG t0 = GetTickCount64();
    for (;;){
        LONG gen = g_reg_gen;
        void *p = g_reg[kind].ptr;
        if (p){ MemoryBarrier(); if (size) *size = g_reg[kind].size; return p; }
        ULONGLONG el = GetTickCount64() - t0;
        if (el >= ms) return 0;
        WaitOnAddress((void *)&g_reg_gen, &gen, sizeof gen, (DWORD)(ms - el));
    }
}
extern uint8_t *g_mem;
/* the base the 32-bit addresses in the shared structures count from (cpu.h) */
uint8_t *nfsu2_host_mem_base(void){ return g_mem; }

static double g_qpf_ms;
static LARGE_INTEGER g_q0;
static INIT_ONCE g_clk = INIT_ONCE_STATIC_INIT;
static BOOL CALLBACK clk_init(PINIT_ONCE o, PVOID p, PVOID *c){
    (void)o; (void)p; (void)c;
    LARGE_INTEGER f; QueryPerformanceFrequency(&f); g_qpf_ms = 1000.0 / (double)f.QuadPart;
    QueryPerformanceCounter(&g_q0);
    return TRUE;
}
/* ms since the program started (the page: performance.now() + timeOrigin; only differences matter) */
double nfsu2_now_ms(void){
    InitOnceExecuteOnce(&g_clk, clk_init, 0, 0);
    LARGE_INTEGER q; QueryPerformanceCounter(&q);
    return (double)(q.QuadPart - g_q0.QuadPart) * g_qpf_ms;
}
/* the page counts the time a thread spent blocked in the host (its file I/O
 * round trips); here the runtime does its own I/O, so there is none */
double nfsu2_blocked_ms(void){ return 0.0; }
int nfsu2_stack_frames(void){ return 0; }

/* The game's movies: the browser decodes them with the page's VideoDecoder.
 * This host has no decoder yet, so the game is told none opened and skips
 * them (runtime/movie.c). */
int nfsu2_movie_open(int w, int h){ (void)w; (void)h; return -1; }
int nfsu2_movie_decode(int handle, uint32_t payload, uint32_t len, uint32_t y, uint32_t u, uint32_t v, int ys, int uvs){
    (void)handle; (void)payload; (void)len; (void)y; (void)u; (void)v; (void)ys; (void)uvs; return -1; }
int nfsu2_movie_close(int handle){ (void)handle; return 0; }

/* The log: in a windowed program stderr goes nowhere, so the host sends the
 * runtime's stderr (and its own lines) to a file, as the page's Download log. */
#include <stdio.h>
#include <io.h>
#include <fcntl.h>
int nfsu2_host_log_to(const char *path){
    if (!freopen(path, "w", stderr)) return -1;
    setvbuf(stderr, 0, _IONBF, 0);
    return 0;
}
void nfsu2_host_log(const char *line){ fputs(line, stderr); fputc('\n', stderr); }
