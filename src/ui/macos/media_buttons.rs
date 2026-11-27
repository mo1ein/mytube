//! Native playback badges. Egui owns their hit targets and accessibility.
use eframe::egui::{self, Context, Id, Painter, Rect};
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::NSString;

use super::settings_glass::view_rect;
use crate::ui::theme::{self, Icon};

#[derive(Clone)]
struct Badge {
    rect: Rect,
    symbol: &'static str,
    opacity: f32,
    strength: u8,
    cut: Option<objc2_foundation::NSRect>,
}

unsafe extern "C" {
    fn mytube_media_button() -> *mut NSView;
    fn mytube_media_style(view: &NSView, symbol: &NSString, size: f64, strength: f64);
    fn mytube_media_glyph(name: *const std::ffi::c_char, side: usize, alpha: *mut u8) -> bool;
}

pub(in crate::ui) fn mini_glyph(ctx: &Context, icon: Icon) -> Option<egui::TextureHandle> {
    let name = match icon {
        Icon::ExpandMini => c"pip.exit",
        Icon::Close => c"xmark",
        _ => return None,
    };
    let key = Id::new(("mini_system_glyph", name));
    if let Some(handle) = ctx.data(|data| data.get_temp(key)) {
        return Some(handle);
    }
    objc2::MainThreadMarker::new()?;
    const SIDE: usize = 64;
    let mut alpha = [0; SIDE * SIDE];
    // The AppKit helper fills this square alpha mask synchronously on the UI thread.
    if !unsafe { mytube_media_glyph(name.as_ptr(), SIDE, alpha.as_mut_ptr()) } {
        return None;
    }
    let pixels = alpha
        .into_iter()
        .map(egui::Color32::from_white_alpha)
        .collect();
    let image = egui::ColorImage::new([SIDE, SIDE], pixels);
    let handle = ctx.load_texture(name.to_string_lossy(), image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(key, handle.clone()));
    Some(handle)
}

#[derive(Default)]
pub(in crate::ui) struct MediaButtons(Vec<(Retained<NSView>, Option<Badge>)>);

impl Drop for MediaButtons {
    fn drop(&mut self) {
        for (view, _) in &self.0 {
            view.removeFromSuperview();
        }
    }
}

impl MediaButtons {
    pub fn begin(&self, ctx: &Context, enabled: bool) {
        let key = Id::new(("native_media_buttons", ctx.viewport_id()));
        ctx.data_mut(|d| d.insert_temp(key, enabled.then(Vec::<Badge>::new)));
    }

    pub fn finish(&mut self, ctx: &Context, host: Option<&NSView>, preview: Option<Rect>) {
        let key = Id::new(("native_media_buttons", ctx.viewport_id()));
        let badges = ctx
            .data_mut(|d| d.remove_temp::<Option<Vec<Badge>>>(key))
            .flatten()
            .unwrap_or_default();
        for (view, _) in self.0.drain(badges.len().min(self.0.len())..) {
            view.removeFromSuperview();
        }
        let Some(host) = host else { return };
        for (i, mut badge) in badges.into_iter().enumerate() {
            if i == self.0.len() {
                let view = unsafe { Retained::from_raw(mytube_media_button()) }
                    .expect("native media button");
                self.0.push((view, None));
            }
            let (view, previous) = &mut self.0[i];
            // AppKit only changes the hierarchy on this UI thread.
            if unsafe { view.superview() }.as_deref() != Some(host) {
                host.addSubview(view);
            }
            let frame = view_rect(
                host.bounds(),
                host.isFlipped(),
                ctx.viewport_rect(),
                badge.rect,
            );
            let old_frame = view.frame();
            if frame != old_frame {
                view.setFrame(frame);
            }
            if frame.size.height != old_frame.size.height
                || previous
                    .as_ref()
                    .is_none_or(|old| old.symbol != badge.symbol || old.strength != badge.strength)
            {
                unsafe {
                    mytube_media_style(
                        view,
                        &NSString::from_str(badge.symbol),
                        frame.size.height,
                        f64::from(badge.strength) / 100.,
                    );
                }
            }
            if previous
                .as_ref()
                .is_none_or(|old| old.opacity != badge.opacity)
            {
                view.setAlphaValue(f64::from(badge.opacity));
            }
            badge.cut = preview
                .map(|rect| rect.intersect(badge.rect))
                .filter(Rect::is_positive)
                .map(|rect| view_rect(view.bounds(), view.isFlipped(), badge.rect, rect));
            if previous.as_ref().and_then(|old| old.cut) != badge.cut {
                let cuts = badge.cut.as_slice();
                unsafe { super::mytube_catalog_button_clip(view, cuts.as_ptr(), cuts.len()) };
            }
            *previous = Some(badge);
        }
    }
}

pub(in crate::ui) fn request(painter: &Painter, rect: Rect, icon: Icon) -> bool {
    let ctx = painter.ctx();
    if !painter.clip_rect().contains_rect(rect)
        || !rect.is_positive()
        || egui::Popup::is_any_open(ctx)
    {
        return false;
    }
    let symbol = match icon {
        Icon::Play => "play.fill",
        Icon::Pause => "pause.fill",
        Icon::Close => "xmark",
        Icon::ExpandMini => "pip.exit",
        _ => return false,
    };
    let badge = Badge {
        rect,
        symbol,
        opacity: painter.opacity(),
        strength: theme::glass_strength(ctx),
        cut: None,
    };
    let key = Id::new(("native_media_buttons", ctx.viewport_id()));
    ctx.data_mut(|d| {
        let Some(buttons) = d.get_temp_mut_or_default::<Option<Vec<Badge>>>(key) else {
            return false;
        };
        buttons.push(badge);
        true
    })
}
