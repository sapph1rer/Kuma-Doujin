use std::sync::Arc;

use thiserror::Error;

use super::traits::{AccessLevel, CatalogProvider, ChapterProvider, ImageProvider, SourceInfo};

/// A registered source and the interfaces it implements.
/// One struct may back several fields (clone the same `Arc`).
#[derive(Clone)]
pub struct Source {
    pub info: SourceInfo,
    pub catalog: Option<Arc<dyn CatalogProvider>>,
    pub chapters: Option<Arc<dyn ChapterProvider>>,
    pub images: Option<Arc<dyn ImageProvider>>,
}

impl Source {
    pub fn can_read(&self) -> bool {
        self.info.access == AccessLevel::Reading && self.chapters.is_some() && self.images.is_some()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("source id {0:?} is already registered")]
    Duplicate(String),
    #[error("source {0:?} declares Reading access but lacks chapter or image providers")]
    IncompleteReader(String),
    #[error("source {0:?} is metadata-only but provides page images")]
    UnauthorizedImages(String),
}

#[derive(Default, Clone)]
pub struct SourceRegistry {
    sources: Vec<Source>,
    /// Ids the user switched off: hidden from catalogs and enrichment, still openable by id.
    disabled: Vec<String>,
}

impl SourceRegistry {
    pub fn register(&mut self, source: Source) -> Result<(), RegistryError> {
        let id = &source.info.id;
        if self.get(id).is_some() {
            return Err(RegistryError::Duplicate(id.clone()));
        }
        match source.info.access {
            AccessLevel::Reading if source.chapters.is_none() || source.images.is_none() => {
                return Err(RegistryError::IncompleteReader(id.clone()));
            }
            // Guard: metadata-only sources must never be wired to the reader.
            AccessLevel::MetadataOnly if source.images.is_some() => {
                return Err(RegistryError::UnauthorizedImages(id.clone()));
            }
            _ => {}
        }
        self.sources.push(source);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.info.id == id)
    }

    pub fn all(&self) -> &[Source] {
        &self.sources
    }

    /// Catalogs of enabled sources.
    pub fn catalogs(&self) -> impl Iterator<Item = (&SourceInfo, &Arc<dyn CatalogProvider>)> {
        self.sources
            .iter()
            .filter(|s| self.is_enabled(&s.info.id))
            .filter_map(|s| s.catalog.as_ref().map(|c| (&s.info, c)))
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.iter().any(|d| d == id)
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) {
        self.disabled.retain(|d| d != id);
        if !on {
            self.disabled.push(id.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::errors::SourceResult;
    use crate::core::models::*;
    use async_trait::async_trait;

    /// In-memory provider used only to exercise the traits and registry.
    struct Fake;

    #[async_trait]
    impl CatalogProvider for Fake {
        async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
            let item = MangaSummary {
                key: MangaKey {
                    source: "fake".into(),
                    id: q.text.clone(),
                },
                titles: Titles {
                    english: Some(q.text.clone()),
                    ..Default::default()
                },
                cover_url: None,
                language: Some("en".into()),
            };
            Ok(Paged {
                items: vec![item],
                page: q.page,
                has_more: false,
            })
        }
        async fn details(&self, _: &str) -> SourceResult<Manga> {
            Err(crate::core::errors::SourceError::NotFound)
        }
        async fn cover(&self, _: &str) -> SourceResult<Vec<u8>> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl ChapterProvider for Fake {
        async fn chapters(&self, _: &str, _: &[String]) -> SourceResult<Vec<Chapter>> {
            Ok(vec![])
        }
        async fn pages(&self, chapter_id: &str) -> SourceResult<Vec<PageRef>> {
            Ok((0..3)
                .map(|i| PageRef {
                    chapter_id: chapter_id.into(),
                    index: i,
                    token: i.to_string(),
                })
                .collect())
        }
    }

    #[async_trait]
    impl ImageProvider for Fake {
        async fn page_image(&self, p: &PageRef) -> SourceResult<Vec<u8>> {
            Ok(p.token.clone().into_bytes())
        }
    }

    fn info(id: &str, access: AccessLevel) -> SourceInfo {
        SourceInfo {
            id: id.into(),
            name: id.into(),
            homepage: "https://example.invalid".into(),
            access,
            languages: vec![],
            adult: false,
            in_discover: true,
        }
    }

    fn reader(id: &str) -> Source {
        let fake = Arc::new(Fake);
        Source {
            info: info(id, AccessLevel::Reading),
            catalog: Some(fake.clone()),
            chapters: Some(fake.clone()),
            images: Some(fake),
        }
    }

    #[test]
    fn rejects_duplicates() {
        let mut r = SourceRegistry::default();
        r.register(reader("a")).unwrap();
        assert_eq!(
            r.register(reader("a")),
            Err(RegistryError::Duplicate("a".into()))
        );
    }

    #[test]
    fn enforces_access_level() {
        let mut r = SourceRegistry::default();
        let mut incomplete = reader("x");
        incomplete.images = None;
        assert_eq!(
            r.register(incomplete),
            Err(RegistryError::IncompleteReader("x".into()))
        );

        let mut meta = reader("m");
        meta.info.access = AccessLevel::MetadataOnly;
        assert_eq!(
            r.register(meta.clone()),
            Err(RegistryError::UnauthorizedImages("m".into()))
        );

        meta.images = None;
        r.register(meta).unwrap();
        assert!(!r.get("m").unwrap().can_read());
    }

    #[test]
    fn disabled_sources_leave_catalogs_but_stay_gettable() {
        let mut r = SourceRegistry::default();
        r.register(reader("a")).unwrap();
        r.register(reader("b")).unwrap();
        r.set_enabled("a", false);
        let ids: Vec<_> = r.catalogs().map(|(i, _)| i.id.clone()).collect();
        assert_eq!(ids, ["b"]);
        assert!(r.get("a").is_some() && !r.is_enabled("a"));
        r.set_enabled("a", true);
        assert_eq!(r.catalogs().count(), 2);
    }

    #[tokio::test]
    async fn providers_are_usable_through_the_registry() {
        let mut r = SourceRegistry::default();
        r.register(reader("fake")).unwrap();
        let q = SearchQuery {
            text: "abc".into(),
            page: 1,
            ..Default::default()
        };
        let (_, catalog) = r.catalogs().next().unwrap();
        let res = catalog.search(&q).await.unwrap();
        assert_eq!(res.items[0].titles.display(), "abc");

        let src = r.get("fake").unwrap();
        let pages = src.chapters.as_ref().unwrap().pages("c1").await.unwrap();
        assert_eq!(pages.iter().map(|p| p.index).collect::<Vec<_>>(), [0, 1, 2]);
        let img = src
            .images
            .as_ref()
            .unwrap()
            .page_image(&pages[2])
            .await
            .unwrap();
        assert_eq!(img, b"2");
    }
}
