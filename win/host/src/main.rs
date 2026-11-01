//! Need for Speed: Underground 2 for Windows 10/11: the native host.
//!
//! The game's code is SPEED2.EXE translated to C (tools/, web/gen.sh) and
//! compiled for x86-64 with the shared runtime and its browser platform layer
//! (runtime/web, runtime/win; win/build.sh).  In the browser a page hosts that
//! runtime: it gives it a canvas, sound, input and a clock through shared
//! memory.  This program is that host as a Windows program: a window, the
//! renderer on Direct3D 12 or Vulkan (wgpu, gpu.rs), WASAPI sound (audio.rs),
//! keyboard, mouse and XInput controllers (input.rs).
//!
//! Usage:  nfsu2.exe ["C:\Games\Need for Speed Underground 2"]
//! The game folder (with SPEED2.EXE v1.2) comes from the command line, from
//! nfsu2.ini next to this program (Data = ...), from this program's own folder,
//! or else from a folder picker; the choice is kept in nfsu2.ini.
//! nfsu2.ini also takes the runtime's settings (docs/web/PLAYING.md lists
//! them), as NAME = value lines: "FPS = 60" or "NFSU2_FPS = 60".
//! Saves and the log: %APPDATA%\NFSU2.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod dxt;
mod ffi;
mod gpu;
mod input;
mod wgsl;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

const TITLE: &str = "Need for Speed: Underground 2";

fn message(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
    let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), w(text).as_ptr(), w(TITLE).as_ptr(), MB_OK | MB_ICONWARNING) };
}

/// nfsu2.ini: NAME = value lines (';' or '#' starts a comment line)
fn read_ini(p: &Path) -> Vec<(String, String)> {
    let Ok(t) = std::fs::read_to_string(p) else { return vec![] };
    t.lines().filter_map(|l| {
        let l = l.trim();
        if l.is_empty() || l.starts_with(';') || l.starts_with('#') || l.starts_with('[') { return None; }
        let (k, v) = l.split_once('=')?;
        Some((k.trim().to_string(), v.trim().trim_matches('"').to_string()))
    }).collect()
}
fn write_data_dir(ini: &Path, dir: &Path) {
    let mut t = std::fs::read_to_string(ini).unwrap_or_default();
    let line = format!("Data = {}", dir.display());
    let mut found = false;
    let mut out: Vec<String> = t.lines().map(|l| {
        if l.trim_start().to_ascii_lowercase().starts_with("data") && l.contains('=') && !found { found = true; line.clone() } else { l.to_string() }
    }).collect();
    if !found {
        if out.is_empty() { out.push("; Need for Speed: Underground 2 for Windows: settings (NAME = value, see win/README.md)".into()); }
        out.push(line);
    }
    t = out.join("\r\n") + "\r\n";
    let _ = std::fs::write(ini, t);
}

fn has_exe(d: &Path) -> bool { d.join("SPEED2.EXE").is_file() || d.join("speed2.exe").is_file() }

/// The folder picker (IFileOpenDialog, folders only)
fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Where is Need for Speed Underground 2 installed? (the folder with SPEED2.EXE)").pick_folder()
}

/// forward slashes: the runtime's file layer joins paths with '/', which Windows takes as well
fn slashes(p: &Path) -> String { p.display().to_string().replace('\\', "/") }

struct App {
    window: Option<Arc<Window>>,
    view: Arc<gpu::View>,
    mods: ModifiersState,
    mb: Arc<OnceLock<Arc<input::Mailbox>>>,
    started: bool,
}

impl App {
    fn mb(&self) -> Option<&Arc<input::Mailbox>> { self.mb.get() }
    fn flags(&self) -> u8 {
        (if self.mods.alt_key() { input::IKF_ALT } else { 0 }) | (if self.mods.control_key() { input::IKF_CTRL } else { 0 })
            | (if self.mods.shift_key() { input::IKF_SHIFT } else { 0 })
    }
    fn toggle_fullscreen(&self) {
        if let Some(w) = &self.window {
            w.set_fullscreen(if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) });
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.started { return; }
        self.started = true;
        let (w, h) = (env_u("NFSU2_WIDTH", 1280), env_u("NFSU2_HEIGHT", 720));
        let attrs = Window::default_attributes().with_title(TITLE)
            .with_inner_size(winit::dpi::PhysicalSize::new(w, h));
        let win = match el.create_window(attrs) { Ok(w) => Arc::new(w), Err(e) => { message(&format!("Could not open a window: {e}")); std::process::exit(1) } };
        if std::env::var("NFSU2_FULLSCREEN").map(|v| v != "0").unwrap_or(false) { win.set_fullscreen(Some(Fullscreen::Borderless(None))); }
        win.set_cursor_visible(false);
        let sz = win.inner_size();
        self.view.w.store(sz.width, Ordering::Relaxed);
        self.view.h.store(sz.height, Ordering::Relaxed);
        let gpu = gpu::Renderer::surface(win.clone());
        { let view = self.view.clone();
          let size = (sz.width, sz.height);
          std::thread::Builder::new().name("renderer".into()).spawn(move || match gpu.and_then(|(i, s)| gpu::Renderer::new(i, s, size, view.clone())) {
              Ok(r) => r.run(),
              Err(e) => { ffi::log(&format!("[gpu] no renderer: {e}")); if !view.quit.load(Ordering::Relaxed) {
                  message(&format!("The graphics could not start:\n{e}\n\nThe game goes on without a picture; see nfsu2.log.")); } }
          }).unwrap(); }
        self.window = Some(win);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => {
                match self.mb() {
                    Some(mb) => mb.push(input::IEV_QUIT, 0, 0, 0, &[], 0, input::IREASON_PAGE),
                    None => std::process::exit(0),
                }
                // the game saves and exits by itself; if it does not, the program ends anyway
                let grace = env_u("NFSU2_QUIT_GRACE", 8) as u64;
                std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_secs(grace)); std::process::exit(0) });
            }
            WindowEvent::Resized(s) => {
                self.view.w.store(s.width, Ordering::Relaxed);
                self.view.h.store(s.height, Ordering::Relaxed);
                self.view.resized.store(true, Ordering::Relaxed);
            }
            WindowEvent::Focused(on) => {
                if let Some(mb) = self.mb() { mb.push(input::IEV_FOCUS, 0, 0, 0, &[], on as i32, input::IREASON_WINDOW); }
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                if is_synthetic { return; }
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let down = event.state == ElementState::Pressed;
                // Alt+Enter and F11: full screen (the browser's and Windows games' own keys)
                if down && !event.repeat && ((code == winit::keyboard::KeyCode::Enter && self.mods.alt_key()) || code == winit::keyboard::KeyCode::F11) {
                    self.toggle_fullscreen(); return;
                }
                let Some((dik, vk)) = input::key_def(code) else { return };
                let Some(mb) = self.mb() else { return };
                if down {
                    let mut chars: Vec<u8> = event.text.as_ref().map(|t| t.chars().filter_map(input::cp1252).take(3).collect()).unwrap_or_default();
                    // Alt: no WM_CHAR, the key's plain character for WM_SYSCHAR (b)
                    let mut sys = 0;
                    if self.mods.alt_key() && !self.mods.control_key() {
                        chars.clear();
                        if let Key::Character(c) = &event.logical_key { sys = c.chars().next().map(|c| c.to_ascii_lowercase()).and_then(input::cp1252).unwrap_or(0) as i32; }
                    }
                    // letters follow the layout (AZERTY's A key gives VK_A), as on Windows
                    let mut vk = vk;
                    if let Key::Character(c) = &event.logical_key {
                        if let Some(ch) = c.chars().next() { let u = ch.to_ascii_uppercase(); if u.is_ascii_uppercase() && (0x41..=0x5A).contains(&vk) { vk = u as u8; } }
                    }
                    mb.push(input::IEV_KEY_DOWN, dik, vk, self.flags() | if event.repeat { input::IKF_REPEAT } else { 0 }, &chars, 0, sys);
                } else {
                    mb.push(input::IEV_KEY_UP, dik, vk, self.flags(), &[], 0, 0);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(mb) = self.mb() else { return };
                let r = *self.view.img.lock().unwrap();
                let (x, y) = if r[2] > 0.0 && r[3] > 0.0 { ((position.x as f32 - r[0]) / r[2], (position.y as f32 - r[1]) / r[3]) }
                    else { let (w, h) = (self.view.w.load(Ordering::Relaxed).max(1) as f32, self.view.h.load(Ordering::Relaxed).max(1) as f32);
                           (position.x as f32 / w, position.y as f32 / h) };
                mb.push(input::IEV_MOUSE_MOVE, 0, 0, self.flags(), &[], (x * 65536.0).round() as i32, (y * 65536.0).round() as i32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(mb) = self.mb() else { return };
                let b = match button { MouseButton::Left => 0, MouseButton::Right => 1, MouseButton::Middle => 2, MouseButton::Back => 3, MouseButton::Forward => 4, _ => return };
                mb.push(if state == ElementState::Pressed { input::IEV_MOUSE_DOWN } else { input::IEV_MOUSE_UP }, 0, 0, self.flags(), &[], b, 0);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some(mb) = self.mb() else { return };
                let d = match delta { MouseScrollDelta::LineDelta(_, y) => (y * 120.0) as i32, MouseScrollDelta::PixelDelta(p) => (p.y * 1.2) as i32 };
                if d != 0 { mb.push(input::IEV_WHEEL, 0, 0, self.flags(), &[], d, 0); }
            }
            _ => {}
        }
        if self.view.quit.load(Ordering::Relaxed) { el.exit(); }
    }
}

fn env_u(n: &str, d: u32) -> u32 { std::env::var(n).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }

#[link(name = "winmm")]
extern "system" { fn timeBeginPeriod(ms: u32) -> u32; }

fn main() {
    // 1 ms timer: the game's frame pacer sleeps between frames (the runtime spins the rest, win32_ext.c)
    unsafe { timeBeginPeriod(1); }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
    let ini = exe_dir.join("nfsu2.ini");
    let settings = read_ini(&ini);

    // the save folder and the log first: everything after this is logged there
    let save = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| exe_dir.clone()).join("NFSU2");
    let _ = std::fs::create_dir_all(&save);
    let log_path = save.join("nfsu2.log");
    if !cfg!(debug_assertions) || std::env::var("NFSU2_LOGFILE").is_ok() { ffi::log_to(&log_path.to_string_lossy()); }
    ffi::log(&format!("{TITLE} for Windows ({}), log {}", env!("CARGO_PKG_VERSION"), log_path.display()));

    // settings: nfsu2.ini, then the environment wins
    let mut data: Option<PathBuf> = None;
    for (k, v) in &settings {
        let ku = k.to_ascii_uppercase();
        if ku == "DATA" || ku == "NFSU2_DATA" { data = Some(PathBuf::from(v)); continue; }
        let name = if ku.starts_with("NFSU2_") || ku.starts_with("WGPU_") { ku } else { format!("NFSU2_{ku}") };
        if std::env::var_os(&name).is_none() { ffi::set_env(&name, v); }
    }
    let arg = std::env::args().nth(1).map(PathBuf::from);
    let mut dir = arg.or_else(|| std::env::var_os("NFSU2_DATA").map(PathBuf::from)).or(data)
        .map(|p| if p.is_file() { p.parent().map(Path::to_path_buf).unwrap_or(p) } else { p });
    if dir.as_ref().map(|d| !has_exe(d)).unwrap_or(true) && has_exe(&exe_dir) { dir = Some(exe_dir.clone()); }
    let selftest = std::env::var("NFSU2_WIN_SELFTEST").map(|v| v == "1").unwrap_or(false);
    if !selftest && dir.as_ref().map(|d| !has_exe(d)).unwrap_or(true) {
        loop {
            match pick_folder() {
                None => return,
                Some(d) if has_exe(&d) => { write_data_dir(&ini, &d); dir = Some(d); break; }
                Some(d) => message(&format!("There is no SPEED2.EXE in\n{}\n\nChoose the folder Need for Speed Underground 2 is installed in.", d.display())),
            }
        }
    }
    let dir = dir.unwrap_or_default();
    let exe = if dir.join("SPEED2.EXE").is_file() { dir.join("SPEED2.EXE") } else { dir.join("speed2.exe") };
    ffi::log(&format!("game folder: {}", dir.display()));

    // what the page sets for the runtime (web/src/app/settings.ts), with this host's choices
    let def = |k: &str, v: &str| if std::env::var_os(k).is_none() { ffi::set_env(k, v) };
    def("NFSU2_RENDERER", "webgpu");
    def("HOME", &slashes(&save));
    def("NFSU2_SAVEDIR", &slashes(&save));
    def("NFSU2_REGFILE", &format!("{}/registry.txt", slashes(&save)));
    def("NFSU2_WIDTH", "1280");
    def("NFSU2_HEIGHT", "720");
    def("NFSU2_NET", "off");        // LAN and online play need the page's network relay: not in this host yet
    def("NFSU2_ONLINE", "0");
    def("NFSU2_ROAM", "0");

    let view = Arc::new(gpu::View { w: AtomicU32::new(1280), h: AtomicU32::new(720), resized: AtomicBool::new(false),
        img: Mutex::new([0.0; 4]), quit: AtomicBool::new(false) });

    ffi::fault_handler();
    // the game on a thread of its own; its end ends the program
    { let args = vec!["nfsu2".to_string(), slashes(&exe)];
      let view = view.clone();
      std::thread::Builder::new().name("game".into()).stack_size(16 << 20).spawn(move || {
          let code = if selftest { ffi::selftest() } else { ffi::game_main(&args) };
          ffi::log(&format!("game exited with code {code}"));
          view.quit.store(true, Ordering::Relaxed);
          std::process::exit(code);
      }).unwrap(); }
    { let v = view.clone(); std::thread::Builder::new().name("vsync".into()).spawn(move || gpu::vsync_thread(v)).unwrap(); }
    { let vol = std::env::var("NFSU2_VOLUME").ok().and_then(|v| v.parse::<f32>().ok()).map(|v| if v > 1.0 { v / 100.0 } else { v }).unwrap_or(1.0);
      let v = view.clone();
      std::thread::Builder::new().name("audio".into()).spawn(move || {
          let _stream = audio::start(vol);
          while !v.quit.load(Ordering::Relaxed) { std::thread::sleep(std::time::Duration::from_millis(200)); }
      }).unwrap(); }
    let mb: Arc<OnceLock<Arc<input::Mailbox>>> = Arc::new(OnceLock::new());
    { let (mb, v) = (mb.clone(), view.clone());
      std::thread::Builder::new().name("input".into()).spawn(move || {
          let Some(m) = input::Mailbox::attach(120_000) else { return };
          let _ = mb.set(m.clone());
          if std::env::var("NFSU2_PAD").map(|p| p != "0" && p != "off").unwrap_or(true) { m.pad_thread(v); }
      }).unwrap(); }

    let el = EventLoop::new().expect("event loop");
    el.set_control_flow(ControlFlow::Wait);
    let mut app = App { window: None, view: view.clone(), mods: ModifiersState::empty(), mb, started: false };
    let _ = el.run_app(&mut app);
    view.quit.store(true, Ordering::Relaxed);
}
