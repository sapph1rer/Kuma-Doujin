//! Hits the real sites; run with `cargo test --test live_mangastream -- --ignored --nocapture`.
use std::sync::Arc;
use std::time::Duration;

use kumadoujin::core::models::{SearchQuery, SortOrder};
use kumadoujin::network::client::HttpClient;
use kumadoujin::sources::adapters::{dark_manga::DarkManga, go_manga::GoManga, nhentai_xxx::NhentaiXxx, up_manga::UpManga};
use kumadoujin::sources::registry::Source;

async fn walk(s: Source) {
    let name = s.info.name.clone();
    let cat = s.catalog.unwrap();
    let ch = s.chapters.unwrap();
    let img = s.images.unwrap();
    let q = SearchQuery {
        page: 1,
        sort: SortOrder::Popular,
        ..Default::default()
    };
    let list = cat.search(&q).await.unwrap();
    println!("[{name}] popular: {} items, more={}", list.items.len(), list.has_more);
    assert!(list.items.len() > 3);
    let first = &list.items[0];
    println!("  first: {} id={} cover={:?}", first.titles.display(), first.key.id, first.cover_url);
    let cover = cat.cover(first.cover_url.as_ref().expect("cover url")).await.unwrap();
    println!("  cover bytes: {}", cover.len());
    let m = cat.details(&first.key.id).await.unwrap();
    println!("  details: {} tags={:?} desc_len={}", m.titles.display(), m.tags, m.description.as_deref().map_or(0, str::len));
    let chapters = ch.chapters(&first.key.id, &[]).await.unwrap();
    println!("  chapters: {} first={:?} last={:?}", chapters.len(), chapters.first().map(|c| c.label()), chapters.last().map(|c| c.label()));
    let pages = ch.pages(&chapters[0].id).await.unwrap();
    println!("  pages: {}", pages.len());
    let bytes = img.page_image(&pages[0]).await.unwrap();
    println!("  page0 bytes: {}", bytes.len());
    let sq = SearchQuery { page: 1, text: first.titles.display().split(' ').next().unwrap().into(), ..Default::default() };
    println!("  search '{}': {} items", sq.text, cat.search(&sq).await.unwrap().items.len());
}

fn http() -> Arc<HttpClient> {
    Arc::new(
        HttpClient::new(
            &["dark-manga.com", "go-manga.com", "up-manga.com"],
            &[
                ("www.dark-manga.com", Duration::from_millis(400)),
                ("www.go-manga.com", Duration::from_millis(400)),
                ("www.up-manga.com", Duration::from_millis(400)),
            ],
        )
        .unwrap(),
    )
}

// One test per source, so a failure names the site that broke.

#[tokio::test]
#[ignore]
async fn dark_manga() {
    walk(DarkManga::source(http()).unwrap()).await;
}

#[tokio::test]
#[ignore]
async fn go_manga() {
    walk(GoManga::source(http()).unwrap()).await;
}

#[tokio::test]
#[ignore]
async fn up_manga() {
    walk(UpManga::source(http()).unwrap()).await;
}

#[tokio::test]
#[ignore]
async fn nhentai_xxx() {
    let http = Arc::new(HttpClient::new(&["nhentai.xxx", "nhentaimg.com"], &[]).unwrap());
    walk(NhentaiXxx::source(http).unwrap()).await;
}
