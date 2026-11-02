# Need for Speed: Underground 2 for Windows 10/11 (native)

A Windows program, `nfsu2.exe`, that runs the recompiled game without a browser: the same lift as the browser
version (`web/gen.sh`), the same shared runtime and platform layer (`runtime/`, `runtime/web/`), compiled to x86-64
instead of WebAssembly, and a native host (`win/host/`, Rust) in the page's place.

> Status: **new, not yet played through on Windows.** Everything below builds, links and starts; the renderer's
> self test passes on Vulkan (Mesa's software Vulkan under Wine). The first runs of the real game on a Windows PC
> are still to come; see "Not yet" at the end.

## The original game

By default this is the PC game as it was, without the browser version's additions: no free roam with other
players, LAN or online, no horns, drift mode, photo mode or text messages of ours, the career garage's own 5 slots,
no Corsa or 106 for sale, none of the extra option rows (Horn Volume, Exposure and White Level, Power Units), the
race HUD's own 4:3 layout, only the player's headlights on the road, the boot movies as the game plays them, and
60 frames a second. What stays is what the port needs to run: the picture filling a wide screen, the sound's
timing, the language setting. `Vanilla = 0` in `nfsu2.ini` brings the additions back (those that need no network).

## What you need

- **The game:** your own PC copy, patched to v1.2 (North America), with the `SPEED2.EXE` the browser version needs
  (README.md, "What you need").
- **To play:** Windows 10 or 11, 64-bit, a GPU with Direct3D 12 or Vulkan.
- **To build:** Linux or WSL 2 (Ubuntu 24.04 tested) with Python 3 (the lifter, as for the browser), clang 17 or
  newer, lld, the MinGW-w64 cross toolchain and Rust with the Windows GNU target:

```sh
sudo apt install clang lld mingw-w64
rustup target add x86_64-pc-windows-gnu
```

## Build

```sh
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
web/gen.sh "/path/to/Need for Speed Underground 2/SPEED2.EXE"   # the game's code as C, in gen/ (as for the browser)
win/build.sh                                                    # out/win/nfsu2.exe
```

`STUB=1 win/build.sh` builds everything with a stand-in for the lifted code (`win/tools/stubgen.sh`), without the
game: for checking the toolchain. That program cannot run the game, but its renderer self test works (below).

The generated C and the program are made from the game, so they are never committed (`.gitignore`).

## Play

Copy `out/win/nfsu2.exe` anywhere (it needs no DLLs next to it) and start it. It finds the game folder:

1. from the command line: `nfsu2.exe "C:\Games\Need for Speed Underground 2"`,
2. from `nfsu2.ini` next to the program (`Data = C:\Games\...`),
3. from its own folder (put it next to `SPEED2.EXE`),
4. or it asks with a folder picker and writes the answer into `nfsu2.ini`.

Saves, the registry and the log (`nfsu2.log`) go to `%APPDATA%\NFSU2`; the game folder is only read.

Keys: as in the browser version (docs/web/PLAYING.md). **Alt+Enter** or **F11**: full screen. Controllers: the
first XInput controller (Xbox and compatible), with rumble.

### Settings

`nfsu2.ini` next to the program takes the runtime's settings as `NAME = value` lines, with or without the `NFSU2_`
prefix (the names the browser version's settings use, `web/src/app/settings.ts`), for example:

```ini
Data = C:\Games\Need for Speed Underground 2
Language = russian
FPS = 60
Fullscreen = 1
```

This host's own:

| Setting | Default | |
|---|---|---|
| `Vanilla` | 1 | 0: the browser version's additions (above) |
| `Width`, `Height` | 1280, 720 | the window's first size; the game follows the window |
| `Fullscreen` | 0 | 1: borderless full screen from the start |
| `VSync` | 1 | 0: present without waiting for the display (Mailbox or Immediate) |
| `GPU_Backend` | (wgpu's choice) | `dx12` or `vulkan` |
| `Volume` | 1 | 0..1 (or 0..100) |
| `Pad` | 1 | 0: no controller |
| `Quit_Grace` | 8 | seconds the game gets to save and close after the window's close button |
| `No_BC` | | set: DXT textures unpacked on the CPU (a GPU without BC compression) |

Environment variables of the same names win over the file.

### Renderer self test

`set NFSU2_WIN_SELFTEST=1` then `nfsu2.exe`: no game needed. It draws through the same path the game's Direct3D
calls take (a render target, a texture, a read back, then two seconds of frames) and writes
`[selftest] ... PASS` or `FAIL` into `nfsu2.log`; the exit code is 0 when it passed. For a new PC or driver.

## How it works

- `runtime/win/win_plat.h` is force-included in every runtime file. `NFSU2_WIN` selects the browser's code paths in
  the shared runtime (`#if defined(__wasm__) || defined(NFSU2_WIN)`); `__wasm__` alone is left where the code is
  about the WebAssembly machine itself (tail calls, the guest at address 0).
- Guest memory: as in the browser, the guest owns `[0, 0x28000000)`, here of a 4 GB reservation at `g_mem`
  (`runtime/mem.c`). The shared blocks the host reads (the renderer's record buffers, the virtual LAN's mailboxes)
  are allocated above that span, so their 32-bit addresses in the records are offsets from `g_mem`, as the page
  reads wasm addresses.
- The page's side of `runtime/web/host_abi.h` is `runtime/win/host_win.c` (`nfsu2_register`, the clock, the log);
  wasm's `memory.atomic.wait32` / `notify` are `WaitOnAddress` / `WakeByAddress*` (`runtime/win/plat_win.c`), which
  the host uses on the same words.
- `win/host/` (Rust): the window (winit); the renderer (`gpu.rs`, wgpu on Direct3D 12 or Vulkan), a port of
  `web/src/gpu/encoder.ts` and `index.ts` that runs the same WGSL (the runtime's translator, `wgsl.rs`); the display
  clock from `DwmFlush` (the page's requestAnimationFrame); sound (`audio.rs`, WASAPI through cpal, the AudioWorklet's
  ring reader); input (`input.rs`, the HostMailbox: keys by scan code, mouse, XInput).

## Not yet

- **Not played on Windows yet.** Expect bugs in the first runs; the log (`%APPDATA%\NFSU2\nfsu2.log`) says what
  happened, a crash included.
- **Movies:** the browser decodes the game's movies with its VideoDecoder; this host has no decoder, so the movies
  are skipped.
- **LAN, online and free-roam multiplayer:** they go through the page's network relay (`runtime/web/net_lan.c`,
  `roam_web.c`), which this host does not have yet: the network is off (`NFSU2_NET=off`).
- The page's own features: its menus and settings screens, cloud saves, the custom radio folder, touch controls,
  quick chat's overlay, frame dumps (autotest `dump`).
