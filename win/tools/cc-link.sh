#!/bin/sh
# The linker for the Rust host (win/host/.cargo/config.toml) on Linux: clang
# driving lld for x86_64-w64-mingw32.  GNU ld (MinGW's own) drops the
# runtime's weak definitions (runtime/web/null_platform.c) from the archive;
# lld keeps them.  gcc's -fno-use-linker-plugin means nothing to clang.
for a; do shift; [ "$a" = -fno-use-linker-plugin ] || set -- "$@" "$a"; done
exec clang --target=x86_64-w64-mingw32 -fuse-ld=lld -Wno-unused-command-line-argument "$@"
