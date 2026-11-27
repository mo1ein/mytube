use super::settings_form::Form;
use crate::player::options::WindowPreferences;
use eframe::egui::{self, Vec2, pos2, vec2};

pub(super) fn settings(ui: &mut Form, prefs: &mut WindowPreferences) {
    ui.checkbox(&mut prefs.initial_size, "Set initial window size");
    if prefs.initial_size {
        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(&mut prefs.size_percent, "Percent of screen")
                .changed()
            {
                prefs.size = if prefs.size_percent {
                    [80, 80]
                } else {
                    [1280, 720]
                };
            }
            for (dimension, label) in prefs.size.iter_mut().zip(["Width", "Height"]) {
                let range = if prefs.size_percent {
                    10..=100
                } else {
                    160..=8192
                };
                ui.number(dimension, range, label, None, false);
            }
        });
    }
    ui.checkbox(&mut prefs.initial_position, "Set initial window position");
    if prefs.initial_position {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut prefs.right, "From right edge");
            ui.checkbox(&mut prefs.bottom, "From bottom edge");
            ui.checkbox(&mut prefs.offset_percent, "Offsets in percent");
            for (offset, label) in prefs.offset.iter_mut().zip(["X offset", "Y offset"]) {
                ui.number(
                    offset,
                    0..=if prefs.offset_percent { 100 } else { 8192 },
                    label,
                    None,
                    false,
                );
            }
        });
    }
}

fn bounded_size(size: Vec2, work: Vec2) -> Vec2 {
    size.max(crate::MIN_WINDOW_SIZE.into()).min(work)
}

pub(super) fn apply_initial(ctx: &egui::Context, frame: &eframe::Frame, prefs: &WindowPreferences) {
    if !prefs.initial_size && !prefs.initial_position {
        return;
    }
    let Some(window) = frame.winit_window() else {
        return;
    };
    let Some(monitor) = window.current_monitor() else {
        return;
    };
    let dpi = window.scale_factor() * f64::from(ctx.zoom_factor());
    let screen = monitor.size().to_logical::<f32>(dpi);
    let origin = monitor.position().to_logical::<f32>(dpi);
    let screen = vec2(screen.width, screen.height);
    let work = (screen - vec2(16., 72.)).max(vec2(1., 1.));
    let current = ctx.viewport_rect().size();
    let size = if prefs.initial_size {
        let requested = vec2(prefs.size[0] as f32, prefs.size[1] as f32);
        let size = bounded_size(
            if prefs.size_percent {
                requested * screen / 100.
            } else {
                requested
            },
            work,
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        size
    } else {
        current
    };
    if prefs.initial_position {
        let mut offset = vec2(prefs.offset[0] as f32, prefs.offset[1] as f32);
        if prefs.offset_percent {
            offset = offset * screen / 100.;
        }
        let free = (work - size).max(Vec2::ZERO);
        offset = offset.clamp(Vec2::ZERO, free);
        if prefs.right {
            offset.x = free.x - offset.x;
        }
        if prefs.bottom {
            offset.y = free.y - offset.y;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
            pos2(origin.x + 8., origin.y + 32.) + offset,
        ));
    }
}

#[derive(Default)]
pub(super) struct AspectLock {
    ratio: Option<f32>,
    last_size: Option<Vec2>,
}

impl AspectLock {
    pub fn update(&mut self, ctx: &egui::Context, _frame: &eframe::Frame, aspect: Option<f32>) {
        let (managed, screen) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.fullscreen == Some(true)
                    || v.maximized == Some(true)
                    || v.minimized == Some(true),
                v.monitor_size,
            )
        });
        let ratio = aspect.filter(|r| !managed && r.is_finite() && *r > 0.);
        let size = ctx.viewport_rect().size();
        let changed = ratio != self.ratio;
        #[cfg(target_os = "macos")]
        if changed {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let Ok(handle) = _frame.window_handle() else {
                return;
            };
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return;
            };
            // The root frame owns this live NSView for the duration of the UI callback.
            let view = unsafe { handle.ns_view.cast::<objc2_app_kit::NSView>().as_ref() };
            let Some(window) = view.window() else { return };
            if let Some(ratio) = ratio {
                window.setContentAspectRatio(objc2_foundation::NSSize::new(ratio.into(), 1.));
            } else {
                // A zero aspect ratio can produce a NaN height when AppKit checks
                // the window's zoom state. Unit increments clear the constraint.
                window.setContentResizeIncrements(objc2_foundation::NSSize::new(1., 1.));
            }
        }
        self.ratio = ratio;
        let previous = self.last_size.replace(size);
        // A new video's aspect only constrains the next user resize.
        if !cfg!(target_os = "macos")
            && !changed
            && let (Some(ratio), Some(previous)) = (ratio, previous)
            && previous != size
        {
            let delta = size - previous;
            let height = if delta.y.abs() * ratio > delta.x.abs() {
                size.y
            } else {
                size.x / ratio
            };
            let min_height = crate::MIN_WINDOW_SIZE[1].max(crate::MIN_WINDOW_SIZE[0] / ratio);
            let max_height = screen.map_or(f32::INFINITY, |screen| {
                (screen.y - 72.).min((screen.x - 16.) / ratio)
            });
            let height = height.max(min_height).min(max_height.max(1.));
            let target = vec2(height * ratio, height);
            if (target - size).abs().max_elem() > 1. {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(target));
            }
        }
    }
}
