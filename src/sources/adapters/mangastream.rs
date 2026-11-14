//! Shared scraper for sites running the "MangaStream" WordPress theme
//! (dark-manga, up-manga, go-manga): listing, series page, chapter list and reader JSON.
use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use reqwest::Url;

use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{
    Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery, SortOrder, Titles,
};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{
    AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo,
};

const MAX_HTML: usize = 4 * 1024 * 1024;
const MAX_IMAGE: usize = 30 * 1024 * 1024;

struct MangaStream {
    http: Arc<HttpClient>,
    id: &'static str,
    base: &'static str,
}

pub fn source(
    http: Arc<HttpClient>,
    id: &'static str,
    name: &str,
    base: &'static str,
    adult: bool,
) -> anyhow::Result<Source> {
    let adapter = Arc::new(MangaStream { http, id, base });
    Ok(Source {
        info: SourceInfo {
            id: id.into(),
            name: name.into(),
            homepage: base.into(),
            access: AccessLevel::Reading,
            languages: vec!["th".into()],
            adult,
            in_discover: true,
        },
        catalog: Some(adapter.clone()),
        chapters: Some(adapter.clone()),
        images: Some(adapter),
    })
}

pub(super) fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

pub(super) fn unescape(s: &str) -> String {
    let s = re(r"&#(\d+);").replace_all(s, |c: &regex::Captures| {
        c[1].parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default()
    });
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

pub(super) fn strip_tags(s: &str) -> String {
    unescape(&re(r"<[^>]*>").replace_all(&s.replace("<br />", "\n").replace("</p>", "\n"), ""))
}

/// First usable image URL of an `<img>` tag, preferring lazy-load attributes over placeholders.
pub(super) fn img_url(tag: &str) -> Option<String> {
    ["data-wpfc-original-src", "data-src", "data-lazy-src", "src"]
        .iter()
        .find_map(|a| {
            re(&format!(r#"\s{a}="([^"]+)""#))
                .captures(tag)
                .map(|c| c[1].to_string())
                .filter(|u| u.starts_with("http"))
        })
}

impl MangaStream {
    fn slug(&self, url: &str) -> String {
        url.strip_prefix(self.base)
            .unwrap_or(url)
            .trim_matches('/')
            .to_string()
    }

    async fn html(&self, url: &str) -> SourceResult<String> {
        self.http.get_text(url, MAX_HTML).await
    }
}

#[async_trait]
impl CatalogProvider for MangaStream {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        let order = match q.sort {
            SortOrder::Popular => "popular",
            SortOrder::Latest => "update",
            SortOrder::Relevance if q.text.is_empty() => "popular",
            SortOrder::Relevance => "",
        };
        let page = q.page.max(1).to_string();
        let url = Url::parse_with_params(
            &format!("{}/manga/", self.base),
            [("page", page.as_str()), ("order", order), ("title", q.text.as_str())],
        )
        .map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        let html = self.html(url.as_str()).await?;

        let card = re(r#"(?s)class="bsx">\s*<a href="([^"]+)" title="([^"]*)"(.*?)</a>"#);
        let img = re(r"<img[^>]*>");
        let items = card
            .captures_iter(&html)
            .map(|c| MangaSummary {
                key: MangaKey {
                    source: self.id.into(),
                    id: self.slug(&c[1]),
                },
                titles: Titles {
                    thai: Some(unescape(&c[2])),
                    ..Default::default()
                },
                cover_url: img.find(&c[3]).and_then(|m| img_url(m.as_str())),
                language: Some("th".into()),
            })
            .collect::<Vec<_>>();
        let has_more = html.contains("next page-numbers") || html.contains(r#"class="r""#);
        Ok(Paged {
            items,
            page: q.page,
            has_more,
        })
    }

    async fn details(&self, id: &str) -> SourceResult<Manga> {
        let url = format!("{}/{}/", self.base, id);
        let html = self.html(&url).await?;
        let title = re(r#"(?s)<h1 class="entry-title"[^>]*>(.*?)</h1>"#)
            .captures(&html)
            .map(|c| strip_tags(&c[1]))
            .ok_or(SourceError::NotFound)?;
        let cover_url = re(r#"(?s)class="thumb".*?(<img[^>]*>)"#)
            .captures(&html)
            .and_then(|c| img_url(&c[1]));
        let description = re(r#"(?s)class="entry-content entry-content-single"[^>]*>(.*?)</div>"#)
            .captures(&html)
            .map(|c| strip_tags(&c[1]))
            .filter(|d| !d.is_empty());
        let info = html.split_once(r#"id="chapterlist""#).map_or(&*html, |p| p.0);
        let mut tags: Vec<String> = Vec::new();
        for c in re(r#"<a href="[^"]*/genres?/[^"]*"[^>]*>(.*?)</a>"#).captures_iter(info) {
            let t = strip_tags(&c[1]);
            if !t.is_empty() && !tags.contains(&t) {
                tags.push(t);
            }
        }
        Ok(Manga {
            key: MangaKey {
                source: self.id.into(),
                id: id.into(),
            },
            titles: Titles {
                thai: Some(title),
                ..Default::default()
            },
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
impl ChapterProvider for MangaStream {
    async fn chapters(&self, manga_id: &str, _: &[String]) -> SourceResult<Vec<Chapter>> {
        let html = self.html(&format!("{}/{}/", self.base, manga_id)).await?;
        let list = html
            .split_once(r#"id="chapterlist""#)
            .ok_or_else(|| SourceError::InvalidResponse("no chapter list".into()))?
            .1;
        let item = re(
            r#"(?s)<li data-num="([^"]*)".*?<a href="([^"]+)".*?class="chapternum">(.*?)</span>"#,
        );
        let mut chapters: Vec<Chapter> = item
            .captures_iter(list)
            .map(|c| Chapter {
                manga: MangaKey {
                    source: self.id.into(),
                    id: manga_id.into(),
                },
                id: self.slug(&c[2]),
                title: Some(unescape(&c[3])),
                volume: None,
                number: Some(unescape(&c[1])),
                language: Some("th".into()),
                page_count: None,
                scanlators: vec![],
            })
            .collect();
        chapters.reverse();
        Ok(chapters)
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let html = self.html(&format!("{}/{}/", self.base, chapter_id)).await?;
        let raw = re(r#"(?s)"images":\s*(\[.*?\])"#)
            .captures(&html)
            .map(|c| c[1].to_string())
            .ok_or_else(|| SourceError::InvalidResponse("no pages found".into()))?;
        let urls: Vec<String> = serde_json::from_str(&raw)
            .map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        if urls.is_empty() {
            return Err(SourceError::InvalidResponse("no pages found".into()));
        }
        Ok(urls
            .into_iter()
            .enumerate()
            .map(|(i, token)| PageRef {
                chapter_id: chapter_id.into(),
                index: i as u32,
                token,
            })
            .collect())
    }
}

#[async_trait]
impl ImageProvider for MangaStream {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        self.http.get_image(&page.token, MAX_IMAGE).await
    }
}
