//! nhentai.xxx (English/Japanese/Chinese doujinshi). Plain HTML, no browser check. Each gallery
//! is one "chapter"; page image names come from the `g_th` JSON on the gallery page.
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Url;

use super::mangastream::{img_url, re, unescape};
use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{
    Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery, SortOrder, Titles,
};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{
    AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo,
};

pub const ID: &str = "nhentai-xxx";
const BASE: &str = "https://nhentai.xxx";
const MAX_HTML: usize = 4 * 1024 * 1024;
const MAX_IMAGE: usize = 30 * 1024 * 1024;

pub struct NhentaiXxx {
    http: Arc<HttpClient>,
}

impl NhentaiXxx {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        let adapter = Arc::new(Self { http });
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "nhentai.xxx".into(),
                homepage: BASE.into(),
                access: AccessLevel::Reading,
                languages: vec!["en".into(), "ja".into(), "zh".into()],
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

    async fn gallery(&self, id: &str) -> SourceResult<String> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(SourceError::NotFound);
        }
        self.html(&format!("{BASE}/g/{id}/")).await
    }
}

/// Gallery cards (listing, search, "more like this").
fn cards(html: &str) -> Vec<MangaSummary> {
    re(r#"(?s)<a href="/g/(\d+)/"[^>]*>\s*(?:<div class="inner_gallery">\s*</div>\s*)?(<img[^>]*>)\s*<div class="caption">(.*?)</div>"#)
        .captures_iter(html)
        .map(|c| MangaSummary {
            key: MangaKey { source: ID.into(), id: c[1].to_string() },
            titles: Titles { english: Some(unescape(&c[3])), ..Default::default() },
            cover_url: img_url(&c[2]),
            language: None,
        })
        .collect()
}

/// Names of the tag buttons linking to `/{kind}/…/` on a gallery page.
fn tagged(html: &str, kind: &str) -> Vec<String> {
    re(&format!(r#"(?s)href='/{kind}/[^']*/'>\s*<span class='tag_name'>([^<]*)</span>"#))
        .captures_iter(html)
        .map(|c| unescape(c[1].trim()))
        .collect()
}

/// Folder of the gallery's images, from its cover URL (`…/cover.jpg`).
fn image_dir(html: &str) -> Option<String> {
    re(r#"(https://i\d*\.nhentaimg\.com/[^"' ]+/)cover\.\w+"#)
        .captures(html)
        .map(|c| c[1].to_string())
}

/// Page image URLs: `g_th.fl` maps page number → "type,w,h" (j=jpg, p=png, w=webp, g=gif).
fn page_urls(html: &str) -> Vec<String> {
    let Some(dir) = image_dir(html) else { return vec![] };
    let Some(fl) = re(r#""fl":\{([^}]*)\}"#).captures(html) else { return vec![] };
    let mut pages: Vec<(u32, String)> = re(r#""(\d+)":"(\w),"#)
        .captures_iter(&fl[1])
        .filter_map(|c| {
            let ext = match &c[2] {
                "j" => "jpg",
                "p" => "png",
                "w" => "webp",
                "g" => "gif",
                _ => return None,
            };
            Some((c[1].parse().ok()?, format!("{dir}{}.{ext}", &c[1])))
        })
        .collect();
    pages.sort_by_key(|p| p.0);
    pages.into_iter().map(|p| p.1).collect()
}

fn language(html: &str) -> Option<String> {
    tagged(html, "language").iter().find_map(|l| match l.as_str() {
        "english" => Some("en".to_string()),
        "japanese" => Some("ja".to_string()),
        "chinese" => Some("zh".to_string()),
        _ => None,
    })
}

#[async_trait]
impl CatalogProvider for NhentaiXxx {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        let page = q.page.max(1).to_string();
        let url = if q.text.is_empty() && q.sort != SortOrder::Popular {
            Url::parse_with_params(&format!("{BASE}/"), [("page", page.as_str())])
        } else {
            let mut p = vec![("key", q.text.as_str()), ("page", page.as_str())];
            if q.sort == SortOrder::Popular {
                p.push(("sort", "popular"));
            }
            Url::parse_with_params(&format!("{BASE}/search/"), p)
        }
        .map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        let html = self.html(url.as_str()).await?;
        let items = cards(&html);
        let has_more = re(&format!(r#"page={}['"&]"#, q.page.max(1) + 1)).is_match(&html);
        Ok(Paged { items, page: q.page, has_more })
    }

    async fn details(&self, id: &str) -> SourceResult<Manga> {
        let html = self.gallery(id).await?;
        let english = re(r#"<div class="info">\s*<h1>(.*?)</h1>"#)
            .captures(&html)
            .map(|c| unescape(&c[1]))
            .filter(|t| !t.is_empty())
            .ok_or(SourceError::NotFound)?;
        let japanese = re(r#"</h1>\s*<h2>(.*?)</h2>"#)
            .captures(&html)
            .map(|c| unescape(&c[1]))
            .filter(|t| !t.is_empty());
        let page_count = re(r#"class="tag_name pages">(\d+)<"#)
            .captures(&html)
            .and_then(|c| c[1].parse().ok());
        let published = re(r#"data-utc="([^"]+)""#).captures(&html).map(|c| c[1].to_string());
        Ok(Manga {
            key: MangaKey { source: ID.into(), id: id.into() },
            titles: Titles { english: Some(english), japanese, ..Default::default() },
            description: None,
            artists: tagged(&html, "artist"),
            circles: tagged(&html, "group"),
            parodies: tagged(&html, "parody"),
            characters: tagged(&html, "character"),
            tags: tagged(&html, "tag"),
            language: language(&html),
            published,
            page_count,
            cover_url: image_dir(&html).map(|d| format!("{d}cover.jpg")),
            source_url: Some(format!("{BASE}/g/{id}/")),
            links: vec![],
            provenance: Default::default(),
        })
    }

    async fn cover(&self, url: &str) -> SourceResult<Vec<u8>> {
        self.http.get_image(url, MAX_IMAGE).await
    }
}

#[async_trait]
impl ChapterProvider for NhentaiXxx {
    async fn chapters(&self, manga_id: &str, _: &[String]) -> SourceResult<Vec<Chapter>> {
        let html = self.gallery(manga_id).await?;
        Ok(vec![Chapter {
            manga: MangaKey { source: ID.into(), id: manga_id.into() },
            id: manga_id.into(),
            title: None,
            volume: None,
            number: None,
            language: language(&html),
            page_count: Some(page_urls(&html).len() as u32),
            scanlators: vec![],
        }])
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let urls = page_urls(&self.gallery(chapter_id).await?);
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
impl ImageProvider for NhentaiXxx {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        self.http.get_image(&page.token, MAX_IMAGE).await
    }

    fn may_cache_pages(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GALLERY: &str = r#"<img src="https://i5.nhentaimg.com/018/abc/cover.jpg" alt="">
        </a></div><div class="info">
            <h1>[Artist] Title 1 [English]</h1>
            <h2>[作者] タイトル 1</h2>
            <li class='tags'><a class='tag_btn ' href='/tag/glasses/'>
                <span class='tag_name'>glasses</span> <span class='tag_count '>81K</span></a></li>
            <a class='tag_btn ' href='/artist/someone/'> <span class='tag_name'>someone</span></a>
            <a class='tag_btn ' href='/language/english/'> <span class='tag_name'>english</span></a>
            <span class="tag_name pages">3</span>
        <script> var g_th = $.parseJSON('{"fl":{"2":"j,1280,1816","1":"w,1280,1816","3":"p,1,1"},"th":{"1":"j,1,1"}}');</script>
        <a href="/g/42/" title="x">
            <div class="inner_gallery"></div>
            <img alt="" class="lazy" src="data:image/svg+xml,x" data-src="https://i5.nhentaimg.com/018/def/thumb.jpg">
            <div class="caption">Other &amp; work</div>
        </a>"#;

    #[test]
    fn parses_gallery_pages_tags_and_cards() {
        assert_eq!(
            page_urls(GALLERY),
            [
                "https://i5.nhentaimg.com/018/abc/1.webp",
                "https://i5.nhentaimg.com/018/abc/2.jpg",
                "https://i5.nhentaimg.com/018/abc/3.png"
            ]
        );
        assert_eq!(tagged(GALLERY, "tag"), ["glasses"]);
        assert_eq!(tagged(GALLERY, "artist"), ["someone"]);
        assert_eq!(language(GALLERY).as_deref(), Some("en"));
        let c = cards(GALLERY);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].key.id, "42");
        assert_eq!(c[0].titles.display(), "Other & work");
        assert_eq!(c[0].cover_url.as_deref(), Some("https://i5.nhentaimg.com/018/def/thumb.jpg"));
    }
}
