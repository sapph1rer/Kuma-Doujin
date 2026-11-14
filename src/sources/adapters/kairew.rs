//! Kairew, metadata only: a paid-coin platform whose reader is protected, so the app lists works
//! (cover, title, synopsis) and the details page links to kairew.com for reading. The listing page
//! renders client-side, so it loads through the helper browser (`HttpClient::get_text_waiting`).
use std::sync::Arc;

use async_trait::async_trait;

use super::mangastream::{re, unescape};
use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{Manga, MangaKey, MangaSummary, Paged, SearchQuery, Titles};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{AccessLevel, CatalogProvider, SourceInfo};

pub const ID: &str = "kairew";
const BASE: &str = "https://kairew.com";
const MAX_HTML: usize = 4 * 1024 * 1024;
const MAX_IMAGE: usize = 10 * 1024 * 1024;

pub struct Kairew {
    http: Arc<HttpClient>,
}

impl Kairew {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "Kairew".into(),
                homepage: BASE.into(),
                access: AccessLevel::MetadataOnly,
                languages: vec!["th".into()],
                adult: false,
                in_discover: true,
            },
            catalog: Some(Arc::new(Self { http })),
            chapters: None,
            images: None,
        })
    }
}

#[async_trait]
impl CatalogProvider for Kairew {
    /// The site's front listing only (no paging or server-side search is used); text filters it.
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        // the front listing mixes formats and has no 18+ section: show it under All only
        if q.page > 1 || q.category != crate::core::models::Category::All {
            return Ok(Paged { items: vec![], page: q.page, has_more: false });
        }
        let html = self
            .http
            .get_text_waiting(&format!("{BASE}/manga"), MAX_HTML, Some(r#"a[href*="/cartoon/"]"#))
            .await?;
        let needle = q.text.to_lowercase();
        let mut seen = std::collections::HashSet::new();
        let items = re(r#"href="https://kairew\.com/cartoon/([^"]+)"[^>]*>\s*<img[^>]*src="([^"]+)" alt="([^"]*)""#)
            .captures_iter(&html)
            .filter(|c| seen.insert(c[1].to_string()))
            .map(|c| (c[1].to_string(), c[2].to_string(), unescape(&c[3])))
            .filter(|(_, _, title)| title.to_lowercase().contains(&needle))
            .map(|(id, cover, title)| MangaSummary {
                key: MangaKey { source: ID.into(), id },
                titles: Titles { thai: Some(title), ..Default::default() },
                cover_url: Some(cover),
                language: Some("th".into()),
            })
            .collect();
        Ok(Paged { items, page: q.page, has_more: false })
    }

    async fn details(&self, id: &str) -> SourceResult<Manga> {
        let url = format!("{BASE}/cartoon/{id}");
        let html = self.http.get_text(&url, MAX_HTML).await?;
        let meta = |prop: &str| {
            re(&format!(r#"<meta property="{prop}" content="([^"]*)""#))
                .captures(&html)
                .map(|c| unescape(&c[1]))
                .filter(|s| !s.is_empty())
        };
        let title = meta("og:title").ok_or(SourceError::NotFound)?;
        Ok(Manga {
            key: MangaKey { source: ID.into(), id: id.into() },
            titles: Titles { thai: Some(title), ..Default::default() },
            description: meta("og:description"),
            artists: vec![],
            circles: vec![],
            parodies: vec![],
            characters: vec![],
            tags: vec![],
            language: Some("th".into()),
            published: None,
            page_count: None,
            cover_url: meta("og:image"),
            source_url: Some(url),
            links: vec![],
            provenance: Default::default(),
        })
    }

    async fn cover(&self, url: &str) -> SourceResult<Vec<u8>> {
        self.http.get_image(url, MAX_IMAGE).await
    }
}
