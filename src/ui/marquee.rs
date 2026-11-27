use std::{collections::HashSet, ops::Range};

use eframe::egui::{Modifiers, Pos2, Rect, Vec2, pos2, vec2};

use crate::{catalog::CatalogRegion, model::LibraryView};

use super::{CATALOG_GUTTER, GRID_COLUMNS, LIST_HEIGHTS, library::CARD_METADATA_HEIGHT};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectionMode {
    Replace,
    Add,
    Remove,
    Toggle,
}

impl SelectionMode {
    pub fn from_modifiers(modifiers: Modifiers) -> Self {
        if if cfg!(target_os = "macos") {
            modifiers.mac_cmd
        } else {
            modifiers.ctrl
        } {
            Self::Toggle
        } else if modifiers.shift {
            Self::Add
        } else {
            Self::Replace
        }
    }

    pub fn selected(self, existing: bool, intersects: bool) -> bool {
        match self {
            Self::Replace => intersects,
            Self::Add => existing || intersects,
            Self::Remove => existing && !intersects,
            Self::Toggle => existing ^ intersects,
        }
    }

    pub fn apply(self, selection: &mut HashSet<i64>, ids: Vec<i64>) {
        if self == Self::Replace {
            selection.clear();
        }
        for id in ids {
            match self {
                Self::Remove => {
                    selection.remove(&id);
                }
                Self::Toggle if selection.remove(&id) => {}
                _ => {
                    selection.insert(id);
                }
            }
        }
    }
}

pub(super) struct Marquee {
    pub start: Pos2,
    pub press: Pos2,
    pub base: HashSet<i64>,
    pub mode: SelectionMode,
    pub dragged: bool,
}

pub(super) struct MarqueeSelection {
    pub base: HashSet<i64>,
    pub region: CatalogRegion,
    pub rect: Rect,
}

#[derive(Clone, Copy)]
pub(super) struct Grid {
    pub size: Vec2,
    pub gap: f32,
    pub columns: usize,
    pub total: usize,
}

impl Grid {
    pub fn catalog(view: LibraryView, width: f32, total: usize) -> Self {
        let width = (width - 2.0 * CATALOG_GUTTER).max(1.0);
        let gap = if view.list { 7.0 } else { 18.0 };
        let columns = if view.list {
            1
        } else {
            let maximum = ((width + gap) / (127.0 + gap)).floor().clamp(1.0, 6.0) as usize;
            GRID_COLUMNS[view.size].min(maximum)
        };
        let cell_width = (width - gap * (columns - 1) as f32) / columns as f32;
        let height = if view.list {
            LIST_HEIGHTS[view.size]
        } else {
            cell_width * 9.0 / 16.0 + CARD_METADATA_HEIGHT
        };
        Self {
            size: vec2(cell_width, height),
            gap,
            columns,
            total,
        }
    }

    fn rows(self) -> usize {
        self.total.div_ceil(self.columns)
    }

    pub fn stride(self) -> f32 {
        self.size.y + self.gap
    }

    pub fn height(self) -> f32 {
        (self.rows() as f32 * self.stride() - self.gap).max(0.0)
    }

    pub fn width(self) -> f32 {
        self.columns as f32 * (self.size.x + self.gap) - self.gap
    }

    pub fn visible_rows(self, viewport: Rect) -> Range<usize> {
        let start = (viewport.top() / self.stride()).floor() as usize;
        let end = (viewport.bottom() / self.stride()).ceil() as usize + 1;
        start.min(self.rows())..end.min(self.rows())
    }

    pub fn item_rect(self, index: usize) -> Rect {
        Rect::from_min_size(
            pos2(
                CATALOG_GUTTER + (index % self.columns) as f32 * (self.size.x + self.gap),
                (index / self.columns) as f32 * self.stride(),
            ),
            self.size,
        )
    }

    pub fn occupied(self, mut point: Pos2) -> bool {
        point.x -= CATALOG_GUTTER;
        if point.x < 0.0 || point.y < 0.0 {
            return false;
        }
        let stride = self.size + Vec2::splat(self.gap);
        let row = (point.y / stride.y).floor() as usize;
        let col = (point.x / stride.x).floor() as usize;
        col < self.columns
            && row * self.columns + col < self.total
            && point.x - col as f32 * stride.x <= self.size.x
            && point.y - row as f32 * stride.y <= self.size.y
    }

    pub fn region(self, rect: Rect) -> CatalogRegion {
        let intersecting = |min: f32, max: f32, size: f32, cells: usize| -> Range<usize> {
            let stride = size + self.gap;
            let start = (((min - size) / stride).floor() + 1.0) as usize;
            let end = (max / stride).ceil() as usize;
            start.min(cells)..end.min(cells).max(start.min(cells))
        };
        let rect = rect.translate(vec2(-CATALOG_GUTTER, 0.0)).expand(0.25);
        CatalogRegion {
            columns: self.columns,
            rows: intersecting(
                rect.top(),
                rect.bottom(),
                self.size.y,
                self.total.div_ceil(self.columns),
            ),
            cells: intersecting(rect.left(), rect.right(), self.size.x, self.columns),
        }
    }
}

pub(super) fn content_point(pointer: Pos2, viewport: Rect, offset: f32) -> Pos2 {
    let local = viewport.clamp(pointer) - viewport.min;
    pos2(local.x, local.y + offset)
}

pub(super) fn marquee_rect(start: Pos2, pointer: Pos2, viewport: Rect, offset: f32) -> Rect {
    Rect::from_two_pos(start, content_point(pointer, viewport, offset))
}

pub(super) fn edge_scroll(pointer: Pos2, viewport: Rect, dt: f32) -> f32 {
    let edge = 38.0_f32.min(viewport.height() * 0.5);
    let weight = |distance: f32| (1.0 - distance / edge.max(1.0)).clamp(0.0, 1.0).powi(2);
    (weight(viewport.bottom() - pointer.y) - weight(pointer.y - viewport.top())) * 1050.0 * dt
}

pub(super) fn paint(ui: &eframe::egui::Ui, rect: Rect, viewport: Rect, offset: f32) {
    use eframe::egui::{Stroke, StrokeKind};
    let color = super::library::selection_color(ui.visuals());
    ui.painter()
        .with_clip_rect(viewport.intersect(ui.clip_rect()))
        .rect(
            rect.translate(viewport.min.to_vec2() - vec2(0.0, offset)),
            3,
            color.gamma_multiply(0.16),
            Stroke::new(1.0, color.gamma_multiply(0.8)),
            StrokeKind::Inside,
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_scroll_extends_the_selection_in_catalog_coordinates() {
        let viewport = Rect::from_min_size(pos2(20.0, 40.0), vec2(300.0, 240.0));
        let start = content_point(pos2(30.0, 70.0), viewport, 0.0);
        let edge = pos2(85.0, viewport.bottom());
        assert!(edge_scroll(edge, viewport, 0.016) > 0.0);
        assert!(edge_scroll(viewport.center(), viewport, 0.016) == 0.0);

        let grid = Grid {
            size: vec2(90.0, 40.0),
            gap: 10.0,
            columns: 3,
            total: 900,
        };
        let first = grid.region(marquee_rect(start, edge, viewport, 0.0));
        let scrolled = grid.region(marquee_rect(start, edge, viewport, 2000.0));
        assert!(first.contains(3));
        assert!(!first.contains(120));
        assert!(scrolled.contains(3) && scrolled.contains(120));
        assert!(!scrolled.contains(121));
    }

    #[test]
    fn marquee_modes_apply_the_current_intersections_to_the_original_selection() {
        let original = HashSet::from([1, 3]);
        for (mode, expected) in [
            (SelectionMode::Replace, HashSet::from([1, 2])),
            (SelectionMode::Add, HashSet::from([1, 2, 3])),
            (SelectionMode::Toggle, HashSet::from([2, 3])),
        ] {
            let mut selection = original.clone();
            mode.apply(&mut selection, vec![1, 2]);
            assert_eq!(selection, expected);
        }
    }
}
