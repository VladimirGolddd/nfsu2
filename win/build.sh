#!/bin/bash
# Build the game for Windows 10/11 (x86-64): out/win/nfsu2.exe
#
#   win/build.sh               lifted shards (incremental) + runtime + host + link
#   FULL=1 win/build.sh        recompile every shard
#   GEN=dir win/build.sh       lifted code from dir/all and dir/server (default gen/)
#   STUB=1 win/build.sh        no game code: a stand-in lift (win/tools/stubgen.sh),
#                              to check that everything compiles and links
#
# The Windows build is the browser build for another machine: the same lift
# (web/gen.sh), the same shared runtime and the same platform layer
# (runtime/web), compiled to x86-64 by clang for MinGW-w64 instead of to
# WebAssembly, and a native host (win/host, Rust: a window, Direct3D 12 or
# Vulkan through wgpu, WASAPI sound, keyboard, mouse and XInput pads) in the
# page's place.  runtime/win/win_plat.h is force-included in every runtime
# file; NFSU2_WIN selects the browser's code paths in the shared runtime.
#
# Builds on Linux (cross) or in MSYS2/WSL with:
#   clang 17+ and the MinGW-w64 headers and libraries (Ubuntu: apt install clang mingw-w64)
#   Rust with the x86_64-pc-windows-gnu target (rustup target add x86_64-pc-windows-gnu)
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
TRIPLE=x86_64-w64-mingw32
CC="${CC:-clang}"
command -v "$CC" >/dev/null || { echo "win/build.sh: $CC not found"; exit 1; }
command -v $TRIPLE-gcc >/dev/null || command -v $TRIPLE-gcc-posix >/dev/null || \
  echo "note: $TRIPLE-gcc not found; clang needs the MinGW-w64 sysroot (apt install mingw-w64)"
OUT="$ROOT/out/win"; mkdir -p "$OUT/obj" "$OUT/obj/srv" "$OUT/rt"
J="${J:-$(nproc 2>/dev/null || sysctl -n hw.ncpu)}"
OPT="${OPT:--O2}"

GEN="${GEN:-$ROOT/gen}"
if [ -n "${STUB:-}" ]; then
  GEN="$OUT/stubgen"; win/tools/stubgen.sh "$GEN" > /dev/null
  echo "STUB=1: a stand-in lift, the program will not run the game"
fi
[ -f "$GEN/all/fntab.c" ] || { echo "$GEN/all missing: run web/gen.sh <path to SPEED2.EXE> (or STUB=1 to check the build)"; exit 1; }
[ -f "$GEN/server/fntab.c" ] || { echo "$GEN/server missing: run web/gen.sh <path to SPEED2.EXE>"; exit 1; }

# x86-64-v2 (SSE4.2, POPCNT): every PC that runs Windows 11.  No FMA
# contraction: the x87 results the game expects are rounded per operation.
TARGET="--target=$TRIPLE"
CFLAGS="$TARGET $OPT -march=x86-64-v2 -ffp-contract=off -fno-delete-null-pointer-checks -DNFSU2_WIN=1 \
  -include runtime/win/win_plat.h -Iruntime/win/compat -Iruntime/web/compat -Iruntime -I$GEN/all -Wno-unknown-pragmas -Wno-ignored-attributes"
export CC CFLAGS OUT FULL GEN
HDRSUM="$( (cat runtime/cpu.h runtime/win/win_plat.h; echo "$CFLAGS") | sha256sum | cut -c1-16)"; export HDRSUM

# ---- lifted shards (content-hash incremental, as web/build.sh) ----
{ (cd "$GEN/all" && ls shard_*.c fntab.c impargs.c $( [ -f stubs.c ] && echo stubs.c ) | sed "s|^|all |")
  (cd "$GEN/server" && ls shard_*.c fntab.c impargs.c modinfo.c $( [ -f stubs.c ] && echo stubs.c ) 2>/dev/null | sed "s|^|server |"); } |
  xargs -P "$J" -n 2 sh -c '
  case "$1" in all) od="$OUT/obj" ;; *) od="$OUT/obj/srv" ;; esac
  src="$GEN/$1/$2"; obj="$od/$(basename "$2" .c).o"
  sum=$( (cat "$src" "$GEN/$1/decls.h"; echo "$HDRSUM") | sha256sum | cut -c1-40)
  if [ -z "$FULL" ] && [ -f "$obj" ] && [ "$(cat "$obj.sum" 2>/dev/null)" = "$sum" ]; then exit 0; fi
  o=$($CC $CFLAGS -I"$GEN/$1" -Wno-unused-label -Wno-parentheses-equality -c -o "$obj" "$src" 2>&1); r=$?
  [ -z "$o" ] || printf "%s\n" "$o" | head -5
  [ $r -eq 0 ] || { echo "FAILED: $src" >&2; exit $r; }
  echo "$sum" > "$obj.sum"; echo "  compiled $src"' sh

# ---- runtime ----
# web/build.sh's list: the shared runtime minus the Mac-only parts, and
# runtime/web/*.c minus the wasm-only ones (thread_exit: MinGW has a real
# pthread_exit; memcmp: wasm SIMD; memchk: a wasm debug build's; tz: local time is Windows' own, plat_win.c), plus
# runtime/win/*.c.
RT="mem dispatch genexec heap watchdog win32 win32_ext com debug res fileio overrides d3dx_hle vsemu oracle autotest lifecycle widescreen framerate dinput_pad dikeys keyboard dinput_kbd career intro drifttest roam roam_core roam_game mprace horn hornmenu optslider tonemenu powerunits radio drift carsfx racehorn racenet winsock online netrace module modheap enhance headlights language sms sponsors lobbyclick olfilter tunegraph menusize regional garage sndlead carshadow hiddenraces carspot movie photo"
WEB="$(cd runtime/web && ls *.c | sed 's/\.c$//' | grep -vxE 'thread_exit|memcmp|memchk|tz')"
WIN="$(cd runtime/win && ls *.c | sed 's/\.c$//')"
pids=""; fail="$OUT/rt/.failed"; rm -f "$fail"
cc1(){ o=$($CC "$@" 2>&1) || { printf "%s\n" "$o" | grep -E "error" | head -20; touch "$fail"; return 1; }
       [ -z "$o" ] || [ -z "${VERBOSE:-}" ] || printf "%s\n" "$o"; }
for f in $RT; do
  fx=""; case "$f" in roam_core|roam_game) fx="-ffp-contract=off" ;; esac
  cc1 $CFLAGS $fx -DNFS_ENHANCED=0 -fno-strict-aliasing -c -o "$OUT/rt/$f.o" "runtime/$f.c" & pids="$pids $!"
done
for f in $WEB; do
  fx=""; [ "$f" = main_web ] && fx="-Dmain=nfsu2_game_main"   # the host's main() starts it on a thread of its own
  cc1 $CFLAGS $fx -DNFS_ENHANCED=0 -fno-strict-aliasing -c -o "$OUT/rt/web_$f.o" "runtime/web/$f.c" & pids="$pids $!"
done
for f in $WIN; do
  cc1 $CFLAGS -fno-strict-aliasing -c -o "$OUT/rt/win_$f.o" "runtime/win/$f.c" & pids="$pids $!"
done
for p in $pids; do wait "$p" || true; done
[ ! -f "$fail" ] || { echo "runtime compile failed"; exit 1; }
rm -f "$OUT/libnfsu2rt.a"
llvm-ar rcs "$OUT/libnfsu2rt.a" "$OUT"/obj/*.o "$OUT"/obj/srv/*.o "$OUT"/rt/*.o 2>/dev/null || \
  $TRIPLE-ar rcs "$OUT/libnfsu2rt.a" "$OUT"/obj/*.o "$OUT"/obj/srv/*.o "$OUT"/rt/*.o
echo "runtime OK: out/win/libnfsu2rt.a"

# ---- host (Rust) + link ----
[ -n "${NOHOST:-}" ] && exit 0
command -v cargo >/dev/null || { echo "cargo not found: install Rust (rustup) and the x86_64-pc-windows-gnu target"; exit 1; }
# from win/host: its .cargo/config.toml names the linker (win/tools/cc-link.sh)
(cd win/host && NFSU2_RT_LIB="$OUT" cargo build --release --target x86_64-pc-windows-gnu)
cp win/host/target/x86_64-pc-windows-gnu/release/nfsu2.exe "$OUT/nfsu2.exe"
echo "link OK: out/win/nfsu2.exe ($(( $(wc -c < "$OUT/nfsu2.exe") / 1048576 )) MB)"
