//! Distraction-free reader view. Owns page textures for the open chapter; `app.rs` runs the
//! loads the view asks for (see `reader::engine::plan`).

use std::collections::HashMap;

use eframe::egui::{
    self, Align2, Color32, FontId, Key, Rect, RichText, Sense, TextureHandle, Vec2, pos2, vec2,
};
use tokio::task::AbortHandle;

use super::{icons, theme};
use crate::core::models::{Chapter, MangaKey, PageRef};
use crate::reader::engine::{self, Fit, Layout, ReaderSettings};

const CONTROLS_HIDE_AFTER: f64 = 2.5;
const PLACEHOLDER_RATIO: f32 = 1.42;

pub enum PageSlot {
    Loading(AbortHandle),
    Ready(TextureHandle),
    Failed(String),
}

pub struct ReaderState {
    pub manga: MangaKey,
    pub title: String,
    pub source_name: String,
    pub chapters: Vec<Chapter>,
    pub chapter: usize,
    pub pages: Option<Result<Vec<PageRef>, String>>,
    /// Bumped on chapter change; results from older chapters are dropped.
    pub generation: u64,
    pub current: usize,
    pub zoom: f32,
    pub slots: HashMap<usize, PageSlot>,
    /// Known page sizes, kept after textures are released so layouts don't jump.
    pub sizes: HashMap<usize, Vec2>,
    /// For progress/bookmark records.
    pub cover_url: Option<String>,
    pub chapter_lang: String,
    /// Last (chapter, page) written to reading progress.
    pub last_saved: Option<(usize, usize)>,
    last_activity: f64,
    scroll_to: Option<usize>,
    fullscreen: bool,
    /// Vertical layout: current scroll offset, and whether auto-scroll is running.
    offset: f32,
    pub auto_scroll: bool,
}

#[derive(Default)]
pub struct ReaderAction {
    pub close: bool,
    /// Open this chapter index; `to_end` starts on its last page.
    pub open_chapter: Option<(usize, bool)>,
    pub retry_page: Option<usize>,
    pub settings_changed: bool,
    /// Bookmark the current page.
    pub bookmark: bool,
}

impl ReaderState {
    pub fn new(
        manga: MangaKey,
        title: String,
        source_name: String,
        chapters: Vec<Chapter>,
        chapter: usize,
        cover_url: Option<String>,
        chapter_lang: String,
    ) -> Self {
        Self {
            manga,
            title,
            source_name,
            chapters,
            chapter,
            pages: None,
            generation: 0,
            current: 0,
            zoom: 1.0,
            slots: HashMap::new(),
            sizes: HashMap::new(),
            cover_url,
            chapter_lang,
            last_saved: None,
            last_activity: 0.0,
            scroll_to: Some(0),
            fullscreen: false,
            offset: 0.0,
            auto_scroll: false,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages
            .as_ref()
            .and_then(|p| p.as_ref().ok())
            .map_or(0, Vec::len)
    }

    /// Releases every texture and cancels in-flight loads.
    pub fn clear_pages(&mut self) {
        for (_, slot) in self.slots.drain() {
            if let PageSlot::Loading(h) = slot {
                h.abort();
            }
        }
        self.sizes.clear();
    }

    pub fn set_page(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        result: Result<egui::ColorImage, String>,
    ) {
        // Ignore results for pages released while loading.
        if !matches!(self.slots.get(&index), Some(PageSlot::Loading(_))) {
            return;
        }
        let slot = match result {
            Ok(img) => {
                self.sizes
                    .insert(index, vec2(img.size[0] as f32, img.size[1] as f32));
                let name = format!("page-{}-{index}", self.generation);
                PageSlot::Ready(ctx.load_texture(name, img, egui::TextureOptions::LINEAR))
            }
            Err(e) => PageSlot::Failed(e),
        };
        self.slots.insert(index, slot);
    }

    fn go(&mut self, delta: i64, layout: Layout) -> ReaderAction {
        let total = self.page_count();
        match engine::step(self.current, total, layout, delta) {
            Some(i) => {
                self.current = i;
                self.scroll_to = Some(i);
                ReaderAction::default()
            }
            None if delta > 0 && self.chapter + 1 < self.chapters.len() => ReaderAction {
                open_chapter: Some((self.chapter + 1, false)),
                ..Default::default()
            },
            None if delta < 0 && self.chapter > 0 => ReaderAction {
                open_chapter: Some((self.chapter - 1, true)),
                ..Default::default()
            },
            None => ReaderAction::default(),
        }
    }

    pub fn jump(&mut self, page: usize) {
        self.current = page.min(self.page_count().saturating_sub(1));
        self.scroll_to = Some(self.current);
    }
}

/// `bookmarked`: the current page is already bookmarked (shown on the button).
pub fn show(
    ui: &mut egui::Ui,
    st: &mut ReaderState,
    s: &mut ReaderSettings,
    bookmarked: bool,
) -> ReaderAction {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    if ctx.input(|i| i.pointer.delta() != Vec2::ZERO || !i.events.is_empty()) {
        st.last_activity = now;
    }
    let mut action = keyboard(&ctx, st, s);
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0, Color32::BLACK);

    match &st.pages {
        None => {
            ui.put(
                Rect::from_center_size(full.center(), vec2(40.0, 40.0)),
                egui::Spinner::new(),
            );
        }
        Some(Err(e)) => {
            ui.painter().text(
                full.center(),
                Align2::CENTER_CENTER,
                format!("Could not open chapter: {e}"),
                FontId::proportional(16.0),
                Color32::LIGHT_RED,
            );
        }
        Some(Ok(_)) => {
            let a = match s.layout {
                Layout::Vertical => vertical(ui, st, s),
                _ => paged(ui, st, s),
            };
            action.open_chapter = action.open_chapter.or(a.open_chapter);
            action.retry_page = a.retry_page;
        }
    }

    page_counter(ui, st, full);
    let pointer_near_top = ctx
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| p.y < full.top() + 70.0);
    let active = now - st.last_activity < CONTROLS_HIDE_AFTER || pointer_near_top;
    let shown = ctx.animate_bool_with_time(
        egui::Id::unique("reader-bar"),
        active && !s.toolbar_collapsed,
        theme::ANIM * 1.5,
    );
    if shown > 0.0 {
        let before = s.toolbar_collapsed;
        controls(&ctx, st, s, bookmarked, &mut action, shown);
        action.settings_changed |= s.toolbar_collapsed != before;
    }
    if s.toolbar_collapsed {
        toolbar_handle(&ctx, s, &mut action);
    }
    if active {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
    action
}

fn keyboard(ctx: &egui::Context, st: &mut ReaderState, s: &mut ReaderSettings) -> ReaderAction {
    let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
    let (fwd_arrow, back_arrow) = if s.rtl {
        (Key::ArrowLeft, Key::ArrowRight)
    } else {
        (Key::ArrowRight, Key::ArrowLeft)
    };
    let paged = s.layout != Layout::Vertical;
    let mut a = ReaderAction::default();
    if pressed(Key::PageDown) || pressed(Key::Space) || (paged && pressed(fwd_arrow)) {
        a = st.go(1, s.layout);
    } else if pressed(Key::PageUp) || pressed(Key::Backspace) || (paged && pressed(back_arrow)) {
        a = st.go(-1, s.layout);
    } else if pressed(Key::Home) {
        st.jump(0);
    } else if pressed(Key::End) {
        st.jump(usize::MAX);
    }
    if pressed(Key::Plus) || pressed(Key::Equals) {
        st.zoom = (st.zoom * 1.15).min(5.0);
    }
    if pressed(Key::Minus) {
        st.zoom = (st.zoom / 1.15).max(0.2);
    }
    if pressed(Key::Num0) {
        st.zoom = 1.0;
    }
    if pressed(Key::B) {
        a.bookmark = true;
    }
    if pressed(Key::A) && !paged {
        st.auto_scroll = !st.auto_scroll;
    }
    if pressed(Key::T) {
        s.toolbar_collapsed = !s.toolbar_collapsed;
        a.settings_changed = true;
    }
    if pressed(Key::F11) || pressed(Key::F) {
        st.fullscreen = !st.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(st.fullscreen));
    }
    if pressed(Key::Escape) {
        if st.fullscreen {
            st.fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        } else {
            a.close = true;
        }
    }
    a
}

/// Scale for a page of `size` inside `avail` (per-page width share already applied).
fn scale(size: Vec2, avail: Vec2, fit: Fit, zoom: f32) -> f32 {
    let base = match fit {
        Fit::Width => avail.x / size.x,
        Fit::Height => avail.y / size.y,
        Fit::Original => 1.0,
    };
    base * zoom
}

fn paged(ui: &mut egui::Ui, st: &mut ReaderState, s: &ReaderSettings) -> ReaderAction {
    let total = st.page_count();
    let avail = ui.available_size();
    let mut idx: Vec<usize> = (st.current..(st.current + s.layout.visible()).min(total)).collect();
    if s.rtl {
        idx.reverse(); // the first page of a spread sits on the right
    }
    let share = vec2(avail.x / idx.len().max(1) as f32, avail.y);
    let sizes: Vec<Vec2> = idx
        .iter()
        .map(|i| {
            let size = st
                .sizes
                .get(i)
                .copied()
                .unwrap_or(vec2(avail.y / PLACEHOLDER_RATIO, avail.y));
            size * scale(size, share, s.fit, st.zoom)
        })
        .collect();
    let content = vec2(
        sizes.iter().map(|v| v.x).sum(),
        sizes.iter().fold(0.0, |m: f32, v| m.max(v.y)),
    );

    let mut action = ReaderAction::default();
    egui::ScrollArea::both()
        .id_salt(("paged", st.generation, st.current))
        .auto_shrink(false)
        .show(ui, |ui| {
            let (rect, resp) = ui.allocate_exact_size(content.max(avail), Sense::click());
            let mut x = rect.center().x - content.x / 2.0;
            for (i, size) in idx.iter().zip(&sizes) {
                let page_rect = Rect::from_min_size(pos2(x, rect.center().y - size.y / 2.0), *size);
                x += size.x;
                match st.slots.get(i) {
                    Some(PageSlot::Ready(t)) => {
                        egui::Image::new((t.id(), t.size_vec2())).paint_at(ui, page_rect);
                    }
                    Some(PageSlot::Failed(e)) => {
                        if failed_page(ui, page_rect, e) {
                            action.retry_page = Some(*i);
                        }
                    }
                    _ => {
                        ui.put(
                            Rect::from_center_size(page_rect.center(), vec2(32.0, 32.0)),
                            egui::Spinner::new(),
                        );
                    }
                }
            }
            // Click the left/right third to turn pages; direction follows the reading order.
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let third = ui.clip_rect().width() / 3.0;
                let left = p.x < ui.clip_rect().left() + third;
                let right = p.x > ui.clip_rect().right() - third;
                let forward = if s.rtl { left } else { right };
                let backward = if s.rtl { right } else { left };
                if forward {
                    action = st.go(1, s.layout);
                } else if backward {
                    action = st.go(-1, s.layout);
                }
            }
        });
    action
}

fn vertical(ui: &mut egui::Ui, st: &mut ReaderState, s: &ReaderSettings) -> ReaderAction {
    let total = st.page_count();
    let gap = s.page_gap.max(0.0);
    let avail = ui.available_size();
    let width = match s.fit {
        Fit::Width => avail.x,
        _ => avail.x.min(900.0),
    } * st.zoom;
    let heights: Vec<f32> = (0..total)
        .map(|i| {
            st.sizes
                .get(&i)
                .map_or(width * PLACEHOLDER_RATIO, |v| width * v.y / v.x)
        })
        .collect();
    let tops: Vec<f32> = heights
        .iter()
        .scan(0.0, |y, h| {
            let top = *y;
            *y += h + gap;
            Some(top)
        })
        .collect();
    let full_height = tops.last().zip(heights.last()).map_or(0.0, |(t, h)| t + h);

    let mut area = egui::ScrollArea::vertical()
        .id_salt(("vertical", st.generation))
        .auto_shrink(false);
    // Extra scrolling on top of egui's own: speed multiplier, held arrow keys, auto-scroll.
    let (wheel, dt, down, up) = ui.input(|i| {
        (i.smooth_scroll_delta.y, i.stable_dt.min(0.1), i.key_down(Key::ArrowDown), i.key_down(Key::ArrowUp))
    });
    let mut extra = -wheel * (s.scroll_speed - 1.0);
    let key_speed = 900.0 * s.scroll_speed * dt;
    if down {
        extra += key_speed;
    }
    if up {
        extra -= key_speed;
    }
    if st.auto_scroll {
        extra += s.auto_scroll_speed * dt;
        ui.ctx().request_repaint();
        if st.offset + 1.0 >= full_height - avail.y {
            st.auto_scroll = false; // reached the end of the chapter
        }
    }
    if down || up {
        ui.ctx().request_repaint();
    }
    if let Some(i) = st.scroll_to.take()
        && i < tops.len()
    {
        area = area.vertical_scroll_offset(tops[i]);
    } else if extra != 0.0 {
        area = area.vertical_scroll_offset((st.offset + extra).max(0.0));
    }
    let mut action = ReaderAction::default();
    let out = area.show_viewport(ui, |ui, viewport| {
        ui.set_height(full_height);
        let origin = ui.min_rect().min;
        let left = origin.x + (avail.x - width).max(0.0) / 2.0;
        // The page under 40% of the viewport counts as current.
        let probe = viewport.top() + viewport.height() * 0.4;
        st.current = tops.iter().rposition(|t| *t <= probe).unwrap_or(0);
        for i in 0..total {
            let (top, h) = (tops[i], heights[i]);
            if top + h < viewport.top() || top > viewport.bottom() {
                continue; // only paint visible pages
            }
            let r = Rect::from_min_size(pos2(left, origin.y + top), vec2(width, h));
            match st.slots.get(&i) {
                Some(PageSlot::Ready(t)) => {
                    egui::Image::new((t.id(), t.size_vec2())).paint_at(ui, r)
                }
                Some(PageSlot::Failed(e)) => {
                    if failed_page(ui, r, e) {
                        action.retry_page = Some(i);
                    }
                }
                _ => {
                    ui.put(
                        Rect::from_center_size(r.center(), vec2(32.0, 32.0)),
                        egui::Spinner::new(),
                    );
                }
            }
        }
    });
    st.offset = out.state.clamped_offset().y;
    action
}

/// Draws an error box with a Retry button; returns true if Retry was clicked.
fn failed_page(ui: &mut egui::Ui, r: Rect, err: &str) -> bool {
    ui.painter()
        .rect_filled(r.shrink(8.0), 8, Color32::from_gray(25));
    let mut retry = false;
    ui.put(
        Rect::from_center_size(r.center(), vec2(260.0, 80.0)),
        |ui: &mut egui::Ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(format!("Page failed: {err}"))
                        .color(Color32::LIGHT_RED)
                        .small(),
                );
                retry = ui.button("Retry").clicked();
            })
            .response
        },
    );
    retry
}

fn page_counter(ui: &egui::Ui, st: &ReaderState, full: Rect) {
    let total = st.page_count();
    if total == 0 {
        return;
    }
    let groups = st
        .chapters
        .get(st.chapter)
        .map(|c| c.scanlators.join(", "))
        .unwrap_or_default();
    let text = if groups.is_empty() {
        format!("{} / {}", st.current + 1, total)
    } else {
        format!("{groups}  ·  {} / {}", st.current + 1, total)
    };
    ui.painter().text(
        full.right_bottom() - vec2(14.0, 10.0),
        Align2::RIGHT_BOTTOM,
        text,
        FontId::proportional(13.0),
        Color32::from_white_alpha(150),
    );
}

fn controls(
    ctx: &egui::Context,
    st: &mut ReaderState,
    s: &mut ReaderSettings,
    bookmarked: bool,
    action: &mut ReaderAction,
    shown: f32,
) {
    // Slides down from above the window edge and fades in.
    egui::Area::new(egui::Id::unique("reader-controls"))
        .anchor(Align2::CENTER_TOP, vec2(0.0, -72.0 + 80.0 * shown))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_opacity(shown);
            egui::Frame::new()
                .fill(Color32::from_black_alpha(225))
                .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(18)))
                .corner_radius(14)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button(theme::label(icons::CARET_LEFT, "Back")).clicked() {
                            action.close = true;
                        }
                        ui.label(RichText::new(&st.title).strong());
                        let current = st
                            .chapters
                            .get(st.chapter)
                            .map(Chapter::label)
                            .unwrap_or_default();
                        egui::ComboBox::from_id_salt("chapter")
                            .selected_text(current)
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for (i, c) in st.chapters.iter().enumerate() {
                                    if ui.selectable_label(i == st.chapter, c.label()).clicked()
                                        && i != st.chapter
                                    {
                                        action.open_chapter = Some((i, false));
                                    }
                                }
                            });
                        ui.separator();
                        let before = s.clone();
                        let layout = |l: Layout| match l {
                            Layout::Single => theme::label(icons::FRAME_CORNERS, "Single"),
                            Layout::Double => theme::label(icons::COLUMNS, "Double"),
                            Layout::Vertical => theme::label(icons::ROWS, "Vertical"),
                        };
                        egui::ComboBox::from_id_salt("layout")
                            .selected_text(layout(s.layout))
                            .show_ui(ui, |ui| {
                                for l in [Layout::Single, Layout::Double, Layout::Vertical] {
                                    ui.selectable_value(&mut s.layout, l, layout(l));
                                }
                            });
                        let dir = if s.rtl { "RTL" } else { "LTR" };
                        if ui
                            .button(theme::label(icons::ARROWS_LEFT_RIGHT, dir))
                            .on_hover_text("Reading direction: right-to-left / left-to-right")
                            .clicked()
                        {
                            s.rtl = !s.rtl;
                        }
                        egui::ComboBox::from_id_salt("fit")
                            .selected_text(match s.fit {
                                Fit::Width => "Fit width",
                                Fit::Height => "Fit height",
                                Fit::Original => "Original",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.fit, Fit::Width, "Fit width");
                                ui.selectable_value(&mut s.fit, Fit::Height, "Fit height");
                                ui.selectable_value(&mut s.fit, Fit::Original, "Original");
                            });
                        if ui
                            .small_button(icons::MINUS)
                            .on_hover_text("Zoom out (-)")
                            .clicked()
                        {
                            st.zoom = (st.zoom / 1.15).max(0.2);
                        }
                        ui.label(format!("{:.0}%", st.zoom * 100.0));
                        if ui
                            .small_button(icons::PLUS)
                            .on_hover_text("Zoom in (+)")
                            .clicked()
                        {
                            st.zoom = (st.zoom * 1.15).min(5.0);
                        }
                        if s.layout == Layout::Vertical {
                            let label = if st.auto_scroll {
                                theme::label(icons::PAUSE, "Auto").color(theme::ACCENT)
                            } else {
                                theme::label(icons::PLAY, "Auto")
                            };
                            if ui.button(label).on_hover_text("Auto-scroll (A)").clicked() {
                                st.auto_scroll = !st.auto_scroll;
                            }
                        }
                        ui.menu_button(icons::GEAR_SIX, |ui| {
                            reader_sliders(ui, s);
                            ui.separator();
                            ui.add(egui::Slider::new(&mut s.preload, 1..=8).text("Preload pages"));
                            ui.add(
                                egui::Slider::new(&mut s.keep_in_memory, 2..=30)
                                    .text("Pages in memory"),
                            );
                        })
                        .response
                        .on_hover_text("Loading settings");
                        let mark = if bookmarked {
                            RichText::new(format!("{}  Saved", icons::BOOKMARK_SIMPLE))
                                .color(theme::ACCENT)
                        } else {
                            theme::label(icons::BOOKMARK_SIMPLE, "Bookmark")
                        };
                        if ui.button(mark).on_hover_text("B").clicked() && !bookmarked {
                            action.bookmark = true;
                        }
                        let fs = if st.fullscreen {
                            icons::ARROWS_IN
                        } else {
                            icons::ARROWS_OUT
                        };
                        if ui
                            .button(fs)
                            .on_hover_text("Fullscreen (F / F11)")
                            .clicked()
                        {
                            st.fullscreen = !st.fullscreen;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(st.fullscreen));
                        }
                        if ui
                            .button(icons::CARET_UP)
                            .on_hover_text("Hide toolbar (T)")
                            .clicked()
                        {
                            s.toolbar_collapsed = true;
                        }
                        if *s != before {
                            action.settings_changed = true;
                            st.scroll_to = Some(st.current);
                        }
                    });
                    let credits = st
                        .chapters
                        .get(st.chapter)
                        .map(|c| c.scanlators.join(", "))
                        .unwrap_or_default();
                    let source = &st.source_name;
                    ui.label(
                        RichText::new(if credits.is_empty() {
                            format!("Source: {source}")
                        } else {
                            format!("Translated by {credits} · Source: {source}")
                        })
                        .small()
                        .color(theme::MUTED),
                    );
                });
        });
}

/// Small tab shown while the toolbar is collapsed; click (or T) to bring it back.
fn toolbar_handle(ctx: &egui::Context, s: &mut ReaderSettings, action: &mut ReaderAction) {
    egui::Area::new(egui::Id::unique("reader-handle"))
        .anchor(Align2::CENTER_TOP, vec2(0.0, 0.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 20.0), Sense::click());
            let hov = theme::anim(ui, resp.id, resp.hovered());
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: 10,
                    se: 10,
                },
                Color32::from_black_alpha((90.0 + 140.0 * hov) as u8),
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                icons::CARET_DOWN,
                FontId::proportional(14.0),
                Color32::from_white_alpha((110.0 + 145.0 * hov) as u8),
            );
            if resp.on_hover_text("Show toolbar (T)").clicked() {
                s.toolbar_collapsed = false;
                action.settings_changed = true;
            }
        });
}

/// Scrolling sliders shared by the reader toolbar and the Settings page.
pub fn reader_sliders(ui: &mut egui::Ui, s: &mut ReaderSettings) {
    ui.add(
        egui::Slider::new(&mut s.scroll_speed, 0.25..=4.0)
            .step_by(0.05)
            .suffix("×")
            .text("Scroll speed"),
    )
    .on_hover_text("Mouse wheel and arrow keys in the vertical layout");
    ui.add(
        egui::Slider::new(&mut s.auto_scroll_speed, 20.0..=800.0)
            .logarithmic(true)
            .suffix(" px/s")
            .text("Auto-scroll speed"),
    );
    ui.add(egui::Slider::new(&mut s.page_gap, 0.0..=40.0).suffix(" px").text("Gap between pages"));
}
