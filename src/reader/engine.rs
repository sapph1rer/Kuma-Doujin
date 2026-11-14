//! Pure reader logic: settings, which pages to load or release, and page stepping.
//! The UI owns textures and tasks; it asks this module what to do.

use std::ops::Range;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    Single,
    Double,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    Width,
    Height,
    Original,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReaderSettings {
    pub layout: Layout,
    /// Right-to-left (Japanese) page order.
    pub rtl: bool,
    pub fit: Fit,
    /// Pages to fetch ahead of the visible ones.
    pub preload: usize,
    /// Decoded pages kept in memory around the current one.
    pub keep_in_memory: usize,
    /// Reader toolbar folded into a small tab (toggle with T).
    pub toolbar_collapsed: bool,
    /// Multiplier for mouse-wheel and arrow-key scrolling in the vertical layout.
    pub scroll_speed: f32,
    /// Auto-scroll speed in points per second (vertical layout, toggle with A).
    pub auto_scroll_speed: f32,
    /// Gap between pages in the vertical layout, in points.
    pub page_gap: f32,
}

impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            layout: Layout::Single,
            rtl: true,
            fit: Fit::Height,
            preload: 3,
            keep_in_memory: 10,
            toolbar_collapsed: false,
            scroll_speed: 1.0,
            auto_scroll_speed: 120.0,
            page_gap: 4.0,
        }
    }
}

impl Layout {
    pub fn visible(self) -> usize {
        match self {
            Layout::Double => 2,
            _ => 1,
        }
    }
}

/// Returns `(load, keep)`: pages that should be loading/loaded now, and pages allowed to stay
/// in memory. Everything outside `keep` is released (and in-flight loads cancelled).
/// `load` starts one page back so stepping backwards is instant, and is always inside `keep`.
pub fn plan(current: usize, s: &ReaderSettings, total: usize) -> (Range<usize>, Range<usize>) {
    if total == 0 {
        return (0..0, 0..0);
    }
    let current = current.min(total - 1);
    let ahead = s.layout.visible() + s.preload;
    let load = current.saturating_sub(1)..(current + ahead).min(total);
    let half = s.keep_in_memory / 2;
    let keep = current.saturating_sub(half.max(1))..(current + ahead.max(half)).min(total);
    (load, keep)
}

/// Moves `delta` steps (one spread in double layout). `None` means past either end of the chapter.
pub fn step(current: usize, total: usize, layout: Layout, delta: i64) -> Option<usize> {
    let next = current as i64 + delta * layout.visible() as i64;
    (0..total as i64).contains(&next).then_some(next as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_loads_ahead_and_keeps_window() {
        let s = ReaderSettings::default(); // single, preload 3, keep 10
        assert_eq!(plan(0, &s, 20), (0..4, 0..5));
        assert_eq!(plan(10, &s, 20), (9..14, 5..15));
        assert_eq!(plan(19, &s, 20), (18..20, 14..20));
        assert_eq!(plan(50, &s, 20), (18..20, 14..20));
        assert_eq!(plan(0, &s, 0), (0..0, 0..0));

        let tight = ReaderSettings {
            keep_in_memory: 0,
            layout: Layout::Double,
            ..s
        };
        let (load, keep) = plan(6, &tight, 20);
        assert_eq!(load, 5..11);
        assert!(keep.start <= load.start && keep.end >= load.end);
    }

    #[test]
    fn step_moves_by_spread_and_stops_at_ends() {
        assert_eq!(step(0, 5, Layout::Single, 1), Some(1));
        assert_eq!(step(0, 5, Layout::Single, -1), None);
        assert_eq!(step(4, 5, Layout::Single, 1), None);
        assert_eq!(step(2, 6, Layout::Double, 1), Some(4));
        assert_eq!(step(4, 6, Layout::Double, 1), None);
    }
}
