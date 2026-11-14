//! Kairew - Manga aggregator (metadata-only).
use async_trait::async_trait;
use std::sync::Arc;

use crate::core::errors::{SourceError, SourceResult};
use crate::core::models::{Manga, MangaSummary, Paged, SearchQuery};
use crate::sources::registry::Source;
use crate::sources::traits::{AccessLevel, CatalogProvider, SourceInfo};

pub const ID: &str = "kairew";

pub struct Kairew;

#[async_trait]
impl CatalogProvider for Kairew {
    async fn search(&self, q: &SearchQuery) -> SourceResult<Paged<MangaSummary>> {
        Ok(Paged {
            items: vec![],
            page: q.page,
            has_more: false,
        })
    }

    async fn details(&self, _: &str) -> SourceResult<Manga> {
        Err(SourceError::NotFound)
    }

    async fn cover(&self, _: &str) -> SourceResult<Vec<u8>> {
        Ok(vec![])
    }
}

impl Kairew {
    pub fn source() -> anyhow::Result<Source> {
        Ok(Source {
            info: SourceInfo {
                id: ID.into(),
                name: "Kairew".into(),
                homepage: "https://kairew.com".into(),
                access: AccessLevel::MetadataOnly,
                languages: vec![],
                adult: false,
                in_discover: true,
            },
            catalog: Some(Arc::new(Self)),
            chapters: None,
            images: None,
        })
    }
}
