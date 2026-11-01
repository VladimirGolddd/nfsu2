/* Windows build: see sys/socket.h */
#pragma once
#include <sys/socket.h>
typedef uint32_t in_addr_t;
typedef uint16_t in_port_t;
struct in_addr { in_addr_t s_addr; };
struct sockaddr_in { sa_family_t sin_family; in_port_t sin_port; struct in_addr sin_addr; char sin_zero[8]; };
#define INADDR_ANY ((in_addr_t)0)
#define INADDR_BROADCAST ((in_addr_t)0xFFFFFFFF)
#define INADDR_LOOPBACK ((in_addr_t)0x7F000001)
#define INADDR_NONE ((in_addr_t)0xFFFFFFFF)
#define IPPROTO_IP 0
#define IPPROTO_ICMP 1
#define IPPROTO_TCP 6
#define IPPROTO_UDP 17
#define INET_ADDRSTRLEN 16
static inline uint16_t htons(uint16_t v){ return __builtin_bswap16(v); }
static inline uint16_t ntohs(uint16_t v){ return __builtin_bswap16(v); }
static inline uint32_t htonl(uint32_t v){ return __builtin_bswap32(v); }
static inline uint32_t ntohl(uint32_t v){ return __builtin_bswap32(v); }
