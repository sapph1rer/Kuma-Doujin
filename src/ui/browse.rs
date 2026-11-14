//! Cover grids (Discover / Popular / Latest / Search / Home rows) and the details page.
//! Views only mutate UI state and return what they want done; `app.rs` runs the work.

use std::collections::HashMap;

use eframe::egui::{
    self, Color32, ColorImage, Label, RichText, Sense, Stroke, StrokeKind, TextureHandle, Vec2,
    vec2,
};
use tokio::task::AbortHandle;

use super::views::empty_state;
use super::{icons, theme};
use crate::core::models::{Chapter, Manga, MangaKey, MangaSummary, SearchQuery, SortOrder, Tag};
use crate::database::library::{Progress, ResumePoint};
use crate::discovery::aggregator::Aggregated;
use crate::downloads::{DlStatus, Entry};
use crate::sources::registry::SourceRegistry;

const CARD: Vec2 = vec2(150.0, 214.0);
pub(crate) const LANGUAGES: &[(&str, &str)] = &[
    ("", "Any language"),
    ("en", "English"),
    ("ja", "Japanese"),
    ("th", "Thai"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("id", "Indonesian"),
    ("vi", "Vietnamese"),
    ("pt-br", "Portuguese (BR)"),
];

pub struct BrowseState {
    pub query: SearchQuery,
    /// `None` = all sources.
    pub source: Option<String>,
    pub result: Option<Aggregated>,
    pub loading: bool,
    pub needs_load: bool,
    /// Incremented per request; stale responses are dropped.
    pub generation: u64,
    pub pending: Option<AbortHandle>,
}

impl BrowseState {
    pub fn new(sort: SortOrder) -> Self {
        Self {
            query: SearchQuery {
                sort,
                page: 1,
                ..Default::default()
            },
            source: None,
            result: None,
            loading: false,
            needs_load: true,
            generation: 0,
            pending: None,
        }
    }
}

pub enum TagsSlot {
    Loading,
    Ready(Vec<Tag>),
    Failed(String),
}

#[derive(Default)]
pub struct Actions {
    pub open: Option<MangaKey>,
    pub load_tags: Option<String>,
    /// User toggled the 18+ filter (the app confirms age before enabling).
    pub set_adult: Option<bool>,
}

enum CoverSlot {
    Loading,
    /// Texture and the time (egui seconds) it arrived.
    Ready(TextureHandle, f64),
    Failed,
}

const COVER_FADE: f64 = 0.35;

/// Cover textures by URL. Each URL is requested once (dedupes downloads).
#[derive(Default)]
pub struct Covers {
    slots: HashMap<String, CoverSlot>,
    /// (source id, url) requested this frame; drained by the app.
    pub wanted: Vec<(String, String)>,
}

impl Covers {
    /// The cover texture and how far its fade-in is (0..1), or None while loading.
    pub(crate) fn get(
        &mut self,
        source: &str,
        url: &str,
        now: f64,
    ) -> Option<(&TextureHandle, f32)> {
        if !self.slots.contains_key(url) {
            self.slots.insert(url.to_string(), CoverSlot::Loading);
            self.wanted.push((source.to_string(), url.to_string()));
        }
        match self.slots.get(url) {
            Some(CoverSlot::Ready(t, at)) => {
                Some((t, ((now - at) / COVER_FADE).clamp(0.0, 1.0) as f32))
            }
            _ => None,
        }
    }

    fn failed(&self, url: &str) -> bool {
        matches!(self.slots.get(url), Some(CoverSlot::Failed))
    }

    pub fn finish(&mut self, ctx: &egui::Context, url: String, img: Result<ColorImage, String>) {
        // shortcut: drop all textures past 600; visible ones reload from the disk cache.
        if self.slots.len() > 600 {
            self.slots.retain(|_, s| matches!(s, CoverSlot::Loading));
        }
        let slot = match img {
            Ok(img) => CoverSlot::Ready(
                ctx.load_texture(&url, img, egui::TextureOptions::LINEAR),
                ctx.input(|i| i.time),
            ),
            Err(e) => {
                tracing::debug!(%url, error = %e, "cover failed");
                CoverSlot::Failed
            }
        };
        self.slots.insert(url, slot);
    }

    pub fn clear(&mut self) {
        self.slots.clear();
    }
}

/// Cover + title + subtitle card. Returns true when clicked.
pub(crate) fn card(
    ui: &mut egui::Ui,
    covers: &mut Covers,
    source: &str,
    cover_url: Option<&str>,
    title: &str,
    subtitle: &str,
) -> bool {
    ui.vertical(|ui| {
        ui.set_width(CARD.x);
        let (slot, resp) = ui.allocate_exact_size(CARD, Sense::click());
        let hov = theme::anim(ui, resp.id, resp.hovered());
        // Lift and grow slightly on hover, with a soft accent glow underneath.
        let rect = slot.translate(vec2(0.0, -5.0 * hov)).expand(3.0 * hov);
        if hov > 0.0 {
            ui.painter().rect_filled(
                rect.translate(vec2(0.0, 6.0)).expand(2.0),
                12,
                theme::ACCENT.gamma_multiply(0.18 * hov),
            );
        }
        cover(ui, covers, source, cover_url, rect, 10);
        if hov > 0.0 {
            ui.painter().rect_stroke(
                rect,
                10,
                Stroke::new(2.0, theme::ACCENT.gamma_multiply(hov)),
                StrokeKind::Inside,
            );
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.add(Label::new(RichText::new(title).strong()).truncate());
        ui.add(Label::new(RichText::new(subtitle).small().color(theme::MUTED)).truncate());
        resp.on_hover_text(title).clicked()
    })
    .inner
}

/// Paints a cover (or a placeholder while it loads) into `rect`.
pub(crate) fn cover(
    ui: &egui::Ui,
    covers: &mut Covers,
    source: &str,
    url: Option<&str>,
    rect: egui::Rect,
    radius: u8,
) {
    let now = ui.input(|i| i.time);
    let failed = url.is_some_and(|u| covers.failed(u));
    match url.and_then(|u| covers.get(source, u, now)) {
        Some((t, fade)) => {
            if fade < 1.0 {
                ui.painter().rect_filled(rect, radius, theme::SURFACE);
                ui.ctx().request_repaint();
            }
            egui::Image::new((t.id(), t.size_vec2()))
                .corner_radius(radius)
                .tint(Color32::WHITE.gamma_multiply(fade))
                .paint_at(ui, rect);
        }
        None if url.is_some() && !failed => theme::skeleton(ui, rect, radius),
        None => {
            ui.painter().rect_filled(rect, radius, theme::SURFACE);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icons::IMAGE_BROKEN,
                egui::FontId::proportional(28.0),
                theme::MUTED,
            );
        }
    }
}

fn cover_card(
    ui: &mut egui::Ui,
    covers: &mut Covers,
    m: &MangaSummary,
    sources: &SourceRegistry,
) -> bool {
    let source = sources
        .get(&m.key.source)
        .map_or(m.key.source.as_str(), |s| &s.info.name);
    let lang = m.language.as_deref().unwrap_or("?");
    card(
        ui,
        covers,
        &m.key.source,
        m.cover_url.as_deref(),
        m.titles.display(),
        &format!("{source} · {lang}"),
    )
}

fn grid(
    ui: &mut egui::Ui,
    items: &[MangaSummary],
    covers: &mut Covers,
    reg: &SourceRegistry,
    actions: &mut Actions,
) {
    const GAP: f32 = 16.0;
    // Explicit rows: `horizontal_wrapped` can't wrap `ui.vertical` children (they claim full width).
    let cols = (((ui.available_width() + GAP) / (CARD.x + GAP)) as usize).max(1);
    for row in items.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for m in row {
                if cover_card(ui, covers, m, reg) {
                    actions.open = Some(m.key.clone());
                }
            }
        });
        ui.add_space(18.0);
    }
}

pub fn browse(
    ui: &mut egui::Ui,
    st: &mut BrowseState,
    (title, subtitle): (&str, &str),
    reg: &SourceRegistry,
    tags: &HashMap<String, TagsSlot>,
    covers: &mut Covers,
    // 18+ sources are listed only in the Doujin category with 18+ mode on
    adult_sources: bool,
) -> Actions {
    let mut actions = Actions::default();
    theme::hero(ui, title, subtitle);
    if reg.catalogs().next().is_none() {
        empty_state(ui, "No sources", super::views::NO_CATALOG);
        return actions;
    }

    let mut reload = false;
    ui.horizontal_wrapped(|ui| {
        let edit = ui.add(
            egui::TextEdit::singleline(&mut st.query.text)
                .hint_text("Search title…")
                .desired_width(280.0),
        );
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        reload |= enter
            || ui
                .button(theme::label(icons::MAGNIFYING_GLASS, "Search"))
                .clicked();

        let before = (st.query.sort, st.query.languages.clone(), st.source.clone());
        egui::ComboBox::from_id_salt("sort")
            .selected_text(match st.query.sort {
                SortOrder::Relevance => "Relevance",
                SortOrder::Popular => "Popular",
                SortOrder::Latest => "Latest",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut st.query.sort, SortOrder::Relevance, "Relevance");
                ui.selectable_value(&mut st.query.sort, SortOrder::Popular, "Popular");
                ui.selectable_value(&mut st.query.sort, SortOrder::Latest, "Latest");
            });

        let current = st.query.languages.first().cloned().unwrap_or_default();
        let label = LANGUAGES
            .iter()
            .find(|l| l.0 == current)
            .map_or("Language", |l| l.1);
        egui::ComboBox::from_id_salt("lang")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (code, name) in LANGUAGES {
                    if ui.selectable_label(current == *code, *name).clicked() {
                        st.query.languages = if code.is_empty() {
                            vec![]
                        } else {
                            vec![code.to_string()]
                        };
                    }
                }
            });

        let src_label = st
            .source
            .as_deref()
            .and_then(|id| reg.get(id))
            .map_or("All sources", |s| s.info.name.as_str());
        egui::ComboBox::from_id_salt("source")
            .selected_text(src_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut st.source, None, "All sources");
                for (info, _) in reg.catalogs().filter(|(i, _)| !i.adult || adult_sources) {
                    ui.selectable_value(&mut st.source, Some(info.id.clone()), &info.name);
                }
            });

        // Tag ids are source-specific, so tag filtering needs one selected source.
        if let Some(src) = st.source.clone() {
            let n = st.query.include_tags.len();
            ui.menu_button(format!("Tags ({n})"), |ui| match tags.get(&src) {
                None => {
                    actions.load_tags = Some(src);
                    ui.spinner();
                }
                Some(TagsSlot::Loading) => {
                    ui.spinner();
                }
                Some(TagsSlot::Failed(e)) => {
                    ui.label(format!("Could not load tags: {e}"));
                }
                Some(TagsSlot::Ready(list)) => {
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            for t in list {
                                let mut on = st.query.include_tags.contains(&t.id);
                                if ui.checkbox(&mut on, &t.name).changed() {
                                    if on {
                                        st.query.include_tags.push(t.id.clone());
                                    } else {
                                        st.query.include_tags.retain(|id| id != &t.id);
                                    }
                                    reload = true;
                                }
                            }
                        });
                }
            });
        }

        let after = (st.query.sort, st.query.languages.clone(), st.source.clone());
        if before.2 != after.2 {
            st.query.include_tags.clear();
        }
        reload |= before != after;
    });
    if reload {
        st.query.page = 1;
        st.needs_load = true;
    }
    ui.add_space(12.0);

    if let Some(res) = &st.result {
        for (source, err) in &res.errors {
            ui.colored_label(
                Color32::from_rgb(0xE5, 0x6B, 0x6F),
                format!("{source}: {err}"),
            );
        }
    }
    match &st.result {
        None if st.loading => skeleton_grid(ui, 12),
        Some(res) if st.loading && res.items.is_empty() => skeleton_grid(ui, 12),
        None => {}
        Some(res) if res.items.is_empty() => empty_state(ui, "No results", "Try other filters."),
        Some(res) => {
            if st.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading more sources…").color(theme::MUTED));
                });
                ui.add_space(8.0);
            }
            grid(ui, &res.items, covers, reg, &mut actions);
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        st.query.page > 1,
                        egui::Button::new(theme::label(icons::CARET_LEFT, "Prev")),
                    )
                    .clicked()
                {
                    st.query.page -= 1;
                    st.needs_load = true;
                }
                ui.label(format!("Page {}", st.query.page));
                if ui
                    .add_enabled(
                        res.has_more,
                        egui::Button::new(theme::label(icons::CARET_RIGHT, "Next")),
                    )
                    .clicked()
                {
                    st.query.page += 1;
                    st.needs_load = true;
                }
            });
        }
    }
    actions
}

/// Horizontal strip of covers for the Home page.
pub fn row(
    ui: &mut egui::Ui,
    title: &str,
    st: &BrowseState,
    reg: &SourceRegistry,
    covers: &mut Covers,
    actions: &mut Actions,
) {
    ui.label(RichText::new(title).strong().size(18.0));
    ui.add_space(6.0);
    match &st.result {
        Some(res) if !res.items.is_empty() => {
            egui::ScrollArea::horizontal()
                .id_salt(title)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 16.0;
                        for m in res.items.iter().take(12) {
                            if cover_card(ui, covers, m, reg) {
                                actions.open = Some(m.key.clone());
                            }
                        }
                    });
                });
        }
        _ => skeleton_row(ui),
    }
    ui.add_space(20.0);
}

pub struct DetailsState {
    pub key: MangaKey,
    pub result: Option<Result<Manga, String>>,
    /// `None` while loading; only fetched for sources with in-app reading.
    pub chapters: Option<Result<Vec<Chapter>, String>>,
    /// Translation language for the chapter list ("" = any).
    pub chapter_lang: String,
}

#[derive(Default)]
pub struct DetailsAction {
    pub back: bool,
    /// Open the reader at this chapter index.
    pub read: Option<usize>,
    pub reload_chapters: bool,
    pub resume: Option<ResumePoint>,
    pub toggle_favorite: bool,
    /// Download these chapter indices for offline reading.
    pub download: Vec<usize>,
}

/// Offline state of the work on the details page.
pub struct DlInfo<'a> {
    pub saved: Option<&'a Entry>,
    /// chapter id -> queued / running / failed
    pub status: HashMap<String, DlStatus>,
}

pub fn details(
    ui: &mut egui::Ui,
    st: &mut DetailsState,
    reg: &SourceRegistry,
    covers: &mut Covers,
    favorite: bool,
    progress: Option<&Progress>,
    dl: &DlInfo,
) -> DetailsAction {
    let mut action = DetailsAction {
        back: ui.button(theme::label(icons::CARET_LEFT, "Back")).clicked(),
        ..Default::default()
    };
    ui.add_space(12.0);
    let m = match &st.result {
        None => {
            super::center_spinner(ui);
            return action;
        }
        Some(Err(e)) => {
            empty_state(ui, "Could not load details", e);
            return action;
        }
        Some(Ok(m)) => m,
    };
    let chapters = match &st.chapters {
        Some(Ok(c)) => c.as_slice(),
        _ => &[],
    };
    let source = reg.get(&m.key.source);
    let source_name = source.map_or(m.key.source.as_str(), |s| &s.info.name);

    ui.horizontal_top(|ui| {
        let size = vec2(240.0, 340.0);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        cover(ui, covers, &m.key.source, m.cover_url.as_deref(), rect, 12);
        ui.add_space(20.0);
        ui.vertical(|ui| {
            ui.heading(RichText::new(m.titles.display()).size(26.0).strong());
            let others: Vec<&str> = [&m.titles.japanese, &m.titles.thai, &m.titles.english]
                .into_iter()
                .flatten()
                .map(String::as_str)
                .filter(|t| *t != m.titles.display())
                .collect();
            if !others.is_empty() {
                ui.label(RichText::new(others.join(" · ")).color(theme::MUTED));
            }
            ui.add_space(10.0);
            // Hover a value to see which source(s) supplied it.
            let from = |key: &str| -> Option<String> {
                let names: Vec<&str> = m
                    .provenance
                    .get(key)?
                    .iter()
                    .map(|id| reg.get(id).map_or(id.as_str(), |s| s.info.name.as_str()))
                    .collect();
                Some(format!("From {}", names.join(" + ")))
            };
            let field = |ui: &mut egui::Ui, name: &str, key: &str, values: &[String]| {
                if !values.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("{name}:")).color(theme::MUTED));
                        let r = ui.label(values.join(", "));
                        if let Some(src) = from(key) {
                            r.on_hover_text(src);
                        }
                    });
                }
            };
            field(ui, "Artists", "artists", &m.artists);
            field(ui, "Circles", "circles", &m.circles);
            field(ui, "Parody", "parodies", &m.parodies);
            field(ui, "Characters", "characters", &m.characters);
            field(ui, "Language", "language", m.language.as_slice());
            field(ui, "Published", "published", m.published.as_slice());
            let pages = m.page_count.map(|p| p.to_string());
            field(ui, "Pages", "page_count", pages.as_slice());
            if !m.tags.is_empty() {
                ui.add_space(6.0);
                // Chips must be single widgets: a Frame in a wrapped row gets squeezed and its
                // label breaks one letter per line, pushing the rest of the page far down.
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for t in &m.tags {
                        ui.add(
                            egui::Button::new(RichText::new(t).small())
                                .fill(theme::SURFACE)
                                .corner_radius(12)
                                .sense(Sense::hover()),
                        );
                    }
                });
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if source.is_some_and(|s| s.can_read()) {
                    let start = egui::Button::new(
                        theme::label(icons::PLAY, "Start Reading")
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(theme::ACCENT)
                    .min_size(vec2(160.0, 38.0));
                    match progress {
                        Some(p) => {
                            let label = format!(
                                "{}  Continue · {} · p. {}",
                                icons::PLAY,
                                p.chapter_label,
                                p.at.page + 1
                            );
                            let resume = egui::Button::new(
                                RichText::new(label).strong().color(egui::Color32::WHITE),
                            )
                            .fill(theme::ACCENT)
                            .min_size(vec2(160.0, 38.0));
                            if ui.add(resume).clicked() {
                                action.resume = Some(p.at.clone());
                            }
                            if ui
                                .add_enabled(
                                    !chapters.is_empty(),
                                    egui::Button::new(theme::label(
                                        icons::ARROW_CLOCKWISE,
                                        "Read from start",
                                    )),
                                )
                                .clicked()
                            {
                                action.read = Some(0);
                            }
                        }
                        None => {
                            if ui.add_enabled(!chapters.is_empty(), start).clicked() {
                                action.read = Some(0);
                            }
                        }
                    }
                } else if let Some(url) = &m.source_url {
                    ui.hyperlink_to(
                        RichText::new(format!(
                            "Read on {source_name}  {}",
                            icons::ARROW_SQUARE_OUT
                        ))
                        .strong(),
                        url,
                    );
                    ui.label(
                        RichText::new("In-app reading is not enabled for this source.")
                            .small()
                            .color(theme::MUTED),
                    );
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let fav = if favorite {
                    theme::label(icons::HEART, "Favorited").color(theme::ACCENT)
                } else {
                    theme::label(icons::HEART, "Add to favorites")
                };
                if ui.button(fav).clicked() {
                    action.toggle_favorite = true;
                }
                ui.label(RichText::new("Source:").small().color(theme::MUTED));
                match &m.source_url {
                    Some(url) => ui.hyperlink_to(RichText::new(source_name).small(), url),
                    None => ui.label(RichText::new(source_name).small()),
                };
                // Attribution for merged metadata.
                for (src, _) in &m.links {
                    let (Some(url), Some(s)) = (
                        m.provenance
                            .get(&format!("url.{src}"))
                            .and_then(|u| u.first()),
                        reg.get(src),
                    ) else {
                        continue;
                    };
                    ui.label(
                        RichText::new("· metadata also from")
                            .small()
                            .color(theme::MUTED),
                    );
                    ui.hyperlink_to(RichText::new(&s.info.name).small(), url);
                }
            });
        });
    });
    if let Some(desc) = &m.description {
        ui.add_space(16.0);
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(desc);
        });
    }
    if source.is_some_and(|s| s.can_read()) {
        ui.add_space(16.0);
        chapter_list(ui, st, &mut action, dl);
    }
    action
}

fn chapter_list(ui: &mut egui::Ui, st: &mut DetailsState, action: &mut DetailsAction, dl: &DlInfo) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Chapters").strong().size(18.0));
        let label = LANGUAGES
            .iter()
            .find(|l| l.0 == st.chapter_lang)
            .map_or("Language", |l| l.1);
        egui::ComboBox::from_id_salt("chapter-lang")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (code, name) in LANGUAGES {
                    if ui
                        .selectable_label(st.chapter_lang == *code, *name)
                        .clicked()
                        && st.chapter_lang != *code
                    {
                        st.chapter_lang = code.to_string();
                        action.reload_chapters = true;
                    }
                }
            });
        if let Some(Ok(list)) = &st.chapters {
            let missing: Vec<usize> = (0..list.len())
                .filter(|i| !dl.saved.is_some_and(|e| e.has(&list[*i].id)))
                .filter(|i| !dl.status.contains_key(&list[*i].id))
                .collect();
            let all = format!("Download all ({})", missing.len());
            if !missing.is_empty()
                && ui
                    .button(theme::label(icons::DOWNLOAD_SIMPLE, &all))
                    .on_hover_text("Save every chapter for offline reading")
                    .clicked()
            {
                action.download = missing;
            }
            let saved = dl.saved.map_or(0, |e| e.chapters.len());
            if saved > 0 {
                ui.label(RichText::new(format!("{} {saved} offline", icons::CHECK)).color(theme::ACCENT));
            }
        }
    });
    ui.add_space(6.0);
    match &st.chapters {
        None => {
            super::center_spinner(ui);
        }
        Some(Err(e)) => empty_state(ui, "Could not load chapters", e),
        Some(Ok(list)) if list.is_empty() => empty_state(
            ui,
            "No readable chapters",
            "Try another language. Chapters hosted only on external sites are not listed.",
        ),
        Some(Ok(list)) => {
            for (i, c) in list.iter().enumerate() {
                let groups = c.scanlators.join(", ");
                let lang = c.language.as_deref().unwrap_or("?");
                let pages = c.page_count.map_or(String::new(), |p| format!(" · {p} p"));
                let text = format!("{}   ·   {lang}{pages}   ·   {groups}", c.label());
                ui.horizontal(|ui| {
                    const STATUS: f32 = 110.0;
                    // painted row: a Button centres its text, which pushed titles to the middle
                    let width = (ui.available_width() - STATUS).max(80.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
                    let hov = theme::anim(ui, resp.id, resp.hovered());
                    if hov > 0.0 {
                        ui.painter().rect_filled(rect, 6, theme::SURFACE.gamma_multiply(hov));
                    }
                    ui.painter().with_clip_rect(rect).text(
                        rect.left_center() + vec2(10.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        text,
                        egui::FontId::proportional(14.5),
                        ui.visuals().text_color(),
                    );
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        action.read = Some(i);
                    }
                    if dl.saved.is_some_and(|e| e.has(&c.id)) {
                        ui.label(RichText::new(format!("{} Offline", icons::CHECK)).color(theme::ACCENT))
                            .on_hover_text("Saved; reads without a connection");
                        return;
                    }
                    match dl.status.get(&c.id) {
                        Some(DlStatus::Queued) => {
                            ui.label(RichText::new("Queued").color(theme::MUTED));
                        }
                        Some(DlStatus::Running { done, total }) => {
                            let frac = if *total == 0 { 0.0 } else { *done as f32 / *total as f32 };
                            ui.add(
                                egui::ProgressBar::new(frac)
                                    .desired_width(90.0)
                                    .text(format!("{done}/{total}")),
                            );
                        }
                        Some(DlStatus::Failed(e)) => {
                            if ui.button("Retry").on_hover_text(format!("Download failed: {e}")).clicked() {
                                action.download = vec![i];
                            }
                        }
                        None => {
                            if ui
                                .small_button(icons::DOWNLOAD_SIMPLE)
                                .on_hover_text("Download for offline reading")
                                .clicked()
                            {
                                action.download = vec![i];
                            }
                        }
                    }
                });
            }
        }
    }
}

/// Pulsing placeholder cards while a grid loads.
fn skeleton_grid(ui: &mut egui::Ui, count: usize) {
    const GAP: f32 = 16.0;
    let cols = (((ui.available_width() + GAP) / (CARD.x + GAP)) as usize).max(1);
    for row in 0..count.div_ceil(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for _ in 0..cols.min(count - row * cols) {
                skeleton_card(ui);
            }
        });
        ui.add_space(18.0);
    }
}

fn skeleton_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        for _ in 0..6 {
            skeleton_card(ui);
        }
    });
}

fn skeleton_card(ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        let (rect, _) = ui.allocate_exact_size(CARD, Sense::hover());
        theme::skeleton(ui, rect, 10);
        let (line, _) = ui.allocate_exact_size(vec2(CARD.x * 0.8, 12.0), Sense::hover());
        theme::skeleton(ui, line, 4);
        let (line, _) = ui.allocate_exact_size(vec2(CARD.x * 0.5, 10.0), Sense::hover());
        theme::skeleton(ui, line, 4);
    });
}
