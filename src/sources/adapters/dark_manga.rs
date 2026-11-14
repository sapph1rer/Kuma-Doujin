//! Dark Manga (MangaStream theme).
use std::sync::Arc;

use crate::network::client::HttpClient;
use crate::sources::registry::Source;

pub const ID: &str = "dark-manga";

pub struct DarkManga;

impl DarkManga {
    pub fn source(http: Arc<HttpClient>) -> anyhow::Result<Source> {
        super::mangastream::source(http, ID, "Dark Manga", "https://www.dark-manga.com", false)
    }
}
