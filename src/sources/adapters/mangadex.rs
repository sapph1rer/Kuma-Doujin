//! MangaDex (https://api.mangadex.org) catalog, chapter and page adapter.
//!
//! Verified 2026-11-14 against https://api.mangadex.org/docs/ (AUP, limitations,
//! 04-chapter/retrieving-chapter) and /docs/static/api.yaml: third-party apps allowed if they
//! credit MangaDex and scanlation groups (shown in the reader), run no ads, send a real
//! User-Agent and no `Via` header. ~5 req/s per IP; `limit` <= 100 (feed 500);
//! `offset + limit` <= 10 000; `/at-home/server` 40 req/min, base URL valid ~15 min.
//! Every image loaded from a MangaDex@Home node (base URL not on mangadex.org) must be
//! reported to https://api.mangadex.network/report, success or failure.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Url;
use serde::Deserialize;

use crate::core::errors::{SourceError, SourceResult};
use crate::core::merge::clean_description;
use crate::core::models::{
    Category, Chapter, Manga, MangaKey, MangaSummary, PageRef, Paged, SearchQuery, SortOrder, Tag,
    Titles,
};
use crate::network::client::HttpClient;
use crate::network::retry::RetryPolicy;
use crate::sources::registry::Source;
use crate::sources::traits::{
    AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo,
};

pub const ID: &str = "mangadex";
const API: &str = "https://api.mangadex.org";
const COVERS: &str = "https://uploads.mangadex.org/covers";
const PAGE_SIZE: u32 = 24;
const MAX_WINDOW: u32 = 10_000;
const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 30 * 1024 * 1024;
const FEED_LIMIT: u32 = 500;
const REPORT_URL: &str = "https://api.mangadex.network/report";
/// Documented validity is ~15 min; refresh early.
const AT_HOME_TTL: Duration = Duration::from_secs(10 * 60);
/// The "Doujinshi" format tag (GET /manga/tag, verified 2026-11-14).
pub const DOUJINSHI_TAG: &str = "b13b2a48-c720-44a9-9c77-39c9979373fb";
const ALL_RATINGS: [&str; 4] = ["safe", "suggestive", "erotica", "pornographic"];

#[derive(Clone)]
struct AtHome {
    base: String,
    hash: String,
    files: Vec<String>,
    fetched: Instant,
}

pub struct MangaDex {
    http: HttpClient,
    /// Page images: MangaDex@Home nodes are `*.mangadex.network` (with `forcePort443`),
    /// the fallback is uploads.mangadex.org. No client retries: a failure is reported
    /// and a fresh node is requested instead, as the docs ask.
    pages: Arc<HttpClient>,
    at_home: Mutex<HashMap<String, AtHome>>,
}

impl MangaDex {
    pub fn new() -> anyhow::Result<Self> {
        let http = HttpClient::new(
            &["api.mangadex.org", "uploads.mangadex.org"],
            // Stay under the documented ~5 req/s global per-IP limit.
            &[
                ("api.mangadex.org", Duration::from_millis(250)),
                ("uploads.mangadex.org", Duration::from_millis(100)),
            ],
        )?;
        let pages =
            HttpClient::new(&["mangadex.network", "mangadex.org"], &[])?.with_retry(RetryPolicy {
                max_attempts: 1,
                ..RetryPolicy::default()
            });
        Ok(Self {
            http,
            pages: Arc::new(pages),
            at_home: Mutex::default(),
        })
    }

    async fn at_home(&self, chapter_id: &str, refresh: bool) -> SourceResult<AtHome> {
        if !refresh {
            let cached = self.at_home.lock().expect("lock").get(chapter_id).cloned();
            if let Some(a) = cached.filter(|a| a.fetched.elapsed() < AT_HOME_TTL) {
                return Ok(a);
            }
        }
        if !valid_id(chapter_id) {
            return Err(SourceError::NotFound);
        }
        let url = Url::parse(&format!(
            "{API}/at-home/server/{chapter_id}?forcePort443=true"
        ))
        .expect("validated id");
        let res: AtHomeResponse = self.http.get_json(&url).await?;
        if !res.chapter.hash.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(SourceError::InvalidResponse("bad chapter hash".into()));
        }
        let a = AtHome {
            base: res.base_url.trim_end_matches('/').to_string(),
            hash: res.chapter.hash,
            files: res.chapter.data,
            fetched: Instant::now(),
        };
        self.at_home
            .lock()
            .expect("lock")
            .insert(chapter_id.to_string(), a.clone());
        Ok(a)
    }

    fn report(&self, url: String, ok_bytes: Option<usize>, cached: bool, took: Duration) {
        let body = serde_json::json!({
            "url": url,
            "success": ok_bytes.is_some(),
            "bytes": ok_bytes.unwrap_or(0),
            "duration": took.as_millis() as u64,
            "cached": cached,
        });
        let client = self.pages.clone();
        tokio::spawn(async move {
            if let Err(e) = client.post_json(REPORT_URL, &body).await {
                tracing::debug!(error = %e, "MangaDex@Home report failed");
            }
        });
    }

    pub fn source() -> anyhow::Result<Source> {
        let md = Arc::new(Self::new()?);
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "MangaDex".into(),
                homepage: "https://mangadex.org".into(),
                access: AccessLevel::Reading,
                languages: vec![],
                adult: false,
                in_discover: true,
            },
            catalog: Some(md.clone()),
            chapters: Some(md.clone()),
            images: Some(md),
        })
    }
}

pub fn search_url(q: &SearchQuery) -> Option<Url> {
    let offset = q.page.saturating_sub(1) * PAGE_SIZE;
    if offset + PAGE_SIZE > MAX_WINDOW {
        return None;
    }
    let mut url = Url::parse(&format!("{API}/manga")).expect("static url");
    {
        let mut p = url.query_pairs_mut();
        p.append_pair("limit", &PAGE_SIZE.to_string());
        p.append_pair("offset", &offset.to_string());
        p.append_pair("includes[]", "cover_art");
        let text = q.text.trim();
        if !text.is_empty() {
            p.append_pair("title", text);
        }
        for l in &q.languages {
            p.append_pair("availableTranslatedLanguage[]", l);
        }
        let doujin = q.category == Category::Doujin;
        if doujin && !q.include_tags.iter().any(|t| t == DOUJINSHI_TAG) {
            p.append_pair("includedTags[]", DOUJINSHI_TAG);
        }
        // Original language decides manga / manhwa / manhua; doujinshi are kept out of those.
        let origin: &[&str] = match q.category {
            Category::Manga => &["ja"],
            Category::Manhwa => &["ko"],
            Category::Manhua => &["zh", "zh-hk"],
            Category::All | Category::Doujin => &[],
        };
        for o in origin {
            p.append_pair("originalLanguage[]", o);
        }
        if !origin.is_empty() && !q.exclude_tags.iter().any(|t| t == DOUJINSHI_TAG) {
            p.append_pair("excludedTags[]", DOUJINSHI_TAG);
        }
        for t in &q.include_tags {
            p.append_pair("includedTags[]", t);
        }
        for t in &q.exclude_tags {
            p.append_pair("excludedTags[]", t);
        }
        let ratings: &[&str] = match (q.include_adult, doujin) {
            // "Doujin 18+" means the explicit ones.
            (true, true) => &["erotica", "pornographic"],
            (true, false) => &ALL_RATINGS,
            (false, _) => &["safe", "suggestive"],
        };
        for r in ratings {
            p.append_pair("contentRating[]", r);
        }
        let order = match q.sort {
            SortOrder::Relevance if !text.is_empty() => "order[relevance]",
            SortOrder::Relevance | SortOrder::Popular => "order[followedCount]",
            SortOrder::Latest => "order[latestUploadedChapter]",
        };
        p.append_pair(order, "desc");
    }
    Some(url)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

#[async_trait]
impl CatalogProvider for MangaDex {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        let Some(url) = search_url(q) else {
            return Ok(Paged {
                items: vec![],
                page: q.page,
                has_more: false,
            });
        };
        let res: Collection<MdManga> = self.http.get_json(&url).await?;
        let has_more = res.offset + res.limit < res.total.min(MAX_WINDOW);
        Ok(Paged {
            items: res.data.into_iter().map(MdManga::summary).collect(),
            page: q.page,
            has_more,
        })
    }

    async fn details(&self, manga_id: &str) -> SourceResult<Manga> {
        if !valid_id(manga_id) {
            return Err(SourceError::NotFound);
        }
        let mut url = Url::parse(&format!("{API}/manga/{manga_id}")).expect("validated id");
        url.query_pairs_mut()
            .append_pair("includes[]", "cover_art")
            .append_pair("includes[]", "author")
            .append_pair("includes[]", "artist");
        let res: Entity<MdManga> = self.http.get_json(&url).await?;
        Ok(res.data.details())
    }

    async fn tags(&self) -> SourceResult<Vec<Tag>> {
        let url = Url::parse(&format!("{API}/manga/tag")).expect("static url");
        let res: Collection<MdTag> = self.http.get_json(&url).await?;
        let mut tags: Vec<Tag> = res
            .data
            .into_iter()
            .map(|t| Tag {
                name: t.attributes.name.pick().unwrap_or_else(|| t.id.clone()),
                id: t.id,
                group: t.attributes.group,
            })
            .collect();
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tags)
    }

    async fn cover(&self, url: &str) -> SourceResult<Vec<u8>> {
        if !url.starts_with(COVERS) {
            return Err(SourceError::InvalidResponse(
                "not a MangaDex cover URL".into(),
            ));
        }
        self.http.get_image(url, MAX_COVER_BYTES).await
    }
}

pub fn feed_url(manga_id: &str, languages: &[String], offset: u32) -> Url {
    let mut url = Url::parse(&format!("{API}/manga/{manga_id}/feed")).expect("validated id");
    {
        let mut p = url.query_pairs_mut();
        p.append_pair("limit", &FEED_LIMIT.to_string());
        p.append_pair("offset", &offset.to_string());
        p.append_pair("includes[]", "scanlation_group");
        // External chapters and empty uploads have no pages to show.
        p.append_pair("includeExternalUrl", "0");
        p.append_pair("includeEmptyPages", "0");
        p.append_pair("order[volume]", "asc");
        p.append_pair("order[chapter]", "asc");
        // The work itself is already gated by the user's content setting.
        for r in ALL_RATINGS {
            p.append_pair("contentRating[]", r);
        }
        for l in languages {
            p.append_pair("translatedLanguage[]", l);
        }
    }
    url
}

fn valid_file(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.contains("..")
}

#[async_trait]
impl ChapterProvider for MangaDex {
    async fn chapters(&self, manga_id: &str, languages: &[String]) -> SourceResult<Vec<Chapter>> {
        if !valid_id(manga_id) {
            return Err(SourceError::NotFound);
        }
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let res: Collection<MdChapter> = self
                .http
                .get_json(&feed_url(manga_id, languages, offset))
                .await?;
            let n = res.data.len();
            out.extend(res.data.into_iter().map(|c| c.into_chapter(manga_id)));
            offset += FEED_LIMIT;
            if n == 0 || offset >= res.total || offset + FEED_LIMIT > MAX_WINDOW {
                break;
            }
        }
        Ok(out)
    }

    async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
        let a = self.at_home(chapter_id, true).await?;
        Ok(a.files
            .into_iter()
            .enumerate()
            .map(|(i, file)| PageRef {
                chapter_id: chapter_id.to_string(),
                index: i as u32,
                token: file,
            })
            .collect())
    }
}

#[async_trait]
impl ImageProvider for MangaDex {
    async fn page_image(&self, page: &PageRef) -> SourceResult<Vec<u8>> {
        if !valid_file(&page.token) {
            return Err(SourceError::InvalidResponse("bad page file name".into()));
        }
        let mut last = SourceError::NotFound;
        for attempt in 0..3 {
            let a = self.at_home(&page.chapter_id, attempt > 0).await?;
            let url = format!("{}/data/{}/{}", a.base, a.hash, page.token);
            let start = Instant::now();
            let res = self
                .pages
                .get_image_with_headers(&url, MAX_PAGE_BYTES)
                .await;
            if !a.base.contains("mangadex.org") {
                let (bytes, cached) = match &res {
                    Ok((b, h)) => {
                        let hit = h
                            .get("x-cache")
                            .and_then(|v| v.to_str().ok())
                            .is_some_and(|v| v.starts_with("HIT"));
                        (Some(b.len()), hit)
                    }
                    Err(_) => (None, false),
                };
                self.report(url, bytes, cached, start.elapsed());
            }
            match res {
                Ok((bytes, _)) => return Ok(bytes),
                // Expired token (403) or a bad node: request a fresh base URL and retry.
                Err(e) if e.is_transient() || matches!(e, SourceError::AccessDenied) => {
                    tracing::warn!(error = %e, attempt, "page load failed, refreshing at-home");
                    last = e;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    fn may_cache_pages(&self) -> bool {
        // No caching restriction in the docs (checked 2026-11-14); pages stay on this machine.
        true
    }
}

// ---- wire format (only the fields we use) ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AtHomeResponse {
    base_url: String,
    chapter: AtHomeChapter,
}

#[derive(Deserialize)]
struct AtHomeChapter {
    hash: String,
    data: Vec<String>,
}

#[derive(Deserialize)]
struct MdChapter {
    id: String,
    attributes: MdChapterAttrs,
    #[serde(default)]
    relationships: Vec<Relationship>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MdChapterAttrs {
    volume: Option<String>,
    chapter: Option<String>,
    title: Option<String>,
    translated_language: Option<String>,
    pages: Option<u32>,
}

impl MdChapter {
    fn into_chapter(self, manga_id: &str) -> Chapter {
        let scanlators = self
            .relationships
            .iter()
            .filter(|r| r.kind == "scanlation_group")
            .filter_map(|r| r.attributes.as_ref()?.name.clone())
            .collect();
        let a = self.attributes;
        Chapter {
            manga: MangaKey {
                source: ID.into(),
                id: manga_id.into(),
            },
            id: self.id,
            title: a.title.filter(|t| !t.is_empty()),
            volume: a.volume,
            number: a.chapter,
            language: a.translated_language,
            page_count: a.pages,
            scanlators,
        }
    }
}

#[derive(Deserialize)]
struct Collection<T> {
    data: Vec<T>,
    limit: u32,
    offset: u32,
    total: u32,
}

#[derive(Deserialize)]
struct Entity<T> {
    data: T,
}

/// MangaDex sends empty localized maps as `[]`; accept both shapes.
#[derive(Deserialize)]
#[serde(untagged)]
enum LocalizedString {
    Map(HashMap<String, String>),
    Other(serde::de::IgnoredAny),
}

impl Default for LocalizedString {
    fn default() -> Self {
        Self::Other(serde::de::IgnoredAny)
    }
}

impl LocalizedString {
    fn map(&self) -> Option<&HashMap<String, String>> {
        match self {
            Self::Map(m) => Some(m),
            Self::Other(_) => None,
        }
    }
    /// English, else any value.
    fn pick(&self) -> Option<String> {
        let m = self.map()?;
        m.get("en").or_else(|| m.values().next()).cloned()
    }
}

#[derive(Deserialize)]
struct MdManga {
    id: String,
    attributes: MdAttrs,
    #[serde(default)]
    relationships: Vec<Relationship>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MdAttrs {
    #[serde(default)]
    title: LocalizedString,
    #[serde(default)]
    alt_titles: Vec<LocalizedString>,
    #[serde(default)]
    description: LocalizedString,
    original_language: Option<String>,
    year: Option<u32>,
    #[serde(default)]
    tags: Vec<MdTag>,
    /// Ids on other sites, e.g. "al" (AniList). `null` or `[]` when empty.
    #[serde(default, deserialize_with = "links_map")]
    links: HashMap<String, String>,
}

fn links_map<'de, D: serde::Deserializer<'de>>(d: D) -> Result<HashMap<String, String>, D::Error> {
    Ok(match LocalizedString::deserialize(d)? {
        LocalizedString::Map(m) => m,
        LocalizedString::Other(_) => HashMap::new(),
    })
}

#[derive(Deserialize)]
struct MdTag {
    id: String,
    attributes: MdTagAttrs,
}

#[derive(Deserialize)]
struct MdTagAttrs {
    #[serde(default)]
    name: LocalizedString,
    group: Option<String>,
}

#[derive(Deserialize)]
struct Relationship {
    #[serde(rename = "type")]
    kind: String,
    attributes: Option<RelAttrs>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelAttrs {
    name: Option<String>,
    file_name: Option<String>,
}

impl MdManga {
    fn titles(&self) -> Titles {
        let mut t = Titles::default();
        let all = std::iter::once(&self.attributes.title)
            .chain(&self.attributes.alt_titles)
            .filter_map(LocalizedString::map)
            .flatten();
        for (lang, value) in all {
            let slot = match lang.as_str() {
                "en" => &mut t.english,
                "ja" => &mut t.japanese,
                "th" => &mut t.thai,
                _ => {
                    if !t.alternatives.contains(value) {
                        t.alternatives.push(value.clone());
                    }
                    continue;
                }
            };
            match slot {
                None => *slot = Some(value.clone()),
                Some(existing) if existing != value && !t.alternatives.contains(value) => {
                    t.alternatives.push(value.clone())
                }
                _ => {}
            }
        }
        t
    }

    fn cover_url(&self, size: u32) -> Option<String> {
        let file = self
            .relationships
            .iter()
            .find(|r| r.kind == "cover_art")?
            .attributes
            .as_ref()?
            .file_name
            .as_ref()?;
        Some(format!("{COVERS}/{}/{file}.{size}.jpg", self.id))
    }

    fn summary(self) -> MangaSummary {
        MangaSummary {
            titles: self.titles(),
            cover_url: self.cover_url(256),
            language: self.attributes.original_language.clone(),
            key: MangaKey {
                source: ID.into(),
                id: self.id,
            },
        }
    }

    fn details(self) -> Manga {
        let mut artists: Vec<String> = Vec::new();
        for r in &self.relationships {
            if let ("author" | "artist", Some(name)) = (
                r.kind.as_str(),
                r.attributes.as_ref().and_then(|a| a.name.clone()),
            ) && !artists.contains(&name)
            {
                artists.push(name);
            }
        }
        Manga {
            titles: self.titles(),
            description: self
                .attributes
                .description
                .pick()
                .map(|d| clean_description(&d))
                .filter(|d| !d.is_empty()),
            artists,
            circles: vec![],
            parodies: vec![],
            characters: vec![],
            tags: self
                .attributes
                .tags
                .iter()
                .filter_map(|t| t.attributes.name.pick())
                .collect(),
            language: self.attributes.original_language.clone(),
            published: self.attributes.year.map(|y| y.to_string()),
            page_count: None,
            cover_url: self.cover_url(512),
            source_url: Some(format!("https://mangadex.org/title/{}", self.id)),
            links: self
                .attributes
                .links
                .get("al")
                .map(|id| vec![("anilist".to_string(), id.clone())])
                .unwrap_or_default(),
            provenance: Default::default(),
            key: MangaKey {
                source: ID.into(),
                id: self.id,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed copy of a live GET /manga/{id}?includes[]=cover_art&includes[]=author response.
    const FIXTURE: &str = r#"{"result":"ok","response":"entity","data":{
        "id":"32d76d19-8a05-4db0-9fc2-e0b0648fe9d0","type":"manga",
        "attributes":{"title":{"ko-ro":"Na Honjaman Level-Up"},
          "altTitles":[{"ko":"나 혼자만 레벨업"},{"en":"Solo Leveling"},{"ja":"俺だけレベルアップな件"},{"en":"I level up alone"}],
          "description":[],"links":{"al":"105398","mal":"121496"},"originalLanguage":"ko","year":2018,"contentRating":"safe",
          "tags":[{"id":"0a39b5a1","type":"tag","attributes":{"name":{"en":"Award Winning"},"description":{},"group":"format","version":1},"relationships":[]}]},
        "relationships":[
          {"id":"5671fb47","type":"author","attributes":{"name":"Chugong"}},
          {"id":"50200613","type":"artist","attributes":{"name":"Jang Sung-Rak"}},
          {"id":"50200614","type":"artist","attributes":{"name":"Chugong"}},
          {"id":"e6583e52","type":"cover_art","attributes":{"fileName":"e90bdc47.jpg"}},
          {"id":"0e88ed2f","type":"manga"}]}}"#;

    #[test]
    fn parses_live_shape_into_unified_model() {
        let m = serde_json::from_str::<Entity<MdManga>>(FIXTURE)
            .unwrap()
            .data
            .details();
        assert_eq!(m.titles.english.as_deref(), Some("Solo Leveling"));
        assert_eq!(m.titles.japanese.as_deref(), Some("俺だけレベルアップな件"));
        assert!(
            m.titles
                .alternatives
                .contains(&"I level up alone".to_string())
        );
        assert!(
            m.titles
                .alternatives
                .contains(&"Na Honjaman Level-Up".to_string())
        );
        assert_eq!(m.description, None);
        assert_eq!(m.links, [("anilist".to_string(), "105398".to_string())]);
        assert_eq!(m.artists, ["Chugong", "Jang Sung-Rak"]);
        assert_eq!(m.tags, ["Award Winning"]);
        assert_eq!(
            m.cover_url.as_deref(),
            Some(
                "https://uploads.mangadex.org/covers/32d76d19-8a05-4db0-9fc2-e0b0648fe9d0/e90bdc47.jpg.512.jpg"
            )
        );
        assert_eq!(
            m.source_url.as_deref(),
            Some("https://mangadex.org/title/32d76d19-8a05-4db0-9fc2-e0b0648fe9d0")
        );
    }

    #[test]
    fn search_url_params() {
        let q = SearchQuery {
            text: " solo ".into(),
            languages: vec!["th".into()],
            include_tags: vec!["t1".into()],
            page: 2,
            ..Default::default()
        };
        let url = search_url(&q).unwrap();
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let has = |k: &str, v: &str| pairs.contains(&(k.into(), v.into()));
        assert!(has("offset", "24") && has("limit", "24"));
        assert!(has("title", "solo"));
        assert!(has("availableTranslatedLanguage[]", "th"));
        assert!(has("includedTags[]", "t1"));
        assert!(has("order[relevance]", "desc"));
        assert!(!has("contentRating[]", "pornographic"));

        let popular = SearchQuery {
            sort: SortOrder::Popular,
            page: 1,
            include_adult: true,
            ..Default::default()
        };
        let url = search_url(&popular).unwrap().to_string();
        assert!(url.contains("order%5BfollowedCount%5D=desc"));
        assert!(url.contains("pornographic"));
        assert!(
            search_url(&SearchQuery {
                page: 500,
                ..Default::default()
            })
            .is_none()
        );
    }

    #[test]
    fn parses_feed_chapter_and_builds_feed_url() {
        let json = r#"{"result":"ok","data":[{"id":"c1","type":"chapter",
            "attributes":{"volume":"1","chapter":"1","title":"","translatedLanguage":"en","externalUrl":null,"pages":5},
            "relationships":[{"id":"g1","type":"scanlation_group","attributes":{"name":"Dead-chan"}},{"id":"m","type":"manga"}]}],
            "limit":500,"offset":0,"total":1}"#;
        let res: Collection<MdChapter> = serde_json::from_str(json).unwrap();
        let ch = res.data.into_iter().next().unwrap().into_chapter("m1");
        assert_eq!(ch.title, None);
        assert_eq!(ch.scanlators, ["Dead-chan"]);
        assert_eq!(ch.label(), "Vol. 1 Ch. 1");

        let url = feed_url("m1", &["th".into()], 500).to_string();
        assert!(url.contains("translatedLanguage%5B%5D=th"));
        assert!(url.contains("contentRating%5B%5D=pornographic"));
        assert!(url.contains("includeExternalUrl=0"));
    }

    #[test]
    fn doujinshi_filter_adds_tag_once() {
        let q = SearchQuery {
            category: Category::Doujin,
            include_tags: vec![DOUJINSHI_TAG.into()],
            page: 1,
            ..Default::default()
        };
        let url = search_url(&q).unwrap().to_string();
        assert_eq!(url.matches(DOUJINSHI_TAG).count(), 1);
        assert!(!url.contains("originalLanguage"));

        let adult = SearchQuery {
            include_adult: true,
            ..q
        };
        let url = search_url(&adult).unwrap().to_string();
        assert!(url.contains("pornographic") && !url.contains("=safe"));
    }

    #[test]
    fn categories_map_to_original_language() {
        let q = |category| SearchQuery {
            category,
            page: 1,
            ..Default::default()
        };
        let manhwa = search_url(&q(Category::Manhwa)).unwrap().to_string();
        assert!(manhwa.contains("originalLanguage%5B%5D=ko"));
        assert!(manhwa.contains(&format!("excludedTags%5B%5D={DOUJINSHI_TAG}")));
        let manhua = search_url(&q(Category::Manhua)).unwrap().to_string();
        assert!(manhua.contains("=zh&") && manhua.contains("zh-hk"));
        let all = search_url(&q(Category::All)).unwrap().to_string();
        assert!(!all.contains("originalLanguage") && !all.contains(DOUJINSHI_TAG));
    }

    #[test]
    fn page_file_names_are_validated() {
        assert!(valid_file("x1-ca711f18bf9c3b81.png"));
        assert!(!valid_file("../../etc/passwd"));
        assert!(!valid_file("a/b.png"));
    }

    #[test]
    fn rejects_non_uuid_ids() {
        assert!(valid_id("32d76d19-8a05-4db0-9fc2-e0b0648fe9d0"));
        assert!(!valid_id("../at-home"));
        assert!(!valid_id(""));
    }

    /// Hits the real API. Run with `cargo test -- --ignored mangadex_live`.
    #[tokio::test]
    #[ignore]
    async fn mangadex_live() {
        let md = MangaDex::new().unwrap();
        let q = SearchQuery {
            sort: SortOrder::Popular,
            page: 1,
            ..Default::default()
        };
        let page = md.search(&q).await.unwrap();
        assert_eq!(page.items.len(), PAGE_SIZE as usize);
        let first = &page.items[0];
        let details = md.details(&first.key.id).await.unwrap();
        assert!(!details.titles.display().is_empty());
        let cover = md.cover(first.cover_url.as_ref().unwrap()).await.unwrap();
        assert!(cover.len() > 1000);
        assert!(!md.tags().await.unwrap().is_empty());
    }

    /// Reads one page of an 18+ doujinshi end to end: `cargo test -- --ignored mangadex_live_reader`
    #[tokio::test]
    #[ignore]
    async fn mangadex_live_reader() {
        let md = MangaDex::new().unwrap();
        let q = SearchQuery {
            sort: SortOrder::Popular,
            category: Category::Doujin,
            include_adult: true,
            languages: vec!["en".into()],
            page: 1,
            ..Default::default()
        };
        let work = &md.search(&q).await.unwrap().items[0];
        let chapters = md.chapters(&work.key.id, &["en".into()]).await.unwrap();
        let pages = md.pages(&chapters[0].id).await.unwrap();
        assert!(!pages.is_empty());
        let bytes = md.page_image(&pages[0]).await.unwrap();
        assert!(crate::reader::image_decoder::decode(&bytes, 20_000, 8192).is_ok());
    }
}
