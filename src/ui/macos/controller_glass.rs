//! Render foreground controls into the content of an AppKit glass view. The original
//! player view keeps all mouse, keyboard, and accessibility interaction.
use super::settings_glass::view_rect;
use crate::player::{Glass, VideoMask};
use dispatch2::MainThreadBound;
use eframe::{
    egui::{self, Context, Rect, Ui, epaint::ClippedShape},
    egui_glow,
};
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::NSView;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

unsafe extern "C" {
    fn mytube_glass_create() -> *mut NSView;
    fn mytube_glass_style(
        view: &NSView,
        radius: f64,
        edge_radius: f64,
        edges: u8,
        opacity: f64,
        symbol: *const std::ffi::c_char,
    );
    fn mytube_glass_begin(view: &NSView, width: i32, height: i32);
    fn mytube_glass_end(view: &NSView);
}

#[derive(Default)]
pub(in crate::ui) struct ControllerGlass {
    surfaces: Vec<Glass>,
    overlays: Vec<Overlay>,
    start: usize,
}

#[derive(Default)]
struct Overlay {
    view: Option<Arc<MainThreadBound<Retained<NSView>>>>,
    previous: Vec<ClippedShape>,
    pixels: [i32; 2],
    painted: Arc<AtomicBool>,
}
impl Drop for Overlay {
    fn drop(&mut self) {
        if let (Some(view), Some(mtm)) = (&self.view, MainThreadMarker::new()) {
            view.get(mtm).removeFromSuperview();
        }
    }
}
impl ControllerGlass {
    pub fn begin(&mut self, ui: &Ui, start: usize, surfaces: &mut Vec<Glass>) {
        self.start = start;
        self.surfaces = std::mem::take(surfaces);
        self.surfaces.retain(|g| ui.clip_rect().intersects(g.rect));
    }
    pub fn finish(&mut self, ui: &Ui, root: Option<&NSView>, mask: VideoMask) {
        self.finish_layer(ui.ctx(), ui.layer_id(), ui.clip_rect(), root, Some(mask));
    }

    pub fn dialog_view(&self) -> Option<&NSView> {
        Some(
            self.overlays
                .first()?
                .view
                .as_ref()?
                .get(MainThreadMarker::new()?)
                .as_ref(),
        )
    }

    pub fn dialog(
        &mut self,
        response: &egui::Response,
        start: usize,
        radius: f32,
        root: Option<&NSView>,
    ) {
        let rect = response.rect;
        self.start = start;
        self.surfaces = vec![Glass {
            rect,
            radius,
            frosted: true,
            symbol: None,
            opacity: 1.,
        }];
        self.finish_layer(&response.ctx, response.layer_id, rect, root, None);
    }

    pub fn sidebar(
        &mut self,
        ctx: &Context,
        layer: egui::LayerId,
        rect: Rect,
        root: Option<&NSView>,
    ) {
        self.start = 0;
        self.surfaces = vec![Glass {
            rect,
            radius: 0.,
            frosted: true,
            symbol: None,
            opacity: 1.,
        }];
        // Include the hovered playlist thumbnail's scrub preview above the native glass.
        // Shapes extending beyond the panel still paint into the main window.
        if root.is_some() {
            ctx.graphics_mut(|graphics| {
                let preview =
                    std::mem::take(graphics.entry(crate::ui::preview::card_preview_layer()));
                let panel = graphics.entry(layer);
                for entry in preview.all_entries() {
                    panel.add(entry.clip_rect, entry.shape.clone());
                }
            });
        }
        self.finish_layer(ctx, layer, rect, root, None);
    }

    fn finish_layer(
        &mut self,
        ctx: &Context,
        layer: egui::LayerId,
        clip: Rect,
        root: Option<&NSView>,
        mask: Option<VideoMask>,
    ) {
        let Some(root) = root else {
            self.hide();
            return;
        };
        let surfaces = std::mem::take(&mut self.surfaces);
        self.overlays.resize_with(surfaces.len(), Overlay::default);
        if surfaces.is_empty() {
            return;
        }
        let entries = ctx.graphics_mut(|graphics| {
            let list = graphics.entry(layer);
            let mut captured = vec![Vec::new(); surfaces.len()];
            let mut moved = vec![];
            for (index, entry) in list.all_entries().enumerate().skip(self.start) {
                let bounds = entry
                    .shape
                    .visual_bounding_rect()
                    .intersect(entry.clip_rect);
                for (i, surface) in surfaces.iter().enumerate() {
                    let rect = surface.rect.intersect(clip);
                    if matches!(entry.shape, egui::Shape::Callback(_))
                        || !bounds.is_positive()
                        || !rect.intersects(bounds)
                    {
                        continue;
                    }
                    let mut entry = entry.clone();
                    entry.clip_rect = entry.clip_rect.intersect(rect);
                    entry.clip_rect = entry.clip_rect.translate(-rect.min.to_vec2());
                    entry.shape.translate(-rect.min.to_vec2());
                    captured[i].push(entry);
                    if rect.expand(1.).contains_rect(bounds) {
                        moved.push(index);
                    }
                }
            }
            for i in moved {
                list.reset_shape(egui::layers::ShapeIdx(i));
            }
            captured
        });
        for ((surface, overlay), entries) in
            surfaces.into_iter().zip(&mut self.overlays).zip(entries)
        {
            let rect = surface.rect.intersect(clip);
            overlay.show(
                ctx,
                layer,
                root,
                rect,
                (surface, mask.map(|m| m.corners(rect)).unwrap_or_default()),
                entries,
            );
        }
    }
    pub fn hide(&mut self) {
        self.overlays.clear();
        self.surfaces.clear();
    }
}
impl Overlay {
    fn show(
        &mut self,
        ctx: &Context,
        layer: egui::LayerId,
        root: &NSView,
        rect: Rect,
        material: (Glass, egui::CornerRadius),
        entries: Vec<ClippedShape>,
    ) {
        let (surface, corners) = material;
        let mtm = root.mtm();
        let host = self.view.get_or_insert_with(|| {
            let view =
                unsafe { Retained::from_raw(mytube_glass_create()) }.expect("AppKit glass view");
            Arc::new(MainThreadBound::new(view, mtm))
        });
        let view = host.get(mtm);
        if unsafe { view.superview() }.as_deref() != Some(root) {
            root.addSubview(view);
        }
        let viewport = ctx.viewport_rect();
        let frame = view_rect(root.bounds(), root.isFlipped(), viewport, rect);
        if view.frame() != frame {
            view.setFrame(frame);
        }
        // AppKit's unflipped layer uses minY for the lower two corners.
        let edges = u8::from(corners.sw > 0)
            | (u8::from(corners.se > 0) << 1)
            | (u8::from(corners.nw > 0) << 2)
            | (u8::from(corners.ne > 0) << 3);
        let unit = frame.size.width / f64::from(rect.width());
        unsafe {
            mytube_glass_style(
                view,
                f64::from(surface.radius) * unit,
                f64::from(corners.nw.max(corners.ne).max(corners.sw).max(corners.se)) * unit,
                edges,
                f64::from(surface.opacity),
                surface.symbol.map_or(std::ptr::null(), |s| s.as_ptr()),
            );
        }
        let scale = ctx.pixels_per_point();
        let pixels = [
            (rect.width() * scale).ceil() as i32,
            (rect.height() * scale).ceil() as i32,
        ];
        if self.painted.load(Ordering::Acquire) && pixels == self.pixels && entries == self.previous
        {
            return;
        }
        // A discarded egui pass never runs its GL callbacks.
        self.painted = Arc::default();
        let painted = Arc::clone(&self.painted);
        self.pixels = pixels;
        self.previous = entries;
        // Egui paints the original meshes and font atlas. A tightly sized framebuffer
        // at the bottom-left of the GL viewport supplies the native foreground layer.
        let offset = egui::vec2(0., viewport.height() - pixels[1] as f32 / scale);
        let begin = Arc::clone(host);
        let end = Arc::clone(host);
        ctx.graphics_mut(|graphics| {
            let list = graphics.entry(layer);
            let callback = |f: egui_glow::CallbackFn| {
                egui::Shape::Callback(egui::PaintCallback {
                    rect,
                    callback: Arc::new(f),
                })
            };
            list.add(
                rect,
                callback(egui_glow::CallbackFn::new(move |_, _| {
                    if let Some(mtm) = MainThreadMarker::new() {
                        unsafe {
                            mytube_glass_begin(begin.get(mtm), pixels[0], pixels[1]);
                        }
                    }
                })),
            );
            for entry in &self.previous {
                let mut shape = entry.shape.clone();
                shape.translate(offset);
                list.add(entry.clip_rect.translate(offset), shape);
            }
            list.add(
                rect,
                callback(egui_glow::CallbackFn::new(move |_, _| {
                    if let Some(mtm) = MainThreadMarker::new() {
                        unsafe {
                            mytube_glass_end(end.get(mtm));
                        }
                        painted.store(true, Ordering::Release);
                    }
                })),
            );
        });
    }
}
