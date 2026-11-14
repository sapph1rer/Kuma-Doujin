//! Doujin LC - Thai doujin hub with full reading support.
use async_trait::async_trait;
use std::sync::Arc;
use regex::Regex;

use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo};

pub const ID: &str = "doujin-lc";

pub struct DoujinLc {
    http: Arc<HttpClient>,
}

#[async_trait]
impl CatalogProvider for DoujinLc {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        Ok(Paged {
            items: vec![],
            page: q.page,
            has_more: false,
        })
    }

    async fn details(&self, _: &str) -> SourceResult<Manga> {
        Err(SourceError::NotFound)
    }

    async fn cover(&self, _: &str) -> SourceResult<Vec<u8>> {
        Ok(vec![])
    }
}

#[async_trait]
impl ChapterProvider for DoujinLc {
    async fn chapters(&self, manga_id: &str, _: &[String]) -> SourceResult<Vec<Chapter>> {
        Ok(vec![Chapter {
            manga: MangaKey {
                source: ID.into(),
                id: manga_id.to_string(),
            },
            id: manga_id.to_string(),
            title: Some("Full".into()),
            volume: None,
            number: Some("1".into()),
            language: Some("th".into()),
            page_count: None,
            scanlators: vec!["doujin-lc".into()],
        }])
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let url = format!("https://doujin-lc.net/doujin/{}/", chapter_id);
        let body = self.http.get_image(&url, 10 * 1024 * 1024).await?;
        let html = String::from_utf8(body).map_err(|_| SourceError::InvalidResponse("invalid utf8".into()))?;

        let re = Regex::new(r#"src="([^"]*(?:wp-content/uploads|cdn)[^"]*\.(?:jpg|jpeg|png|webp))""#)
            .unwrap_or_else(|_| Regex::new(r#"data-src="([^"]*(?:wp-content/uploads|cdn)[^"]*\.(?:jpg|jpeg|png|webp))""#).unwrap());

        let mut pages: Vec<PageRef> = re
            .captures_iter(&html)
            .enumerate()
            .filter_map(|(i, caps)| {
                let url_str = caps.get(1).map(|m| m.as_str())?;
                let full_url = if url_str.starts_with("http") {
                    url_str.to_string()
                } else {
                    format!("https://doujin-lc.net{}", url_str)
                };
                Some(PageRef {
                    chapter_id: chapter_id.to_string(),
                    index: i as u32,
                    token: full_url,
                })
            })
            .collect();

        if pages.is_empty() {
            Err(SourceError::InvalidResponse("no pages found".into()))
        } else {
            Ok(pages)
        }
    }
}

#[async_trait]
impl ImageProvider for DoujinLc {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        self.http.get_image(&page.token, 30 * 1024 * 1024).await
    }
}

impl DoujinLc {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        let adapter = Arc::new(Self { http });
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "Doujin LC".into(),
                homepage: "https://doujin-lc.net".into(),
                access: AccessLevel::Reading,
                languages: vec!["th".into()],
                adult: true,
                in_discover: true,
            },
            catalog: Some(adapter.clone()),
            chapters: Some(adapter.clone()),
            images: Some(adapter),
        })
    }
}
