//! Go Manga (MangaStream theme).
use std::sync::Arc;

use crate::network::client::HttpClient;
use crate::sources::registry::Source;

pub const ID: &str = "go-manga";

pub struct GoManga;

impl GoManga {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        super::mangastream::source(http, ID, "Go Manga", "https://www.go-manga.com", false)
    }
}
