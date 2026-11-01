/* NFSU2_WIN_SELFTEST=1 (win/host): the renderer's path without the game.
 * Draws through the same metal_* calls com.c makes (runtime/web/gpu_web.c
 * records them, win/host's renderer draws them): a render target cleared and
 * drawn with a textured quad, read back as the game reads a cover picture
 * (metal_read_texture), its pixels checked; then frames on the screen until
 * NFSU2_WIN_SELFTEST_FRAMES (default 120).  Exit code 0 when the pixels are
 * right.  Needs no game files: for checking a build on a new PC or driver. */
#include "cpu.h"
#include "gfx.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

unsigned metal_create_texture_ex(unsigned w, unsigned h, unsigned levels, unsigned f, unsigned is_rt);
void metal_upload_texture(unsigned id, unsigned level, const void *data, unsigned pitch, unsigned w, unsigned h, unsigned f);
void metal_set_blend(unsigned key, float aref); void metal_set_cull(unsigned m); void metal_set_depth(unsigned mode);
void metal_set_zsurf(unsigned zid); void metal_set_render_target(unsigned t); void metal_set_samplers(const unsigned k[4]);
void metal_set_clear_color(unsigned argb); void metal_clear_target_ex(unsigned argb, unsigned flags, float z);
void metal_present(void); void metal_resize(unsigned w, unsigned h); void metal_set_ps(int id, const float *c32);
int metal_read_texture(unsigned id, void *dst, unsigned pitch, unsigned rows);
void mem_init(void);

static void quad(TVert *v, float x0, float y0, float x1, float y1, float r, float g, float b){
    float p[6][2] = { {x0,y0},{x1,y0},{x0,y1}, {x1,y0},{x1,y1},{x0,y1} };
    float uv[6][2] = { {0,0},{1,0},{0,1}, {1,0},{1,1},{0,1} };
    memset(v, 0, 6 * sizeof *v);
    for (int i = 0; i < 6; i++){
        v[i].x = p[i][0]; v[i].y = p[i][1]; v[i].z = 0.5f; v[i].w = 1.f;
        v[i].r = r; v[i].g = g; v[i].b = b; v[i].a = 1.f;
        v[i].t[0][0] = uv[i][0]; v[i].t[0][1] = uv[i][1]; v[i].t[0][3] = 1.f;
    }
}

int nfsu2_win_selftest(void){
    mem_init();
    const char *fe = getenv("NFSU2_WIN_SELFTEST_FRAMES");
    int frames = fe ? atoi(fe) : 120;
    metal_resize(640, 480);
    /* a 2x2 texture: white, black / black, white (BGRA) */
    static const uint32_t px[4] = { 0xFFFFFFFFu, 0xFF000000u, 0xFF000000u, 0xFFFFFFFFu };
    unsigned tex = metal_create_texture_ex(2, 2, 1, 21, 0);
    metal_upload_texture(tex, 0, px, 8, 2, 2, 21);
    unsigned rt = metal_create_texture_ex(64, 64, 1, 21, 1);
    unsigned smp[4] = { 1u | 1u << 2 | 3u << 6 | 3u << 9, 0, 0, 0 };   /* point, clamp */
    metal_set_samplers(smp);
    metal_set_cull(1); metal_set_depth(0); metal_set_zsurf(0); metal_set_ps(0, 0);
    metal_set_blend(0xFu << 14, 0.f);
    TVert v[6];
    /* the target: cleared blue, its left half drawn red x the texture */
    metal_set_render_target(rt);
    metal_set_clear_color(0xFF0000FFu);
    metal_clear_target_ex(0xFF0000FFu, 1u, 1.f);
    quad(v, -1.f, 1.f, 0.f, -1.f, 1.f, 0.f, 0.f);
    metal_draw_tv(v, 6, 4, tex);
    /* read it back as the game reads its cover pictures (into guest memory) */
    uint32_t dst = 0x01000000u;
    memset(g_mem + dst, 0x55, 64 * 64 * 4);
    int got = metal_read_texture(rt, g_mem + dst, 64 * 4, 64);
    const uint32_t *o = (const uint32_t *)(g_mem + dst);
    uint32_t tl = o[2 * 64 + 2], tr2 = o[2 * 64 + 20], rr = o[32 * 64 + 50];
    int ok = got == 1 && (tl & 0xFFFFFF) == 0xFF0000u && (tr2 & 0xFFFFFF) == 0x000000u && (rr & 0xFFFFFF) == 0x0000FFu;
    fprintf(stderr, "[selftest] read back %s: (2,2) %08x (want red), (20,2) %08x (want black), (50,32) %08x (want blue): %s\n",
            got == 1 ? "done" : "FAILED", tl, tr2, rr, ok ? "PASS" : "FAIL");
    metal_set_render_target(0);
    metal_present();
    for (int f = 0; f < frames; f++){
        float t = (float)(f % 120) / 120.f;
        metal_set_render_target(0);
        metal_set_clear_color(0xFF202020u);
        metal_clear_target_ex(0xFF202020u, 1u, 1.f);
        quad(v, -0.8f + t, 0.8f, -0.2f + t, 0.2f, 1.f, 0.6f, 0.1f);
        metal_draw_tv(v, 6, 4, tex);
        quad(v, -0.8f, -0.1f, 0.6f, -0.8f, 1.f, 1.f, 1.f);
        metal_draw_tv(v, 6, 4, rt);
        metal_present();
    }
    fprintf(stderr, "[selftest] %d frames presented: %s\n", frames, ok ? "PASS" : "FAIL");
    return ok ? 0 : 1;
}
