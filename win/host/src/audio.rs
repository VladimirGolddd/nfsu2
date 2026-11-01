//! Sound: the page's AudioWorklet (web/src/audio/worklet.ts) on the default
//! output device through WASAPI (cpal).  It plays the mixer's float32 stereo
//! frames from the ring in the runtime's memory (runtime/web/audio_proto.h),
//! resampling linearly when the device does not run at 44.1 kHz, and keeps the
//! ring's counters the way the worklet does so the pump keeps its fill.

use crate::ffi::{self, Shm};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const AR_FRAMES: usize = 2;
const AR_RATE: usize = 3;
const AR_W: usize = 4;
const AR_R: usize = 5;
const AR_UNDERRUNS: usize = 6;
const AR_CTX: usize = 7;
const AR_QUANTA: usize = 8;
const AR_LOUD: usize = 15;
const AR_PAGE_RATE: usize = 19;
const AR_CTX_RUNNING: i32 = 1;
const AR_CTX_CLOSED: i32 = 3;
const OFF_PCM: usize = 128;

fn log(s: &str) { crate::ffi::log(s); }

/// Plays the game's sound until the program ends (the stream lives as long as the returned value).
pub fn start(volume: f32) -> Option<cpal::Stream> {
    let (p, _) = ffi::lookup(ffi::KIND_AUDIO, 120_000)?;
    let ring = Shm(p);
    let hw = |w: usize| w * 4;
    let frames = ring.u32(hw(AR_FRAMES)) as usize;
    let rate = ring.u32(hw(AR_RATE)).max(8000);
    if frames == 0 || frames & (frames - 1) != 0 { log("[audio] the ring is not one this host knows: no sound"); return None; }
    let host = cpal::default_host();
    let Some(dev) = host.default_output_device() else { log("[audio] no output device: no sound"); return None };
    let cfg = match dev.default_output_config() { Ok(c) => c, Err(e) => { log(&format!("[audio] no output configuration: {e}")); return None } };
    let out_rate = cfg.sample_rate();
    let chans = cfg.channels() as usize;
    let sc = cpal::StreamConfig { channels: cfg.channels(), sample_rate: out_rate, buffer_size: cpal::BufferSize::Default };
    let step = rate as f64 / out_rate as f64;
    let mask = frames - 1;
    let pcm = ring.at(OFF_PCM);
    let mut frac = 0.0f64;
    let vol = volume.clamp(0.0, 1.0);
    let data = move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let n = out.len() / chans.max(1);
        ring.add(hw(AR_QUANTA), 1);
        let w = ring.load(hw(AR_W)) as u32;
        let mut r = ring.load(hw(AR_R)) as u32;
        let mut avail = w.wrapping_sub(r) as usize;
        if avail > frames { avail = 0; }
        let mut k = 0;
        let mut peak = 0.0f32;
        let mut put = |out: &mut [f32], k: usize, a: f32, b: f32| {
            let o = &mut out[k * chans..(k + 1) * chans];
            o[0] = a * vol;
            if chans > 1 { o[1] = b * vol; for x in &mut o[2..] { *x = 0.0; } }
            peak = peak.max(a.abs()).max(b.abs());
        };
        if step == 1.0 {
            let m = n.min(avail);
            while k < m {
                let j = ((r as usize + k) & mask) * 8;
                put(out, k, pcm.f32(j), pcm.f32(j + 4));
                k += 1;
            }
            r = r.wrapping_add(k as u32);
        } else {
            let mut fpos = frac;
            while k < n {
                let i0 = fpos.floor() as usize;
                if i0 + 1 >= avail { break; }
                let t = (fpos - i0 as f64) as f32;
                let (j0, j1) = (((r as usize + i0) & mask) * 8, ((r as usize + i0 + 1) & mask) * 8);
                let (a0, b0, a1, b1) = (pcm.f32(j0), pcm.f32(j0 + 4), pcm.f32(j1), pcm.f32(j1 + 4));
                put(out, k, a0 + (a1 - a0) * t, b0 + (b1 - b0) * t);
                fpos += step;
                k += 1;
            }
            let used = fpos.floor();
            frac = fpos - used;
            r = r.wrapping_add(used as u32);
        }
        if k < n {
            for x in &mut out[k * chans..] { *x = 0.0; }
            ring.add(hw(AR_UNDERRUNS), 1);
        }
        if peak > 1.0 / 32768.0 { ring.add(hw(AR_LOUD), 1); }
        ring.store(hw(AR_R), r as i32);
        ffi::notify(ring.atomic(hw(AR_R)));
    };
    let stream = match dev.build_output_stream::<f32, _, _>(sc, data, |e| log(&format!("[audio] stream error: {e}")), None) {
        Ok(s) => s,
        Err(e) => { log(&format!("[audio] could not open the output: {e}: no sound")); ring.store(hw(AR_CTX), AR_CTX_CLOSED); return None }
    };
    if let Err(e) = stream.play() { log(&format!("[audio] could not start the output: {e}")); }
    ring.store(hw(AR_PAGE_RATE), out_rate as i32);
    ring.store(hw(AR_CTX), AR_CTX_RUNNING);
    log(&format!("[audio] WASAPI output: {out_rate} Hz, {chans} channels (the game mixes at {rate} Hz)"));
    Some(stream)
}
