//! One AppKit backdrop beneath the catalog, sidebar, and toolbar.
use objc2::{MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSGlassEffectView, NSView, NSWindowOrderingMode,
};

#[derive(Default)]
pub(in crate::ui) struct LibraryGlass(Option<Retained<NSGlassEffectView>>);

impl LibraryGlass {
    pub fn visible(&self) -> bool {
        self.0.is_some()
    }

    pub fn update(&mut self, root: Option<&NSView>, strength: f32) {
        let enabled = strength > 0.;
        if enabled == self.visible() {
            return;
        }
        let Some(root) = root else {
            return;
        };
        let Some(window) = root.window() else {
            return;
        };
        if enabled {
            // The backing view is a sibling of the transparent GL surface. The
            // GL surface still performs hit testing and draws every control.
            let Some(parent) = (unsafe { root.superview() }) else {
                return;
            };
            let glass = NSGlassEffectView::initWithFrame(
                NSGlassEffectView::alloc(root.mtm()),
                root.frame(),
            );
            glass.setCornerRadius(0.);
            glass.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            parent.addSubview_positioned_relativeTo(
                &glass,
                NSWindowOrderingMode::Below,
                Some(root),
            );
            self.0 = Some(glass);
        } else if let Some(glass) = self.0.take() {
            glass.removeFromSuperview();
            window.setOpaque(true);
            window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        }
    }
}
