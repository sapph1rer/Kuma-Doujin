//! Simple page bodies. Each takes only the state it needs so views stay independent of
//! networking and storage details.

use eframe::egui::{self, RichText};

use super::{icons, theme};
use crate::sources::health::{HealthBook, Status};
use crate::sources::registry::SourceRegistry;
use crate::sources::traits::AccessLevel;

pub(crate) fn heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.heading(RichText::new(title).size(26.0).strong());
    ui.label(RichText::new(subtitle).color(theme::MUTED));
    ui.add_space(16.0);
}

pub(crate) fn empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).strong().size(16.0));
        ui.add_space(4.0);
        ui.label(RichText::new(body).color(theme::MUTED));
    });
    ui.add_space(12.0);
}

pub(crate) const NO_CATALOG: &str = "No catalog source is connected. Sources are added only \
after their API terms confirm the access is permitted. See the Sources page.";

/// Returns `(source id, enabled)` when the user flips a source's switch.
pub fn sources(
    ui: &mut egui::Ui,
    registry: &SourceRegistry,
    health: &HealthBook,
) -> Option<(String, bool)> {
    let mut toggled = None;
    heading(ui, "Sources", "Connected content providers");
    if registry.all().is_empty() {
        empty_state(ui, "No sources registered", NO_CATALOG);
    }
    for s in registry.all() {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let mut on = registry.is_enabled(&s.info.id);
                if ui.checkbox(&mut on, "").on_hover_text("Enabled").changed() {
                    toggled = Some((s.info.id.clone(), on));
                }
                ui.label(RichText::new(&s.info.name).strong().size(16.0));
                let (badge, color) = match s.info.access {
                    AccessLevel::Reading => ("Reading", theme::ACCENT),
                    AccessLevel::MetadataOnly => ("Metadata only", theme::MUTED),
                };
                ui.label(RichText::new(badge).color(color));
            });
            ui.hyperlink(&s.info.homepage);
            let h = health.get(&s.info.id);
            let (dot, color, text) = match h.status() {
                Status::Unknown => (icons::CIRCLE_DASHED, theme::MUTED, "No requests yet".to_string()),
                Status::Healthy => (
                    icons::CHECK_CIRCLE,
                    egui::Color32::from_rgb(0x5C, 0xC8, 0x7A),
                    format!(
                        "Healthy · {} ok · avg {} ms",
                        h.ok,
                        h.avg_latency().map_or(0, |d| d.as_millis())
                    ),
                ),
                Status::Degraded => (
                    icons::WARNING_CIRCLE,
                    egui::Color32::from_rgb(0xE8, 0xB0, 0x4F),
                    format!("Last request failed · {} ok / {} failed", h.ok, h.failed),
                ),
                Status::Down => (
                    icons::X_CIRCLE,
                    egui::Color32::from_rgb(0xE5, 0x6B, 0x6F),
                    format!("Down · {} failures in a row", h.consecutive_failures),
                ),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(dot).color(color));
                ui.label(RichText::new(text).small());
            });
            if let Some(e) = h.last_error.as_ref().filter(|_| h.status() != Status::Healthy) {
                ui.label(RichText::new(e).small().color(theme::MUTED));
            }
            if !s.info.in_discover {
                ui.label(
                    RichText::new(
                        "Enriches details pages; not included in \"All sources\" (pick it in the source filter to search).",
                    )
                    .small()
                    .color(theme::MUTED),
                );
            }
            if s.info.access == AccessLevel::MetadataOnly {
                ui.label(
                    RichText::new(
                        "Search and details are shown here; reading opens the original site.",
                    )
                    .color(theme::MUTED),
                );
            }
        });
        ui.add_space(8.0);
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Only providers with documented, permitted API access are supported. \
             Sites without such terms are not scraped.",
        )
        .small()
        .color(theme::MUTED),
    );
    toggled
}

#[derive(Default)]
pub struct SettingsActions {
    pub adult_changed: bool,
    pub clear_caches: bool,
    pub page_cache_changed: bool,
}

pub fn settings(
    ui: &mut egui::Ui,
    data_dir: &std::path::Path,
    show_adult: &mut bool,
    page_cache_mb: &mut u64,
) -> SettingsActions {
    heading(ui, "Settings", "Preferences are stored locally");
    let mut out = SettingsActions::default();
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        out.adult_changed = ui
            .checkbox(show_adult, "Show adult (18+) content")
            .changed();
        ui.add_space(8.0);
        let slider = egui::Slider::new(page_cache_mb, 256..=8192)
            .logarithmic(true)
            .suffix(" MB")
            .text("Page cache size");
        // Apply on release, not on every drag step.
        let r = ui.add(slider);
        out.page_cache_changed = r.drag_stopped() || (r.changed() && !r.dragged());
        ui.label(
            RichText::new("Pages unused for 30 days are removed automatically.")
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(8.0);
        out.clear_caches = ui
            .button(theme::label(icons::TRASH, "Clear caches"))
            .on_hover_text("Deletes cached search results, details and cover images. History and bookmarks are kept.")
            .clicked();
        ui.add_space(8.0);
        ui.label(RichText::new(format!("Data folder: {}", data_dir.display())).color(theme::MUTED));
    });
    out
}
