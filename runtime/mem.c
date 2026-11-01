/* Guest memory: 4 GB reserved (PROT_NONE), the regions in use get committed. */
#include <errno.h>
#include <pthread.h>
#include "cpu.h"
#include <stdio.h>
#include <stdlib.h>
#if !defined(__wasm__) && !defined(NFSU2_WIN)
#include <signal.h>
#include <sys/mman.h>
#endif
#ifdef NFSU2_WIN
#define WIN32_LEAN_AND_MEAN   /* windows.h would include <winsock.h>: runtime/winsock.h on this path */
#include <windows.h>
#endif
#include <string.h>
#include <unistd.h>
#include <stdlib.h>
void (*g_backtrace_fn)(struct CPU*,int) = 0;   /* set by dispatch.c */
CPU *g_cur_cpu = 0;

#ifndef __wasm__
uint8_t *g_mem = 0;
#endif
uint32_t g_image_base = 0x400000u, g_res_dir_va = 0, g_res_dir_size = 0;

/* watch state: must be defined in every build (debug.c is optional) */
uint32_t g_itrace[ITRACE_SIZE];
uint64_t g_itrace_n = 0;
uint32_t g_watch_addr = 0, g_watch_val = 0, g_watch_prev_ip = 0;
void (*g_itrace_dump_fn)(int) = 0;      /* set by debug.c */
void (*g_report_fn)(void) = 0;          /* set by boot.c */
__attribute__((weak)) void watch_report_change(uint32_t ip, uint32_t o, uint32_t n){
    (void)ip;(void)o;(void)n;
}
static uint64_t g_committed = 0;
/* bitmap of the committed 16 KB pages (4 GB / 16 KB = 262144 bits = 32 KB) */
uint8_t g_cbits[1u<<15] = {0};
/* The body is now inline in cpu.h (three instructions).  As a separate call
 * it was 1.45% of the guest thread in the profile -- call overhead, not work. */


void mprot_arm(uint32_t);
#if defined(NFSU2_WIN)
/* Windows (cpu.h): 4 GB reserved at g_mem; the guest span [0, GUEST_END)
 * is committed whole at start, so it reads and writes like the browser's
 * linear memory (Windows hands out the zero pages as they are touched; the
 * commit charge is 640 MB).  Committing only keeps the bitmap, as on wasm;
 * release gives the pages back (decommit + commit: they read zero again).
 * Above GUEST_END: the host blocks (mem_host_alloc), the guest never goes
 * there.  A fault inside the reservation is reported with the guest state. */
uint32_t guest_fn_from_host(const void *pc);   /* dispatch.c */
#define HOST_LO 0x30000000ull
#define HOST_HI 0xF0000000ull
static uint64_t g_host_next = HOST_LO;
static SRWLOCK g_host_lk = SRWLOCK_INIT;
static LONG WINAPI win_fault(EXCEPTION_POINTERS *x){
    DWORD code = x->ExceptionRecord->ExceptionCode;
    if (code != EXCEPTION_ACCESS_VIOLATION && code != EXCEPTION_STACK_OVERFLOW) return EXCEPTION_CONTINUE_SEARCH;
    static volatile LONG once;
    if (InterlockedExchange(&once, 1)) return EXCEPTION_CONTINUE_SEARCH;
    uint8_t *p = code == EXCEPTION_ACCESS_VIOLATION ? (uint8_t *)x->ExceptionRecord->ExceptionInformation[1] : 0;
    void *pc = x->ExceptionRecord->ExceptionAddress;
    uint32_t gfn = guest_fn_from_host(pc);
    if (gfn) fprintf(stderr, "\n*** FAULT IN GUEST FUNCTION: sub_%08x\n", gfn);
    if (code == EXCEPTION_STACK_OVERFLOW)
        fprintf(stderr, "\n*** HOST STACK OVERFLOW (endless recursion in guest code?) at %p\n", pc);
    else if (p >= g_mem && p < g_mem + GSZ)
        fprintf(stderr, "\n*** GUEST MEMORY FAULT: guest address 0x%08llx (pc %p)\n", (unsigned long long)(p - g_mem), pc);
    else
        fprintf(stderr, "\n*** HOST ACCESS VIOLATION: %p (pc %p)\n", (void *)p, pc);
    if (g_cur_cpu){
        CPU *c = (CPU *)g_cur_cpu;
        fprintf(stderr, "    eax=%08x ecx=%08x edx=%08x ebx=%08x\n", c->eax, c->ecx, c->edx, c->ebx);
        fprintf(stderr, "    esp=%08x ebp=%08x esi=%08x edi=%08x\n", c->esp, c->ebp, c->esi, c->edi);
    }
    if (g_cur_cpu && g_backtrace_fn) g_backtrace_fn(g_cur_cpu, 12);
    if (g_report_fn) g_report_fn();
    fflush(stderr);
    return EXCEPTION_CONTINUE_SEARCH;
}
void mem_init(void){
    if (g_mem) return;
    g_mem = VirtualAlloc(0, (SIZE_T)GSZ, MEM_RESERVE, PAGE_NOACCESS);
    if (!g_mem){ fprintf(stderr, "VirtualAlloc: could not reserve 4 GB of address space (error %lu)\n", GetLastError()); exit(1); }
    if (!VirtualAlloc(g_mem, GUEST_END, MEM_COMMIT, PAGE_READWRITE)){
        fprintf(stderr, "VirtualAlloc: could not commit the guest's %u MB (error %lu)\n", GUEST_END >> 20, GetLastError()); exit(1); }
}
void mem_commit(uint32_t addr, uint32_t size){
    uint64_t lo = addr & ~0x3FFFull;
    uint64_t hi = ((uint64_t)addr + size + 0x3FFF) & ~0x3FFFull;
    if (hi > GUEST_END){
        static int shown = 0;
        if (shown++ < 8) fprintf(stderr, "mem_commit 0x%08x+%u beyond the guest span (0x%08x): ignored\n", addr, size, GUEST_END);
        if (lo >= GUEST_END) return;
        hi = GUEST_END;
    }
    for (uint64_t a2 = lo; a2 < hi; a2 += 0x4000ull){
        uint32_t pg = (uint32_t)(a2 >> 14);
        if (!(g_cbits[pg>>3] & (1u << (pg&7)))){ g_cbits[pg>>3] |= (uint8_t)(1u << (pg&7)); g_committed += 0x4000u; }
    }
}
uint64_t mem_committed(void){ return g_committed; }
void mem_release(uint32_t addr, uint32_t size, int rw){
    uint64_t lo = ((uint64_t)addr + 0x3FFF) & ~0x3FFFull;
    uint64_t hi = ((uint64_t)addr + size) & ~0x3FFFull;
    if (hi <= lo || hi > GUEST_END) return;
    if (!rw) memset(g_mem + lo, 0xDD, (size_t)(hi - lo));          /* HEAPCHECK: poison, no fault */
    else if (!VirtualFree(g_mem + lo, (SIZE_T)(hi - lo), MEM_DECOMMIT)
             || !VirtualAlloc(g_mem + lo, (SIZE_T)(hi - lo), MEM_COMMIT, PAGE_READWRITE))
        memset(g_mem + lo, 0, (size_t)(hi - lo));
    for (uint64_t a2 = lo; a2 < hi; a2 += 0x4000ull){
        uint32_t pg = (uint32_t)(a2 >> 14);
        if (rw) g_cbits[pg>>3] |= (uint8_t)(1u << (pg&7));
        else    g_cbits[pg>>3] &= (uint8_t)~(1u << (pg&7));
    }
}
/* A zeroed, committed block above the guest span for the host's shared
 * structures; its offset from g_mem fits 32 bits (HADDR, runtime/web).
 * Never freed (the renderer's arenas live as long as the process). */
void *mem_host_alloc(size_t size, size_t align){
    if (align < 64) align = 64;
    AcquireSRWLockExclusive(&g_host_lk);
    uint64_t a = (g_host_next + align - 1) & ~(uint64_t)(align - 1);
    uint64_t e = (a + size + 0xFFFF) & ~0xFFFFull;
    void *p = 0;
    if (e <= HOST_HI && VirtualAlloc(g_mem + a, (SIZE_T)(e - a), MEM_COMMIT, PAGE_READWRITE)){ p = g_mem + a; g_host_next = e; }
    ReleaseSRWLockExclusive(&g_host_lk);
    if (!p) fprintf(stderr, "mem_host_alloc: no room for %zu bytes\n", size);
    return p;
}
void mprot_arm(uint32_t a){ fprintf(stderr, "mprot: not available on Windows (0x%08x)\n", a); }
void mprot_rearm(void){ }
uint8_t *g_mprot_page = 0;
void mem_altstack_install(void){ }
void mem_fault_handler_install(void){ AddVectoredExceptionHandler(0, win_fault); }
#elif defined(__wasm__)
/* wasm: the guest span [0, GUEST_END) is ordinary linear memory, readable,
 * writable and zero from the start.  Committing only keeps the bitmap
 * (mem_is_committed, genexec, autotest probes) and the statistics; release
 * zeroes the whole 16 KB pages so a released range still reads as zero
 * (heap.c relies on it).  No page is ever given back to the browser. */
void mem_init(void){ }
void mem_commit(uint32_t addr, uint32_t size){
    uint64_t lo = addr & ~0x3FFFull;
    uint64_t hi = ((uint64_t)addr + size + 0x3FFF) & ~0x3FFFull;
    if (hi > GUEST_END){
        static int shown = 0;
        if (shown++ < 8) fprintf(stderr, "mem_commit 0x%08x+%u beyond the guest span (0x%08x): ignored\n", addr, size, GUEST_END);
        if (lo >= GUEST_END) return;
        hi = GUEST_END;
    }
    for (uint64_t a2 = lo; a2 < hi; a2 += 0x4000ull){
        uint32_t pg = (uint32_t)(a2 >> 14);
        if (!(g_cbits[pg>>3] & (1u << (pg&7)))){ g_cbits[pg>>3] |= (uint8_t)(1u << (pg&7)); g_committed += 0x4000u; }
    }
}
uint64_t mem_committed(void){ return g_committed; }
void mem_release(uint32_t addr, uint32_t size, int rw){
    uint64_t lo = ((uint64_t)addr + 0x3FFF) & ~0x3FFFull;
    uint64_t hi = ((uint64_t)addr + size) & ~0x3FFFull;
    if (hi <= lo || hi > GUEST_END) return;
    memset(g_mem + lo, rw ? 0 : 0xDD, (size_t)(hi - lo));     /* rw=0 (HEAPCHECK): poison, no fault */
    for (uint64_t a2 = lo; a2 < hi; a2 += 0x4000ull){
        uint32_t pg = (uint32_t)(a2 >> 14);
        if (rw) g_cbits[pg>>3] |= (uint8_t)(1u << (pg&7));
        else    g_cbits[pg>>3] &= (uint8_t)~(1u << (pg&7));
    }
}
void mprot_arm(uint32_t a){ fprintf(stderr, "mprot: not available in the browser (0x%08x)\n", a); }
void mprot_rearm(void){ }
uint8_t *g_mprot_page = 0;
void mem_altstack_install(void){ }
void mem_fault_handler_install(void){ }
#else
void mprot_arm(uint32_t);
void mem_init(void){
    if (g_mem) return;
    g_mem = mmap(0, GSZ, PROT_NONE, MAP_PRIVATE|MAP_ANON|MAP_NORESERVE, -1, 0);
    if (g_mem == MAP_FAILED){ perror("mmap 4GB"); exit(1); }
}

void mem_commit(uint32_t addr, uint32_t size){
    uint64_t lo = addr & ~0x3FFFull;                    /* 16 KB page */
    uint64_t hi = ((uint64_t)addr + size + 0x3FFF) & ~0x3FFFull;
    if (hi > 0x100000000ull) hi = 0x100000000ull;
    if (mprotect(g_mem + lo, hi - lo, PROT_READ|PROT_WRITE) != 0){
        fprintf(stderr,"mem_commit 0x%08x+%u failed\n", addr, size); exit(1);
    }
    for (uint64_t a2 = lo; a2 < hi; a2 += 0x4000ull){
        uint32_t pg = (uint32_t)(a2 >> 14);
        g_cbits[pg>>3] |= (uint8_t)(1u << (pg&7));
    }
    g_committed += hi - lo;
    /* WATCHADDR=<address>: arm the watchpoint as soon as that address is
     * committed, so even the first writer is caught. */
    { static int wa_done = 0;
      const char *wa = getenv("WATCHADDR");
      if (wa && !wa_done){
          uint32_t t = (uint32_t)strtoul(wa, 0, 0);
          if ((uint64_t)t >= lo && (uint64_t)t < hi){ wa_done = 1; mprot_arm(t); }
      } }
}
uint64_t mem_committed(void){ return g_committed; }

/* Guest heap (heap.c): give a released range back to the host.  The range is
 * replaced by fresh anonymous pages, so its physical memory is freed at once
 * and it reads as zero afterwards.  rw=1 keeps it accessible (a stale guest
 * pointer then reads zeros, as it read old data under the bump allocator);
 * rw=0 makes it PROT_NONE so a use-after-release faults (NFSU2_HEAPCHECK).
 * Only whole 16 KB host pages inside [addr, addr+size) are touched. */
void mem_release(uint32_t addr, uint32_t size, int rw){
    uint64_t lo = ((uint64_t)addr + 0x3FFF) & ~0x3FFFull;
    uint64_t hi = ((uint64_t)addr + size) & ~0x3FFFull;
    if (hi <= lo) return;
    void *p = mmap(g_mem + lo, hi - lo, rw ? (PROT_READ|PROT_WRITE) : PROT_NONE,
                   MAP_FIXED|MAP_PRIVATE|MAP_ANON|MAP_NORESERVE, -1, 0);
    if (p == MAP_FAILED){ fprintf(stderr,"mem_release 0x%08x+%u failed (%s)\n", addr, size, strerror(errno)); return; }
    for (uint64_t a2 = lo; a2 < hi; a2 += 0x4000ull){
        uint32_t pg = (uint32_t)(a2 >> 14);
        if (rw) g_cbits[pg>>3] |= (uint8_t)(1u << (pg&7));
        else    g_cbits[pg>>3] &= (uint8_t)~(1u << (pg&7));
    }
}

/* Diagnose a guest access to an uncommitted region */
static volatile int g_in_handler = 0;
/* --- write capture: make a guest page read-only and find the writer ---
 * Answers "who writes this memory?" without watchpoint instrumentation:
 * the write raises SIGSEGV, and the generated function (hence the guest
 * function) is resolved from the faulting host PC.                       */
uint8_t *g_mprot_page = 0;
static size_t g_mprot_sz = 0;
static int g_mprot_hits = 0;
static uint32_t g_mprot_target = 0;      /* the exact address we care about */
static volatile int g_mprot_found = 0;   /* the target's writer was found */
static int      g_mprot_persist = 0;     /* re-arm every frame */
/* provided by dispatch.c; weak default for standalone-linked test runs */
__attribute__((weak)) uint32_t guest_fn_from_host(const void *pc){ (void)pc; return 0; }

/* The protected page is 16 KB and very busy: the first unrelated write drops
 * the protection.  A separate thread keeps re-arming it, so the target's
 * writer gets caught. */
static void *mprot_rearm_thread(void *a0){
    (void)a0;
    for (;;){
        /* re-protect even when g_mprot_page is set: an intervening
         * mem_commit may have silently removed the protection. */
        if (g_mprot_target && !g_mprot_found){
            size_t ps = (size_t)getpagesize();
            uint8_t *pg = (uint8_t*)((uintptr_t)(g_mem + g_mprot_target) & ~(uintptr_t)(ps-1));
            if (mprotect(pg, ps, PROT_READ) == 0){ g_mprot_page = pg; g_mprot_sz = ps; }
        }
        struct timespec ts = {0, 50 * 1000};    /* 50 us */
        nanosleep(&ts, 0);
    }
    return 0;
}

void mprot_arm(uint32_t guest_addr){
    /* On Apple Silicon a page is 16 KB; take the alignment from the system value. */
    size_t ps = (size_t)getpagesize();
    uint8_t *pg = (uint8_t*)((uintptr_t)(g_mem + guest_addr) & ~(uintptr_t)(ps - 1));
    if (mprotect(pg, ps, PROT_READ) != 0){
        fprintf(stderr,"mprot: could not be set up (%s)\n", strerror(errno)); return;
    }
    g_mprot_page = pg; g_mprot_sz = ps; g_mprot_target = guest_addr;
    g_mprot_persist = getenv("MPROT_PERSIST") ? 1 : 0;
    if (getenv("MPROT_HARD")){
        static pthread_t th; static int started = 0;
        if (!started){ started = 1; pthread_create(&th, 0, mprot_rearm_thread, 0); }
    }
    fprintf(stderr,"mprot: guest 0x%08x page read-only (to catch the writer)\n",
            guest_addr);
}

/* Re-arm the protection at the start of a frame (when one capture is not enough). */
void mprot_rearm(void){
    if (!g_mprot_persist || g_mprot_page || !g_mprot_target) return;
    size_t ps = (size_t)getpagesize();
    uint8_t *pg = (uint8_t*)((uintptr_t)(g_mem + g_mprot_target) & ~(uintptr_t)(ps - 1));
    if (mprotect(pg, ps, PROT_READ) == 0){ g_mprot_page = pg; g_mprot_sz = ps; }
}

static void segv(int sig, siginfo_t *si, void *uc){
    (void)sig;
    /* a write to the protected page? */
    if (g_mprot_page){
        uint8_t *fa = (uint8_t*)si->si_addr;
        if (fa >= g_mprot_page && fa < g_mprot_page + g_mprot_sz){
            void *pc = 0;
#if defined(__aarch64__)
            ucontext_t *u = (ucontext_t*)uc;
            if (u && u->uc_mcontext) pc = (void*)(uintptr_t)u->uc_mcontext->__ss.__pc;
#endif
            uint32_t gfn = pc ? guest_fn_from_host(pc) : 0;
            uint32_t ga = (uint32_t)(fa - g_mem);
            int interesting = !g_mprot_target ||
                (ga >= g_mprot_target && ga < g_mprot_target + 64u);
            if (getenv("MPROT_ANY")) interesting = 1;   /* mechanism test */
            if (interesting && g_mprot_hits < 12){
                g_mprot_hits++;
                fprintf(stderr,"mprot: WRITE TO TARGET guest 0x%08x  guest function=sub_%08x\n",
                        ga, gfn);
                if (g_mprot_hits >= 6) g_mprot_found = 1;
            }
            mprotect(g_mprot_page, g_mprot_sz, PROT_READ|PROT_WRITE);
            g_mprot_page = 0;           /* one-shot */
            return;                      /* let the instruction run again */
        }
    }
    if (g_in_handler){ fprintf(stderr,"\n*** a second fault inside the handler - exiting\n"); _exit(4); }
    g_in_handler = 1;
    /* The GUEST FUNCTION that faulted: resolved from the host PC.  The stack
     * trace only gives the callers; this tells exactly where the fault was. */
    { void *fpc = 0;
#if defined(__aarch64__)
      ucontext_t *u2 = (ucontext_t*)uc;
      if (u2 && u2->uc_mcontext) fpc = (void*)(uintptr_t)u2->uc_mcontext->__ss.__pc;
#endif
      uint32_t ffn = fpc ? guest_fn_from_host(fpc) : 0;
      if (ffn) fprintf(stderr,"\n*** FAULT IN GUEST FUNCTION: sub_%08x\n", ffn);
    }
    uint8_t *p = (uint8_t*)si->si_addr;
    if (p >= g_mem && p < g_mem + GSZ)
        fprintf(stderr,"\n*** GUEST MEMORY FAULT: guest address 0x%08llx (unmapped)\n",
                (unsigned long long)(p - g_mem));
    else
        fprintf(stderr,"\n*** HOST SEGV: %p (outside the guest area - probably a HOST STACK OVERFLOW:"
                       " endless recursion in guest code)\n", p);
    { extern uint32_t g_watch_prev_ip; extern uint64_t g_itrace_n;
      if (g_itrace_n) fprintf(stderr,"    GUEST INSTRUCTION THAT FAULTED: 0x%08x\n", g_watch_prev_ip); }
    if (g_cur_cpu){
        CPU *c = (CPU*)g_cur_cpu;
        fprintf(stderr,"    eax=%08x ecx=%08x edx=%08x ebx=%08x\n", c->eax,c->ecx,c->edx,c->ebx);
        fprintf(stderr,"    esp=%08x ebp=%08x esi=%08x edi=%08x  df=%u\n", c->esp,c->ebp,c->esi,c->edi,c->df);
        fprintf(stderr,"    fs_base=%08x tid=%u   [ebp+8]=%08x [ebp+4]=%08x\n",
                c->fs_base,
                (c->fs_base && mem_is_committed(c->fs_base+0x24u)) ? MR32(c->fs_base+0x24u) : 0u,
                mem_is_committed(c->ebp+8u) ? MR32(c->ebp+8u) : 0xDEADu,
                mem_is_committed(c->ebp+4u) ? MR32(c->ebp+4u) : 0xDEADu);
    }
    if (g_cur_cpu && g_backtrace_fn) g_backtrace_fn(g_cur_cpu, 12);
    if (g_itrace_dump_fn && getenv("ITRACE")) g_itrace_dump_fn(atoi(getenv("ITRACE")));
    if (g_report_fn) g_report_fn();
    _exit(3);
}
/* sigaltstack is set up PER THREAD: every guest thread must set up its own
 * signal stack, or on a stack overflow the handler crashes too.        */
void mem_altstack_install(void){
    size_t sz = (size_t)SIGSTKSZ * 8;
    void *p = malloc(sz);
    if (!p) return;
    stack_t ss; ss.ss_sp = p; ss.ss_size = sz; ss.ss_flags = 0;
    sigaltstack(&ss, 0);
}
void mem_fault_handler_install(void){
    mem_altstack_install();
    struct sigaction sa; memset(&sa,0,sizeof sa);
    sa.sa_sigaction = segv; sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigaction(SIGSEGV,&sa,0); sigaction(SIGBUS,&sa,0);
}

#endif /* !__wasm__ && !NFSU2_WIN */

int mem_load_pe(const char *path){
    FILE *f = fopen(path, "rb");
    if(!f){ perror(path); return -1; }
    fseek(f,0,SEEK_END); long sz=ftell(f); fseek(f,0,SEEK_SET);
    uint8_t *buf = malloc(sz);
    if (fread(buf,1,sz,f)!=(size_t)sz){ fclose(f); free(buf); return -1; }
    fclose(f);
    uint32_t e_lfanew = *(uint32_t*)(buf+0x3c);
    uint8_t *nt = buf + e_lfanew;
    if (*(uint32_t*)nt != 0x00004550u){ fprintf(stderr,"no PE signature\n"); free(buf); return -1; }
    uint16_t nsec  = *(uint16_t*)(nt+6);
    uint16_t optsz = *(uint16_t*)(nt+20);
    uint32_t base  = *(uint32_t*)(nt+24+28);
    uint32_t imgsz = *(uint32_t*)(nt+24+56);
    /* DataDirectory[2] = RESOURCE */
    { /* PE32: DataDirectory at byte 96 of the optional header; RESOURCE = index 2 */
      uint32_t dd = 24 + 96 + 2*8;
      uint32_t rva = *(uint32_t*)(nt + dd);
      uint32_t sz  = *(uint32_t*)(nt + dd + 4);
      g_image_base = base; g_res_dir_va = rva ? base + rva : 0; g_res_dir_size = sz; }
    uint8_t *sh    = nt + 24 + optsz;
    mem_init();
    mem_commit(base, imgsz);                 /* the whole image range */
    for (int i=0;i<nsec;i++){
        uint8_t *s = sh + i*40;
        char nm[9]={0}; memcpy(nm,s,8);
        uint32_t vsize=*(uint32_t*)(s+8), va=*(uint32_t*)(s+12);
        uint32_t rsize=*(uint32_t*)(s+16), roff=*(uint32_t*)(s+20);
        uint32_t n = rsize < vsize ? rsize : vsize;
        memcpy(g_mem + base + va, buf + roff, n);
        fprintf(stderr,"  loaded %-9s -> 0x%08x  %u bytes\n", nm[0]?nm:"<unnamed>", base+va, n);
    }
    free(buf);
    fprintf(stderr,"  image 0x%08x .. 0x%08x   resource directory 0x%08x (%u bytes)\n",
            base, base+imgsz, g_res_dir_va, g_res_dir_size);
    return 0;
}
