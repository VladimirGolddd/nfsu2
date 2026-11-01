/* Windows build: see sys/socket.h (only virtual fds are polled) */
#pragma once
typedef unsigned long nfds_t;
struct pollfd { int fd; short events, revents; };
#define POLLIN 0x0001
#define POLLPRI 0x0002
#define POLLOUT 0x0004
#define POLLERR 0x0008
#define POLLHUP 0x0010
#define POLLNVAL 0x0020
#define POLLRDNORM 0x0040
#define POLLWRNORM 0x0100
