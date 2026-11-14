//! Unified, source-independent data model.
//!
//! Every external entity is addressed by `(source_id, source-local id)` so catalog
//! entries, chapters and pages from different providers never collide.

use serde::{Deserialize, Serialize};

/// Stable id of a registered source, e.g. `"mangadex"`.
pub type SourceId = String;

/// Globally unique reference to a work: the source plus the source's own id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MangaKey {
    pub source: SourceId,
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Titles {
    pub japanese: Option<String>,
    pub english: Option<String>,
    pub thai: Option<String>,
    pub alternatives: Vec<String>,
}

impl Titles {
    /// Best title for display: English, then Japanese, then Thai, then any alternative.
    pub fn display(&self) -> &str {
        self.english
            .as_deref()
            .or(self.japanese.as_deref())
            .or(self.thai.as_deref())
            .or(self.alternatives.first().map(String::as_str))
            .unwrap_or("Untitled")
    }
}

/// Lightweight entry used in cover grids and search results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MangaSummary {
    pub key: MangaKey,
    pub titles: Titles,
    pub cover_url: Option<String>,
    pub language: Option<String>,
}

/// Full metadata shown on the details page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manga {
    pub key: MangaKey,
    pub titles: Titles,
    pub description: Option<String>,
    pub artists: Vec<String>,
    pub circles: Vec<String>,
    pub parodies: Vec<String>,
    pub characters: Vec<String>,
    pub tags: Vec<String>,
    /// BCP-47-ish language code as reported by the source, e.g. `"en"`, `"ja"`, `"th"`.
    pub language: Option<String>,
    /// ISO-8601 date string as reported by the source.
    pub published: Option<String>,
    pub page_count: Option<u32>,
    pub cover_url: Option<String>,
    /// Link to the work on the original site, always shown for attribution.
    pub source_url: Option<String>,
    /// The same work on other sources: (source id, that source's id), e.g. ("anilist", "105398").
    #[serde(default)]
    pub links: Vec<(String, String)>,
    /// Which source(s) supplied each field, e.g. "tags" -> ["mangadex", "anilist"].
    /// Filled by `core::merge`; empty until then.
    #[serde(default)]
    pub provenance: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub manga: MangaKey,
    pub id: String,
    pub title: Option<String>,
    pub volume: Option<String>,
    /// Kept as text: sources use values like "10.5" or "Extra".
    pub number: Option<String>,
    pub language: Option<String>,
    pub page_count: Option<u32>,
    /// Translation/scanlation credits; shown in the reader.
    pub scanlators: Vec<String>,
}

impl Chapter {
    /// e.g. "Vol. 1 Ch. 3 · The Title", or "Oneshot".
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = &self.volume {
            parts.push(format!("Vol. {v}"));
        }
        if let Some(n) = &self.number {
            parts.push(format!("Ch. {n}"));
        }
        let mut label = if parts.is_empty() {
            "Oneshot".to_string()
        } else {
            parts.join(" ")
        };
        if let Some(t) = self.title.as_deref().filter(|t| !t.is_empty()) {
            label.push_str(" · ");
            label.push_str(t);
        }
        label
    }
}

/// One page in a chapter's manifest. `index` is 0-based and defines reading order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRef {
    pub chapter_id: String,
    pub index: u32,
    /// Opaque source-specific value the `ImageProvider` turns into a request.
    pub token: String,
}

/// The kind of work the user wants to read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    #[default]
    All,
    /// Fan-made works; adult ones when the 18+ setting is on.
    Doujin,
    /// Japanese comics.
    Manga,
    /// Korean comics.
    Manhwa,
    /// Chinese comics.
    Manhua,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::All,
        Category::Doujin,
        Category::Manga,
        Category::Manhwa,
        Category::Manhua,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::All => "All",
            Category::Doujin => "Doujin 18+",
            Category::Manga => "Manga",
            Category::Manhwa => "Manhwa",
            Category::Manhua => "Manhua",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortOrder {
    #[default]
    Relevance,
    Popular,
    Latest,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub languages: Vec<String>,
    /// Tag ids from `CatalogProvider::tags` (source-specific).
    pub include_tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub sort: SortOrder,
    /// Include adult (18+) content; off unless the user opts in.
    pub include_adult: bool,
    /// What kind of work to show; each source maps it to its own filters.
    pub category: Category,
    /// 1-based page number.
    pub page: u32,
}

/// A source-specific tag usable in `SearchQuery::include_tags` / `exclude_tags`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_title_falls_back_in_order() {
        let mut t = Titles::default();
        assert_eq!(t.display(), "Untitled");
        t.alternatives.push("Alt".into());
        assert_eq!(t.display(), "Alt");
        t.japanese = Some("日本語".into());
        assert_eq!(t.display(), "日本語");
        t.english = Some("English".into());
        assert_eq!(t.display(), "English");
    }
}
