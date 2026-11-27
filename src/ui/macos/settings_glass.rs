//! AppKit settings controls, embedded in the player or hosted by a library settings window.
use eframe::egui::{self, Context, Rect};
use objc2::{MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAccessibilityHighContrastAqua,
    NSAppearanceNameAccessibilityHighContrastDarkAqua, NSAppearanceNameAqua,
    NSAppearanceNameDarkAqua, NSApplication, NSAutoresizingMaskOptions, NSColor, NSGlassEffectView,
    NSView, NSWindow,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::ui::control_bar::DisplayMaterial;

use crate::player::options::Section;
use crate::ui::settings_form::NativeForm;
use objc2_foundation::NSString;
use std::ptr::NonNull;

unsafe extern "C" {
    fn mytube_settings_window(parent: &NSWindow) -> *mut NSWindow;
    fn mytube_form_create(parent: &NSView) -> *mut NSView;
    fn mytube_settings_surface_style(surface: &NSView, radius: f64);
    fn mytube_form_accessibility(form: &NSView, mounted: bool);
    fn mytube_form_background(form: &NSView, color: &NSColor);
    fn mytube_form_update(form: &NSView, bytes: *const u8, length: usize);
    fn mytube_form_event(form: &NSView) -> *mut NSString;
}

#[derive(Default)]
pub(in crate::ui) struct SettingsGlass {
    root: Option<Retained<NSView>>,
    panel: Option<Retained<NSWindow>>,
    container: Option<Retained<NSView>>,
    sidebar_surface: Option<Retained<NSView>>,
    app_active: Option<bool>,
    appearance: Option<(egui::Theme, DisplayMaterial, u8)>,
    glass: Option<Retained<NSGlassEffectView>>,
    form: Option<Retained<NSView>>,
    model: Vec<u8>,
    last_refresh: Option<std::time::Instant>,
}

impl SettingsGlass {
    pub fn attach(&mut self, root: Option<&NSView>) {
        if self.root.is_none()
            && let Some(root) = root
        {
            if let Some(window) = root.window() {
                window.setTitlebarSeparatorStyle(objc2_app_kit::NSTitlebarSeparatorStyle::None);
            }
            self.root = Some(Retained::from(root));
        }
    }

    pub fn raise_sidebar(&self) {
        if self.panel.is_none()
            && let (Some(container), Some(root)) = (&self.container, &self.root)
            && root.subviews().lastObject().as_deref() != Some(container.as_ref())
        {
            root.addSubview_positioned_relativeTo(
                container,
                objc2_app_kit::NSWindowOrderingMode::Above,
                None,
            );
        }
    }

    pub fn available(&self, ctx: &Context) -> bool {
        self.root.is_some() && !ctx.embed_viewports()
    }

    pub fn lost_focus(&mut self, root_lost_focus: bool) -> bool {
        let Some(root) = &self.root else {
            return root_lost_focus;
        };
        let active = NSApplication::sharedApplication(root.mtm()).isActive();
        let previous = self.app_active.replace(active);
        if self.panel.is_some() {
            previous == Some(true) && !active
        } else {
            root_lost_focus
        }
    }

    pub fn detach(&mut self) {
        if self.panel.is_none()
            && let Some(form) = &self.form
        {
            unsafe {
                mytube_form_accessibility(form, false);
            }
        }
        if let Some(panel) = self.panel.take() {
            panel.orderOut(None);
        }
        if let Some(glass) = self.glass.take() {
            glass.removeFromSuperview();
        }
        if let Some(form) = self.form.take() {
            form.removeFromSuperview();
        }
        self.sidebar_surface = None;
        if let Some(container) = self.container.take() {
            container.removeFromSuperview();
        }
        self.model.clear();
        self.last_refresh = None;
        self.appearance = None;
    }

    pub fn show(
        &mut self,
        ctx: &Context,
        sidebar: Option<(Rect, Rect)>,
        tabs: Section,
        compact: bool,
        glass: bool,
        mut contents: impl FnMut(Section, &mut NativeForm),
    ) -> bool {
        let open = if let Some((rect, clip)) = sidebar {
            if self.panel.is_some() {
                self.detach();
            }
            if let Some(root) = self.root.clone() {
                if root.inLiveResize() {
                    ctx.request_repaint();
                }
                // AppKit resizes its views while the next player layout is still pending.
                // Anchor the exposed edge and let a fully revealed form follow its container.
                let edge = if rect.center().x >= ctx.viewport_rect().center().x {
                    NSAutoresizingMaskOptions::ViewMinXMargin
                } else {
                    NSAutoresizingMaskOptions::ViewMaxXMargin
                };
                let child_mask = if rect == clip {
                    NSAutoresizingMaskOptions::ViewWidthSizable
                        | NSAutoresizingMaskOptions::ViewHeightSizable
                } else {
                    NSAutoresizingMaskOptions::ViewNotSizable
                };
                let frame = view_rect(root.bounds(), root.isFlipped(), ctx.viewport_rect(), rect);
                let radius = f64::from(crate::ui::player_layout::settings_radius(ctx))
                    * frame.size.width
                    / f64::from(rect.width());
                let clip = view_rect(root.bounds(), root.isFlipped(), ctx.viewport_rect(), clip);
                self.mount(&root, clip, false);
                if let Some(glass) = &self.glass
                    && glass.cornerRadius() != radius
                {
                    glass.setCornerRadius(radius);
                }
                if let (Some(container), Some(surface)) = (&self.container, &self.sidebar_surface) {
                    let mask = edge | NSAutoresizingMaskOptions::ViewHeightSizable;
                    if container.autoresizingMask() != mask {
                        container.setAutoresizingMask(mask);
                    }
                    container.setClipsToBounds(true);
                    let content = NSRect::new(
                        NSPoint::new(
                            frame.origin.x - clip.origin.x,
                            frame.origin.y - clip.origin.y,
                        ),
                        frame.size,
                    );
                    if surface.autoresizingMask() != child_mask {
                        surface.setAutoresizingMask(child_mask);
                    }
                    if surface.frame() != content {
                        surface.setFrame(content);
                    }
                    unsafe { mytube_settings_surface_style(surface, radius) };
                }
                self.update_material(ctx, glass);
            }
            self.update_form(ctx, tabs, compact, &mut contents)
        } else {
            if self.container.is_some() && self.panel.is_none() {
                self.detach();
            }
            if self.panel.is_none()
                && let Some(parent) = self.root.as_ref().and_then(|root| root.window())
                && let Some(panel) = unsafe { Retained::from_raw(mytube_settings_window(&parent)) }
                && let Some(content) = panel.contentView()
            {
                self.mount(&content, content.bounds(), true);
                panel.makeKeyAndOrderFront(None);
                self.panel = Some(panel);
            }
            self.update_material(ctx, glass);
            self.panel.as_ref().is_some_and(|panel| panel.isVisible())
                && self.update_form(ctx, tabs, compact, &mut contents)
        };
        if !open {
            if sidebar.is_none() {
                self.detach();
            }
            ctx.request_repaint();
        }
        open
    }

    fn mount(&mut self, parent: &NSView, frame: NSRect, resize: bool) {
        if let Some(container) = &self.container {
            if container.frame() != frame {
                container.setFrame(frame);
            }
            return;
        }
        let size_mask = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;
        let container = NSView::initWithFrame(NSView::alloc(parent.mtm()), frame);
        let surface = if resize {
            container.setAutoresizingMask(size_mask);
            &container
        } else {
            // The outer view clips the slide animation. The full-size inner view
            // gives the glass, tinted background and scrolling controls one silhouette.
            let surface = NSView::initWithFrame(NSView::alloc(parent.mtm()), container.bounds());
            container.addSubview(&surface);
            self.sidebar_surface.insert(surface)
        };
        let glass = NSGlassEffectView::initWithFrame(
            NSGlassEffectView::alloc(parent.mtm()),
            surface.bounds(),
        );
        glass.setCornerRadius(0.);
        glass.setAutoresizingMask(size_mask);
        surface.addSubview(&glass);
        self.form = unsafe { Retained::from_raw(mytube_form_create(surface)) };
        parent.addSubview(&container);
        if !resize && let Some(form) = &self.form {
            form.setAutoresizingMask(size_mask);
            unsafe {
                mytube_form_accessibility(form, true);
            }
        }
        self.glass = Some(glass);
        self.container = Some(container);
    }

    fn update_form(
        &mut self,
        ctx: &Context,
        section: Section,
        compact: bool,
        contents: &mut impl FnMut(Section, &mut NativeForm),
    ) -> bool {
        let mut open = true;
        if let Some(view) = &self.form {
            // AppKit returns an autoreleased string on the main thread.
            let event =
                unsafe { NonNull::new(mytube_form_event(view)).map(|p| p.as_ref().to_string()) };
            let action = event
                .and_then(|value| serde_json::from_str::<(String, serde_json::Value)>(&value).ok());
            let mut current = section;
            if let Some((id, value)) = &action {
                if id == "close" {
                    open = false;
                } else if id == "section" {
                    current = Section::ALL
                        .into_iter()
                        .find(|tab| Some(tab.title()) == value.as_str())
                        .unwrap_or(section);
                }
            }
            // Respond to edits immediately. During live resize AppKit reflows the existing
            // controls, so only refresh playback values at the standalone window cadence.
            if action.is_some()
                || (self.panel.is_none() && !view.inLiveResize())
                || self
                    .last_refresh
                    .is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(200))
            {
                let mut form = NativeForm::new(ctx, current.title(), action);
                contents(current, &mut form);
                let model = serde_json::to_vec(&serde_json::json!({
                    "rows": form.rows,
                    "section": current.title(),
                    "tabs": Section::ALL.map(Section::title),
                    "compact": compact,
                    "sidebar": self.panel.is_none(),
                }))
                .expect("valid native control values");
                if self.model != model {
                    // The bridge consumes the borrowed bytes before returning.
                    unsafe { mytube_form_update(view, model.as_ptr(), model.len()) };
                    self.model = model;
                }
                self.last_refresh = Some(std::time::Instant::now());
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(32));
        }
        let close_key = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma)
                || i.consume_key(egui::Modifiers::COMMAND, egui::Key::W)
        });
        let escape = !egui::Popup::is_any_open(ctx)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        open && !escape && !close_key
    }

    fn update_material(&mut self, ctx: &Context, enabled: bool) {
        let Some(container) = &self.container else {
            return;
        };
        let theme = ctx.theme();
        let system = DisplayMaterial::system(ctx);
        let material = if !enabled && system == DisplayMaterial::Glass {
            DisplayMaterial::Opaque
        } else {
            system
        };
        let strength = crate::ui::theme::glass_strength(ctx);
        if self.appearance != Some((theme, material, strength)) {
            let amount = if material == DisplayMaterial::Glass {
                f64::from(strength) / 100.
            } else {
                0.
            };
            if let Some(glass) = &self.glass {
                glass.setHidden(amount == 0.);
            }
            let palette = crate::ui::theme::colors(ctx, &ctx.global_style().visuals);
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(
                f64::from(palette.side.r()) / 255.,
                f64::from(palette.side.g()) / 255.,
                f64::from(palette.side.b()) / 255.,
                1. - amount,
            );
            if let Some(form) = &self.form {
                unsafe {
                    mytube_form_background(form, &color);
                }
            }
            if let Some(panel) = &self.panel {
                panel.setOpaque(amount == 0.);
                panel.setTitlebarAppearsTransparent(true);
                panel.setBackgroundColor(Some(&NSColor::clearColor()));
            }
            let name = unsafe {
                match (theme, material == DisplayMaterial::HighContrast) {
                    (egui::Theme::Dark, true) => NSAppearanceNameAccessibilityHighContrastDarkAqua,
                    (egui::Theme::Light, true) => NSAppearanceNameAccessibilityHighContrastAqua,
                    (egui::Theme::Dark, false) => NSAppearanceNameDarkAqua,
                    (egui::Theme::Light, false) => NSAppearanceNameAqua,
                }
            };
            let appearance = NSAppearance::appearanceNamed(name);
            container.setAppearance(appearance.as_deref());
            if let Some(panel) = &self.panel {
                panel.setAppearance(appearance.as_deref());
            }
            self.appearance = Some((theme, material, strength));
        }
    }
}

pub(super) fn view_rect(bounds: NSRect, flipped: bool, viewport: Rect, rect: Rect) -> NSRect {
    let sx = bounds.size.width / f64::from(viewport.width());
    let sy = bounds.size.height / f64::from(viewport.height());
    let y = if flipped {
        rect.top() - viewport.top()
    } else {
        viewport.bottom() - rect.bottom()
    };
    NSRect::new(
        NSPoint::new(
            bounds.origin.x + f64::from(rect.left() - viewport.left()) * sx,
            bounds.origin.y + f64::from(y) * sy,
        ),
        NSSize::new(f64::from(rect.width()) * sx, f64::from(rect.height()) * sy),
    )
}

impl Drop for SettingsGlass {
    fn drop(&mut self) {
        self.detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_coordinates_follow_zoom_and_appkit_origin() {
        let bounds = NSRect::new(NSPoint::new(10., 20.), NSSize::new(800., 600.));
        let viewport = Rect::from_min_size(egui::pos2(4., 8.), egui::vec2(400., 300.));
        let sidebar = Rect::from_min_size(egui::pos2(304., 28.), egui::vec2(100., 230.));
        let size = NSSize::new(200., 460.);
        assert_eq!(
            view_rect(bounds, true, viewport, sidebar),
            NSRect::new(NSPoint::new(610., 60.), size)
        );
        assert_eq!(
            view_rect(bounds, false, viewport, sidebar),
            NSRect::new(NSPoint::new(610., 120.), size)
        );
    }
}
