//! Miku Doujin. The site sits behind a browser check, so pages load through `HttpClient`'s
//! helper-browser fallback.
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Url;

use super::mangastream::{img_url, re, unescape};
use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{
    Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery, Titles,
};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{
    AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo,
};

pub const ID: &str = "miku-doujin";
const BASE: &str = "https://miku-doujin.com";
const MAX_HTML: usize = 4 * 1024 * 1024;
const MAX_IMAGE: usize = 30 * 1024 * 1024;

pub struct MikuDoujin {
    http: Arc<HttpClient>,
}

impl MikuDoujin {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        let adapter = Arc::new(Self { http });
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "Miku Doujin".into(),
                homepage: BASE.into(),
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

    async fn html(&self, url: &str) -> SourceResult<String> {
        self.http.get_text(url, MAX_HTML).await
    }
}

#[async_trait]
impl CatalogProvider for MikuDoujin {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        let page = q.page.max(1).to_string();
        let url = if q.text.is_empty() {
            Url::parse_with_params(&format!("{BASE}/"), [("page", page.as_str())])
        } else {
            Url::parse_with_params(
                &format!("{BASE}/search/"),
                [("keyword", q.text.as_str()), ("page", page.as_str())],
            )
        }
        .map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        let html = self.html(url.as_str()).await?;

        let card = re(
            r#"(?s)class="no-underline inz-a" href="https://miku-doujin.com/([^/"]+)/">\s*(<img[^>]*>).*?class="inz-title[^"]*">(.*?)</div>"#,
        );
        let items = card
            .captures_iter(&html)
            .map(|c| MangaSummary {
                key: MangaKey { source: ID.into(), id: c[1].to_string() },
                titles: Titles { thai: Some(unescape(&c[3])), ..Default::default() },
                cover_url: img_url(&c[2]),
                language: Some("th".into()),
            })
            .collect();
        let has_more = html.contains(&format!("page={}\"", q.page.max(1) + 1));
        Ok(Paged { items, page: q.page, has_more })
    }

    async fn details(&self, id: &str) -> SourceResult<Manga> {
        let url = format!("{BASE}/{id}/");
        let html = self.html(&url).await?;
        let title = re(r#"<meta property="og:title"\s+content="([^"]*)""#)
            .captures(&html)
            .map(|c| unescape(&c[1]))
            .map(|t| {
                t.trim_start_matches("อ่านโดจินเรื่อง")
                    .trim_end_matches("แปลไทย")
                    .trim()
                    .to_string()
            })
            .filter(|t| !t.is_empty())
            .ok_or(SourceError::NotFound)?;
        let cover_url = re(r#"src="(https://miku-doujin.com/uploads/[^"]+)""#)
            .captures(&html)
            .map(|c| c[1].to_string());
        Ok(Manga {
            key: MangaKey { source: ID.into(), id: id.into() },
            titles: Titles { thai: Some(title), ..Default::default() },
            description: None,
            artists: vec![],
            circles: vec![],
            parodies: vec![],
            characters: vec![],
            tags: vec![],
            language: Some("th".into()),
            published: None,
            page_count: None,
            cover_url,
            source_url: Some(url),
            links: vec![],
            provenance: Default::default(),
        })
    }

    async fn cover(&self, url: &str) -> SourceResult<Vec<u8>> {
        self.http.get_image(url, MAX_IMAGE).await
    }
}

#[async_trait]
impl ChapterProvider for MikuDoujin {
    async fn chapters(&self, manga_id: &str, _: &[String]) -> SourceResult<Vec<Chapter>> {
        let html = self.html(&format!("{BASE}/{manga_id}/")).await?;
        let mut eps: Vec<u32> = re(&format!(r"/{}/ep-(\d+)/", regex::escape(manga_id)))
            .captures_iter(&html)
            .filter_map(|c| c[1].parse().ok())
            .collect();
        eps.sort_unstable();
        eps.dedup();
        if eps.is_empty() {
            eps.push(1);
        }
        Ok(eps
            .into_iter()
            .map(|n| Chapter {
                manga: MangaKey { source: ID.into(), id: manga_id.into() },
                id: format!("{manga_id}/ep-{n}"),
                title: None,
                volume: None,
                number: Some(n.to_string()),
                language: Some("th".into()),
                page_count: None,
                scanlators: vec![],
            })
            .collect())
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let html = self.html(&format!("{BASE}/{chapter_id}/")).await?;
        let urls: Vec<String> = re(r#"<img src="([^"]+)" class="page-img""#)
            .captures_iter(&html)
            .map(|c| c[1].to_string())
            .collect();
        if urls.is_empty() {
            return Err(SourceError::InvalidResponse("no pages found".into()));
        }
        Ok(urls
            .into_iter()
            .enumerate()
            .map(|(i, token)| PageRef { chapter_id: chapter_id.into(), index: i as u32, token })
            .collect())
    }
}

#[async_trait]
impl ImageProvider for MikuDoujin {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        self.http.get_image(&page.token, MAX_IMAGE).await
    }
}
