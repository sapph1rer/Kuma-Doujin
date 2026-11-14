//! HentaiThai. The site sits behind a browser check, so pages load through `HttpClient`'s
//! helper-browser fallback. Each work is one page `/t<id>` holding every image, so it is a one-shot.
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Url;

use super::mangastream::{re, strip_tags, unescape};
use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{
    Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery, Titles,
};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{
    AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo,
};

pub const ID: &str = "hentaithai";
const BASE: &str = "https://hentaithai.net";
const MAX_HTML: usize = 4 * 1024 * 1024;
const MAX_IMAGE: usize = 30 * 1024 * 1024;

pub struct HentaiThai {
    http: Arc<HttpClient>,
}

impl HentaiThai {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        let adapter = Arc::new(Self { http });
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "HentaiThai".into(),
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

    fn cards(html: &str) -> Vec<MangaSummary> {
        // each card's cover is a `.post_N { background-image: url('…') }` rule in a <style> before it
        let covers: HashMap<String, String> = re(r"\.post_(\d+)\s*\{\s*background-image:\s*url\('([^']+)'\)")
            .captures_iter(html)
            .map(|c| (c[1].to_string(), c[2].to_string()))
            .collect();
        re(r#"(?s)<a [^>]*href="/t(\d+)" title="([^"]*)"[^>]*>\s*<div class="embed-responsive post_(\d+)"#)
            .captures_iter(html)
            .map(|c| {
                let full = unescape(&c[2]);
                let (thai, english) = match full.split_once(" - ") {
                    Some((t, e)) => (t.trim().to_string(), Some(e.trim().to_string())),
                    None => (full, None),
                };
                MangaSummary {
                    key: MangaKey { source: ID.into(), id: c[1].to_string() },
                    titles: Titles { thai: Some(thai), english, ..Default::default() },
                    cover_url: covers.get(&c[3]).cloned(),
                    language: Some("th".into()),
                }
            })
            .collect()
    }
}

#[async_trait]
impl CatalogProvider for HentaiThai {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        let page = q.page.max(1);
        if !q.text.is_empty() {
            let url = Url::parse_with_params(&format!("{BASE}/search"), [("q", q.text.as_str())])
                .map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
            let items = Self::cards(&self.html(url.as_str()).await?);
            return Ok(Paged { items, page: q.page, has_more: false });
        }
        let home = self.html(&format!("{BASE}/")).await?;
        if page == 1 {
            return Ok(Paged { items: Self::cards(&home), page: q.page, has_more: true });
        }
        // the home page links to the newest older page `/page-N`; each step back is N-1
        let first: u32 = re(r#"href="/page-(\d+)""#)
            .captures(&home)
            .and_then(|c| c[1].parse().ok())
            .ok_or_else(|| SourceError::InvalidResponse("no pagination found".into()))?;
        let n = first.saturating_sub(page - 2);
        if n == 0 {
            return Ok(Paged { items: vec![], page: q.page, has_more: false });
        }
        let items = Self::cards(&self.html(&format!("{BASE}/page-{n}")).await?);
        Ok(Paged { items, page: q.page, has_more: n > 1 })
    }

    async fn details(&self, id: &str) -> SourceResult<Manga> {
        let url = format!("{BASE}/t{id}");
        let html = self.html(&url).await?;
        let full = re(r"(?s)<h1[^>]*>(.*?)</h1>")
            .captures(&html)
            .map(|c| strip_tags(&c[1]))
            .filter(|t| !t.is_empty())
            .ok_or(SourceError::NotFound)?;
        let (thai, english) = match full.split_once(" | ") {
            Some((t, e)) => (t.trim().to_string(), Some(e.trim().to_string())),
            None => (full, None),
        };
        let description = re(r#"(?s)<p class="card-text">(.*?)</p>"#)
            .captures(&html)
            .map(|c| strip_tags(&c[1]))
            .filter(|d| !d.is_empty());
        let tags = re(r#"(?s)<a class="badge[^"]*" href="/tag-[^"]*"[^>]*>(.*?)</a>"#)
            .captures_iter(&html)
            .map(|c| strip_tags(&c[1]))
            .collect();
        let cover_url = re(r#"data-layer="([^"]+)""#).captures(&html).map(|c| c[1].to_string());
        Ok(Manga {
            key: MangaKey { source: ID.into(), id: id.into() },
            titles: Titles { thai: Some(thai), english, ..Default::default() },
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
impl ChapterProvider for HentaiThai {
    async fn chapters(&self, manga_id: &str, _: &[String]) -> SourceResult<Vec<Chapter>> {
        Ok(vec![Chapter {
            manga: MangaKey { source: ID.into(), id: manga_id.into() },
            id: manga_id.into(),
            title: None,
            volume: None,
            number: None,
            language: Some("th".into()),
            page_count: None,
            scanlators: vec![],
        }])
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let html = self.html(&format!("{BASE}/t{chapter_id}")).await?;
        // `data-layer` is the Thai-translated page laid over the untranslated `clean` one
        let urls: Vec<String> = re(r#"<img src="([^"]+)" data-layer="([^"]+)""#)
            .captures_iter(&html)
            .map(|c| c[2].to_string())
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
impl ImageProvider for HentaiThai {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        self.http.get_image(&page.token, MAX_IMAGE).await
    }
}
