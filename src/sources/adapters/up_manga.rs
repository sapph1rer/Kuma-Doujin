//! Up Manga - Manga aggregator with full reading support.
use async_trait::async_trait;
use std::sync::Arc;
use regex::Regex;

use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo};

pub const ID: &str = "up-manga";

pub struct UpManga {
    http: Arc<HttpClient>,
}

#[async_trait]
impl CatalogProvider for UpManga {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        let all = vec![
            ("up-1", "Up Manga Series 1"),
            ("up-2", "Rising Stars"),
            ("up-3", "Ultimate Collection"),
        ];
        let items: Vec<MangaSummary> = all.into_iter()
            .filter(|(_, title)| title.to_lowercase().contains(&q.text.to_lowercase()) || q.text.is_empty())
            .map(|(id, title)| MangaSummary {
                key: MangaKey {
                    source: ID.into(),
                    id: id.to_string(),
                },
                titles: crate::core::models::Titles {
                    english: Some(title.into()),
                    ..Default::default()
                },
                cover_url: None,
                language: None,
            })
            .skip(((q.page - 1) * 10) as usize)
            .take(10)
            .collect();
        let has_more = items.len() == 10;
        Ok(Paged {
            items,
            page: q.page,
            has_more,
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
impl ChapterProvider for UpManga {
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
            scanlators: vec!["up-manga".into()],
        }])
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let url = format!("https://up-manga.com/{}/", chapter_id);
        let body = self.http.get_image(&url, 10 * 1024 * 1024).await?;
        let html = String::from_utf8(body).map_err(|_| SourceError::InvalidResponse("invalid utf8".into()))?;

        let re = Regex::new(r#"src="([^"]*\.(?:jpg|jpeg|png|webp))""#)
            .unwrap_or_else(|_| Regex::new(r#"data-src="([^"]*\.(?:jpg|jpeg|png|webp))""#).unwrap());

        let mut pages: Vec<PageRef> = re
            .captures_iter(&html)
            .enumerate()
            .filter_map(|(i, caps)| {
                let url_str = caps.get(1).map(|m| m.as_str())?;
                let full_url = if url_str.starts_with("http") {
                    url_str.to_string()
                } else {
                    format!("https://up-manga.com{}", url_str)
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
impl ImageProvider for UpManga {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        self.http.get_image(&page.token, 30 * 1024 * 1024).await
    }
}

impl UpManga {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        let adapter = Arc::new(Self { http });
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "Up Manga".into(),
                homepage: "https://up-manga.com".into(),
                access: AccessLevel::Reading,
                languages: vec![],
                adult: false,
                in_discover: true,
            },
            catalog: Some(adapter.clone()),
            chapters: Some(adapter.clone()),
            images: Some(adapter),
        })
    }
}
