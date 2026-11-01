/* Windows build: see sys/socket.h */
#pragma once
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
static inline const char *inet_ntop(int af, const void *src, char *dst, socklen_t n){
    (void)af; const uint8_t *b = (const uint8_t *)src;
    snprintf(dst, n, "%u.%u.%u.%u", b[0], b[1], b[2], b[3]); return dst; }
static inline int inet_pton(int af, const char *s, void *dst){
    (void)af; unsigned a, b, c, d; char x;
    if (sscanf(s, "%u.%u.%u.%u%c", &a, &b, &c, &d, &x) != 4 || a > 255 || b > 255 || c > 255 || d > 255) return 0;
    uint8_t *o = (uint8_t *)dst; o[0] = a; o[1] = b; o[2] = c; o[3] = d; return 1; }
static inline in_addr_t inet_addr(const char *s){ in_addr_t a; return inet_pton(AF_INET, s, &a) == 1 ? a : INADDR_NONE; }
static inline char *inet_ntoa(struct in_addr a){ static __thread char b[16]; return (char *)inet_ntop(AF_INET, &a, b, sizeof b); }
static inline int inet_aton(const char *s, struct in_addr *a){ return inet_pton(AF_INET, s, &a->s_addr); }
