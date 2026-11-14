//! Doujin LC (Madara WordPress theme). The site sits behind a browser check, so pages load through
//! `HttpClient`'s helper-browser fallback.
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Url;

use super::mangastream::{img_url, re, strip_tags, unescape};
use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{
    Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery, SortOrder, Titles,
};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{
    AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo,
};

pub const ID: &str = "doujin-lc";
const BASE: &str = "https://doujin-lc.net";
const MAX_HTML: usize = 4 * 1024 * 1024;
const MAX_IMAGE: usize = 30 * 1024 * 1024;

pub struct DoujinLc {
    http: Arc<HttpClient>,
}

impl DoujinLc {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        let adapter = Arc::new(Self { http });
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "Doujin LC".into(),
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

    fn series_id(url: &str) -> String {
        url.strip_prefix(&format!("{BASE}/doujin/"))
            .unwrap_or(url)
            .trim_matches('/')
            .to_string()
    }

    async fn html(&self, url: &str) -> SourceResult<String> {
        self.http.get_text(url, MAX_HTML).await
    }
}

#[async_trait]
impl CatalogProvider for DoujinLc {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        let page = q.page.max(1);
        let url = if q.text.is_empty() {
            let order = if q.sort == SortOrder::Latest { "latest" } else { "views" };
            Url::parse_with_params(&format!("{BASE}/doujin/page/{page}/"), [("m_orderby", order)])
        } else {
            Url::parse_with_params(
                &format!("{BASE}/page/{page}/"),
                [("s", q.text.as_str()), ("post_type", "wp-manga")],
            )
        }
        .map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        let html = self.html(url.as_str()).await?;

        let card = re(
            r#"(?s)(?:page-item-detail|c-tabs-item__content).*?<a href="([^"]+/doujin/[^"]+)" title="([^"]*)"(.*?)</a>"#,
        );
        let img = re(r"<img[^>]*>");
        let items = card
            .captures_iter(&html)
            .map(|c| MangaSummary {
                key: MangaKey { source: ID.into(), id: Self::series_id(&c[1]) },
                titles: Titles { thai: Some(unescape(&c[2])), ..Default::default() },
                cover_url: img.find(&c[3]).and_then(|m| img_url(m.as_str())),
                language: Some("th".into()),
            })
            .collect();
        Ok(Paged { items, page: q.page, has_more: html.contains("nextpostslink") })
    }

    async fn details(&self, id: &str) -> SourceResult<Manga> {
        let url = format!("{BASE}/doujin/{id}/");
        let html = self.html(&url).await?;
        let title = re(r"(?s)<h1[^>]*>(.*?)</h1>")
            .captures(&html)
            .map(|c| strip_tags(&c[1]))
            .filter(|t| !t.is_empty())
            .ok_or(SourceError::NotFound)?;
        let cover_url = re(r#"(?s)summary_image.*?(<img[^>]*>)"#)
            .captures(&html)
            .and_then(|c| img_url(&c[1]));
        let description = re(r#"(?s)class="summary__content[^"]*">(.*?)</div>"#)
            .captures(&html)
            .map(|c| strip_tags(&c[1]))
            .filter(|d| !d.is_empty());
        let tags = re(r#"(?s)class="genres-content">(.*?)</div>"#)
            .captures(&html)
            .map(|c| {
                re(r"<a [^>]*>(.*?)</a>")
                    .captures_iter(&c[1])
                    .map(|a| strip_tags(&a[1]))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Manga {
            key: MangaKey { source: ID.into(), id: id.into() },
            titles: Titles { thai: Some(title), ..Default::default() },
            description,
            artists: vec![],
            circles: vec![],
            parodies: vec![],
            characters: vec![],
            tags,
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
impl ChapterProvider for DoujinLc {
    async fn chapters(&self, manga_id: &str, _: &[String]) -> SourceResult<Vec<Chapter>> {
        let html = self.html(&format!("{BASE}/doujin/{manga_id}/")).await?;
        let num = re(r"\d+(?:\.\d+)?");
        let mut chapters: Vec<Chapter> = re(
            r#"(?s)<li class="wp-manga-chapter[^"]*">\s*<a href="([^"]+)">(.*?)</a>"#,
        )
        .captures_iter(&html)
        .map(|c| {
            let title = strip_tags(&c[2]);
            Chapter {
                manga: MangaKey { source: ID.into(), id: manga_id.into() },
                id: c[1].strip_prefix(BASE).unwrap_or(&c[1]).trim_matches('/').to_string(),
                number: num.find(&title).map(|m| m.as_str().to_string()),
                title: Some(title),
                volume: None,
                language: Some("th".into()),
                page_count: None,
                scanlators: vec![],
            }
        })
        .collect();
        chapters.reverse();
        Ok(chapters)
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let html = self.html(&format!("{BASE}/{chapter_id}/")).await?;
        let body = html
            .split_once(r#"class="reading-content""#)
            .ok_or_else(|| SourceError::InvalidResponse("no reader found".into()))?
            .1;
        let body = body.split_once(r#"id="text-chapter-toolbar""#).map_or(body, |p| p.0);
        let urls: Vec<String> = re(r"<img[^>]*>")
            .find_iter(body)
            .filter_map(|m| img_url(m.as_str()))
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
impl ImageProvider for DoujinLc {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        self.http.get_image(&page.token, MAX_IMAGE).await
    }

    fn may_cache_pages(&self) -> bool {
        true
    }
}
