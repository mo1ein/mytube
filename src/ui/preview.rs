use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Color32, ColorImage, Context, Id, LayerId, Order, Pos2, Rect, Response, Stroke,
    TextureHandle, TextureId, TextureOptions, Ui, pos2, vec2,
};

use super::{library::duration, theme::Palette};

use crate::{
    media::{PREVIEW_COLUMNS, PREVIEW_FRAME, PREVIEW_ROWS, preview_count, timeline_sample},
    model::Video,
    service::{
        PreviewRequest, Service, TimelineKey, TimelineOrigin, TimelinePrefetch, TimelineRequest,
    },
};

type Key = (i64, i64);
const DWELL: Duration = Duration::from_millis(80);
const CACHE_SIZE: usize = 4;
const FRAME_COUNT: usize = PREVIEW_COLUMNS * PREVIEW_ROWS;

#[derive(Clone, Copy)]
pub struct Frame {
    pub id: TextureId,
    pub uv: Rect,
    pub progress: f32,
}

struct Target {
    key: Key,
    since: Duration,
    animation: Option<Duration>,
    requested: bool,
}

#[derive(Default)]
pub struct HoverPreview {
    target: Option<Target>,
    cache: VecDeque<(Key, Vec<Option<TextureHandle>>)>,
    outgoing: Option<Option<PreviewRequest>>,
    peek: Peek,
    pub(super) timeline_bounds: Option<Rect>,
    warm_requested: Option<Video>,
    warm_seen: bool,
    warmed: Option<Key>,
    enabled: bool,
    reduce_motion: bool,
    seen: bool,
}

impl HoverPreview {
    pub fn begin(&mut self, enabled: bool, reduce_motion: bool) {
        self.enabled = enabled;
        self.reduce_motion = reduce_motion;
        self.seen = false;
        self.timeline_bounds = None;
        self.warm_seen = false;
        self.warm_requested = None;
    }

    pub fn frame(&mut self, response: &Response, cover: Rect, video: &Video) -> Option<Frame> {
        if !self.enabled || !response.hovered() {
            return None;
        }
        if video.media.duration.is_some_and(|end| end > 0.0)
            && response
                .ctx
                .pointer_hover_pos()
                .is_some_and(|point| timeline_band(cover).contains(point))
        {
            return None;
        }
        self.seen = true;
        let now = Duration::from_secs_f64(response.ctx.input(|i| i.time));
        let key = video.thumbnail_key();
        if self.target.as_ref().is_none_or(|target| target.key != key) {
            self.cancel();
            self.target = Some(Target {
                key,
                since: now,
                animation: None,
                requested: false,
            });
        }
        let target = self.target.as_mut()?;
        let elapsed = now.saturating_sub(target.since);
        if elapsed < DWELL
            && !self
                .cache
                .iter()
                .any(|(k, images)| *k == key && images.iter().any(Option::is_some))
        {
            response.ctx.request_repaint_after(DWELL - elapsed);
            return None;
        }

        let segments = self.cache.iter().find(|(cached, _)| *cached == key);
        let first_segment = segments.map_or(0, |(_, images)| {
            images
                .iter()
                .position(Option::is_none)
                .unwrap_or(images.len())
        });
        if !target.requested && first_segment < preview_count(video) {
            self.outgoing = Some(Some(PreviewRequest {
                video: video.clone(),
                first_segment,
            }));
            target.requested = true;
        }
        let images = &segments?.1;
        let count = images.iter().flatten().count() * FRAME_COUNT;
        if count == 0 {
            return None;
        }
        let elapsed = now.saturating_sub(*target.animation.get_or_insert(now));
        // The thumbnail is playing media. Reduce Motion applies to the card highlight.
        response.ctx.request_repaint_after(
            PREVIEW_FRAME
                - Duration::from_millis((elapsed.as_millis() % PREVIEW_FRAME.as_millis()) as u64),
        );
        let frame = (elapsed.as_millis() / PREVIEW_FRAME.as_millis()) as usize % count;
        let atlas = images.iter().flatten().nth(frame / FRAME_COUNT)?;
        let index = frame % FRAME_COUNT;
        let pixels = atlas.size_vec2();
        let x = (index % PREVIEW_COLUMNS) as f32;
        let y = (index / PREVIEW_COLUMNS) as f32;
        let (w, h) = (PREVIEW_COLUMNS as f32, PREVIEW_ROWS as f32);
        Some(Frame {
            id: atlas.id(),
            uv: Rect::from_min_max(
                pos2(x / w + 0.5 / pixels.x, y / h + 0.5 / pixels.y),
                pos2(
                    (x + 1.0) / w - 0.5 / pixels.x,
                    (y + 1.0) / h - 0.5 / pixels.y,
                ),
            ),
            progress: (frame + 1) as f32 / count as f32,
        })
    }

    pub fn draw_timeline(
        &mut self,
        ui: &Ui,
        response: &Response,
        cover: Rect,
        video: &Video,
        preview: Option<Frame>,
        cover_image: Option<&TextureHandle>,
    ) -> bool {
        let colors = Palette::DARK;
        let Some(end) = video.media.duration.filter(|end| *end > 0.0) else {
            return false;
        };
        if !self.enabled || !over_artwork(response, cover) {
            return false;
        }
        self.warm_seen = true;
        if self.warmed != Some(video.thumbnail_key()) {
            self.warm_requested = Some(video.clone());
        }
        let track = timeline_band(cover);
        let pointer = ui
            .ctx()
            .pointer_hover_pos()
            .filter(|point| track.contains(*point));
        let y = track.bottom() - 3.0;
        let left = pos2(track.left(), y);
        let right = pos2(track.right(), y);
        let painter = ui.painter();
        painter.line_segment(
            [left, right],
            Stroke::new(3.0, Color32::from_black_alpha(185)),
        );
        let progress = pointer
            .map(|point| ((point.x - left.x) / track.width()).clamp(0.0, 1.0))
            .or_else(|| preview.map(|frame| frame.progress));
        if let Some(progress) = progress {
            let reached = left + vec2(track.width() * progress, 0.0);
            painter.line_segment([left, reached], Stroke::new(2.5, colors.accent));
            if pointer.is_some() {
                let geometry =
                    card_peek_geometry(ui.ctx().content_rect(), pos2(reached.x, track.top()));
                painter.circle_filled(reached, 4.0, colors.accent);
                let at = end * f64::from(progress);
                self.peek.set_cover(video, cover_image);
                let image = self
                    .peek
                    .update(ui.ctx(), video, at, end, TimelineOrigin::Library);
                self.timeline_bounds = Some(if image.is_some() {
                    geometry.frame.union(geometry.time)
                } else {
                    geometry.time
                });
                paint_peek(
                    &ui.ctx().layer_painter(card_preview_layer()),
                    image.as_ref(),
                    at,
                    geometry,
                );
            }
        }
        pointer.is_some()
    }

    pub fn flush(&mut self, service: &Service) {
        if !self.seen {
            self.cancel();
        }
        if self.timeline_bounds.is_none() {
            self.peek.reset();
        }
        if let Some(video) = self.warm_requested.take() {
            self.warmed = Some(video.thumbnail_key());
            self.peek.clear_prefetched();
            let duration = video.media.duration.unwrap_or_default();
            service.prefetch_timeline(
                TimelineOrigin::Library,
                Some(TimelinePrefetch {
                    video,
                    duration,
                    remote: true,
                    focus: 0.5,
                }),
            );
        } else if !self.warm_seen && self.warmed.take().is_some() {
            self.peek.clear_prefetched();
            service.prefetch_timeline(TimelineOrigin::Library, None);
        }
        if let Some(request) = self.outgoing.take() {
            service.preview(request);
        }
    }

    pub fn timeline_update(&mut self) -> Option<Option<TimelineRequest>> {
        self.peek.take_request()
    }

    pub fn highlight(&self, response: &Response) -> f32 {
        let hovered = self.enabled && response.hovered();
        if self.reduce_motion {
            return f32::from(hovered);
        }
        response.ctx.animate_bool_with_time_and_easing(
            response.id.with("catalog_hover"),
            hovered,
            0.12,
            egui::emath::easing::cubic_out,
        )
    }

    pub fn load(&mut self, ctx: &Context, key: Key, segment: usize, image: Option<ColorImage>) {
        let mut images = self
            .cache
            .iter()
            .position(|(cached, _)| *cached == key)
            .and_then(|index| self.cache.remove(index))
            .map_or_else(Vec::new, |(_, images)| images);
        images.resize_with(images.len().max(segment + 1), || None);
        images[segment] = image.map(|image| {
            ctx.load_texture(
                format!("preview-{}-{}-{segment}", key.0, key.1),
                image,
                TextureOptions::LINEAR,
            )
        });
        self.cache.push_back((key, images));
        if self.cache.len() > CACHE_SIZE {
            self.cache.pop_front();
        }
    }

    pub fn load_timeline(
        &mut self,
        ctx: &Context,
        key: TimelineKey,
        image: Option<ColorImage>,
        exact: bool,
    ) {
        self.peek.load(ctx, key, image, exact);
    }

    pub fn load_prefetched(&mut self, ctx: &Context, key: TimelineKey, image: ColorImage) {
        if self.warmed == Some((key.0, key.1)) {
            self.peek.receive_prefetched(ctx, key, image);
        }
    }

    fn cancel(&mut self) {
        if self.target.take().is_some_and(|target| target.requested) {
            self.outgoing = Some(None);
        }
    }
}

pub fn over_artwork(response: &Response, cover: Rect) -> bool {
    response.hovered()
        && response
            .ctx
            .pointer_hover_pos()
            .is_some_and(|point| cover.contains(point))
}

pub(super) fn card_preview_layer() -> LayerId {
    LayerId::new(Order::Foreground, Id::new("card_timeline_preview"))
}

fn timeline_band(cover: Rect) -> Rect {
    Rect::from_min_max(
        pos2(cover.left() + 4.0, cover.bottom() - 12.0),
        cover.right_bottom() - vec2(4.0, 0.0),
    )
}

struct PreviewTarget {
    key: TimelineKey,
    since: Instant,
    moving: bool,
    sent: bool,
}

#[derive(Default)]
pub(super) struct Peek {
    target: Option<PreviewTarget>,
    cache: VecDeque<(TimelineKey, Option<TextureHandle>)>,
    draft: Option<(TimelineKey, TextureHandle)>,
    cover: Option<(Key, TextureHandle)>,
    last_request: Option<Instant>,
    prefetched: BTreeMap<TimelineKey, TextureHandle>,
    outgoing: Option<Option<TimelineRequest>>,
    shown: bool,
}

pub(super) struct PeekPlacement {
    pub width: f32,
    pub player: Rect,
    pub below: bool,
    pub hint: bool,
}

struct PeekGeometry {
    frame: Rect,
    time: Rect,
    hint: Option<Rect>,
}

impl Peek {
    pub(super) fn reset(&mut self) {
        self.draft = None;
        self.cancel_target();
        self.last_request = None;
    }

    fn cancel_target(&mut self) {
        if self.target.take().is_some() && self.last_request.is_some() {
            self.outgoing = Some(None);
        }
    }

    pub(super) fn take_request(&mut self) -> Option<Option<TimelineRequest>> {
        self.outgoing.take()
    }

    pub(super) fn load(
        &mut self,
        ctx: &Context,
        key: TimelineKey,
        image: Option<ColorImage>,
        exact: bool,
    ) {
        if !exact && self.target.as_ref().is_none_or(|target| target.key != key) {
            return;
        }
        let image = image.map(|image| {
            ctx.load_texture(
                format!("seek-preview-{key:?}"),
                image,
                TextureOptions::LINEAR,
            )
        });
        if !exact {
            self.draft = image.map(|image| (key, image));
            return;
        }
        if image.is_some() && self.draft.as_ref().is_some_and(|(draft, _)| *draft == key) {
            self.draft = None;
        }
        if image.is_none()
            && self
                .cache
                .iter()
                .any(|(cached, image)| *cached == key && image.is_some())
        {
            return;
        }
        self.cache.retain(|(existing, _)| *existing != key);
        self.cache.push_back((key, image));
        if self.cache.len() > 2 * super::precise_seek::MAX_FILMSTRIP_TILES {
            self.cache.pop_front();
        }
    }

    pub(super) fn cached(&self, key: TimelineKey) -> bool {
        self.cache.iter().any(|(cached, _)| *cached == key) || self.prefetched.contains_key(&key)
    }

    pub(super) fn nearest_frame(&self, key: TimelineKey) -> Option<&TextureHandle> {
        self.cache
            .iter()
            .filter_map(|(k, image)| Some((k, image.as_ref()?)))
            .chain(self.draft.iter().map(|(k, image)| (k, image)))
            .chain(self.prefetched.range(..=key).next_back())
            .chain(self.prefetched.range(key..).next())
            .filter(|(k, _)| (k.0, k.1) == (key.0, key.1))
            .min_by_key(|(k, _)| k.2.abs_diff(key.2))
            .map(|(_, image)| image)
            .or_else(|| {
                self.cover
                    .as_ref()
                    .filter(|(k, _)| *k == (key.0, key.1))
                    .map(|(_, image)| image)
            })
    }

    pub(super) fn clear_prefetched(&mut self) {
        self.prefetched.clear();
    }

    pub(super) fn set_cover(&mut self, video: &Video, texture: Option<&TextureHandle>) {
        self.cover = texture.map(|image| (video.thumbnail_key(), image.clone()));
    }

    pub(super) fn receive_prefetched(
        &mut self,
        ctx: &Context,
        key: TimelineKey,
        image: ColorImage,
    ) {
        if self.prefetched.contains_key(&key) {
            return;
        }
        let texture = ctx.load_texture(
            format!("prefetched-preview-{key:?}"),
            image,
            TextureOptions::LINEAR,
        );
        self.prefetched.insert(key, texture);
    }

    fn update(
        &mut self,
        ctx: &Context,
        video: &Video,
        at: f64,
        end: f64,
        origin: TimelineOrigin,
    ) -> Option<TextureHandle> {
        let sample = timeline_sample(video, end, at);
        let key = (video.id, video.rev, (sample * 1000.0).round() as i64);
        let video_key = video.thumbnail_key();
        let first_hover = self
            .target
            .as_ref()
            .is_none_or(|target| (target.key.0, target.key.1) != video_key);
        if self.target.as_ref().is_none_or(|target| target.key != key) {
            if first_hover {
                self.cancel_target();
                self.last_request = None;
            }
            self.target = Some(PreviewTarget {
                key,
                since: Instant::now(),
                moving: !first_hover,
                sent: false,
            });
        }
        let exact = self.cache.iter().find(|(cached, _)| *cached == key);
        if let Some((_, Some(texture))) = exact {
            return Some(texture.clone());
        }
        if let Some(texture) = self.prefetched.get(&key) {
            return Some(texture.clone());
        }
        let sample_image = self
            .draft
            .iter()
            .find(|(cached, _)| *cached == key)
            .map(|(_, image)| image.clone());
        let has_samples =
            self.draft.is_some() || !self.prefetched.is_empty() || !self.cache.is_empty();
        let target = self.target.as_mut()?;
        if !target.sent {
            // Display nearby frames at pointer speed. Refine once the pointer rests.
            let remaining = if target.moving && has_samples {
                Duration::from_millis(50).saturating_sub(target.since.elapsed())
            } else {
                self.last_request.map_or(Duration::ZERO, |sent| {
                    Duration::from_millis(50).saturating_sub(sent.elapsed())
                })
            };
            if remaining.is_zero() {
                target.sent = true;
                self.last_request = Some(Instant::now());
                self.outgoing = Some(Some(TimelineRequest {
                    key,
                    video: video.clone(),
                    at: sample,
                    has_preview: sample_image.is_some() || !self.prefetched.is_empty(),
                    neighbors: Vec::new(),
                    origin,
                }));
            } else {
                ctx.request_repaint_after(remaining);
            }
        }

        sample_image.or_else(|| self.nearest_frame(key).cloned())
    }

    pub(super) fn show(
        &mut self,
        ui: &Ui,
        video: &Video,
        hovered: Option<(Pos2, f64)>,
        end: f64,
        placement: PeekPlacement,
    ) {
        if let Some((position, at)) = hovered {
            self.shown = true;
            let image = self.update(ui.ctx(), video, at, end, TimelineOrigin::Player);
            let layer = LayerId::new(Order::Foreground, Id::new("watch_timeline_preview"));
            paint_peek(
                &ui.ctx().layer_painter(layer),
                image.as_ref(),
                at,
                peek_geometry(
                    placement.player,
                    position,
                    placement.below,
                    placement.hint,
                    placement.width,
                ),
            );
        }
    }

    pub(super) fn finish_frame(&mut self) {
        if !std::mem::take(&mut self.shown) {
            self.reset();
        }
    }
}

fn paint_peek(
    painter: &egui::Painter,
    texture: Option<&TextureHandle>,
    at: f64,
    geometry: PeekGeometry,
) {
    let colors = Palette::DARK;
    let PeekGeometry { frame, time, hint } = geometry;
    let image = frame.shrink(1.0);
    if let Some(texture) = texture {
        painter.rect_filled(frame, 8, colors.back);
        let size = texture.size_vec2();
        let fitted = size * (image.width() / size.x).min(image.height() / size.y);
        let artwork = Rect::from_center_size(image.center(), fitted);
        painter.add(
            egui::epaint::RectShape::filled(artwork, 7, Color32::WHITE)
                .with_texture(texture.id(), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0))),
        );
        painter.rect_stroke(
            frame,
            8,
            Stroke::new(1.0, colors.line),
            egui::StrokeKind::Inside,
        );
    }
    if let Some(hint) = hint {
        let label = "Pull up for precise seeking";
        let font_size = 12.0 * (hint.width() / 165.0).min(hint.height() / 17.0).min(1.0);
        let galley = painter.layout_no_wrap(
            label.into(),
            egui::FontId::proportional(font_size),
            Color32::WHITE,
        );
        painter.galley_with_override_text_color(
            hint.center() - galley.size() * 0.5 + vec2(0., 1.5),
            galley.clone(),
            Color32::from_black_alpha(144),
        );
        painter.with_clip_rect(hint).galley(
            hint.center() - galley.size() * 0.5,
            galley,
            Color32::WHITE,
        );
    }
    super::theme::paint_overlay_time(
        painter,
        time.center(),
        duration(Some(at.floor())),
        12.0 * (time.width() / 76.0).min(time.height() / 17.0).min(1.0),
    );
}

fn card_peek_geometry(viewport: Rect, pointer: Pos2) -> PeekGeometry {
    let bounds = viewport.shrink(5.0);
    let size = vec2(176.0, 99.0);
    let half = size.x / 2.0;
    let x = if bounds.width() >= size.x {
        pointer.x.clamp(bounds.left() + half, bounds.right() - half)
    } else {
        bounds.center().x
    };
    let time_height = 20.0;
    let gap = 4.0;
    let height = size.y + gap + time_height;
    let above = pointer.y - height - 6.0;
    let below = pointer.y + 6.0;
    let top = if above < bounds.top() && below + height <= bounds.bottom() {
        below
    } else {
        above
    };
    let top = top.clamp(bounds.top(), (bounds.bottom() - height).max(bounds.top()));
    let frame = Rect::from_min_size(pos2(x - half, top), size);
    let time = Rect::from_center_size(
        pos2(x, frame.bottom() + gap + time_height * 0.5),
        vec2(76.0, time_height),
    );
    PeekGeometry {
        frame,
        time,
        hint: None,
    }
}

fn peek_geometry(
    viewport: Rect,
    pointer: Pos2,
    below: bool,
    show_hint: bool,
    preferred_width: f32,
) -> PeekGeometry {
    const GAP: f32 = 4.0;
    const CHIP_HEIGHT: f32 = 20.0;
    let bounds = viewport.shrink(5.0_f32.min(viewport.size().min_elem() * 0.25));
    let chip_height = CHIP_HEIGHT.min(bounds.height() * 0.35);
    let gap = GAP.min(bounds.height() * 0.1);
    let hint_height = if show_hint { chip_height + gap } else { 0.0 };
    let image_width = preferred_width
        .min(bounds.width())
        .min((bounds.height() - chip_height - gap - hint_height) * 16.0 / 9.0);
    let image_size = vec2(image_width, image_width * 9.0 / 16.0);
    let width = if show_hint {
        image_width.max(176.0_f32.min(bounds.width()))
    } else {
        image_width
    };
    let half = width / 2.0;
    let x = pointer.x.clamp(
        bounds.left() + half,
        (bounds.right() - half).max(bounds.left() + half),
    );
    let height = hint_height + image_size.y + gap + chip_height;
    let above = pointer.y - height - 6.0;
    let underneath = pointer.y + 6.0;
    let below = match (
        above >= bounds.top(),
        underneath + height <= bounds.bottom(),
    ) {
        (true, false) => false,
        (false, true) => true,
        _ => below,
    };
    let top = if below { underneath } else { above };
    let top = top.clamp(bounds.top(), (bounds.bottom() - height).max(bounds.top()));
    let frame = Rect::from_min_size(pos2(x - image_width / 2.0, top + hint_height), image_size);
    let time = Rect::from_center_size(
        pos2(x, frame.bottom() + gap + chip_height * 0.5),
        vec2(76.0_f32.min(bounds.width()), chip_height),
    );
    let hint =
        show_hint.then(|| Rect::from_min_size(pos2(x - half, top), vec2(width, hint_height - gap)));
    PeekGeometry { frame, time, hint }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::catalog_video;

    #[test]
    fn preview_geometry_is_fixed_and_stays_inside_the_viewport() {
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 400.0));
        let PeekGeometry {
            frame: left,
            time: clock,
            ..
        } = peek_geometry(viewport, pos2(210.0, 180.0), false, true, 176.0);
        let PeekGeometry {
            frame: right,
            time: right_clock,
            ..
        } = peek_geometry(viewport, pos2(310.0, 180.0), false, true, 176.0);
        let PeekGeometry {
            frame: edge,
            time: edge_clock,
            ..
        } = peek_geometry(viewport, pos2(639.0, 7.0), false, true, 176.0);
        let PeekGeometry {
            frame: bottom,
            time: bottom_clock,
            ..
        } = peek_geometry(viewport, pos2(350.0, 394.0), true, true, 176.0);
        for (frame, time) in [
            (left, clock),
            (right, right_clock),
            (edge, edge_clock),
            (bottom, bottom_clock),
        ] {
            assert_eq!(frame.size(), vec2(176.0, 99.0));
            assert!(viewport.contains_rect(frame.union(time)));
            assert_eq!(time.center().x, frame.center().x);
            assert_eq!(time.top(), frame.bottom() + 4.0);
        }
        assert_eq!(right_clock.center().x - clock.center().x, 100.0);
        assert!(edge.top() > 7.0);
        assert!(bottom_clock.bottom() < 394.0);

        let mini = Rect::from_min_size(pos2(4.0, 20.0), vec2(140.0, 80.0));
        for below in [true, false] {
            let PeekGeometry { frame, time, hint } =
                peek_geometry(mini, mini.center(), below, false, 176.0);
            assert!(hint.is_none());
            assert!(mini.contains_rect(frame.union(time)));
            assert!(time.top() > frame.bottom());
        }

        let player = Rect::from_min_size(pos2(25.0, 50.0), vec2(300.0, 124.0));
        for pointer in [player.left_top(), player.right_bottom()] {
            let PeekGeometry { frame, time, hint } =
                peek_geometry(player, pointer, false, true, 176.0);
            let hint = hint.unwrap();
            assert!(player.contains_rect(hint.union(frame).union(time)));
            assert!(hint.bottom() < frame.top());
            assert!(frame.bottom() < time.top());
        }
        let PeekGeometry { frame, time, .. } = card_peek_geometry(viewport, pos2(210.0, 180.0));
        assert!(viewport.contains_rect(frame.union(time)));
        assert!(time.top() > frame.bottom());
    }

    #[test]
    fn returning_to_a_failed_timestamp_retries_the_preview() {
        let ctx = Context::default();
        let video = catalog_video(1);
        let mut peek = Peek::default();
        peek.update(&ctx, &video, 4., 90., TimelineOrigin::Player);
        let request = peek.take_request().flatten().unwrap();
        peek.load(&ctx, request.key, None, true);
        peek.update(&ctx, &video, 4., 90., TimelineOrigin::Player);
        assert!(peek.take_request().is_none());
        peek.reset();
        peek.update(&ctx, &video, 4., 90., TimelineOrigin::Player);
        assert_eq!(peek.take_request().flatten().unwrap().key, request.key);
    }

    #[test]
    fn late_samples_never_displace_the_current_preview() {
        let ctx = Context::default();
        let video = catalog_video(1);
        let mut peek = Peek::default();
        let make = |color| ColorImage::filled([8, 8], color);
        let cover = ctx.load_texture("cover", make(Color32::WHITE), TextureOptions::LINEAR);
        peek.set_cover(&video, Some(&cover));
        let first = peek
            .update(&ctx, &video, 4.0, 90.0, TimelineOrigin::Player)
            .unwrap();
        let old_key = peek.target.as_ref().unwrap().key;
        assert_eq!(first.id(), cover.id());
        let next = peek
            .update(&ctx, &video, 47.0, 90.0, TimelineOrigin::Player)
            .unwrap();
        assert_eq!(next.id(), first.id());
        let current = peek.target.as_ref().unwrap().key;
        peek.load(&ctx, old_key, Some(make(Color32::RED)), true);
        peek.load(&ctx, old_key, Some(make(Color32::RED)), false);
        assert_eq!(
            peek.update(&ctx, &video, 47.0, 90.0, TimelineOrigin::Player)
                .unwrap()
                .id(),
            peek.nearest_frame(current).unwrap().id()
        );

        peek.load(&ctx, current, Some(make(Color32::GREEN)), true);
        let exact = peek
            .update(&ctx, &video, 47.0, 90.0, TimelineOrigin::Player)
            .unwrap()
            .id();
        assert_ne!(exact, cover.id());
        assert_eq!(
            peek.update(&ctx, &video, 70.0, 90.0, TimelineOrigin::Player)
                .unwrap()
                .id(),
            exact
        );
        assert!(
            peek.update(&ctx, &catalog_video(2), 70.0, 90.0, TimelineOrigin::Player)
                .is_none()
        );
    }
}

#[cfg(test)]
#[test]
#[ignore = "requires MYTUBE_BENCH_VIDEO and MYTUBE_BENCH_SECONDS"]
fn benchmark_pointer_preview() {
    let ctx = Context::default();
    let mut video = super::tests::catalog_video(1);
    video.path = std::env::var_os("MYTUBE_BENCH_VIDEO").unwrap().into();
    video.stamp = crate::model::Stamp::read(&video.path).unwrap();
    let end: f64 = std::env::var("MYTUBE_BENCH_SECONDS")
        .unwrap()
        .parse()
        .unwrap();
    video.media.duration = Some(end);
    let mut peek = Peek::default();
    let now = Instant::now();
    peek.update(&ctx, &video, end * 0.1, end, TimelineOrigin::Player);
    let request = peek.take_request().flatten().unwrap();
    let mut decoder = crate::player::thumbnail::DraftDecoder::default();
    let tool = std::path::Path::new("ffmpeg");
    decoder
        .frame(tool, &video, request.at, false, &|| true)
        .unwrap();
    let setup = now.elapsed();
    let now = Instant::now();
    decoder
        .frame(tool, &video, end * 0.66, false, &|| true)
        .unwrap();
    println!(
        "decoder setup and first draft: {setup:?}; first uncached seek with active decoder: {:?}",
        now.elapsed()
    );
    let cold = setup;
    for ms in crate::service::preview_positions(&video, end, 0., TimelineOrigin::Player) {
        let (_, image) = decoder
            .frame(tool, &video, ms as f64 / 1000., true, &|| true)
            .unwrap();
        peek.receive_prefetched(&ctx, (video.id, video.rev, ms), image);
    }
    for (label, positions) in [
        (
            "motion",
            (10..90).map(|i| i as f64 / 100.).collect::<Vec<_>>(),
        ),
        ("jumps", vec![0.9, 0.1, 0.8, 0.2]),
    ] {
        let now = Instant::now();
        let ids = positions
            .iter()
            .map(|fraction| {
                peek.update(&ctx, &video, end * fraction, end, TimelineOrigin::Player)
                    .unwrap()
                    .id()
            })
            .collect::<std::collections::HashSet<_>>();
        println!(
            "{label}: {} updates, {} distinct samples, {:?}; cold draft {cold:?}",
            positions.len(),
            ids.len(),
            now.elapsed()
        );
    }
}
