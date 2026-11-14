//! Home "Continue reading", History, and Bookmarks/Favorites views.

use eframe::egui::{self, RichText, Sense, vec2};

use super::browse::{CardAct, CardCmd, Covers, card, card_menu, cover};
use super::views::{empty_state, heading};
use super::{icons, theme};
use crate::core::models::MangaKey;
use crate::database::library::{Bookmark, Favorite, Progress, ResumePoint, SHELVES};

/// What the user asked for on a library page.
#[derive(Default)]
pub struct LibraryAction {
    pub resume: Option<ResumePoint>,
    pub open: Option<MangaKey>,
    pub remove_progress: Option<MangaKey>,
    pub clear_history: bool,
    pub delete_bookmark: Option<i64>,
    pub clear_viewed: bool,
    pub remove_viewed: Option<MangaKey>,
    pub remove_favorite: Option<MangaKey>,
    /// Other card menu choices (open, download all, favorite).
    pub card: Option<CardAct>,
}

/// Routes a card menu choice: Remove goes to the list's own field, the rest to `card`.
fn route(action: &mut LibraryAction, a: Option<CardAct>, remove: impl FnOnce(&mut LibraryAction, MangaKey)) {
    match a {
        Some(a) if a.cmd == CardCmd::Remove => remove(action, a.key),
        Some(a) => action.card = Some(a),
        None => {}
    }
}

/// Home row of recently opened titles (`added_at` = view time, unix ms).
pub fn viewed_row(
    ui: &mut egui::Ui,
    viewed: &[Favorite],
    covers: &mut Covers,
    now_ms: i64,
    action: &mut LibraryAction,
) {
    if viewed.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Recently viewed").strong().size(18.0));
        if ui.small_button("Clear").clicked() {
            action.clear_viewed = true;
        }
    });
    ui.add_space(6.0);
    egui::ScrollArea::horizontal().id_salt("viewed").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for f in viewed.iter().take(16) {
                let sub = ago(now_ms, f.added_at);
                let resp = card(ui, covers, &f.key.source, f.cover_url.as_deref(), &f.title, &sub);
                if resp.clicked() {
                    action.open = Some(f.key.clone());
                }
                let m = card_menu(&resp, &f.key, &f.title, f.cover_url.as_deref(), true, Some("Remove from recently viewed"));
                route(action, m, |a, k| a.remove_viewed = Some(k));
            }
        });
    });
    ui.add_space(20.0);
}

/// Human-friendly age of a timestamp, both in unix milliseconds.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let secs = (now_ms - then_ms).max(0) / 1000;
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

fn page_text(page: usize, count: Option<u32>) -> String {
    match count {
        Some(n) => format!("p. {} / {n}", page + 1),
        None => format!("p. {}", page + 1),
    }
}

pub fn continue_row(
    ui: &mut egui::Ui,
    recent: &[Progress],
    covers: &mut Covers,
    action: &mut LibraryAction,
) {
    ui.label(RichText::new("Continue reading").strong().size(18.0));
    ui.add_space(6.0);
    if recent.is_empty() {
        empty_state(
            ui,
            "Nothing in progress",
            "Works you open in the reader show up here.",
        );
        return;
    }
    egui::ScrollArea::horizontal()
        .id_salt("continue")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for p in recent.iter().take(12) {
                    let sub = format!(
                        "{} · {}",
                        p.chapter_label,
                        page_text(p.at.page, p.page_count)
                    );
                    let resp = card(
                        ui,
                        covers,
                        &p.at.key.source,
                        p.cover_url.as_deref(),
                        &p.title,
                        &sub,
                    );
                    if resp.clicked() {
                        action.resume = Some(p.at.clone());
                    }
                    let m = card_menu(&resp, &p.at.key, &p.title, p.cover_url.as_deref(), true, Some("Remove from Continue reading"));
                    route(action, m, |a, k| a.remove_progress = Some(k));
                }
            });
        });
    ui.add_space(20.0);
}

/// One list row: small cover, title, detail line, and trailing buttons. Returns true if the
/// cover was clicked.
fn row(
    ui: &mut egui::Ui,
    covers: &mut Covers,
    source: &str,
    cover_url: Option<&str>,
    title: &str,
    detail: &str,
    buttons: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut clicked = false;
    theme::card().inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, resp) = ui.allocate_exact_size(vec2(56.0, 80.0), Sense::click());
            cover(ui, covers, source, cover_url, rect, 6);
            clicked |= resp.clicked();
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).strong());
                ui.label(RichText::new(detail).color(theme::MUTED));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), buttons);
        });
    });
    ui.add_space(6.0);
    clicked
}

pub fn history(
    ui: &mut egui::Ui,
    recent: &[Progress],
    covers: &mut Covers,
    now_ms: i64,
) -> LibraryAction {
    let mut action = LibraryAction::default();
    ui.horizontal(|ui| {
        heading(
            ui,
            "History",
            "Everything you have opened, most recent first",
        );
    });
    if recent.is_empty() {
        empty_state(
            ui,
            "No history",
            "Works you read are listed here with your position.",
        );
        return action;
    }
    if ui
        .button(theme::label(icons::TRASH, "Clear history"))
        .clicked()
    {
        action.clear_history = true;
    }
    ui.add_space(8.0);
    for p in recent {
        let detail = format!(
            "{} · {} · {}",
            p.chapter_label,
            page_text(p.at.page, p.page_count),
            ago(now_ms, p.updated_at)
        );
        let clicked = row(
            ui,
            covers,
            &p.at.key.source,
            p.cover_url.as_deref(),
            &p.title,
            &detail,
            |ui| {
                if ui.button(theme::label(icons::X, "Remove")).clicked() {
                    action.remove_progress = Some(p.at.key.clone());
                }
                if ui.button(theme::label(icons::PLAY, "Continue")).clicked() {
                    action.resume = Some(p.at.clone());
                }
            },
        );
        if clicked {
            action.open = Some(p.at.key.clone());
        }
    }
    action
}

pub fn bookmarks(
    ui: &mut egui::Ui,
    favorites: &[Favorite],
    marks: &[Bookmark],
    covers: &mut Covers,
    now_ms: i64,
    // shown shelf id ("" = all)
    shelf: &mut String,
) -> LibraryAction {
    let mut action = LibraryAction::default();
    heading(ui, "Bookmarks", "Favorite titles and saved pages");

    ui.horizontal(|ui| {
        ui.label(RichText::new("Favorites").strong().size(18.0));
        ui.add_space(8.0);
        let count = |id: &str| favorites.iter().filter(|f| id.is_empty() || f.shelf == id).count();
        for (id, name) in std::iter::once(("", "All")).chain(SHELVES) {
            if theme::pill(ui, &format!("{name} ({})", count(id)), shelf == id).clicked() {
                *shelf = id.to_string();
            }
        }
    });
    ui.add_space(6.0);
    let shown: Vec<&Favorite> = favorites.iter().filter(|f| shelf.is_empty() || f.shelf == *shelf).collect();
    if shown.is_empty() {
        empty_state(
            ui,
            "No favorites",
            "Use \"Add to favorites\" on a details page to save a title here.",
        );
    } else {
        egui::ScrollArea::horizontal()
            .id_salt("favorites")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for f in shown {
                        let shelf_name = SHELVES.iter().find(|s| s.0 == f.shelf).map_or("", |s| s.1);
                        let sub = format!("{shelf_name} · added {}", ago(now_ms, f.added_at * 1000));
                        let resp = card(
                            ui,
                            covers,
                            &f.key.source,
                            f.cover_url.as_deref(),
                            &f.title,
                            &sub,
                        );
                        if resp.clicked() {
                            action.open = Some(f.key.clone());
                        }
                        let m = card_menu(&resp, &f.key, &f.title, f.cover_url.as_deref(), true, Some("Remove from favorites"));
                        route(&mut action, m, |a, k| a.remove_favorite = Some(k));
                    }
                });
            });
    }
    ui.add_space(20.0);

    ui.label(RichText::new("Saved pages").strong().size(18.0));
    ui.add_space(6.0);
    if marks.is_empty() {
        empty_state(
            ui,
            "No bookmarks",
            "Use the bookmark button in the reader to save a page.",
        );
    }
    for b in marks {
        let detail = format!(
            "{} · p. {} · {}",
            b.chapter_label,
            b.at.page + 1,
            ago(now_ms, b.created_at * 1000)
        );
        let clicked = row(
            ui,
            covers,
            &b.at.key.source,
            b.cover_url.as_deref(),
            &b.title,
            &detail,
            |ui| {
                if ui.button(theme::label(icons::TRASH, "Delete")).clicked() {
                    action.delete_bookmark = Some(b.id);
                }
                if ui.button(theme::label(icons::BOOK_OPEN, "Open")).clicked() {
                    action.resume = Some(b.at.clone());
                }
            },
        );
        if clicked {
            action.resume = Some(b.at.clone());
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_buckets() {
        let now = 10_000_000_000;
        assert_eq!(ago(now, now - 5_000), "just now");
        assert_eq!(ago(now, now - 5 * 60_000), "5 min ago");
        assert_eq!(ago(now, now - 3 * 3_600_000), "3 h ago");
        assert_eq!(ago(now, now - 2 * 86_400_000), "2 d ago");
        assert_eq!(ago(now, now + 1_000), "just now");
    }
}

#[derive(Default)]
pub struct DownloadsAction {
    pub open: Option<usize>,
    pub delete: Option<usize>,
    pub favorite: Option<(usize, &'static str)>,
}

/// Offline library: each downloaded work with its cover (`cover_urls[i]`), chapters and size.
/// `progress` is (chapters in the queue, pages done, pages total).
pub fn downloads(
    ui: &mut egui::Ui,
    entries: &[crate::downloads::Entry],
    cover_urls: &[Option<String>],
    (jobs, pages_done, pages_total): (usize, usize, usize),
    covers: &mut Covers,
) -> DownloadsAction {
    let mut out = DownloadsAction::default();
    heading(ui, "Downloads", "Saved chapters read without a connection");
    if jobs > 0 {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let frac = if pages_total == 0 { 0.0 } else { pages_done as f32 / pages_total as f32 };
            ui.label(
                RichText::new(format!("{} Downloading {jobs} chapter(s)", icons::DOWNLOAD_SIMPLE))
                    .strong(),
            );
            ui.add(
                egui::ProgressBar::new(frac)
                    .text(format!("{pages_done} / {pages_total} pages"))
                    .animate(true),
            );
        });
        ui.add_space(12.0);
    }
    if entries.is_empty() {
        empty_state(
            ui,
            "Nothing downloaded yet",
            "Open a title and use the download button beside a chapter, or Download all.",
        );
        return out;
    }
    const GAP: f32 = 16.0;
    let cols = (((ui.available_width() + GAP) / (150.0 + GAP)) as usize).max(1);
    for (row, chunk) in entries.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for (j, e) in chunk.iter().enumerate() {
                let i = row * cols + j;
                ui.vertical(|ui| {
                    let mb = e.size as f64 / (1024.0 * 1024.0);
                    let sub = format!("{} ch · {mb:.0} MB", e.chapters.len());
                    let title = e.manga.titles.display();
                    let resp = card(ui, covers, &e.manga.key.source, cover_urls[i].as_deref(), title, &sub);
                    if resp.clicked() {
                        out.open = Some(i);
                    }
                    match card_menu(&resp, &e.manga.key, title, None, false, Some("Delete download")).map(|a| a.cmd) {
                        Some(CardCmd::Open) => out.open = Some(i),
                        Some(CardCmd::Remove) => out.delete = Some(i),
                        Some(CardCmd::Favorite(shelf)) => out.favorite = Some((i, shelf)),
                        _ => {}
                    }
                    if ui
                        .small_button(theme::label(icons::TRASH, "Delete"))
                        .on_hover_text("Remove the downloaded files of this title")
                        .clicked()
                    {
                        out.delete = Some(i);
                    }
                });
            }
        });
        ui.add_space(18.0);
    }
    out
}
