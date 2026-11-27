use std::collections::{BTreeSet, HashSet};

use eframe::egui::{Event, MouseWheelUnit, Pos2, Rect, TouchDeviceId, TouchId, TouchPhase, Vec2};

use super::marquee::{Grid, SelectionMode, content_point};

const PAN_THRESHOLD: f32 = 7.0;

#[derive(Default)]
pub(super) struct PanInput {
    contacts: Vec<Contact>,
    direct: Option<Path>,
    trackpad: Option<Path>,
    suppress_contacts: bool,
    segments: Vec<(Pos2, Pos2)>,
    finished: bool,
}

struct Contact {
    device: TouchDeviceId,
    id: TouchId,
    point: Pos2,
    origin: Pos2,
}

struct Path {
    origin: Pos2,
    point: Pos2,
    active: bool,
}

impl Path {
    fn start(point: Pos2) -> Self {
        Self {
            origin: point,
            point,
            active: false,
        }
    }

    fn advance(&mut self, point: Pos2, recognize: bool, segments: &mut Vec<(Pos2, Pos2)>) {
        if self.active {
            if self.point != point {
                segments.push((self.point, point));
            }
        } else if recognize {
            self.active = true;
            segments.push((self.origin, point));
        }
        self.point = point;
    }
}

impl PanInput {
    pub fn blocking(&self) -> bool {
        self.suppress_contacts || self.finished || self.point().is_some()
    }

    pub fn point(&self) -> Option<Pos2> {
        self.direct
            .as_ref()
            .or(self.trackpad.as_ref())
            .filter(|path| path.active)
            .map(|path| path.point)
    }

    pub fn has_path(&self) -> bool {
        self.point().is_some() || !self.segments.is_empty()
    }

    pub fn take_segments(&mut self) -> Vec<(Pos2, Pos2)> {
        std::mem::take(&mut self.segments)
    }

    pub fn take_finished(&mut self) -> bool {
        std::mem::take(&mut self.finished)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn read(
        &mut self,
        events: &mut Vec<Event>,
        bounds: Rect,
        mut cursor: Option<Pos2>,
        can_start: impl Fn(Pos2) -> bool,
    ) {
        events.retain(|event| {
            match *event {
                Event::PointerMoved(position) => cursor = Some(position),
                Event::Touch {
                    device_id,
                    id,
                    phase,
                    pos,
                    ..
                } => {
                    self.touch(device_id, id, phase, pos, &can_start);
                }
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    phase,
                    delta,
                    modifiers,
                } if modifiers.is_none() => {
                    return !self.wheel(phase, delta, cursor, bounds, &can_start);
                }
                _ => {}
            }
            true
        });
    }

    fn touch(
        &mut self,
        device: TouchDeviceId,
        id: TouchId,
        phase: TouchPhase,
        point: Pos2,
        can_start: &impl Fn(Pos2) -> bool,
    ) {
        let index = self
            .contacts
            .iter()
            .position(|touch| touch.device == device && touch.id == id);
        if phase == TouchPhase::Start && index.is_none() && can_start(point) {
            self.contacts.push(Contact {
                device,
                id,
                point,
                origin: point,
            });
            if self.contacts.len() == 2 {
                for contact in &mut self.contacts {
                    contact.origin = contact.point;
                }
            }
            if let [a, b] = self.contacts.as_slice()
                && a.device == b.device
                && !self.suppress_contacts
            {
                let center = a.point.lerp(b.point, 0.5);
                self.direct = Some(Path::start(center));
                self.suppress_contacts = true;
            }
            if self.contacts.len() > 2
                && let Some(path) = self.direct.take()
            {
                self.finished |= path.active;
            }
            return;
        }
        let Some(index) = index else { return };
        self.contacts[index].point = point;
        if phase != TouchPhase::Cancel
            && let (Some(path), [a, b]) = (&mut self.direct, self.contacts.as_slice())
        {
            let center = a.point.lerp(b.point, 0.5);
            let da = a.point - a.origin;
            let db = b.point - b.origin;
            let pan = center.distance(path.origin) >= PAN_THRESHOLD && da.dot(db) > 0.0;
            path.advance(center, pan, &mut self.segments);
        }
        if matches!(phase, TouchPhase::End | TouchPhase::Cancel) {
            self.contacts.remove(index);
            if let Some(path) = self.direct.take() {
                self.finished |= path.active;
            }
            // The primary touch can release after the second finger. Absorb its synthesized click.
            if self.contacts.is_empty() && self.suppress_contacts {
                self.finished = true;
                self.suppress_contacts = false;
            }
        }
    }

    fn wheel(
        &mut self,
        phase: TouchPhase,
        delta: Vec2,
        cursor: Option<Pos2>,
        bounds: Rect,
        can_start: &impl Fn(Pos2) -> bool,
    ) -> bool {
        if phase == TouchPhase::Start {
            self.trackpad = cursor.filter(|&point| can_start(point)).map(Path::start);
        }
        let Some(path) = &mut self.trackpad else {
            return false;
        };
        let next = bounds.clamp(path.point + delta);
        let travel = next - path.origin;
        let horizontal = travel.x.abs() >= PAN_THRESHOLD && travel.x.abs() > travel.y.abs() * 1.2;
        if !path.active && travel.y.abs() >= PAN_THRESHOLD && !horizontal {
            self.trackpad = None;
            return false;
        }
        if phase != TouchPhase::Cancel {
            path.advance(next, horizontal, &mut self.segments);
        }
        let active = path.active;
        if matches!(phase, TouchPhase::End | TouchPhase::Cancel) {
            self.finished |= active;
            self.trackpad = None;
        }
        active
    }
}

#[derive(Default)]
pub(super) struct PanSelection {
    last: Option<Pos2>,
    mode: Option<SelectionMode>,
    missing: BTreeSet<usize>,
    visited: HashSet<usize>,
}

impl PanSelection {
    pub fn trace(
        &mut self,
        grid: Grid,
        bounds: Rect,
        offset: f32,
        input: &mut PanInput,
        mut video_at: impl FnMut(usize) -> Option<i64>,
        selection: &mut HashSet<i64>,
    ) {
        let visible = grid.visible_rows(Rect::from_min_size(Pos2::new(0.0, offset), bounds.size()));
        let available = self
            .missing
            .range((visible.start * grid.columns)..(visible.end * grid.columns))
            .filter_map(|&index| video_at(index).map(|id| (index, id)))
            .collect::<Vec<_>>();
        for (index, id) in available {
            self.missing.remove(&index);
            self.select(id, selection);
        }
        let local = |point| content_point(point, bounds, offset);
        let segments = input.take_segments();
        let first = segments.first().map(|segment| segment.0).or(input.point());
        if let Some(first) = first {
            let first = local(first);
            self.visit(
                grid,
                self.last.unwrap_or(first),
                first,
                &mut video_at,
                selection,
            );
            self.last = Some(first);
        }
        for (start, end) in segments {
            let end = local(end);
            self.visit(grid, local(start), end, &mut video_at, selection);
            self.last = Some(end);
        }
    }

    fn visit(
        &mut self,
        grid: Grid,
        start: Pos2,
        end: Pos2,
        video_at: &mut impl FnMut(usize) -> Option<i64>,
        selection: &mut HashSet<i64>,
    ) {
        let range = grid.region(Rect::from_two_pos(start, end));
        let mut hits = Vec::new();
        for row in range.rows {
            for column in range.cells.clone() {
                let index = row * grid.columns + column;
                if index < grid.total
                    && !self.visited.contains(&index)
                    && let Some(entry) = segment_entry(grid.item_rect(index), start, end)
                {
                    hits.push((entry, index));
                }
            }
        }
        if self.mode.is_none() {
            hits.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        }
        for (_, index) in hits {
            self.visited.insert(index);
            if let Some(id) = video_at(index) {
                self.select(id, selection);
            } else {
                self.missing.insert(index);
            }
        }
    }

    fn select(&mut self, id: i64, selection: &mut HashSet<i64>) {
        let mode = *self.mode.get_or_insert_with(|| {
            if selection.contains(&id) {
                SelectionMode::Remove
            } else {
                SelectionMode::Add
            }
        });
        if mode == SelectionMode::Remove {
            selection.remove(&id);
        } else {
            selection.insert(id);
        }
    }

    pub fn finish(self) -> (SelectionMode, Vec<usize>) {
        (
            self.mode.unwrap_or(SelectionMode::Add),
            self.missing.into_iter().collect(),
        )
    }
}

fn segment_entry(rect: Rect, from: Pos2, to: Pos2) -> Option<f32> {
    let mut entry = 0.0_f32;
    let mut exit = 1.0_f32;
    for (start, end, low, high) in [
        (from.x, to.x, rect.left(), rect.right()),
        (from.y, to.y, rect.top(), rect.bottom()),
    ] {
        let delta = end - start;
        if delta == 0.0 {
            if !(low..=high).contains(&start) {
                return None;
            }
        } else {
            let a = (low - start) / delta;
            let b = (high - start) / delta;
            entry = entry.max(a.min(b));
            exit = exit.min(a.max(b));
        }
    }
    (entry <= exit).then_some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    #[test]
    fn trackpad_keeps_vertical_scroll_and_captures_horizontal_led_pan() {
        let bounds = Rect::from_min_size(Pos2::ZERO, Vec2::splat(250.0));
        let pointer = Some(pos2(65.0, 65.0));
        let mut input = PanInput::default();
        let mut wheel =
            |phase, x, y| input.wheel(phase, Vec2::new(x, y), pointer, bounds, &|_| true);
        assert!(!wheel(TouchPhase::Start, 0.0, 0.0));
        assert!(!wheel(TouchPhase::Move, 1.0, 18.0));
        assert!(!wheel(TouchPhase::Move, 14.0, 40.0));
        assert!(!wheel(TouchPhase::End, 0.0, 0.0));
        assert!(!wheel(TouchPhase::Start, 0.0, 0.0));
        assert!(wheel(TouchPhase::Move, 18.0, 1.0));
        assert!(wheel(TouchPhase::Move, 0.0, 50.0));
        assert!(wheel(TouchPhase::End, 0.0, 0.0));
        assert_eq!(
            input.take_segments(),
            [
                (pos2(65.0, 65.0), pos2(83.0, 66.0)),
                (pos2(83.0, 66.0), pos2(83.0, 116.0)),
            ]
        );
        assert!(input.take_finished());
    }

    #[test]
    fn held_pan_crosses_rows_when_the_viewport_autoscrolls() {
        let bounds = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 120.0));
        let grid = Grid {
            size: Vec2::new(150.0, 32.0),
            gap: 8.0,
            columns: 1,
            total: 20,
        };
        let mut input = PanInput::default();
        let mut pan = PanSelection::default();
        let mut selection = HashSet::new();
        let point = pos2(80.0, 116.0);
        let mut path = Path::start(pos2(80.0, 14.0));
        path.advance(point, true, &mut input.segments);
        input.direct = Some(path);
        pan.trace(
            grid,
            bounds,
            0.0,
            &mut input,
            |index| Some(index as i64),
            &mut selection,
        );
        assert_eq!(selection, HashSet::from([0, 1, 2]));
        pan.trace(
            grid,
            bounds,
            130.0,
            &mut input,
            |index| (index != 4).then_some(index as i64),
            &mut selection,
        );
        assert_eq!(selection, HashSet::from([0, 1, 2, 3, 5, 6]));
        assert!(pan.missing.contains(&4));
        pan.trace(
            grid,
            bounds,
            130.0,
            &mut input,
            |index| Some(index as i64),
            &mut selection,
        );
        assert!(selection.contains(&4));
        assert!(pan.finish().1.is_empty());
    }
}
