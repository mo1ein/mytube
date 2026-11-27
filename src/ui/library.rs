use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use super::theme::{self, Icon};
use crate::{
    model::{Filter, PAGE, Video},
    service::{Search, SearchPathStatus, Service},
};

struct Page {
    epoch: u64,
    rows: Vec<Video>,
    touched: u64,
}

#[derive(Default)]
pub struct Browser {
    pages: HashMap<usize, Page>,
    recent_views: VecDeque<(Filter, usize, Page)>,
    total: Option<usize>,
    request: u64,
    inflight: bool,
    epoch: u64,
    counted: u64,
    touched: u64,
    target: Range<usize>,
    filter: Filter,
    reset_scroll: bool,
    path_status: Option<SearchPathStatus>,
    refresh_at: Option<Instant>,
    failed: bool,
}

impl Browser {
    pub fn total(&self) -> Option<usize> {
        self.total
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    pub fn set_filter(&mut self, filter: Filter) {
        if self.filter != filter {
            if self.filter.text.is_empty()
                && self.refresh_at.is_none()
                && self.counted == self.epoch
                && let Some(total) = self.total
                && let Some(page) = self.pages.remove(&0)
                && page.epoch == self.epoch
            {
                if self.recent_views.len() == 6 {
                    self.recent_views.pop_front();
                }
                self.recent_views
                    .push_back((self.filter.clone(), total, page));
            }
            self.filter = filter;
            self.pages.clear();
            self.total = None;
            self.target = 0..1;
            self.reset_scroll = true;
            self.inflight = false;
            self.request += 1;
            self.epoch += 1;
            self.failed = false;
            self.path_status = None;
            if let Some(index) = self
                .recent_views
                .iter()
                .position(|(f, _, _)| f == &self.filter)
                && let Some((_, total, mut first_page)) = self.recent_views.remove(index)
            {
                self.total = Some(total);
                self.counted = self.epoch;
                first_page.epoch = self.epoch;
                self.pages.insert(0, first_page);
            }
        }
    }

    pub fn set_path_status(&mut self, request: u64, status: SearchPathStatus) {
        if request == self.request && self.total == Some(0) {
            self.path_status = Some(status);
        }
    }

    pub fn path_status(&self) -> Option<SearchPathStatus> {
        self.path_status
    }

    pub fn loaded(&mut self, page: crate::catalog::SearchPage) {
        self.epoch += 1;
        self.target = 0..1;
        if self.filter == Filter::default() {
            self.applied(self.request, page.index, page.total, page.rows, true);
        }
    }

    pub fn watch_later_changed(&mut self) {
        self.recent_views
            .retain(|(filter, _, _)| !filter.watch_later);
        if self.filter.watch_later {
            self.changed();
        }
    }

    pub fn changed(&mut self) {
        self.recent_views.clear();
        self.refresh_at
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(280));
    }

    pub fn remove_missing(&mut self, ids: &[i64]) {
        self.pages
            .retain(|_, page| !page.rows.iter().any(|video| ids.contains(&video.id)));
        self.refresh_now();
    }

    pub fn refresh_now(&mut self) {
        self.recent_views.clear();
        self.request += 1;
        self.inflight = false;
        self.refresh_at = Some(Instant::now());
    }

    pub fn applied(
        &mut self,
        request: u64,
        index: usize,
        total: Option<usize>,
        rows: Vec<Video>,
        replace_pages: bool,
    ) -> bool {
        if self.request != request {
            return false;
        }
        if replace_pages {
            self.pages.clear();
        }
        self.inflight = false;
        if let Some(total) = total {
            self.total = Some(total);
            self.path_status = None;
            self.counted = self.epoch;
        }
        self.touched += 1;
        self.pages.insert(
            index,
            Page {
                epoch: self.epoch,
                rows,
                touched: self.touched,
            },
        );
        if self.pages.len() > 12 {
            let farthest = self
                .pages
                .iter()
                .filter(|(page, _)| **page != index)
                .min_by_key(|(_, page)| page.touched)
                .map(|(&index, _)| index);
            if let Some(remove) = farthest {
                self.pages.remove(&remove);
            }
        }
        true
    }

    pub fn fail(&mut self, request: u64) -> bool {
        if self.request != request {
            return false;
        }
        self.inflight = false;
        self.failed = true;
        true
    }

    pub fn update(&mut self, ctx: &egui::Context, svc: &Service, anchor: Option<i64>) -> bool {
        let now = Instant::now();
        let mut updated = false;
        if let Some(refresh) = self.refresh_at {
            if refresh <= now && !self.inflight {
                self.refresh_at = None;
                self.epoch += 1;
                self.failed = false;
                updated = true;
            } else if refresh > now {
                ctx.request_repaint_after(refresh - now);
            }
        }
        if self.inflight || self.failed {
            return updated;
        }
        let count = self.counted != self.epoch || self.total.is_none();
        let first = self.target.start / PAGE;
        let last = self.target.end.saturating_sub(1) / PAGE;
        let page_count = self.total.unwrap_or(1).div_ceil(PAGE).max(1);
        let stale = |index: &usize| {
            self.pages
                .get(index)
                .is_none_or(|page| page.epoch != self.epoch)
        };
        let page = if count {
            Some(first.min(page_count - 1))
        } else {
            (first..=last)
                .chain(last.saturating_add(1)..=last.saturating_add(1))
                .chain(first.checked_sub(1))
                .filter(|&index| index < page_count)
                .find(stale)
        };
        if let Some(index) = page {
            self.request += 1;
            self.inflight = true;
            svc.search(Search {
                id: self.request,
                filter: self.filter.clone(),
                index,
                count,
                anchor,
            });
        }
        updated
    }

    pub fn set_target(&mut self, range: Range<usize>) {
        if self.target != range {
            self.touched += 1;
            for page in range.start / PAGE..=range.end.saturating_sub(1) / PAGE {
                if let Some(page) = self.pages.get_mut(&page) {
                    page.touched = self.touched;
                }
            }
            self.target = range;
        }
    }

    pub fn viewport(&self) -> Range<usize> {
        self.target.clone()
    }

    pub fn keep_loaded(&mut self, id: i64) {
        if let Some(index) = self.position(id)
            && let Some(page) = self.pages.get_mut(&(index / PAGE))
        {
            self.touched += 1;
            page.touched = self.touched;
        }
    }

    pub fn position(&self, id: i64) -> Option<usize> {
        // A just-refreshed page takes precedence if a scan has shifted a row.
        self.pages
            .iter()
            .filter_map(|(index, page)| {
                page.rows
                    .iter()
                    .position(|video| video.id == id)
                    .map(|offset| (page.epoch, page.touched, index * PAGE + offset))
            })
            .max()
            .map(|(_, _, position)| position)
    }

    pub fn get(&self, position: usize) -> Option<&Video> {
        self.pages
            .get(&(position / PAGE))
            .and_then(|page| page.rows.get(position % PAGE))
    }

    pub fn loaded_range(&self, first: i64, last: i64) -> Option<Vec<i64>> {
        if self.refresh_at.is_some() || self.counted != self.epoch {
            return None;
        }
        let a = self.position(first)?;
        let b = self.position(last)?;
        (a.min(b)..=a.max(b))
            .map(|index| {
                let page = self.pages.get(&(index / PAGE))?;
                if page.epoch != self.epoch {
                    return None;
                }
                page.rows.get(index % PAGE).map(|video| video.id)
            })
            .collect()
    }

    pub fn neighbor(&self, position: usize, excluded: &[i64]) -> Option<(usize, i64)> {
        let total = self.total?;
        let excluded: HashSet<_> = excluded.iter().copied().collect();
        self.pages
            .iter()
            .flat_map(|(&page, items)| {
                items
                    .rows
                    .iter()
                    .enumerate()
                    .map(move |(offset, video)| (page * PAGE + offset, video.id))
            })
            .filter(|(index, id)| *index < total && !excluded.contains(id))
            .min_by_key(|(index, _)| (index.abs_diff(position), *index < position))
    }

    pub fn suggestions(&self) -> Vec<String> {
        let query = self.filter.text.trim().to_lowercase();
        if query.is_empty() || self.path_status.is_some() {
            return Vec::new();
        }
        let Some(page) = self.pages.get(&0) else {
            return Vec::new();
        };
        let terms: Vec<_> = query.split_whitespace().collect();
        let mut titles: Vec<_> = page
            .rows
            .iter()
            .filter(|video| video.title.chars().count() <= 240)
            .map(|video| (video.title.to_lowercase(), &video.title))
            .filter(|(title, _)| title != &query && terms.iter().all(|term| title.contains(term)))
            .collect();
        titles.sort_by(|(left, _), (right, _)| {
            (!left.starts_with(&query), !left.contains(&query), left).cmp(&(
                !right.starts_with(&query),
                !right.contains(&query),
                right,
            ))
        });
        titles.dedup_by(|(left, _), (right, _)| left == right);
        titles
            .into_iter()
            .take(6)
            .map(|(_, title)| title.clone())
            .collect()
    }

    pub fn take_scroll_reset(&mut self) -> bool {
        std::mem::take(&mut self.reset_scroll)
    }

    pub fn failed(&self) -> bool {
        self.failed
    }
}

#[derive(Default)]
pub struct Thumbnails {
    pub(super) hover: super::preview::HoverPreview,
    images: HashMap<(i64, i64), (u64, Option<TextureHandle>)>,
    clock: u64,
    wanted: Vec<(i64, i64)>,
    visible: Vec<Video>,
}

impl Thumbnails {
    pub fn begin(&mut self) {
        self.clock += 1;
        self.visible.clear();
    }

    pub fn find(&mut self, video: &Video) -> Option<&TextureHandle> {
        if let Some((clock, image)) = self.images.get_mut(&video.thumbnail_key()) {
            *clock = self.clock;
            return image.as_ref();
        }
        if !self.visible.iter().any(|item| item.id == video.id) {
            self.visible.push(video.clone());
        }
        None
    }

    pub(super) fn preview_artwork(
        &mut self,
        ui: &Ui,
        response: &Response,
        rect: Rect,
        video: &Video,
    ) -> bool {
        let image = self.find(video).cloned();
        let frame = self.hover.frame(response, rect, video);
        artwork(
            ui.painter(),
            rect,
            video,
            image.as_ref(),
            response.hovered() || response.has_focus(),
            frame,
        );
        self.hover
            .draw_timeline(ui, response, rect, video, frame, image.as_ref())
    }

    pub fn flush(&mut self, svc: &Service) {
        let keys: Vec<_> = self.visible.iter().map(Video::thumbnail_key).collect();
        if self.wanted != keys {
            self.wanted = keys;
            svc.thumbnails(std::mem::take(&mut self.visible));
        }
    }

    pub fn load(&mut self, ctx: &egui::Context, key: (i64, i64), image: Option<egui::ColorImage>) {
        let texture = image.map(|image| {
            ctx.load_texture(
                format!("video-{}-{}", key.0, key.1),
                image,
                TextureOptions::LINEAR,
            )
        });
        self.images.insert(key, (self.clock, texture));
        if self.images.len() > 144 {
            let victim = self
                .images
                .iter()
                .filter(|(other, _)| **other != key)
                .min_by_key(|(_, (clock, _))| *clock)
                .map(|(&key, _)| key);
            if let Some(victim) = victim {
                self.images.remove(&victim);
            }
        }
    }
}

pub const CARD_METADATA_HEIGHT: f32 = 66.0;

pub(super) fn selection_color(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(139, 186, 255)
    } else {
        Color32::from_rgb(42, 100, 178)
    }
}

fn item_surface(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    radius: u8,
    base: Color32,
    selected: bool,
    highlight: f32,
) {
    let colors = theme::palette(ui);
    let mut hover = theme::control_fill(ui, colors.hover);
    if theme::interface_theme(ui.ctx()).translucent() && !theme::solid_glass(ui.ctx()) {
        // Keep feedback legible as the native material shows more of the desktop.
        hover = hover.blend(
            colors
                .text
                .gamma_multiply(0.10 * theme::glass_amount(ui.ctx())),
        );
    }
    let mut fill = base.lerp_to_gamma(hover, highlight);
    let stroke = if selected {
        let accent = selection_color(ui.visuals());
        fill = fill.blend(accent.gamma_multiply(0.15));
        Stroke::new(1., accent.gamma_multiply(0.74))
    } else {
        Stroke::NONE
    };
    ui.painter()
        .rect(rect, radius, fill, stroke, StrokeKind::Inside);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.5),
            radius + 1,
            Stroke::new(1.5, colors.muted),
            StrokeKind::Outside,
        );
    }
}

pub fn card(
    ui: &mut Ui,
    video: &Video,
    selected: bool,
    size: Vec2,
    thumbs: &mut Thumbnails,
) -> (Response, bool, bool) {
    let colors = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("video_card", video.id)),
        Sense::click(),
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, &video.title));
    let painter = ui.painter();
    item_surface(
        ui,
        &response,
        rect.expand(3.),
        14,
        Color32::TRANSPARENT,
        selected,
        thumbs.hover.highlight(&response),
    );
    let cover = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width() * 9.0 / 16.0));
    let over_timeline = thumbs.preview_artwork(ui, &response, cover, video);
    let footer = cover.max.y + 8.0;
    let left = rect.min.x + 2.0;
    theme::cut(
        painter,
        pos2(left, footer),
        rect.width() - 4.0,
        &video.title,
        theme::font(14.0),
        colors.text,
    );
    let location = video
        .path
        .parent()
        .map_or_else(|| "Unknown folder".into(), folder_label);
    let date = catalog_date(video);
    let format = video.ext.to_uppercase();
    let resolution = resolution(video);
    let time = duration(video.media.duration);
    let size = bytes(video.stamp.bytes);
    let full_meta = format!("{time}  ·  {size}  ·  {resolution}  ·  {format}");
    let short_meta = format!("{time}  ·  {size}");
    let font = FontId::proportional(12.0);
    let width = rect.width() - 4.0;
    let meta = if painter
        .layout_no_wrap(full_meta.clone(), font.clone(), colors.muted)
        .size()
        .x
        <= width
    {
        full_meta
    } else {
        short_meta
    };
    theme::cut(
        painter,
        pos2(left, footer + 20.0),
        width,
        &meta,
        font.clone(),
        colors.muted,
    );

    let (folder_width, date_label) = if let Some((_, label)) = &date {
        let galley = painter.layout_no_wrap(label.clone(), font.clone(), colors.muted);
        (width - galley.size().x - 8.0, Some(galley))
    } else {
        (width, None)
    };
    theme::cut(
        painter,
        pos2(left, footer + 37.0),
        folder_width,
        &location,
        font,
        colors.muted,
    );
    if let Some(galley) = date_label {
        painter.galley(
            pos2(rect.right() - 2.0 - galley.size().x, footer + 37.0),
            galley,
            colors.muted,
        );
    }
    let play = response.clicked()
        && !over_timeline
        && response
            .interact_pointer_pos()
            .is_some_and(|point| cover.contains(point));
    let response = response.on_hover_cursor(if over_timeline {
        egui::CursorIcon::Crosshair
    } else {
        egui::CursorIcon::PointingHand
    });
    let response = if super::preview::over_artwork(&response, cover) {
        response
    } else {
        response.on_hover_text(video_tooltip(video, date.as_ref()))
    };
    (response, play, over_timeline)
}

pub fn watch_later_button(
    ui: &mut Ui,
    card: &Response,
    list: bool,
    saved: bool,
    label: &str,
) -> bool {
    let rect = card.rect;
    let right = if list {
        rect.left() + 7.0 + ((rect.height() - 12.0) * 16.0 / 9.0).min(rect.width() * 0.42)
    } else {
        rect.right()
    };
    let center = pos2(right - 18.0, rect.top() + if list { 22.0 } else { 19.0 });
    let hit = Rect::from_center_size(center, vec2(30.0, 30.0));
    let button = ui.interact(hit, card.id.with("watch_later"), Sense::click());
    // The clock sits on video artwork and needs the same contrast in either theme.
    let ink = if saved {
        Color32::from_gray(18)
    } else {
        Color32::WHITE
    };
    button.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), saved, label));
    if saved
        || card.contains_pointer()
        || card.has_focus()
        || button.hovered()
        || button.has_focus()
    {
        let fill = if saved {
            Color32::WHITE
        } else {
            Color32::from_black_alpha(220)
        };
        ui.painter().circle_filled(center, 13.0, fill);
        if button.hovered() || button.has_focus() {
            ui.painter()
                .circle_stroke(center, 13.0, Stroke::new(1.0, Color32::WHITE));
        }
        Icon::Clock.paint(ui.painter(), center, 17.0, ink);
    }
    button.on_hover_text(label).clicked()
}

pub fn list_item(
    ui: &mut Ui,
    video: &Video,
    selected: bool,
    height: f32,
    thumbs: &mut Thumbnails,
) -> (Response, bool, bool) {
    let colors = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("video_row", video.id)),
        Sense::click(),
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, &video.title));
    let painter = ui.painter();
    item_surface(
        ui,
        &response,
        rect,
        10,
        theme::control_fill(ui, colors.card),
        selected,
        thumbs.hover.highlight(&response),
    );
    let cover_width = ((height - 12.0) * 16.0 / 9.0).min(rect.width() * 0.42);
    let cover = Rect::from_min_size(
        pos2(
            rect.left() + 7.0,
            rect.center().y - cover_width * 9.0 / 32.0,
        ),
        vec2(cover_width, cover_width * 9.0 / 16.0),
    );
    let over_timeline = thumbs.preview_artwork(ui, &response, cover, video);
    let left = cover.max.x + 11.0;
    let columns = if rect.width() > 600.0 { 310.0 } else { 0.0 };
    let title_width = rect.max.x - left - 12.0 - columns;
    theme::cut(
        painter,
        pos2(left, rect.center().y - 19.0),
        title_width,
        &video.title,
        theme::font(14.0),
        colors.text,
    );
    let location = video
        .path
        .parent()
        .map_or_else(String::new, |path| path.display().to_string());
    let date = catalog_date(video);
    let date_text = date.as_ref().map_or("—", |(_, date)| date);
    let line = if columns == 0.0 {
        format!(
            "{date_text}  ·  {}  ·  {}  ·  {}  ·  {location}",
            video.ext.to_uppercase(),
            resolution(video),
            bytes(video.stamp.bytes)
        )
    } else {
        location
    };
    theme::cut(
        painter,
        pos2(left, rect.center().y + 3.0),
        title_width,
        &line,
        FontId::proportional(12.0),
        colors.muted,
    );
    if columns > 0.0 {
        for (offset, label) in [
            (269.0, date_text.to_owned()),
            (194.0, video.ext.to_uppercase()),
            (121.0, resolution(video)),
            (37.0, bytes(video.stamp.bytes)),
        ] {
            theme::centered_text(
                painter,
                pos2(rect.right() - offset, rect.center().y),
                &label,
                12.0,
                colors.muted,
            );
        }
    }
    let play = response.clicked()
        && !over_timeline
        && response
            .interact_pointer_pos()
            .is_some_and(|point| cover.contains(point));
    let response = response.on_hover_cursor(if over_timeline {
        egui::CursorIcon::Crosshair
    } else {
        egui::CursorIcon::PointingHand
    });
    let response = if super::preview::over_artwork(&response, cover) {
        response
    } else {
        response.on_hover_text(video_tooltip(video, date.as_ref()))
    };
    (response, play, over_timeline)
}

pub(super) fn created_date(video: &Video) -> Option<String> {
    video.created.map(format_file_date)
}

fn catalog_date(video: &Video) -> Option<(&'static str, String)> {
    if let Some(date) = created_date(video) {
        return Some(("File created", date));
    }
    // A zero modification timestamp represents unavailable filesystem metadata.
    (video.stamp.modified != 0).then(|| ("File modified", format_file_date(video.stamp.modified)))
}

fn format_file_date(nanos: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(nanos)
        .with_timezone(&chrono::Local)
        .format("%Y.%m.%d")
        .to_string()
}

fn video_tooltip(video: &Video, date: Option<&(&str, String)>) -> String {
    let date = date.map_or_else(
        || "File date unavailable".into(),
        |(source, date)| format!("{source} {date}"),
    );
    format!("{}\n{date}", video.path.display())
}

pub fn artwork(
    painter: &egui::Painter,
    rect: Rect,
    video: &Video,
    texture: Option<&TextureHandle>,
    hot: bool,
    preview: Option<super::preview::Frame>,
) {
    let colors = theme::colors(painter.ctx(), &painter.ctx().global_style().visuals);
    if let Some(frame) = preview {
        painter.add(
            egui::epaint::RectShape::filled(rect, 11, Color32::WHITE)
                .with_texture(frame.id, frame.uv),
        );
    } else if let Some(texture) = texture {
        let dims = texture.size_vec2();
        let image_ratio = dims.x / dims.y;
        let view_ratio = rect.width() / rect.height();
        let uv = if image_ratio < view_ratio {
            if image_ratio < 0.9 {
                painter.rect_filled(rect, 11, colors.well);
                let width = rect.height() * image_ratio;
                let centered = Rect::from_center_size(rect.center(), vec2(width, rect.height()));
                painter.image(
                    texture.id(),
                    centered,
                    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                None
            } else {
                let height = image_ratio / view_ratio;
                Some(Rect::from_center_size(pos2(0.5, 0.5), vec2(1.0, height)))
            }
        } else {
            Some(Rect::from_center_size(
                pos2(0.5, 0.5),
                vec2(view_ratio / image_ratio, 1.0),
            ))
        };
        if let Some(uv) = uv {
            painter.add(
                egui::epaint::RectShape::filled(rect, 11, Color32::WHITE)
                    .with_texture(texture.id(), uv),
            );
        }
    } else {
        painter.rect_filled(rect, 11, colors.well);
        Icon::Film.paint(
            painter,
            rect.center(),
            if rect.width() < 110.0 { 18.0 } else { 34.0 },
            colors.muted.gamma_multiply(0.8),
        );
    }
    if hot && preview.is_none() {
        let radius = if rect.width() < 110.0 { 14.0 } else { 22.0 };
        if theme::interface_theme(painter.ctx()).glass_ui() {
            theme::media_badge(
                painter,
                Rect::from_center_size(rect.center(), Vec2::splat(2. * radius)),
                Icon::Play,
            );
        } else {
            painter.circle_filled(rect.center(), radius, Color32::from_black_alpha(192));
            Icon::Play.paint(painter, rect.center(), radius, Color32::WHITE);
        }
    }
    if video.media.duration.is_some() {
        let time = duration(video.media.duration);
        let font = theme::timestamp_font(if rect.width() < 110.0 { 9.0 } else { 11.0 });
        let galley = painter.layout_no_wrap(time, font, Color32::WHITE);
        let size = galley.size() + vec2(9.0, 4.0);
        let badge = Rect::from_min_size(rect.right_bottom() - size - vec2(4.0, 4.0), size);
        painter.rect_filled(badge, 4, Color32::from_black_alpha(224));
        painter.galley(badge.center() - galley.size() * 0.5, galley, Color32::WHITE);
    }
}

pub fn placeholder(ui: &mut Ui, size: Vec2) {
    let colors = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(size, Sense::CLICK);
    ui.painter().rect_filled(rect, 8, colors.side);
    ui.painter().rect_filled(
        Rect::from_min_size(
            rect.min + vec2(8.0, 8.0),
            vec2(rect.width() - 16.0, (rect.height() - 16.0) * 0.66),
        ),
        5,
        colors.well,
    );
}

pub fn duration(value: Option<f64>) -> String {
    let Some(value) = value else {
        return "—".into();
    };
    let value = value.round() as u64;
    if value >= 3600 {
        format!("{}:{:02}:{:02}", value / 3600, value / 60 % 60, value % 60)
    } else {
        format!("{}:{:02}", value / 60, value % 60)
    }
}

pub fn resolution(video: &Video) -> String {
    match (video.media.width, video.media.height) {
        (Some(w), Some(h)) if w.min(h) >= 2160 => "4K+".into(),
        (Some(w), Some(h)) => format!("{}p", w.min(h)),
        _ => "—".into(),
    }
}

pub fn bytes(size: i64) -> String {
    if size >= 1_000_000_000 {
        format!("{:.2} GB", size as f64 / 1_000_000_000.0)
    } else if size >= 1_000_000 {
        format!("{:.1} MB", size as f64 / 1_000_000.0)
    } else if size >= 1_000 {
        format!("{:.1} KB", size as f64 / 1_000.0)
    } else {
        format!("{size} B")
    }
}

pub fn folder_label(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

pub fn empty_illustration(ui: &mut Ui, scale: f32) {
    let colors = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(204.0, 120.0) * scale, Sense::hover());
    let p = ui.painter();
    let center = rect.center();
    p.circle_filled(center, 51.0 * scale, colors.active);
    p.circle_stroke(
        center,
        55.0,
        Stroke::new(1.0, colors.active.lerp_to_gamma(colors.accent, 0.23)),
    );
    p.circle_stroke(center, 63.0 * scale, Stroke::new(1.0, colors.line));
    let back = Rect::from_center_size(center + vec2(10.0, -8.0) * scale, vec2(96.0, 57.0) * scale);
    p.rect(
        back,
        8,
        colors.well,
        Stroke::new(1.0, colors.line),
        StrokeKind::Inside,
    );
    let front = Rect::from_center_size(center + vec2(-4.0, 3.0) * scale, vec2(97.0, 61.0) * scale);
    p.rect(
        front,
        8,
        colors.active,
        Stroke::new(1.4, colors.accent),
        StrokeKind::Inside,
    );
    for x in [front.left() + 11.0 * scale, front.right() - 11.0 * scale] {
        for y in [-17.0, 0.0, 17.0] {
            p.rect_filled(
                Rect::from_center_size(
                    pos2(x, front.center().y + y * scale),
                    vec2(4.0, 6.0) * scale,
                ),
                1,
                colors.accent.gamma_multiply(0.65),
            );
        }
    }
    Icon::Play.paint(p, front.center(), 34.0 * scale, colors.accent);
    for (offset, radius) in [
        (vec2(-79.0, -31.0) * scale, 3.0),
        (vec2(83.0, 30.0) * scale, 2.0),
    ] {
        p.circle_filled(center + offset, radius, colors.accent.gamma_multiply(0.8));
    }
    p.text(
        center + vec2(74.0, -33.0) * scale,
        Align2::CENTER_CENTER,
        "+",
        FontId::proportional(17.0),
        colors.muted,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Stamp;
    use chrono::{Local, TimeZone};

    #[test]
    fn catalogue_hover_animates_grid_and_list_previews_in_every_appearance() {
        use crate::model::Appearance;
        use crate::player::options::InterfaceTheme;

        for style in InterfaceTheme::ALL {
            for appearance in [Appearance::Light, Appearance::Dark] {
                for list in [false, true] {
                    let ctx = egui::Context::default();
                    theme::install(&ctx);
                    theme::apply(&ctx, appearance);
                    theme::apply_style(&ctx, style, 100);
                    let mut thumbs = Thumbnails::default();
                    let video = super::super::tests::catalog_video(7);
                    let pointer = if list {
                        pos2(54., 36.)
                    } else {
                        pos2(100., 50.)
                    };
                    let draw = |thumbs: &mut Thumbnails,
                                time,
                                pos: Option<Pos2>,
                                reduced,
                                selected| {
                        let input = egui::RawInput {
                            time: Some(time),
                            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(520., 240.))),
                            events: pos.into_iter().map(egui::Event::PointerMoved).collect(),
                            ..Default::default()
                        };
                        let mut surface = Rect::NOTHING;
                        let output = ctx.run_ui(input, |ui| {
                            thumbs.hover.begin(true, reduced);
                            surface = if list {
                                list_item(ui, &video, selected, 72., thumbs).0.rect
                            } else {
                                card(ui, &video, selected, vec2(200., 180.), thumbs)
                                    .0
                                    .rect
                                    .expand(3.)
                            };
                        });
                        let mut frame = None;
                        let mut play_visible = false;
                        let mut fill = Color32::TRANSPARENT;
                        let mut ring = Stroke::NONE;
                        for shape in &output.shapes {
                            if let egui::Shape::Rect(rect) = &shape.shape {
                                if let Some(brush) = &rect.brush {
                                    frame = Some(brush.uv);
                                }
                                if rect.rect == surface {
                                    fill = rect.fill;
                                    ring = rect.stroke;
                                }
                            } else if let egui::Shape::Path(path) = &shape.shape
                                && path.points.len() == 3
                                && path.fill == Color32::WHITE
                                && surface.contains_rect(shape.shape.visual_bounding_rect())
                            {
                                play_visible = true;
                            }
                        }
                        output.drop_without_applying_deltas();
                        (frame, fill, ring, play_visible)
                    };
                    let idle = draw(&mut thumbs, 0., Some(pos2(500., 220.)), false, false);
                    assert!(!idle.3);
                    let loading = draw(&mut thumbs, 0.05, Some(pointer), false, false);
                    assert!(loading.0.is_none() && loading.3);
                    thumbs.hover.load(
                        &ctx,
                        video.thumbnail_key(),
                        0,
                        Some(egui::ColorImage::filled([80, 40], Color32::WHITE)),
                    );
                    let first = draw(&mut thumbs, 0.1, None, false, true);
                    let later = draw(&mut thumbs, 0.41, None, false, false);
                    assert_ne!(first.0.unwrap(), later.0.unwrap());
                    assert!(
                        !first.3 && !later.3,
                        "Play covers the {style:?} preview (list={list})"
                    );
                    assert_ne!(later.1, idle.1);
                    assert!(!first.2.is_empty() && later.2.is_empty());
                    let reduced = draw(&mut thumbs, 0.5, None, true, false);
                    let later = draw(&mut thumbs, 0.8, None, true, false);
                    assert_ne!(reduced.0.unwrap(), later.0.unwrap());
                    assert!(!reduced.3 && !later.3);
                    assert!(
                        draw(&mut thumbs, 1., Some(pos2(500., 220.)), true, false)
                            .0
                            .is_none()
                    );
                }
            }
        }
    }

    #[test]
    fn shift_selection_uses_only_complete_current_pages() {
        let mut browser = Browser::default();
        let video = super::super::tests::catalog_video;
        browser.applied(0, 0, Some(3), vec![video(3), video(1), video(2)], false);
        assert_eq!(browser.loaded_range(2, 3), Some(vec![3, 1, 2]));
        assert_eq!(browser.loaded_range(3, 9), None);
        browser.refresh_now();
        assert_eq!(browser.loaded_range(3, 2), None);
    }

    #[test]
    fn revisiting_a_library_view_has_rows_in_the_same_frame_until_the_catalog_changes() {
        let mut browser = Browser::default();
        let saved = Filter {
            watch_later: true,
            ..Default::default()
        };
        let video = super::super::tests::catalog_video;
        assert!(browser.applied(0, 0, Some(2), vec![video(1), video(2)], false));
        browser.set_filter(saved.clone());
        assert!(browser.total().is_none());
        let request = browser.request;
        assert!(browser.applied(request, 0, Some(1), vec![video(2)], false));
        browser.set_filter(Filter::default());
        assert_eq!(browser.total(), Some(2));
        assert_eq!(browser.get(0).unwrap().id, 1);
        assert!(!browser.applied(request, 0, Some(0), vec![], false));
        browser.set_filter(saved.clone());
        assert_eq!(browser.get(0).unwrap().id, 2);
        browser.set_filter(Filter::default());
        browser.watch_later_changed();
        browser.set_filter(saved);
        assert!(browser.total().is_none());
        browser.changed();
        browser.set_filter(Filter::default());
        assert!(browser.get(0).is_none());
    }

    #[test]
    fn catalog_date_uses_modified_when_creation_time_is_missing() {
        let timestamp = |day| {
            Local
                .with_ymd_and_hms(2024, 5, day, 12, 0, 0)
                .earliest()
                .unwrap()
                .timestamp_nanos_opt()
                .unwrap()
        };
        let mut video = Video {
            id: 1,
            rev: 1,
            path: "video.mp4".into(),
            title: "video".into(),
            ext: "mp4".into(),
            stamp: Stamp {
                bytes: 1,
                modified: timestamp(6),
            },
            added: 0,
            created: Some(timestamp(2)),
            media: Default::default(),
        };
        assert_eq!(
            catalog_date(&video),
            Some(("File created", "2024.05.02".into()))
        );
        video.created = None;
        assert_eq!(
            catalog_date(&video),
            Some(("File modified", "2024.05.06".into()))
        );
        video.stamp.modified = 0;
        assert_eq!(catalog_date(&video), None);
    }
}
