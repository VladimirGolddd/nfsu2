//! Keyboard, mouse, the game pad and the window's life for the game, through
//! the HostMailbox in the runtime's memory (runtime/web/input_proto.h,
//! input_web.c), as the page's web/src/input does.  The runtime applies the
//! Windows delivery rules (WM_KEYDOWN / WM_CHAR / WM_SYSKEYDOWN, autorepeat,
//! releases) itself; this side only reports what happened.

use crate::ffi::{self, Shm};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use winit::keyboard::KeyCode as K;

const IM_MAGIC: usize = 0;
const IM_EV_W: usize = 4;
const IM_EV_R: usize = 5;
const IM_EV_DROP: usize = 6;
const IM_ATTACHED: usize = 7;
const IM_PAD_SEQ: usize = 11;
const IM_PAD_CONNECTED: usize = 12;
const IM_PAD_FAMILY: usize = 13;
const IM_PAD_BUTTONS: usize = 14;
const IM_PAD_GEN: usize = 15;
const IM_PAD_AXIS: usize = 16;
const IM_RUMBLE_SEQ: usize = 29;
const IM_RUMBLE_STRONG: usize = 30;
const IM_RUMBLE_WEAK: usize = 31;
const IM_PAD_RUMBLE: usize = 32;
const IM_PAD_WHEEL: usize = 37;
const OFF_PAD_NAME: usize = 256;
const OFF_EV: usize = 4416;
const EV_N: u32 = 1024;
const INPUT_MAGIC: u32 = 0x4E49_4E50;

pub const IEV_KEY_DOWN: u8 = 1;
pub const IEV_KEY_UP: u8 = 2;
pub const IEV_MOUSE_MOVE: u8 = 4;
pub const IEV_MOUSE_DOWN: u8 = 5;
pub const IEV_MOUSE_UP: u8 = 6;
pub const IEV_WHEEL: u8 = 7;
pub const IEV_FOCUS: u8 = 8;
pub const IEV_QUIT: u8 = 10;
pub const IKF_REPEAT: u8 = 1;
pub const IKF_ALT: u8 = 2;
pub const IKF_CTRL: u8 = 4;
pub const IKF_SHIFT: u8 = 8;
pub const IREASON_WINDOW: i32 = 1;
pub const IREASON_PAGE: i32 = 8;

fn log(s: &str) { crate::ffi::log(s); }
fn w(i: usize) -> usize { i * 4 }

/// The game's mailbox, once it registered and this host claimed its event ring.
pub struct Mailbox { m: Shm }

impl Mailbox {
    pub fn attach(ms: u32) -> Option<Arc<Mailbox>> {
        let (p, _) = ffi::lookup(ffi::KIND_INPUT, ms)?;
        let m = Shm(p);
        if m.u32(w(IM_MAGIC)) != INPUT_MAGIC { log("[input] the mailbox is not one this host knows: no input"); return None; }
        let owner = m.cas(w(IM_ATTACHED), 0, 1);
        if owner != 0 { log(&format!("[input] the event ring is already written by {}: no input", if owner == 2 { "the game (ns* input)" } else { "another host" })); return None; }
        log("[input] attached");
        Some(Arc::new(Mailbox { m }))
    }

    /// One event into the ring (the game's input thread wakes on IM_EV_W)
    pub fn push(&self, ty: u8, dik: u8, vk: u8, flags: u8, chars: &[u8], a: i32, b: i32) {
        let m = self.m;
        let wv = m.load(w(IM_EV_W)) as u32;
        let rv = m.load(w(IM_EV_R)) as u32;
        if wv.wrapping_sub(rv) >= EV_N { m.add(w(IM_EV_DROP), 1); return; }
        let o = OFF_EV + ((wv & (EV_N - 1)) as usize) * 16;
        m.set_u8(o, ty); m.set_u8(o + 1, dik); m.set_u8(o + 2, vk); m.set_u8(o + 3, flags);
        let n = chars.len().min(3);
        m.set_u8(o + 4, n as u8);
        for k in 0..3 { m.set_u8(o + 5 + k, *chars.get(k).unwrap_or(&0)); }
        m.set_u32(o + 8, a as u32); m.set_u32(o + 12, b as u32);
        m.store(w(IM_EV_W), wv.wrapping_add(1) as i32);
        ffi::notify(m.atomic(w(IM_EV_W)));
    }

    /// Polls the first XInput controller every 4 ms into the pad words (a seqlock), and plays the game's rumble.
    pub fn pad_thread(self: Arc<Self>, quit: Arc<crate::gpu::View>) {
        use windows_sys::Win32::UI::Input::XboxController::*;
        let m = self.m;
        let mut last: Option<(bool, u32, [f32; 6])> = None;
        let mut rumble_seq = m.load(w(IM_RUMBLE_SEQ));
        let mut slot: Option<u32> = None;
        let mut idle = 0u32;
        while !quit.quit.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(4));
            // a controller: the one in use, else look for one every ~0.5 s
            let mut st: XINPUT_STATE = unsafe { std::mem::zeroed() };
            let mut got = None;
            if let Some(s) = slot { if unsafe { XInputGetState(s, &mut st) } == 0 { got = Some(s); } }
            if got.is_none() {
                idle += 1;
                if slot.is_some() || idle % 128 == 0 {
                    for s in 0..4 { if unsafe { XInputGetState(s, &mut st) } == 0 { got = Some(s); break; } }
                }
            }
            let new_pad = got.is_some() && got != slot;
            if got != slot { log(&format!("[input] controller {}", match got { Some(s) => format!("{s} connected (XInput)"), None => "disconnected".into() })); }
            slot = got;
            let snap = if got.is_some() {
                let g = st.Gamepad;
                let b = g.wButtons as u32;
                let map = [(XINPUT_GAMEPAD_A, 0), (XINPUT_GAMEPAD_B, 1), (XINPUT_GAMEPAD_X, 2), (XINPUT_GAMEPAD_Y, 3),
                    (XINPUT_GAMEPAD_LEFT_SHOULDER, 4), (XINPUT_GAMEPAD_RIGHT_SHOULDER, 5), (XINPUT_GAMEPAD_BACK, 6), (XINPUT_GAMEPAD_START, 7),
                    (XINPUT_GAMEPAD_LEFT_THUMB, 8), (XINPUT_GAMEPAD_RIGHT_THUMB, 9), (XINPUT_GAMEPAD_DPAD_UP, 11), (XINPUT_GAMEPAD_DPAD_DOWN, 12),
                    (XINPUT_GAMEPAD_DPAD_LEFT, 13), (XINPUT_GAMEPAD_DPAD_RIGHT, 14)];
                let mut bits = 0u32;
                for (mask, bit) in map { if b & mask as u32 != 0 { bits |= 1 << bit; } }
                let ax = |v: i16| (v as f32 / if v < 0 { 32768.0 } else { 32767.0 }).clamp(-1.0, 1.0);
                (true, bits, [ax(g.sThumbLX), -ax(g.sThumbLY), ax(g.sThumbRX), -ax(g.sThumbRY), g.bLeftTrigger as f32 / 255.0, g.bRightTrigger as f32 / 255.0])
            } else { (false, 0, [0.0; 6]) };
            if last != Some(snap) || new_pad {
                let seq = m.load(w(IM_PAD_SEQ));
                m.store(w(IM_PAD_SEQ), seq.wrapping_add(1));
                m.store(w(IM_PAD_CONNECTED), snap.0 as i32);
                m.store(w(IM_PAD_FAMILY), 1);   // PADF_XBOX
                m.store(w(IM_PAD_BUTTONS), snap.1 as i32);
                m.store(w(IM_PAD_WHEEL), 0);
                for i in 0..6 { m.store(w(IM_PAD_AXIS + i), snap.2[i].to_bits() as i32); }
                if new_pad {
                    let name = b"Xbox controller (XInput)";
                    for i in 0..64 { m.set_u8(OFF_PAD_NAME + i, *name.get(i).unwrap_or(&0)); }
                    m.store(w(IM_PAD_RUMBLE), 1);
                }
                m.store(w(IM_PAD_SEQ), seq.wrapping_add(2));
                if new_pad { m.add(w(IM_PAD_GEN), 1); }
                last = Some(snap);
            }
            let rs = m.load(w(IM_RUMBLE_SEQ));
            if rs != rumble_seq {
                rumble_seq = rs;
                if let Some(s) = slot {
                    let mut v = XINPUT_VIBRATION { wLeftMotorSpeed: m.u32(w(IM_RUMBLE_STRONG)).min(65535) as u16,
                                                   wRightMotorSpeed: m.u32(w(IM_RUMBLE_WEAK)).min(65535) as u16 };
                    unsafe { XInputSetState(s, &mut v) };
                }
            }
        }
        if let Some(s) = slot { let mut v = XINPUT_VIBRATION { wLeftMotorSpeed: 0, wRightMotorSpeed: 0 }; unsafe { XInputSetState(s, &mut v) }; }
    }
}

/// The key's DirectInput scan code (its position; bit 7 = extended) and Windows virtual-key code, as web/src/input/keymap.ts.
pub fn key_def(k: K) -> Option<(u8, u8)> {
    Some(match k {
        K::Escape => (0x01, 0x1B),
        K::Digit1 => (0x02, 0x31), K::Digit2 => (0x03, 0x32), K::Digit3 => (0x04, 0x33), K::Digit4 => (0x05, 0x34), K::Digit5 => (0x06, 0x35),
        K::Digit6 => (0x07, 0x36), K::Digit7 => (0x08, 0x37), K::Digit8 => (0x09, 0x38), K::Digit9 => (0x0A, 0x39), K::Digit0 => (0x0B, 0x30),
        K::Minus => (0x0C, 0xBD), K::Equal => (0x0D, 0xBB), K::Backspace => (0x0E, 0x08), K::Tab => (0x0F, 0x09),
        K::KeyQ => (0x10, 0x51), K::KeyW => (0x11, 0x57), K::KeyE => (0x12, 0x45), K::KeyR => (0x13, 0x52), K::KeyT => (0x14, 0x54),
        K::KeyY => (0x15, 0x59), K::KeyU => (0x16, 0x55), K::KeyI => (0x17, 0x49), K::KeyO => (0x18, 0x4F), K::KeyP => (0x19, 0x50),
        K::BracketLeft => (0x1A, 0xDB), K::BracketRight => (0x1B, 0xDD), K::Enter => (0x1C, 0x0D), K::ControlLeft => (0x1D, 0x11),
        K::KeyA => (0x1E, 0x41), K::KeyS => (0x1F, 0x53), K::KeyD => (0x20, 0x44), K::KeyF => (0x21, 0x46), K::KeyG => (0x22, 0x47),
        K::KeyH => (0x23, 0x48), K::KeyJ => (0x24, 0x4A), K::KeyK => (0x25, 0x4B), K::KeyL => (0x26, 0x4C),
        K::Semicolon => (0x27, 0xBA), K::Quote => (0x28, 0xDE), K::Backquote => (0x29, 0xC0), K::ShiftLeft => (0x2A, 0x10), K::Backslash => (0x2B, 0xDC),
        K::KeyZ => (0x2C, 0x5A), K::KeyX => (0x2D, 0x58), K::KeyC => (0x2E, 0x43), K::KeyV => (0x2F, 0x56), K::KeyB => (0x30, 0x42),
        K::KeyN => (0x31, 0x4E), K::KeyM => (0x32, 0x4D), K::Comma => (0x33, 0xBC), K::Period => (0x34, 0xBE), K::Slash => (0x35, 0xBF),
        K::ShiftRight => (0x36, 0x10), K::NumpadMultiply => (0x37, 0x6A), K::AltLeft => (0x38, 0x12), K::Space => (0x39, 0x20), K::CapsLock => (0x3A, 0x14),
        K::F1 => (0x3B, 0x70), K::F2 => (0x3C, 0x71), K::F3 => (0x3D, 0x72), K::F4 => (0x3E, 0x73), K::F5 => (0x3F, 0x74),
        K::F6 => (0x40, 0x75), K::F7 => (0x41, 0x76), K::F8 => (0x42, 0x77), K::F9 => (0x43, 0x78), K::F10 => (0x44, 0x79),
        K::NumLock => (0x45, 0x90), K::ScrollLock => (0x46, 0x91),
        K::Numpad7 => (0x47, 0x67), K::Numpad8 => (0x48, 0x68), K::Numpad9 => (0x49, 0x69), K::NumpadSubtract => (0x4A, 0x6D),
        K::Numpad4 => (0x4B, 0x64), K::Numpad5 => (0x4C, 0x65), K::Numpad6 => (0x4D, 0x66), K::NumpadAdd => (0x4E, 0x6B),
        K::Numpad1 => (0x4F, 0x61), K::Numpad2 => (0x50, 0x62), K::Numpad3 => (0x51, 0x63), K::Numpad0 => (0x52, 0x60), K::NumpadDecimal => (0x53, 0x6E),
        K::IntlBackslash => (0x56, 0xE2), K::F11 => (0x57, 0x7A), K::F12 => (0x58, 0x7B),
        K::F13 => (0x64, 0x7C), K::F14 => (0x65, 0x7D), K::F15 => (0x66, 0x7E),
        K::KanaMode | K::Lang1 => (0x70, 0x15), K::IntlRo => (0x73, 0xE2), K::Lang2 => (0x7B, 0x1D), K::IntlYen => (0x7D, 0xDC),
        K::NumpadEqual => (0x8D, 0x92), K::NumpadEnter => (0x9C, 0x0D), K::ControlRight => (0x9D, 0x11), K::NumpadComma => (0xB3, 0x6C),
        K::NumpadDivide => (0xB5, 0x6F), K::PrintScreen => (0xB7, 0x2C), K::AltRight => (0xB8, 0x12), K::Pause => (0xC5, 0x13),
        K::Home => (0xC7, 0x24), K::ArrowUp => (0xC8, 0x26), K::PageUp => (0xC9, 0x21), K::ArrowLeft => (0xCB, 0x25), K::ArrowRight => (0xCD, 0x27),
        K::End => (0xCF, 0x23), K::ArrowDown => (0xD0, 0x28), K::PageDown => (0xD1, 0x22), K::Insert => (0xD2, 0x2D), K::Delete => (0xD3, 0x2E),
        K::SuperLeft => (0xDB, 0x5B), K::SuperRight => (0xDC, 0x5C), K::ContextMenu => (0xDD, 0x5D),
        _ => return None,
    })
}

/// Unicode -> Windows code page 1252 (the game's WM_CHAR), None where it has no place there
pub fn cp1252(u: char) -> Option<u8> {
    let u = u as u32;
    if u < 0x80 || (0xA0..=0xFF).contains(&u) { return Some(u as u8); }
    const HI: [u32; 32] = [0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
        0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
        0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178];
    HI.iter().position(|&c| c == u && u != 0).map(|i| 0x80 + i as u8)
}
