use crate::player::options::ToolbarItem;
use eframe::egui::{
    self, Align, Align2, Color32, Context, CornerRadius, Layout, Pos2, Rect, Response, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2,
};

use crate::{
    model::{ControlBarLayout, ProgressBarPosition},
    player::Glass,
};

use super::theme::{self, Icon};

pub(super) const ACCENT: Color32 = Color32::WHITE;
pub(super) const MIN_PLAYER_HEIGHT: f32 = 124.0;
const MUTED: Color32 = Color32::from_gray(224);
pub(super) const BUTTON_WIDTH: f32 = 34.0;
const PLAY_WIDTH: f32 = 40.0;
pub(super) const TRANSPORT_WIDTH: f32 = BUTTON_WIDTH * 2.0 + PLAY_WIDTH;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum DisplayMaterial {
    #[default]
    Glass,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Opaque,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    HighContrast,
}

impl DisplayMaterial {
    const ID: &str = "glass_display_material";

    pub fn current(ctx: &Context) -> Self {
        let system = Self::system(ctx);
        if system == Self::Glass
            && theme::interface_theme(ctx).translucent()
            && (theme::glass_strength(ctx) == 0
                || (cfg!(target_os = "macos") && egui::Popup::is_any_open(ctx)))
        {
            return Self::Opaque;
        }
        ctx.data(|data| {
            if system == Self::Glass
                && data
                    .get_temp::<bool>(egui::Id::new("opaque_player_controls"))
                    .unwrap_or(false)
            {
                Self::Opaque
            } else {
                system
            }
        })
    }

    pub fn system(ctx: &Context) -> Self {
        ctx.data(|data| data.get_temp(egui::Id::new(Self::ID)).unwrap_or_default())
    }

    #[cfg(target_os = "macos")]
    pub fn apply(self, ctx: &Context) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new(Self::ID), self));
        ctx.request_repaint();
    }

    pub(super) fn surface(self, ctx: &Context) -> (Color32, Stroke) {
        match self {
            Self::Glass => (
                Color32::from_gray(38)
                    .lerp_to_gamma(Color32::from_white_alpha(3), theme::glass_amount(ctx)),
                Stroke::new(1.0, Color32::from_white_alpha(18)),
            ),
            Self::Opaque => (
                Color32::from_gray(38),
                Stroke::new(1.0, Color32::from_gray(119)),
            ),
            Self::HighContrast => (Color32::BLACK, Stroke::new(1.0, Color32::WHITE)),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Bar {
    pub player: Rect,
    pub rect: Rect,
    pub layout: ControlBarLayout,
    pub detached_timeline: Option<Rect>,
    pub window_actions: bool,
    pub(super) enabled_actions: usize,
}

impl Bar {
    pub fn new(
        player: Rect,
        layout: ControlBarLayout,
        position: ProgressBarPosition,
        fullscreen: bool,
    ) -> Self {
        let timeline_size = vec2(760.0_f32.min((player.width() - 24.0).max(1.0)), 34.0);
        let detached_timeline = match position {
            ProgressBarPosition::FollowLayout => None,
            ProgressBarPosition::Top => Some(Rect::from_center_size(
                pos2(player.center().x, player.top() + 29.0),
                timeline_size,
            )),
            ProgressBarPosition::Bottom => Some(Rect::from_center_size(
                pos2(player.center().x, player.bottom() - 29.0),
                timeline_size,
            )),
        };
        let (width, height, margin) = match layout {
            ControlBarLayout::Floating if detached_timeline.is_some() => (560.0, 54.0, 18.0),
            ControlBarLayout::Floating => (560.0, 88.0, 18.0),
            ControlBarLayout::Compact if detached_timeline.is_some() => {
                (if fullscreen { 338.0 } else { 404.0 }, 54.0, 14.0)
            }
            ControlBarLayout::Compact => (780.0, 54.0, 14.0),
            ControlBarLayout::Docked | ControlBarLayout::Top => (player.width(), 48.0, 0.0),
            ControlBarLayout::Classic if detached_timeline.is_some() => (player.width(), 47.0, 0.0),
            ControlBarLayout::Classic => (player.width(), player.height().min(65.0), 0.0),
        };
        let bottom = detached_timeline
            .filter(|_| position == ProgressBarPosition::Bottom)
            .map_or(player.bottom() - margin, |strip| strip.top() - 6.0);
        let top = if layout == ControlBarLayout::Top {
            detached_timeline
                .filter(|_| position == ProgressBarPosition::Top)
                .map_or(player.top(), |strip| strip.bottom() + 6.0)
        } else {
            bottom - height
        };
        let width = width.min((player.width() - margin * 2.0).max(1.0));
        let mut bar = Self {
            player,
            enabled_actions: 0,
            rect: Rect::from_center_size(
                pos2(player.center().x, top + height / 2.0),
                vec2(width, height),
            ),
            layout,
            detached_timeline,
            window_actions: !fullscreen,
        };
        bar.set_toolbar(&ToolbarItem::DEFAULTS);
        bar
    }

    pub fn compact_spacing(&mut self) {
        let reduction = match self.layout {
            ControlBarLayout::Classic => return,
            ControlBarLayout::Floating => 6.,
            _ => 4.,
        };
        if self.layout == ControlBarLayout::Top {
            self.rect.max.y -= reduction;
        } else {
            self.rect.min.y += reduction;
        }
    }

    fn radius(self) -> u8 {
        match self.layout {
            ControlBarLayout::Floating => 16,
            ControlBarLayout::Compact => 27,
            ControlBarLayout::Docked | ControlBarLayout::Top | ControlBarLayout::Classic => 0,
        }
    }

    pub fn glass(self, material: DisplayMaterial, opacity: f32) -> Option<Glass> {
        (material == DisplayMaterial::Glass && self.layout != ControlBarLayout::Classic).then_some(
            Glass {
                rect: self.rect,
                radius: f32::from(self.radius()),
                frosted: false,
                symbol: None,
                opacity,
            },
        )
    }

    pub fn timeline_glass(self, material: DisplayMaterial, opacity: f32) -> Option<Glass> {
        self.detached_timeline
            .and_then(|rect| timeline_glass(self.layout, material, rect, opacity))
    }

    pub fn paint(self, ui: &Ui, mask: crate::player::VideoMask) {
        let material = DisplayMaterial::current(ui.ctx());
        if (self.layout == ControlBarLayout::Classic
            || !theme::interface_theme(ui.ctx()).glass_ui())
            && DisplayMaterial::system(ui.ctx()) == DisplayMaterial::Glass
        {
            return;
        }
        let painter = ui.painter();
        let radius = mask.corners(self.rect).at_least(self.radius());
        let (fill, edge) = material.surface(ui.ctx());
        painter.rect_filled(self.rect, radius, fill);
        match self.layout {
            ControlBarLayout::Docked => {
                painter.line_segment([self.rect.left_top(), self.rect.right_top()], edge);
            }
            ControlBarLayout::Top => {
                painter.line_segment([self.rect.left_bottom(), self.rect.right_bottom()], edge);
            }
            _ => {
                painter.rect_stroke(self.rect, radius, edge, StrokeKind::Inside);
            }
        }
    }

    pub fn row(self) -> Rect {
        let center_y = match self.layout {
            ControlBarLayout::Floating if self.detached_timeline.is_none() => {
                self.rect.top() + self.rect.height() * (30. / 88.)
            }
            ControlBarLayout::Floating | ControlBarLayout::Compact => self.rect.center().y,
            ControlBarLayout::Classic => self.rect.bottom() - 23.0,
            ControlBarLayout::Docked | ControlBarLayout::Top => self.rect.center().y,
        };
        Rect::from_center_size(
            pos2(self.rect.center().x, center_y),
            vec2((self.rect.width() - 28.0).max(1.0), 40.0),
        )
    }

    pub fn timeline(self) -> Rect {
        let row = self.row();
        let (left, right, y, height) = match self.layout {
            ControlBarLayout::Floating => (
                row.left() + 3.0,
                row.right() - 3.0,
                self.rect.bottom() - 22.0,
                24.0,
            ),
            ControlBarLayout::Compact => (
                row.left() + TRANSPORT_WIDTH + BUTTON_WIDTH + 6.0,
                row.right() - self.actions_width() - 5.0,
                row.center().y,
                32.0,
            ),
            ControlBarLayout::Classic => (
                row.left() + 1.0,
                row.right() - 1.0,
                self.rect.top() + 17.0,
                24.0,
            ),
            ControlBarLayout::Docked | ControlBarLayout::Top => (
                row.left() + TRANSPORT_WIDTH + 6.0,
                self.volume_x() - 6.0,
                row.center().y,
                32.0,
            ),
        };
        Rect::from_min_max(pos2(left, y - height * 0.5), pos2(right, y + height * 0.5))
    }

    pub fn inline_volume(self) -> bool {
        match self.layout {
            ControlBarLayout::Compact => false,
            ControlBarLayout::Floating => self.row().width() >= 360.0,
            _ => self.row().width() >= 595.0,
        }
    }

    pub fn volume_x(self) -> f32 {
        let row = self.row();
        match self.layout {
            ControlBarLayout::Floating => row.left(),
            ControlBarLayout::Compact | ControlBarLayout::Classic => row.left() + TRANSPORT_WIDTH,
            ControlBarLayout::Docked | ControlBarLayout::Top => {
                row.right()
                    - self.actions_width()
                    - BUTTON_WIDTH
                    - if self.inline_volume() { 78.0 } else { 0.0 }
            }
        }
    }

    // Normalized coordinates keep a moved controller inside the picture after
    // resizing, including when the timeline occupies a separate edge.
    pub fn floating_travel(self) -> Rect {
        let mut bounds = self.player.shrink(4.0);
        if let Some(strip) = self.detached_timeline {
            if strip.center().y < self.player.center().y {
                bounds.min.y = strip.bottom() + 4.0;
            } else {
                bounds.max.y = strip.top() - 4.0;
            }
        }
        Rect::from_min_max(bounds.min, (bounds.max - self.rect.size()).max(bounds.min))
    }

    pub fn place_floating(&mut self, fraction: egui::Vec2) {
        let travel = self.floating_travel();
        self.rect = Rect::from_min_size(travel.min + fraction * travel.size(), self.rect.size());
    }

    pub fn set_toolbar(&mut self, items: &[ToolbarItem]) {
        let row = self.row();
        let available = match self.layout {
            ControlBarLayout::Floating => (row.width() - TRANSPORT_WIDTH) * 0.5,
            ControlBarLayout::Compact if self.detached_timeline.is_none() => {
                row.width() - TRANSPORT_WIDTH - BUTTON_WIDTH - 90.0
            }
            _ => {
                row.width()
                    - TRANSPORT_WIDTH
                    - BUTTON_WIDTH
                    - if self.inline_volume() { 180.0 } else { 70.0 }
            }
        };
        let count = items.iter().filter(|&&item| self.shows(item)).count();
        self.enabled_actions = count.min((available / BUTTON_WIDTH).floor().max(1.) as usize);
    }

    pub fn shows(self, item: ToolbarItem) -> bool {
        match item {
            ToolbarItem::Theater | ToolbarItem::Mini => self.window_actions,
            ToolbarItem::Text => cfg!(target_os = "macos"),
            _ => true,
        }
    }

    pub fn actions_width(self) -> f32 {
        BUTTON_WIDTH * self.enabled_actions as f32
    }
}

pub(super) fn timeline_glass(
    layout: ControlBarLayout,
    material: DisplayMaterial,
    rect: Rect,
    opacity: f32,
) -> Option<Glass> {
    (material == DisplayMaterial::Glass && layout != ControlBarLayout::Classic).then_some(Glass {
        rect,
        radius: 16.0,
        frosted: false,
        symbol: None,
        opacity,
    })
}

pub(super) fn paint_timeline(ui: &Ui, layout: ControlBarLayout, rect: Rect) {
    let material = DisplayMaterial::current(ui.ctx());
    let classic = layout == ControlBarLayout::Classic;
    if (classic || !theme::interface_theme(ui.ctx()).glass_ui())
        && DisplayMaterial::system(ui.ctx()) == DisplayMaterial::Glass
    {
        return;
    }
    let (fill, edge) = material.surface(ui.ctx());
    ui.painter().rect(
        rect,
        if classic { 6 } else { 16 },
        fill,
        edge,
        StrokeKind::Inside,
    );
}

pub(super) fn mini_timeline(
    video: Rect,
    layout: ControlBarLayout,
    position: ProgressBarPosition,
) -> Rect {
    let (y, inset) = if mini_timeline_at_top(layout, position) {
        (video.top() + 26.0, 52.0)
    } else {
        (video.bottom() - 22.0, 8.0)
    };
    Rect::from_center_size(
        pos2(video.center().x, y),
        vec2((video.width() - 2.0 * inset).max(1.0), 28.0),
    )
}

pub(super) fn mini_timeline_at_top(
    layout: ControlBarLayout,
    position: ProgressBarPosition,
) -> bool {
    position == ProgressBarPosition::Top
        || (position == ProgressBarPosition::FollowLayout && layout == ControlBarLayout::Top)
}

pub(super) fn group(ui: &mut Ui, id: &str, row: Rect, x: f32) -> Ui {
    let mut group = ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(Rect::from_min_max(pos2(x, row.top()), row.right_bottom()))
            .layout(Layout::left_to_right(Align::Center)),
    );
    group.spacing_mut().item_spacing.x = 0.0;
    group
}

pub(super) fn button(
    ui: &mut Ui,
    label: &str,
    icon: Option<Icon>,
    enabled: bool,
    selected: Option<bool>,
    primary: bool,
) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        let key_label = super::player_keys::label(ui.ctx(), label);
        let label = key_label.as_ref();
        let size = if primary { PLAY_WIDTH } else { BUTTON_WIDTH };
        let (_, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
        response.widget_info(|| match selected {
            Some(selected) => {
                egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, label)
            }
            None => egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label),
        });
        paint_action(
            ui,
            &response,
            icon,
            if primary { 23. } else { 19. },
            selected == Some(true),
        );
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(label)
    })
    .inner
}

pub(super) fn paint_action(
    ui: &Ui,
    response: &Response,
    icon: Option<Icon>,
    icon_size: f32,
    selected: bool,
) {
    let rect = response.rect;
    let painter = ui.painter();
    let face = rect.shrink(if response.is_pointer_button_down_on() {
        3.0
    } else {
        1.5
    });
    if selected || response.hovered() || response.has_focus() {
        let fill = Color32::from_white_alpha(if response.hovered() {
            49
        } else if response.has_focus() {
            34
        } else {
            24
        });
        let edge = Color32::from_white_alpha(if response.has_focus() { 233 } else { 0 });
        painter.rect(
            face,
            CornerRadius::same((face.height() * 0.5) as u8),
            fill,
            Stroke::new(1.0, edge),
            StrokeKind::Inside,
        );
    }
    let foreground = if selected { ACCENT } else { Color32::WHITE };
    if let Some(icon) = icon {
        icon.paint(painter, face.center(), icon_size, foreground);
    } else {
        painter.text(
            face.center(),
            Align2::CENTER_CENTER,
            "CC",
            theme::font(12.0),
            foreground,
        );
    }
}

pub(super) fn time(ui: &Ui, position: Pos2, alignment: Align2, label: &str) {
    ui.painter().text(
        position,
        alignment,
        label,
        theme::timestamp_font(12.0),
        MUTED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_and_next_share_the_controller_hover_highlight() {
        for style in crate::player::options::InterfaceTheme::ALL {
            for (label, icon, primary) in [("Play", Icon::Play, true), ("Next", Icon::Next, false)]
            {
                let ctx = Context::default();
                theme::apply_style(&ctx, style, crate::player::options::DEFAULT_GLASS_STRENGTH);
                let mut center = Pos2::ZERO;
                ctx.run_ui(Default::default(), |ui| {
                    center = button(ui, label, Some(icon), true, None, primary)
                        .rect
                        .center();
                })
                .drop_without_applying_deltas();
                let frame = ctx.run_ui(
                    egui::RawInput {
                        events: vec![egui::Event::PointerMoved(center)],
                        ..Default::default()
                    },
                    |ui| {
                        assert!(button(ui, label, Some(icon), true, None, primary).hovered());
                    },
                );
                assert!(frame.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Rect(rect) if rect.rect.center() == center && rect.fill == Color32::from_white_alpha(49))));
                frame.drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn edge_progress_bars_fit_short_players_without_covering_transport() {
        let player = Rect::from_min_size(pos2(25.0, 70.0), vec2(550.0, MIN_PLAYER_HEIGHT));
        for fullscreen in [false, true] {
            for layout in ControlBarLayout::ALL {
                let top = Bar::new(player, layout, ProgressBarPosition::Top, fullscreen);
                let top_strip = top.detached_timeline.unwrap();
                assert!(player.contains_rect(top_strip));
                assert!(top_strip.bottom() < top.rect.top());
                let bottom = Bar::new(player, layout, ProgressBarPosition::Bottom, fullscreen);
                let bottom_strip = bottom.detached_timeline.unwrap();
                assert!(player.contains_rect(bottom_strip));
                assert!(bottom.rect.bottom() < bottom_strip.top());
                assert!(player.contains_rect(bottom.rect));
            }
        }
    }

    #[test]
    fn edge_layouts_share_one_row_and_leave_room_for_the_seek_control() {
        for width in [550.0, 700.0, 1920.0] {
            let player = Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
            for layout in [ControlBarLayout::Top, ControlBarLayout::Docked] {
                let bar = Bar::new(player, layout, ProgressBarPosition::FollowLayout, false);
                assert_eq!(bar.row().center().y, bar.timeline().center().y);
                assert!(bar.timeline().left() >= bar.row().left() + TRANSPORT_WIDTH);
                assert!(bar.timeline().width() >= 100.0);
                assert!(bar.timeline().right() < bar.volume_x());
            }
            for position in ProgressBarPosition::ALL {
                let mut bar = Bar::new(player, ControlBarLayout::Floating, position, false);
                for fraction in [vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.5, 0.2)] {
                    bar.place_floating(fraction);
                    assert!(player.contains_rect(bar.rect));
                    assert!(
                        bar.detached_timeline
                            .is_none_or(|strip| !strip.intersects(bar.rect))
                    );
                }
            }
        }
    }

    #[test]
    fn mini_player_progress_uses_the_selected_layout_and_position() {
        let video = Rect::from_min_size(pos2(40.0, 50.0), vec2(340.0, 191.0));
        for layout in ControlBarLayout::ALL {
            for position in ProgressBarPosition::ALL {
                let strip = mini_timeline(video, layout, position);
                let top = position == ProgressBarPosition::Top
                    || (layout == ControlBarLayout::Top
                        && position == ProgressBarPosition::FollowLayout);
                assert!(video.contains_rect(strip));
                assert_eq!(strip.center().y < video.center().y, top);
                assert_eq!(
                    timeline_glass(layout, DisplayMaterial::Glass, strip, 1.0).is_some(),
                    layout != ControlBarLayout::Classic
                );
                for material in [DisplayMaterial::Opaque, DisplayMaterial::HighContrast] {
                    assert!(timeline_glass(layout, material, strip, 1.0).is_none());
                }
            }
        }
    }
}
