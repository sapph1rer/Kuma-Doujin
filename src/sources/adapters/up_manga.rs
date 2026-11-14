//! Up Manga (MangaStream theme).
use std::sync::Arc;

use crate::network::client::HttpClient;
use crate::sources::registry::Source;

pub const ID: &str = "up-manga";

pub struct UpManga;

impl UpManga {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        super::mangastream::source(http, ID, "Up Manga", "https://www.up-manga.com", false)
    }
}
