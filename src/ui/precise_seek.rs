use eframe::egui::{Pos2, Rect};

use crate::{
    model::Video,
    player::{Control, Playback, PreciseFinish, PreciseId},
    service::{TimelineKey, TimelineOrigin, TimelineRequest},
};

pub(super) const MAX_FILMSTRIP_TILES: usize = 48;
// At one thumbnail per second this many samples cover a short clip.
const SHORT_CLIP_SECONDS: f64 = 32.0;

pub(super) fn pulled_up(origin: Pos2, pointer: Pos2) -> bool {
    origin.y - pointer.y >= 24.0
}

pub(super) const SCRUB_THRESHOLD: f32 = 6.0;

#[derive(Clone, Copy)]
pub(super) struct HeldScrub {
    pub origin: Pos2,
    start_x: f32,
    width: f32,
    horizontal: bool,
}

impl HeldScrub {
    pub fn new(origin: Pos2, start_x: f32, width: f32) -> Self {
        Self {
            origin,
            start_x,
            width: width.max(1.0),
            horizontal: false,
        }
    }

    pub fn update(&mut self, pointer: Pos2, duration: f64) -> f64 {
        let movement = pointer - self.origin;
        self.horizontal |=
            movement.x.abs() >= SCRUB_THRESHOLD && movement.x.abs() > 1.5 * movement.y.abs();
        f64::from(((pointer.x - self.start_x) / self.width).clamp(0.0, 1.0)) * duration
    }

    pub fn commits_on_release(self) -> bool {
        self.horizontal
    }

    pub fn enter_precise(&mut self, pointer: Pos2, duration: f64, travel: Option<f32>) -> f64 {
        let at = self.update(pointer, duration);
        let width = travel.unwrap_or(self.width).max(1.);
        if duration > 0. && (width != self.width || duration < SHORT_CLIP_SECONDS) {
            // Use the same fine movement across layouts, anchored at the pull-up time.
            self.width = width / (duration / SHORT_CLIP_SECONDS).clamp(0.25, 1.) as f32;
            self.start_x = pointer.x - self.width * (at / duration) as f32;
        }
        // A diagonal lift opens the editor. Only a subsequent sideways scrub commits it.
        self.origin = pointer;
        self.horizontal = false;
        at
    }
}

#[derive(Clone, Copy)]
pub(super) enum Finish {
    Cancel,
    Play,
    Scrub,
}

pub(super) struct Gesture {
    position: f64,
    pub duration: f64,
    pub original: f64,
    pub held: Option<HeldScrub>,
    pub id: PreciseId,
    pub rail_drag: Option<Rect>,
    tiles: usize,
    frame_ms: u64,
    drag: Option<StripDrag>,
}

pub(super) struct Filmstrip {
    pub positions: Vec<f64>,
    pub selected: usize,
    pub offset: f32,
}

struct StripDrag {
    origin: Pos2,
    position: f64,
    dragged: bool,
}

impl Gesture {
    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn select(&mut self, at: f64) {
        self.position = (at * 1000.0).round().clamp(0.0, self.end_ms()) / 1000.0;
    }

    fn end_ms(&self) -> f64 {
        (self.duration.mul_add(1000.0, -1.0).floor()).max(0.0)
    }

    pub fn configure_strip(&mut self, tiles: usize, fps: Option<f64>) {
        self.tiles = tiles.clamp(1, MAX_FILMSTRIP_TILES);
        self.frame_ms = fps
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .map_or(1, |fps| (1000.0 / fps).ceil() as u64);
    }

    pub fn is_short_clip(&self) -> bool {
        self.duration < SHORT_CLIP_SECONDS
    }

    fn interval_ms(&self) -> u64 {
        let samples = (self.tiles as f64).max(SHORT_CLIP_SECONDS);
        let interval = (self.duration * 1000.0 / samples).round() as u64;
        interval.clamp(1, 1000).max(self.frame_ms)
    }

    pub fn seconds_per_tile(&self) -> f64 {
        self.interval_ms() as f64 / 1000.0
    }

    pub fn filmstrip(&self) -> Filmstrip {
        let end = self.end_ms() as u64;
        let position = (self.position * 1000.0).round() as u64;
        let interval = self.interval_ms();
        let grid_position = position as f64 / interval as f64;
        let (last, anchor, offset) = if self.is_short_clip() {
            // Each thumbnail covers its interval, including the partial last one.
            // The playhead can reach the end without requesting an extra EOF frame.
            (
                end / interval,
                position / interval,
                grid_position.fract() - 0.5,
            )
        } else {
            let last = end.div_ceil(interval);
            let anchor = ((position + interval / 2) / interval).min(last);
            (last, anchor, grid_position - anchor as f64)
        };
        let offset = offset as f32;
        let half = (self.tiles / 2) as u64;
        let start = anchor.saturating_sub(half + u64::from(offset < 0.0));
        let end_index = (anchor + half + u64::from(offset > 0.0)).min(last);
        let selected = (anchor - start) as usize;
        // Short clips keep even the center thumbnail on the sample grid. The
        // playhead and main preview still track the exact selected time.
        let positions = (start..=end_index)
            .enumerate()
            .map(|(index, frame)| {
                let at = if index == selected && !self.is_short_clip() {
                    position
                } else {
                    (frame * interval).min(end)
                };
                at as f64 / 1000.0
            })
            .collect();
        Filmstrip {
            positions,
            selected,
            offset,
        }
    }

    pub fn start_drag(&mut self, origin: Pos2) {
        self.drag = Some(StripDrag {
            origin,
            position: self.position,
            dragged: false,
        });
    }

    pub fn drag_to(&mut self, x: f32, points_per_second: f32) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        let distance = x - drag.origin.x;
        drag.dragged |= distance.abs() >= 4.0;
        if drag.dragged {
            let at = drag.position - f64::from(distance / points_per_second);
            self.select(at);
            return true;
        }
        false
    }

    pub fn strip_press(&self) -> Option<Pos2> {
        self.drag.as_ref().map(|drag| drag.origin)
    }

    pub fn stop_drag(&mut self) {
        self.drag = None;
    }

    pub fn key(&self, video: &Video, at: f64) -> TimelineKey {
        (video.id, video.rev, (at * 1000.0).round() as i64)
    }
}

#[derive(Default)]
pub(super) struct PreciseSeek {
    pub gesture: Option<Gesture>,
    sequence: u64,
    gesture_number: u64,
    in_flight: Option<(u64, Vec<TimelineKey>)>,
    cancel_request: bool,
}

impl PreciseSeek {
    pub fn start(
        &mut self,
        state: &Playback,
        position: f64,
        duration: f64,
        held: Option<HeldScrub>,
    ) -> Control {
        self.gesture_number += 1;
        let id = state.precise_id(self.gesture_number);
        let mut gesture = Gesture {
            position: 0.0,
            duration,
            original: state.confirmed_position.unwrap_or(state.position),
            held,
            id,
            rail_drag: None,
            tiles: 1,
            frame_ms: 1,
            drag: None,
        };
        gesture.select(position);
        let at = gesture.position();
        self.gesture = Some(gesture);
        Control::BeginPrecise(id, at)
    }

    pub fn finish(&mut self, finish: Finish) -> Option<Control> {
        let gesture = self.gesture.take()?;
        self.cancel_request |= self.in_flight.take().is_some();
        Some(Control::FinishPrecise(
            gesture.id,
            match finish {
                Finish::Cancel => PreciseFinish::Cancel,
                Finish::Play => PreciseFinish::Play(gesture.position()),
                Finish::Scrub => PreciseFinish::Scrub(gesture.position()),
            },
        ))
    }

    pub fn request(
        &mut self,
        video: Option<&Video>,
        cached: impl Fn(TimelineKey) -> bool,
    ) -> Option<Option<TimelineRequest>> {
        if let Some((gesture, video)) = self.gesture.as_ref().zip(video) {
            let Filmstrip {
                positions,
                selected,
                ..
            } = gesture.filmstrip();
            let mut missing: Vec<_> = positions
                .into_iter()
                .enumerate()
                .map(|(index, at)| (selected.abs_diff(index), gesture.key(video, at)))
                .filter(|(_, key)| !cached(*key))
                .collect();
            missing.sort_by_key(|(distance, _)| *distance);
            if let Some(&(_, key)) = missing.first() {
                // Let useful work finish through small pointer movements. A jump
                // to a different window immediately supersedes the old decoder.
                if self
                    .in_flight
                    .as_ref()
                    .is_some_and(|(_, pending)| missing.iter().any(|(_, k)| pending.contains(k)))
                {
                    return None;
                }
                let keys: Vec<_> = missing
                    .into_iter()
                    .map(|(_, k)| k)
                    .filter(|k| k.2.abs_diff(key.2) <= 5000)
                    .take(crate::media::FILMSTRIP_BATCH)
                    .collect();
                let neighbors = keys.iter().skip(1).map(|k| k.2).collect();
                self.sequence += 1;
                self.in_flight = Some((self.sequence, keys));
                self.cancel_request = false;
                return Some(Some(TimelineRequest {
                    key,
                    video: video.clone(),
                    at: key.2 as f64 / 1000.0,
                    has_preview: false,
                    neighbors,
                    origin: TimelineOrigin::Precise(self.sequence),
                }));
            }
            self.cancel_request |= self.in_flight.take().is_some();
        }
        std::mem::take(&mut self.cancel_request).then_some(None)
    }

    pub fn accept(&mut self, sequence: u64, key: TimelineKey) -> bool {
        let Some((active, pending)) = &mut self.in_flight else {
            return false;
        };
        if *active != sequence {
            return false;
        }
        let Some(index) = pending.iter().position(|&k| k == key) else {
            return false;
        };
        pending.swap_remove(index);
        if pending.is_empty() {
            self.in_flight = None;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::catalog_video;
    use std::collections::HashSet;

    #[test]
    fn pull_up_threshold_uses_the_initial_press() {
        let origin = Pos2::new(60.0, 100.0);
        assert!(!pulled_up(origin, Pos2::new(60.0, 124.0)));
        assert!(!pulled_up(origin, Pos2::new(160.0, 77.0)));
        assert!(pulled_up(origin, Pos2::new(160.0, 76.0)));
    }

    #[test]
    fn pulling_up_on_a_short_clip_reduces_sensitivity_without_moving_the_playhead() {
        let pull = Pos2::new(200.0, 50.0);
        for (duration, expected_delta) in [(4.0, 0.25), (16.0, 2.0), (60.0, 15.0)] {
            let mut held = HeldScrub::new(Pos2::new(100.0, 100.0), 0.0, 400.0);
            let before = held.update(pull, duration);
            assert_eq!(held.enter_precise(pull, duration, None), before);
            assert_eq!(held.update(pull, duration), before);
            assert_eq!(
                held.update(Pos2::new(300.0, 25.0), duration),
                before + expected_delta
            );
        }
    }

    #[test]
    fn pulling_up_from_a_compact_rail_uses_the_same_travel_as_inline() {
        let pull = Pos2::new(400.0, 50.0);
        for duration in [8.0, 120.0] {
            let mut inline = HeldScrub::new(pull, 0.0, 800.0);
            let mut compact = HeldScrub::new(pull, 300.0, 200.0);
            for held in [&mut inline, &mut compact] {
                assert_eq!(
                    held.enter_precise(pull, duration, Some(800.0)),
                    duration / 2.0
                );
            }
            for x in [200.0, 440.0, 720.0] {
                let pointer = Pos2::new(x, 40.0);
                assert_eq!(
                    compact.update(pointer, duration),
                    inline.update(pointer, duration)
                );
            }
        }
    }

    #[test]
    fn short_clips_reuse_subsecond_thumbnails_while_the_target_moves() {
        let mut seek = PreciseSeek::default();
        seek.start(&Playback::default(), 3.0, 8.0, None);
        let gesture = seek.gesture.as_mut().unwrap();
        gesture.configure_strip(7, Some(30.0));
        assert!((0.24..=0.25).contains(&gesture.seconds_per_tile()));
        let before = gesture.filmstrip();
        gesture.select(3.01);
        let after = gesture.filmstrip();
        assert_eq!(before.positions, after.positions);
        assert!(after.offset > before.offset);
        assert_eq!(gesture.position(), 3.01);

        seek.start(&Playback::default(), 0.1, 0.4, None);
        let gesture = seek.gesture.as_mut().unwrap();
        gesture.configure_strip(17, Some(10.0));
        assert_eq!(gesture.seconds_per_tile(), 0.1);
        gesture.select(0.4);
        assert_eq!(gesture.position(), 0.399);
        assert_eq!(gesture.filmstrip().positions, [0.0, 0.1, 0.2, 0.3]);
    }

    #[test]
    fn adjustments_commit_once_and_cancel_restores_playback() {
        let mut seek = PreciseSeek::default();
        let state = Playback::default();
        assert!(matches!(
            seek.start(&state, 20.0, 60.0, None),
            Control::BeginPrecise(_, 20.0)
        ));
        let gesture = seek.gesture.as_mut().unwrap();
        gesture.configure_strip(5, None);
        assert_eq!(
            gesture.filmstrip().positions,
            [18.0, 19.0, 20.0, 21.0, 22.0]
        );
        gesture.start_drag(Pos2::new(100.0, 0.0));
        assert!(!gesture.drag_to(98.0, 50.0));
        assert!(gesture.drag_to(75.0, 50.0));
        assert!(gesture.drag_to(25.0, 50.0));
        assert_eq!(gesture.position, 21.5);
        gesture.stop_drag();
        gesture.start_drag(Pos2::new(100.0, 0.0));
        assert!(gesture.drag_to(125.0, 50.0));
        assert_eq!(gesture.position, 21.0);
        gesture.select(gesture.filmstrip().positions[4]);
        assert!(matches!(
            seek.finish(Finish::Play),
            Some(Control::FinishPrecise(_, PreciseFinish::Play(23.0)))
        ));
        assert!(seek.finish(Finish::Play).is_none());
        seek.start(&state, 0.25, 60.0, None);
        let gesture = seek.gesture.as_mut().unwrap();
        gesture.configure_strip(5, None);
        let beginning = gesture.filmstrip();
        assert_eq!(beginning.positions, [0.25, 1.0, 2.0, 3.0]);
        assert_eq!(beginning.selected, 0);
        assert!(matches!(
            seek.finish(Finish::Cancel),
            Some(Control::FinishPrecise(_, PreciseFinish::Cancel))
        ));
        assert!(seek.finish(Finish::Cancel).is_none());
    }

    #[test]
    fn wide_filmstrip_reuses_neighbors_and_reaches_both_ends() {
        let mut seek = PreciseSeek::default();
        seek.start(&Playback::default(), 30.1, 60.0, None);
        let gesture = seek.gesture.as_mut().unwrap();
        gesture.configure_strip(17, None);
        let first = gesture.filmstrip();
        assert_eq!(first.positions.len(), 18);
        assert_eq!(first.positions[first.selected], 30.1);
        gesture.select(30.4);
        let second = gesture.filmstrip();
        assert_eq!(second.positions[second.selected], 30.4);
        assert!(second.offset > first.offset);
        assert_eq!(
            first
                .positions
                .iter()
                .zip(&second.positions)
                .filter(|(a, b)| a != b)
                .count(),
            1
        );

        gesture.select(0.0);
        let start = gesture.filmstrip();
        assert_eq!(start.positions.len(), 9);
        assert_eq!(start.selected, 0);
        assert_eq!(start.positions[start.selected], 0.0);
        gesture.select(60.0);
        let end = gesture.filmstrip();
        assert!(end.positions.len() < 17);
        assert_eq!(end.selected, end.positions.len() - 1);
        assert_eq!(end.positions[end.selected], 59.999);
    }

    #[test]
    fn requests_center_first_and_rejects_superseded_slots_and_videos() {
        let mut seek = PreciseSeek::default();
        let mut cached = HashSet::new();
        let video = catalog_video(1);
        let request = |seek: &mut PreciseSeek, video: &Video, cached: &HashSet<_>| {
            seek.gesture.as_mut().unwrap().configure_strip(11, None);
            let next = seek
                .request(Some(video), |key| cached.contains(&key))
                .flatten()
                .unwrap();
            let TimelineOrigin::Precise(seq) = next.origin else {
                panic!("expected precise request")
            };
            (seq, next.key)
        };
        seek.start(&Playback::default(), 20.0, 60.0, None);
        let center = request(&mut seek, &video, &cached);
        assert_eq!(center.1, (video.id, video.rev, 20_000));
        assert!(seek.request(Some(&video), |_| false).is_none());
        seek.gesture.as_mut().unwrap().select(20.5);
        assert!(seek.request(Some(&video), |_| false).is_none());
        assert!(seek.accept(center.0, center.1));
        assert!(!seek.accept(center.0, center.1));
        cached.insert(center.1);
        seek.gesture.as_mut().unwrap().select(45.5);
        let adjusted = request(&mut seek, &video, &cached);
        assert_eq!(adjusted.1.2, 45_500);
        assert!(!seek.accept(center.0, (video.id, video.rev, 21_000)));
        assert!(seek.accept(adjusted.0, adjusted.1));
        assert!(seek.accept(adjusted.0, (video.id, video.rev, 45_000)));
        seek.finish(Finish::Cancel);
        assert!(matches!(seek.request(Some(&video), |_| false), Some(None)));
        let mut new_revision = video;
        new_revision.rev += 1;
        seek.start(&Playback::default(), 45.5, 60.0, None);
        let next = request(&mut seek, &new_revision, &cached);
        assert!(!seek.accept(adjusted.0, adjusted.1));
        assert!(seek.accept(next.0, next.1));
    }
}
