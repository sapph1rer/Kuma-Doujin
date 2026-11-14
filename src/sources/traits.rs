//! Provider interfaces. A source implements whichever of these it is authorized to offer.
//!
//! Providers use the shared `network::client::HttpClient` for all I/O, so URL checks,
//! rate limits, retries and size caps apply to every source. Caching lives in the reader.

use async_trait::async_trait;

use crate::core::errors::SourceResult;
use crate::core::models::{Chapter, Manga, MangaSummary, PageRef, Paged, SearchQuery, Tag};

/// What a source is allowed to do in this app. Decided from the source's
/// documented terms, never from what is merely technically reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLevel {
    /// Metadata and covers only; the details page links to the original site.
    MetadataOnly,
    /// Page images may be displayed in the in-app reader.
    Reading,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub id: String,
    pub name: String,
    pub homepage: String,
    pub access: AccessLevel,
    /// Languages the source reports content in; empty means unknown/any.
    pub languages: Vec<String>,
    /// The source contains adult content; hidden unless the user opts in.
    pub adult: bool,
    /// Included when browsing "All sources". Metadata-only enrichment sources set this
    /// to false; they stay selectable in the source filter.
    pub in_discover: bool,
}

#[async_trait]
pub trait CatalogProvider: Send + Sync {
    /// Search and browse. Popular/latest listings are `SearchQuery::sort` with empty text.
    async fn search(&self, query: &SearchQuery) -> SourceResult<Paged<MangaSummary>>;
    async fn details(&self, manga_id: &str) -> SourceResult<Manga>;
    /// Tags available for filtering on this source.
    async fn tags(&self) -> SourceResult<Vec<Tag>> {
        Ok(Vec::new())
    }
    /// Raw bytes of a cover URL previously returned by this source.
    async fn cover(&self, url: &str) -> SourceResult<Vec<u8>>;
}

#[async_trait]
pub trait ChapterProvider: Send + Sync {
    /// Readable chapters (or a single entry for one-shot works) in reading order,
    /// limited to `languages` when non-empty.
    async fn chapters(&self, manga_id: &str, languages: &[String]) -> SourceResult<Vec<Chapter>>;
    /// Page manifest, sorted by `PageRef::index`.
    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>>;
}

#[async_trait]
pub trait ImageProvider: Send + Sync {
    /// Encoded bytes of one page, fetched through the source's `HttpClient` so URL checks,
    /// limits and retries are shared. Source-specific steps (e.g. usage reports) live here.
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>>;
    /// Whether the source permits storing pages in the local page cache.
    fn may_cache_pages(&self) -> bool {
        false
    }
}
