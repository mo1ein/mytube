use eframe::egui::{self, Align, Context, Id, Layout, Rect, Stroke, Ui};

use super::control_bar::DisplayMaterial;
use crate::player::options::PlayerPreferences;

#[derive(Default, Clone, Copy)]
pub(super) struct Sidebars {
    pub settings: Option<Rect>,
    pub playlist: Option<Rect>,
}

impl Sidebars {
    pub fn fullscreen(
        bounds: Rect,
        prefs: &PlayerPreferences,
        settings: Option<bool>,
        playlist: bool,
    ) -> Self {
        let playlist = playlist && (!prefs.edge_to_edge || settings != Some(prefs.playlist_left));
        let preferred = prefs.sidebar_width as f32;
        let limit = bounds.width() * 0.42;
        // Give fullscreen settings more room for labels and controls as the display grows.
        let extra = (bounds.width() / 12.).min(140.);
        let frame = |left: bool, width: f32| {
            if left {
                bounds.with_max_x(bounds.left() + width)
            } else {
                bounds.with_min_x(bounds.right() - width)
            }
        };
        Self {
            settings: settings.map(|left| frame(left, (preferred + extra).min(limit))),
            playlist: playlist.then(|| frame(prefs.playlist_left, preferred.min(limit))),
        }
    }

    pub fn video_area(self, mut bounds: Rect) -> Rect {
        let center = bounds.center().x;
        if let Some(sidebar) = self.playlist {
            if sidebar.center().x < center {
                bounds.min.x = sidebar.right();
            } else {
                bounds.max.x = sidebar.left();
            }
        }
        bounds
    }
}

pub(super) fn settings_radius(ctx: &Context) -> u8 {
    if super::theme::interface_theme(ctx).glass_ui() {
        18
    } else {
        12
    }
}

pub(super) fn sidebar(
    ctx: &Context,
    id: Id,
    geometry: (Rect, Rect),
    title: &str,
    style: SidebarStyle,
    body: impl FnOnce(&mut Ui),
) -> bool {
    let (rect, clip) = geometry;
    let mut open = true;
    let SidebarStyle {
        glass,
        compact,
        radius,
        outlined,
    } = style;
    let youtube = !super::theme::interface_theme(ctx).glass_ui();
    let glass = glass && DisplayMaterial::system(ctx) == DisplayMaterial::Glass;
    let inset = if compact { 6. } else { 10. };
    egui::Area::new(id)
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .movable(false)
        .constrain(false)
        .default_size(rect.size())
        .show(ctx, |ui| {
            ui.set_clip_rect(clip);
            ui.set_min_size(rect.size());
            let fill = if youtube {
                super::theme::palette(ui).back
            } else {
                ui.visuals().panel_fill
            };
            ui.painter().rect_filled(
                rect,
                radius,
                if glass {
                    super::theme::glass_tint(ctx, fill)
                } else {
                    fill
                },
            );
            if outlined {
                ui.painter().rect_stroke(
                    rect,
                    radius,
                    Stroke::new(1., ui.visuals().widgets.noninteractive.bg_stroke.color),
                    egui::StrokeKind::Inside,
                );
            }
            let inner = rect.shrink(inset);
            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                ui.set_clip_rect(inner.intersect(clip));
                if compact {
                    ui.spacing_mut().item_spacing.y = 2.;
                }
                if youtube {
                    super::classic_settings::style(ui);
                }
                ui.horizontal(|ui| {
                    ui.strong(title);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close = if youtube {
                            super::classic_settings::close_button(ui, "Close sidebar")
                        } else {
                            ui.button("×").on_hover_text("Close sidebar")
                        };
                        if close.clicked() {
                            open = false;
                        }
                    });
                });
                if outlined {
                    ui.separator();
                }
                body(ui);
            });
        });
    open
}

#[derive(Default)]
pub(super) struct SidebarStyle {
    pub glass: bool,
    pub compact: bool,
    pub radius: u8,
    pub outlined: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    #[test]
    fn settings_overlay_the_video_while_the_playlist_can_dock() {
        let bounds = Rect::from_min_size(pos2(20., 50.), vec2(1200., 600.));
        let prefs = PlayerPreferences {
            edge_to_edge: false,
            playlist_left: true,
            ..Default::default()
        };
        let both = Sidebars::fullscreen(bounds, &prefs, Some(false), true);
        let video = both.video_area(bounds);
        assert_eq!(video.left(), both.playlist.unwrap().right());
        assert_eq!(video.right(), bounds.right());
        assert!(video.contains_rect(both.settings.unwrap()));
        assert_eq!(
            Sidebars::fullscreen(bounds, &prefs, Some(false), false).video_area(bounds),
            bounds
        );
        assert!(video.width() >= 550.);
        let narrow = bounds.with_max_x(720.);
        for area in [bounds, narrow] {
            let docked = Sidebars::fullscreen(area, &prefs, None, true).video_area(area);
            for left in [true, false] {
                let shown = Sidebars::fullscreen(area, &prefs, Some(left), true);
                assert_eq!(shown.video_area(area), docked);
                let panel = shown.settings.unwrap();
                if left {
                    assert_eq!(panel.left(), area.left());
                } else {
                    assert_eq!(panel.right(), area.right());
                }
            }
        }
        assert_eq!(
            Sidebars::fullscreen(narrow, &prefs, None, false).video_area(narrow),
            narrow
        );
    }
}
