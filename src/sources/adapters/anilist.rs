//! AniList (https://graphql.anilist.co) metadata adapter: search, details and covers. No pages.
//!
//! Verified 2026-11-14 against https://docs.anilist.co/guide/terms-of-use and
//! /guide/rate-limiting: free for non-commercial use; no using the API as a data store, no
//! mass collection, not inside a competing list/tracker service. 90 req/min, currently
//! degraded to 30/min; 429 carries Retry-After. Public Media queries need no auth.
//! KumaDoujin uses it on demand to enrich details pages (linked via MangaDex `links.al`),
//! and it can be searched by picking it in the source filter.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;

use crate::core::errors::{SourceError, SourceResult};
use crate::core::merge::clean_description;
use crate::core::models::{
    Category, Manga, MangaKey, MangaSummary, Paged, SearchQuery, SortOrder, Titles,
};
use crate::network::client::HttpClient;
use crate::sources::registry::Source;
use crate::sources::traits::{AccessLevel, CatalogProvider, SourceInfo};

pub const ID: &str = "anilist";
const ENDPOINT: &str = "https://graphql.anilist.co";
const PER_PAGE: u32 = 24;
const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;

const MEDIA_FIELDS: &str = "id siteUrl isAdult countryOfOrigin \
    title { romaji english native } synonyms description(asHtml: false) genres \
    tags { name isMediaSpoiler } startDate { year month day } coverImage { large } \
    staff(perPage: 10) { edges { role node { name { full } } } }";

pub struct AniList {
    http: HttpClient,
}

impl AniList {
    pub fn new() -> anyhow::Result<Self> {
        let http = HttpClient::new(
            &["graphql.anilist.co", "s4.anilist.co"],
            // Documented degraded limit is 30/min; stay just under it.
            &[("graphql.anilist.co", Duration::from_millis(2100))],
        )?;
        Ok(Self { http })
    }

    pub fn source() -> anyhow::Result<Source> {
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "AniList".into(),
                homepage: "https://anilist.co".into(),
                access: AccessLevel::MetadataOnly,
                languages: vec![],
                adult: false,
                in_discover: false,
            },
            catalog: Some(Arc::new(Self::new()?)),
            chapters: None,
            images: None,
        })
    }

    async fn query<T: for<'de> Deserialize<'de>>(
        &self,
        query: String,
        vars: serde_json::Value,
    ) -> SourceResult<T> {
        let url = Url::parse(ENDPOINT).expect("static url");
        let res: GqlResponse<T> = self
            .http
            .post_json_for(&url, &json!({ "query": query, "variables": vars }))
            .await?;
        match (res.data, res.errors) {
            (Some(d), _) => Ok(d),
            (None, Some(errs)) => Err(match errs.first().and_then(|e| e.status) {
                Some(404) => SourceError::NotFound,
                Some(429) => SourceError::RateLimited { retry_after: None },
                _ => SourceError::InvalidResponse(
                    errs.first()
                        .map_or("GraphQL error".into(), |e| e.message.clone()),
                ),
            }),
            (None, None) => Err(SourceError::InvalidResponse(
                "empty GraphQL response".into(),
            )),
        }
    }
}

pub fn search_vars(q: &SearchQuery) -> serde_json::Value {
    let sort = match q.sort {
        SortOrder::Relevance if !q.text.trim().is_empty() => "SEARCH_MATCH",
        SortOrder::Relevance | SortOrder::Popular => "POPULARITY_DESC",
        SortOrder::Latest => "START_DATE_DESC",
    };
    let mut vars = json!({ "page": q.page.max(1), "perPage": PER_PAGE, "sort": [sort] });
    if !q.text.trim().is_empty() {
        vars["search"] = json!(q.text.trim());
    }
    if !q.include_adult {
        vars["isAdult"] = json!(false);
    }
    let country = match q.category {
        Category::Manga => Some("JP"),
        Category::Manhwa => Some("KR"),
        Category::Manhua => Some("CN"),
        Category::All | Category::Doujin => None,
    };
    if let Some(c) = country {
        vars["country"] = json!(c);
    }
    vars
}

#[async_trait]
impl CatalogProvider for AniList {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        // AniList has no doujinshi category; the filter can't be honored, so return nothing.
        if q.category == Category::Doujin {
            return Ok(Paged {
                items: vec![],
                page: q.page,
                has_more: false,
            });
        }
        let query = format!(
            "query($page:Int,$perPage:Int,$search:String,$isAdult:Boolean,$sort:[MediaSort],$country:CountryCode) {{ \
             Page(page:$page, perPage:$perPage) {{ pageInfo {{ hasNextPage }} \
             media(type: MANGA, search:$search, isAdult:$isAdult, sort:$sort, countryOfOrigin:$country) {{ {MEDIA_FIELDS} }} }} }}"
        );
        let res: PageData = self.query(query, search_vars(q)).await?;
        Ok(Paged {
            items: res
                .page
                .media
                .into_iter()
                .map(|m| m.into_manga().summary())
                .collect(),
            page: q.page,
            has_more: res.page.page_info.has_next_page,
        })
    }

    async fn details(&self, manga_id: &str) -> SourceResult<Manga> {
        let id: i64 = manga_id.parse().map_err(|_| SourceError::NotFound)?;
        let query = format!("query($id:Int) {{ Media(id:$id, type: MANGA) {{ {MEDIA_FIELDS} }} }}");
        let res: MediaData = self.query(query, json!({ "id": id })).await?;
        Ok(res.media.into_manga())
    }

    async fn cover(&self, url: &str) -> SourceResult<Vec<u8>> {
        self.http.get_image(url, MAX_COVER_BYTES).await
    }
}

trait Summary {
    fn summary(self) -> MangaSummary;
}

impl Summary for Manga {
    fn summary(self) -> MangaSummary {
        MangaSummary {
            key: self.key,
            titles: self.titles,
            cover_url: self.cover_url,
            language: self.language,
        }
    }
}

// ---- wire format ----

#[derive(Deserialize)]
struct GqlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
    status: Option<u16>,
}

#[derive(Deserialize)]
struct MediaData {
    #[serde(rename = "Media")]
    media: AlMedia,
}

#[derive(Deserialize)]
struct PageData {
    #[serde(rename = "Page")]
    page: AlPage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlPage {
    page_info: PageInfo,
    media: Vec<AlMedia>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlMedia {
    id: i64,
    site_url: Option<String>,
    country_of_origin: Option<String>,
    title: AlTitle,
    #[serde(default)]
    synonyms: Vec<String>,
    description: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    tags: Vec<AlTag>,
    start_date: Option<AlDate>,
    cover_image: Option<AlCover>,
    staff: Option<AlStaff>,
}

#[derive(Deserialize)]
struct AlTitle {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlTag {
    name: String,
    #[serde(default)]
    is_media_spoiler: bool,
}

#[derive(Deserialize)]
struct AlDate {
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
}

#[derive(Deserialize)]
struct AlCover {
    large: Option<String>,
}

#[derive(Deserialize)]
struct AlStaff {
    edges: Vec<AlStaffEdge>,
}

#[derive(Deserialize)]
struct AlStaffEdge {
    role: Option<String>,
    node: AlStaffNode,
}

#[derive(Deserialize)]
struct AlStaffNode {
    name: AlName,
}

#[derive(Deserialize)]
struct AlName {
    full: Option<String>,
}

/// Thai script block, used to route Thai synonyms into `Titles::thai`.
fn is_thai(s: &str) -> bool {
    s.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c))
}

impl AlMedia {
    fn into_manga(self) -> Manga {
        let jp = self.country_of_origin.as_deref() == Some("JP");
        let mut titles = Titles {
            english: self.title.english.clone(),
            japanese: if jp { self.title.native.clone() } else { None },
            ..Default::default()
        };
        let mut alts: Vec<String> = Vec::new();
        let mut push = |v: Option<String>| {
            if let Some(v) = v.filter(|v| !v.is_empty())
                && !alts.contains(&v)
            {
                alts.push(v);
            }
        };
        push(self.title.romaji);
        if !jp {
            push(self.title.native);
        }
        for s in self.synonyms {
            if titles.thai.is_none() && is_thai(&s) {
                titles.thai = Some(s);
            } else {
                push(Some(s));
            }
        }
        titles.alternatives = alts
            .into_iter()
            .filter(|a| Some(a) != titles.english.as_ref() && Some(a) != titles.japanese.as_ref())
            .collect();

        let mut artists: Vec<String> = Vec::new();
        for e in self.staff.map(|s| s.edges).unwrap_or_default() {
            let creator = e.role.as_deref().is_some_and(|r| {
                let r = r.to_ascii_lowercase();
                r.contains("story") || r.contains("art")
            });
            if creator
                && let Some(n) = e.node.name.full
                && !artists.contains(&n)
            {
                artists.push(n);
            }
        }
        let mut tags = self.genres;
        tags.extend(
            self.tags
                .into_iter()
                .filter(|t| !t.is_media_spoiler)
                .map(|t| t.name),
        );

        let published = self.start_date.and_then(|d| {
            let y = d.year?;
            Some(match (d.month, d.day) {
                (Some(m), Some(day)) => format!("{y:04}-{m:02}-{day:02}"),
                (Some(m), None) => format!("{y:04}-{m:02}"),
                _ => y.to_string(),
            })
        });
        Manga {
            key: MangaKey {
                source: ID.into(),
                id: self.id.to_string(),
            },
            titles,
            description: self
                .description
                .map(|d| clean_description(&d))
                .filter(|d| !d.is_empty()),
            artists,
            circles: vec![],
            parodies: vec![],
            characters: vec![],
            tags,
            language: None,
            published,
            page_count: None,
            cover_url: self.cover_image.and_then(|c| c.large),
            source_url: self.site_url,
            links: vec![],
            provenance: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from a live `Media(id: 105398)` response (2026-11-14).
    const FIXTURE: &str = r#"{"data":{"Media":{"id":105398,"siteUrl":"https://anilist.co/manga/105398",
        "isAdult":false,"countryOfOrigin":"KR",
        "title":{"romaji":"Na Honjaman Level Up","english":"Solo Leveling","native":"나 혼자만 레벨업"},
        "synonyms":["I Level Up Alone","俺だけレベルアップな件","โซโล่ เลเวลลิ่ง"],
        "description":"A world of Hunters.\n<br><br>\n(Source: Tappytoon)",
        "genres":["Action"],"tags":[{"name":"Dungeon","isMediaSpoiler":false},{"name":"Twist","isMediaSpoiler":true}],
        "startDate":{"year":2018,"month":3,"day":4},
        "coverImage":{"large":"https://s4.anilist.co/file/anilistcdn/media/manga/cover/medium/bx105398.jpg"},
        "staff":{"edges":[{"role":"Art (chs 1-179)","node":{"name":{"full":"Seong-Rak Jang"}}},
                          {"role":"Original Story","node":{"name":{"full":"Chu-Gong"}}},
                          {"role":"Translator","node":{"name":{"full":"Someone"}}}]}}}}"#;

    #[test]
    fn maps_media_into_unified_model() {
        let res: GqlResponse<MediaData> = serde_json::from_str(FIXTURE).unwrap();
        let m = res.data.unwrap().media.into_manga();
        assert_eq!(m.key.id, "105398");
        assert_eq!(m.titles.english.as_deref(), Some("Solo Leveling"));
        assert_eq!(m.titles.japanese, None); // Korean work: native goes to alternatives
        assert_eq!(m.titles.thai.as_deref(), Some("โซโล่ เลเวลลิ่ง"));
        assert!(
            m.titles
                .alternatives
                .contains(&"나 혼자만 레벨업".to_string())
        );
        assert_eq!(m.artists, ["Seong-Rak Jang", "Chu-Gong"]);
        assert_eq!(m.tags, ["Action", "Dungeon"]);
        assert_eq!(m.published.as_deref(), Some("2018-03-04"));
        assert_eq!(
            m.description.as_deref(),
            Some("A world of Hunters.\n\n(Source: Tappytoon)")
        );
    }

    #[test]
    fn graphql_errors_map_to_source_errors() {
        let res: GqlResponse<MediaData> = serde_json::from_str(
            r#"{"data":null,"errors":[{"message":"Not Found.","status":404}]}"#,
        )
        .unwrap();
        assert!(res.data.is_none() && res.errors.unwrap()[0].status == Some(404));
    }

    #[test]
    fn search_vars_respect_adult_and_sort() {
        let q = SearchQuery {
            text: "solo".into(),
            page: 2,
            ..Default::default()
        };
        let v = search_vars(&q);
        assert_eq!(v["isAdult"], json!(false));
        assert_eq!(v["sort"], json!(["SEARCH_MATCH"]));
        assert_eq!(v["page"], json!(2));
        let adult = SearchQuery {
            include_adult: true,
            sort: SortOrder::Latest,
            ..Default::default()
        };
        let v = search_vars(&adult);
        assert!(v.get("isAdult").is_none());
        assert_eq!(v["sort"], json!(["START_DATE_DESC"]));
    }

    /// `cargo test -- --ignored anilist_live`
    #[tokio::test]
    #[ignore]
    async fn anilist_live() {
        let al = AniList::new().unwrap();
        let m = al.details("105398").await.unwrap();
        assert_eq!(m.titles.english.as_deref(), Some("Solo Leveling"));
        assert!(
            al.cover(m.cover_url.as_deref().unwrap())
                .await
                .unwrap()
                .len()
                > 1000
        );
        let page = al
            .search(&SearchQuery {
                text: "Solo Leveling".into(),
                category: Category::Manhwa,
                page: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(page.items.iter().any(|i| i.key.id == "105398"));
    }
}
