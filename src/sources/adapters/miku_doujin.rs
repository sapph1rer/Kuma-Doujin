//! Miku Doujin - Thai doujin reader with full page support.
use async_trait::async_trait;
use std::sync::Arc;
use regex::Regex;

use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo};

pub const ID: &str = "miku-doujin";

pub struct MikuDoujin {
    http: Arc<HttpClient>,
}

#[async_trait]
impl CatalogProvider for MikuDoujin {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        let all = vec![
            ("miku-1", "Miku Collection Vol. 1"),
            ("miku-2", "Miku Doujin Festival"),
            ("miku-3", "Summer Doujin Special"),
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
                language: Some("th".into()),
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

    async fn details(&self, id: &str) -> SourceResult<Manga> {
        Ok(Manga {
            key: MangaKey {
                source: ID.into(),
                id: id.to_string(),
            },
            titles: crate::core::models::Titles {
                english: Some("Miku Doujin Collection".into()),
                japanese: Some("ミクドウジン".into()),
                thai: Some("มิกุโดจิน".into()),
                ..Default::default()
            },
            description: Some("Thai doujin and manga collection featuring Miku content".into()),
            artists: vec!["Various Artists".into()],
            circles: vec![],
            parodies: vec!["Vocaloid".into()],
            characters: vec!["Miku".into()],
            tags: vec!["doujin".into(), "adult".into()],
            language: Some("th".into()),
            published: None,
            page_count: None,
            cover_url: None,
            source_url: Some("https://miku-doujin.com".into()),
            links: vec![],
            provenance: Default::default(),
        })
    }

    async fn cover(&self, _: &str) -> SourceResult<Vec<u8>> {
        Ok(vec![])
    }
}

#[async_trait]
impl ChapterProvider for MikuDoujin {
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
            scanlators: vec!["miku-doujin".into()],
        }])
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let url = format!("https://miku-doujin.com/{}/ep-1/", chapter_id);
        let body = self.http.get_image(&url, 10 * 1024 * 1024).await?;
        let html = String::from_utf8(body).map_err(|_| SourceError::InvalidResponse("invalid utf8".into()))?;

        // Extract image URLs from HTML: /uploads/TIMESTAMP/ID.jpg
        let re = Regex::new(r#"src="(/uploads/\d+/[^"]+\.jpg)""#)
            .unwrap_or_else(|_| Regex::new(r#"data-src="(/uploads/\d+/[^"]+\.jpg)""#).unwrap());

        let pages: Vec<PageRef> = re
            .captures_iter(&html)
            .enumerate()
            .map(|(i, caps)| {
                let rel_url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let full_url = format!("https://miku-doujin.com{}", rel_url);
                PageRef {
                    chapter_id: chapter_id.to_string(),
                    index: i as u32,
                    token: full_url,
                }
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
impl ImageProvider for MikuDoujin {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        self.http.get_image(&page.token, 30 * 1024 * 1024).await
    }
}

impl MikuDoujin {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        let adapter = Arc::new(Self { http });
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "Miku Doujin".into(),
                homepage: "https://miku-doujin.com".into(),
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
