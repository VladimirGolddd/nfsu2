#!/bin/bash
# A stand-in for gen/all (and gen/server) to check that the Windows build
# compiles and links without SPEED2.EXE: every sub_XXXXXXXX the runtime names,
# declared and defined as a function that reports and exits.  The program it
# makes cannot run the game; it is for CI and for working on win/ without the
# game.  Usage: win/tools/stubgen.sh <dir>  (makes <dir>/all and <dir>/server)
set -e
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; OUT="${1:?usage: stubgen.sh dir}"
mkdir -p "$OUT/all" "$OUT/server"
# the names the runtime writes out, and the _orig of every override (some are
# made by token pasting: d3dx_hle.c's DX_WRAP table; tools/genstamp.sh)
NAMES=$( { grep -rhoE '\bsub_[0-9a-f]{8}(_orig)?\b' "$ROOT/runtime"
           "$ROOT/tools/genstamp.sh" overrides | tr ',' '\n' | grep -E '^[0-9a-f]+$' | \
             while read -r a; do printf 'sub_%08x\nsub_%08x_orig\n' "0x$a" "0x$a"; done; } | sort -u)
{ echo '#ifndef DECLS_H'; echo '#define DECLS_H'; echo '#include "cpu.h"'
  for n in $NAMES; do echo "void $n(CPU *c);"; done; echo '#endif'; } > "$OUT/all/decls.h"
{ echo '#include "decls.h"'; echo '#include <stdio.h>'; echo '#include <stdlib.h>'
  echo 'static void stub(const char *n){ fprintf(stderr, "stub gen/all: %s called (this build has no lifted game code)\n", n); exit(70); }'
  for n in $NAMES; do echo "__attribute__((weak)) void $n(CPU *c){ (void)c; stub(\"$n\"); }"; done; } > "$OUT/all/shard_0000.c"
cat > "$OUT/all/fntab.c" <<'C'
#include "cpu.h"
typedef void(*guest_fn)(CPU*);
const uint32_t g_fn_count = 0;
const uint32_t g_fn_addr[1] = {0};
const guest_fn g_fn_ptr[1] = {0};
C
cat > "$OUT/all/impargs.c" <<'C'
#include <stdint.h>
typedef struct { uint32_t iat; const char*dll; const char*name; int nargs; } ImpEnt;
const uint32_t g_imp_count = 0;
const ImpEnt g_imp[1] = {{0}};
C
echo "stub" > "$OUT/all/web-lift.txt"
printf '#include "module.h"\nconst ModInfo g_srv_modinfo = { "server.dll", "", 0x00a00000u, 0, 0, 0, 0, 0, 0 };\n' > "$OUT/server/modinfo.c"
printf '#include "cpu.h"\ntypedef void(*guest_fn)(CPU*);\nconst uint32_t g_srv_fn_count = 0;\nconst uint32_t g_srv_fn_addr[1] = {0};\nconst guest_fn g_srv_fn_ptr[1] = {0};\n' > "$OUT/server/fntab.c"
printf '#include <stdint.h>\ntypedef struct { uint32_t iat; const char*dll; const char*name; int nargs; } ImpEnt;\nconst uint32_t g_srv_imp_count = 0;\nconst ImpEnt g_srv_imp[1] = {{0}};\n' > "$OUT/server/impargs.c"
printf '#ifndef DECLS_H\n#define DECLS_H\n#include "cpu.h"\n#endif\n' > "$OUT/server/decls.h"
echo "stub gen: $(echo "$NAMES" | wc -l) functions -> $OUT"
