/* Windows build: no dynamic symbol lookup (runtime/web/null_platform.c dlsym) */
#pragma once
#define RTLD_DEFAULT ((void *)0)
#define RTLD_NOW 2
void *dlsym(void *h, const char *n);
static inline void *dlopen(const char *f, int m){ (void)f; (void)m; return 0; }
