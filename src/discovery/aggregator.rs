//! Queries several catalog sources concurrently and merges their results into one list,
//! so Discover/Search show every connected source in a single UI.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::models::{MangaSummary, SearchQuery};
use crate::sources::health::HealthBook;
use crate::sources::traits::CatalogProvider;

/// One catalog to query: source id (for health), display name (for errors), provider.
pub struct CatalogRef {
    pub id: String,
    pub name: String,
    pub provider: Arc<dyn CatalogProvider>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Aggregated {
    pub items: Vec<MangaSummary>,
    pub has_more: bool,
    /// (source name, error message) for sources that failed; others still show.
    pub errors: Vec<(String, String)>,
}

/// A source that has not answered by then is reported as an error; the others still show.
const SOURCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

pub async fn search_all(
    catalogs: Vec<CatalogRef>,
    query: SearchQuery,
    health: Arc<HealthBook>,
) -> Aggregated {
    let tasks: Vec<_> = catalogs
        .into_iter()
        .map(|c| {
            let (q, health) = (query.clone(), health.clone());
            let task = tokio::spawn(async move {
                let started = std::time::Instant::now();
                let res = tokio::time::timeout(SOURCE_TIMEOUT, c.provider.search(&q))
                    .await
                    .unwrap_or_else(|_| Err(crate::core::errors::SourceError::Network("timed out".into())));
                health.record(&c.id, started, &res);
                res
            });
            (c.name, task)
        })
        .collect();

    let mut out = Aggregated::default();
    let mut lists = Vec::new();
    for (name, task) in tasks {
        match task.await {
            Ok(Ok(page)) => {
                out.has_more |= page.has_more;
                lists.push(page.items.into_iter());
            }
            Ok(Err(e)) => out.errors.push((name, e.to_string())),
            Err(e) => out.errors.push((name, format!("task failed: {e}"))),
        }
    }
    // Round-robin so no single source dominates the first rows.
    while !lists.is_empty() {
        lists.retain_mut(|it| it.next().map(|m| out.items.push(m)).is_some());
    }
    if !query.text.trim().is_empty() {
        // best title matches first; the sort is stable, so equal scores keep the round-robin order
        let words = words(&query.text);
        out.items.sort_by_key(|m| std::cmp::Reverse(relevance(&words, m)));
    }
    out
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Higher is better: exact title, then prefix, then every word present, then some words.
fn relevance(query: &[String], m: &MangaSummary) -> u32 {
    let t = &m.titles;
    [&t.english, &t.japanese, &t.thai]
        .into_iter()
        .flatten()
        .chain(&t.alternatives)
        .map(|title| {
            let title_words = words(title);
            let joined = title_words.join(" ");
            let q = query.join(" ");
            let hits = query.iter().filter(|w| title_words.iter().any(|t| t.starts_with(*w))).count() as u32;
            if joined == q {
                1000
            } else if joined.starts_with(&q) {
                500 + hits
            } else if hits as usize == query.len() {
                200 + hits
            } else {
                hits * 10
            }
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::errors::{SourceError, SourceResult};
    use crate::core::models::*;
    use async_trait::async_trait;

    struct Fixed(&'static str, usize, bool);

    #[async_trait]
    impl CatalogProvider for Fixed {
        async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
            if self.2 {
                return Err(SourceError::AccessDenied);
            }
            let items = (0..self.1)
                .map(|i| MangaSummary {
                    key: MangaKey {
                        source: self.0.into(),
                        id: i.to_string(),
                    },
                    titles: Titles::default(),
                    cover_url: None,
                    language: None,
                })
                .collect();
            Ok(Paged {
                items,
                page: q.page,
                has_more: self.0 == "a",
            })
        }
        async fn details(&self, _: &str) -> SourceResult<Manga> {
            Err(SourceError::NotFound)
        }
        async fn cover(&self, _: &str) -> SourceResult<Vec<u8>> {
            Err(SourceError::NotFound)
        }
    }

    #[test]
    fn relevance_prefers_exact_then_prefix_then_all_words() {
        let m = |title: &str| MangaSummary {
            key: MangaKey { source: "s".into(), id: title.into() },
            titles: Titles { english: Some(title.into()), ..Default::default() },
            cover_url: None,
            language: None,
        };
        let q = words("solo leveling");
        let exact = relevance(&q, &m("Solo Leveling"));
        let prefix = relevance(&q, &m("Solo Leveling: Ragnarok"));
        let all = relevance(&q, &m("The Leveling of Solo Heroes"));
        let some = relevance(&q, &m("Solo Camping"));
        assert!(exact > prefix && prefix > all && all > some && some > 0);
    }

    #[tokio::test]
    async fn interleaves_sources_and_keeps_partial_results() {
        let cat = |id: &'static str, n, fail| CatalogRef {
            id: id.into(),
            name: id.to_uppercase(),
            provider: Arc::new(Fixed(id, n, fail)),
        };
        let cats = vec![cat("a", 3, false), cat("b", 1, false), cat("c", 5, true)];
        let health = Arc::new(HealthBook::default());
        let res = search_all(cats, SearchQuery::default(), health.clone()).await;
        use crate::sources::health::Status;
        assert_eq!(health.get("a").status(), Status::Healthy);
        assert_eq!(health.get("c").status(), Status::Degraded);
        let order: Vec<_> = res
            .items
            .iter()
            .map(|m| format!("{}{}", m.key.source, m.key.id))
            .collect();
        assert_eq!(order, ["a0", "b0", "a1", "a2"]);
        assert!(res.has_more);
        assert_eq!(res.errors.len(), 1);
        assert_eq!(res.errors[0].0, "C");
    }
}
