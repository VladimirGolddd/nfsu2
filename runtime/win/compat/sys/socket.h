/* Windows build: the socket names the runtime's virtual-LAN code paths use
 * (runtime/winsock.c, runtime/web/net_lan.c).  As in the browser, the game's
 * sockets are virtual (runtime/web/lan_vfd.h): no host socket is ever made,
 * so these are only types and constants, not Winsock's (which would clash
 * with runtime/winsock.h's guest-side names). */
#pragma once
#include <stdint.h>
#include <stddef.h>
#include <sys/types.h>
typedef uint32_t socklen_t;
typedef uint16_t sa_family_t;
struct sockaddr { sa_family_t sa_family; char sa_data[14]; };
struct sockaddr_storage { sa_family_t ss_family; char pad[126]; };
#define AF_UNSPEC 0
#define AF_INET 2
#define AF_UNIX 1
#define PF_INET AF_INET
#define SOCK_STREAM 1
#define SOCK_DGRAM 2
#define SOCK_RAW 3
#define SOL_SOCKET 0xFFFF
#define SO_REUSEADDR 0x0004
#define SO_BROADCAST 0x0020
#define SO_LINGER 0x0080
#define SO_SNDBUF 0x1001
#define SO_RCVBUF 0x1002
#define SO_ERROR 0x1007
#define SO_KEEPALIVE 0x0008
#define MSG_PEEK 0x2
#define MSG_DONTWAIT 0x40
#define MSG_NOSIGNAL 0
#define SHUT_RD 0
#define SHUT_WR 1
#define SHUT_RDWR 2
struct linger { int l_onoff, l_linger; };
