#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod catalog;
mod files;
mod media;
mod model;
mod player;
mod scan;
mod service;
mod ui;

const DEFAULT_WINDOW_SIZE: [f32; 2] = [1370.0, 886.0];
const MIN_WINDOW_SIZE: [f32; 2] = [700.0, 550.0];
const PLACEMENT_WINDOW_SIZE: [f32; 2] = [800.0, 550.0];

fn setting(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("MYTUBE_{name}"))
}

fn native_settings_transparency() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2::available!(macos = 26.0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn main() -> eframe::Result {
    let defer_window_size = cfg!(target_os = "linux")
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
            desktop
                .split(':')
                .any(|name| name.eq_ignore_ascii_case("gnome"))
        });
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/mytube.png"))
        .expect("embedded MyTube icon must be a valid PNG");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_app_id("mytube")
            .with_icon(icon)
            .with_inner_size(if defer_window_size {
                PLACEMENT_WINDOW_SIZE
            } else {
                DEFAULT_WINDOW_SIZE
            })
            .with_min_inner_size(MIN_WINDOW_SIZE)
            .with_title_shown(false)
            .with_fullsize_content_view(native_settings_transparency())
            .with_titlebar_shown(!native_settings_transparency())
            // Glow shares one pixel format with the transparent Mac settings viewport.
            .with_transparent(native_settings_transparency()),
        ..Default::default()
    };
    let service = service::Service::preload();
    eframe::run_native(
        "MyTube",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::Desktop::new(ui::MyTube::new(
                &cc.egui_ctx,
                cc.get_proc_address.clone(),
                defer_window_size,
                service(cc.egui_ctx.clone()),
            ))))
        }),
    )
}

#[cfg(test)]
mod verification;
