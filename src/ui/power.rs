use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Battery {
    value: Option<u8>,
    sampled: Option<Instant>,
    pending: Option<Receiver<Option<u8>>>,
}

impl Battery {
    pub fn update(&mut self, ctx: &eframe::egui::Context) -> Option<u8> {
        if let Some(value) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.value = value;
            self.pending = None;
        }
        if self.pending.is_none()
            && self
                .sampled
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
        {
            self.sampled = Some(Instant::now());
            let (tx, rx) = channel();
            self.pending = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(capacity());
                ctx.request_repaint();
            });
        }
        self.value
    }
}

#[cfg(target_os = "macos")]
fn capacity() -> Option<u8> {
    let output = std::process::Command::new("/usr/bin/pmset")
        .args(["-g", "batt"])
        .output()
        .ok()?;
    let text = std::str::from_utf8(&output.stdout).ok()?;
    text.split_once('%')?
        .0
        .split(|c: char| !c.is_ascii_digit())
        .next_back()?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn capacity() -> Option<u8> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    entries.flatten().find_map(|entry| {
        let path = entry.path();
        if std::fs::read_to_string(path.join("type")).ok()?.trim() != "Battery" {
            return None;
        }
        std::fs::read_to_string(path.join("capacity"))
            .ok()?
            .trim()
            .parse::<u8>()
            .ok()
            .filter(|&p| p <= 100)
    })
}

#[cfg(windows)]
fn capacity() -> Option<u8> {
    #[repr(C)]
    #[derive(Default)]
    struct PowerStatus {
        ac: u8,
        flags: u8,
        percent: u8,
        saver: u8,
        seconds: u32,
        full: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut PowerStatus) -> i32;
    }
    let mut status = PowerStatus::default();
    // The OS writes the fixed-size SYSTEM_POWER_STATUS structure into this local value.
    let ok = unsafe { GetSystemPowerStatus(&mut status) };
    (ok != 0 && status.flags & 128 == 0 && status.percent <= 100).then_some(status.percent)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn capacity() -> Option<u8> {
    None
}
