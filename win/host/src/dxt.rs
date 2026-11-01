//! DXT1/3/5 (BC1/2/3) unpacked to BGRA8 on the CPU, as web/src/gpu/dxt.ts:
//! for a GPU without BC compression and for BC textures whose size is not a
//! multiple of 4 (wgpu wants whole blocks).  The D3D10 decoding rules.

pub const DXT1: u32 = 0x31545844;
pub const DXT5: u32 = 0x35545844;

fn c5(i: u32) -> u32 { (i << 3) | (i >> 2) }
fn c6(i: u32) -> u32 { (i << 2) | (i >> 4) }

/// One mip level of w x h texels (pitch = bytes per row of 4x4 blocks) as BGRA8, w * 4 bytes per row.
pub fn unpack(fmt: u32, src: &[u8], pitch: usize, w: usize, h: usize, out: &mut Vec<u8>) {
    out.clear();
    out.resize(w * h * 4, 0);
    let dxt1 = fmt == DXT1;
    let dxt5 = fmt == DXT5;
    let bsz = if dxt1 { 8 } else { 16 };
    let co = if dxt1 { 0 } else { 8 };
    let (bw, bh) = ((w + 3) >> 2, (h + 3) >> 2);
    let mut alpha = [255u8; 16];
    for by in 0..bh {
        for bx in 0..bw {
            let b = by * pitch + bx * bsz;
            if b + bsz > src.len() { return; }
            if dxt5 {
                let (a0, a1) = (src[b] as u32, src[b + 1] as u32);
                let mut at = [0u32; 8];
                at[0] = a0; at[1] = a1;
                if a0 > a1 { for i in 1..7 { at[i + 1] = ((7 - i as u32) * a0 + i as u32 * a1) / 7; } }
                else { for i in 1..5 { at[i + 1] = ((5 - i as u32) * a0 + i as u32 * a1) / 5; } at[6] = 0; at[7] = 255; }
                let lo = src[b + 2] as u32 | (src[b + 3] as u32) << 8 | (src[b + 4] as u32) << 16;
                let hi = src[b + 5] as u32 | (src[b + 6] as u32) << 8 | (src[b + 7] as u32) << 16;
                for i in 0..8 { alpha[i] = at[((lo >> (3 * i)) & 7) as usize] as u8; alpha[i + 8] = at[((hi >> (3 * i)) & 7) as usize] as u8; }
            } else if !dxt1 {
                for i in 0..8 { let v = src[b + i]; alpha[2 * i] = (v & 15) * 17; alpha[2 * i + 1] = (v >> 4) * 17; }
            }
            let c0 = src[b + co] as u32 | (src[b + co + 1] as u32) << 8;
            let c1 = src[b + co + 2] as u32 | (src[b + co + 3] as u32) << 8;
            let (r0, g0, b0) = (c5(c0 >> 11), c6((c0 >> 5) & 63), c5(c0 & 31));
            let (r1, g1, b1) = (c5(c1 >> 11), c6((c1 >> 5) & 63), c5(c1 & 31));
            let px = |b: u32, g: u32, r: u32| b | g << 8 | r << 16 | 0xFF00_0000;
            let mut pal = [px(b0, g0, r0), px(b1, g1, r1), 0, 0];
            if !dxt1 || c0 > c1 {
                pal[2] = px((2 * b0 + b1) / 3, (2 * g0 + g1) / 3, (2 * r0 + r1) / 3);
                pal[3] = px((b0 + 2 * b1) / 3, (g0 + 2 * g1) / 3, (r0 + 2 * r1) / 3);
            } else {
                pal[2] = px((b0 + b1) >> 1, (g0 + g1) >> 1, (r0 + r1) >> 1);
                pal[3] = 0;
            }
            let idx = u32::from_le_bytes([src[b + co + 4], src[b + co + 5], src[b + co + 6], src[b + co + 7]]);
            let ph = (h - by * 4).min(4);
            let pw = (w - bx * 4).min(4);
            for py in 0..ph {
                for pxi in 0..pw {
                    let i = py * 4 + pxi;
                    let c = pal[((idx >> (2 * i)) & 3) as usize];
                    let c = if dxt1 { c } else { (c & 0xFF_FFFF) | (alpha[i] as u32) << 24 };
                    let o = ((by * 4 + py) * w + bx * 4 + pxi) * 4;
                    out[o..o + 4].copy_from_slice(&c.to_le_bytes());
                }
            }
        }
    }
}
