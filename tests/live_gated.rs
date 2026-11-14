//! Hits the real sites through the helper browser:
//! `KUMA_BROWSER_EXE=target/debug/kuma-browser.exe cargo test --test live_gated -- --ignored --nocapture`
use std::sync::Arc;

use kumadoujin::core::models::{SearchQuery, SortOrder};
use kumadoujin::network::browser::BrowserProcess;
use kumadoujin::network::client::HttpClient;
use kumadoujin::sources::adapters::{doujin_lc::DoujinLc, hentaithai::HentaiThai, kairew::Kairew, miku_doujin::MikuDoujin};
use kumadoujin::sources::registry::Source;

async fn walk(s: Source, search_word: &str) {
    let name = s.info.name.clone();
    let cat = s.catalog.unwrap();
    let ch = s.chapters.unwrap();
    let img = s.images.unwrap();
    let q = SearchQuery { page: 1, sort: SortOrder::Popular, ..Default::default() };
    let list = cat.search(&q).await.unwrap();
    println!("[{name}] p1: {} items, more={}", list.items.len(), list.has_more);
    assert!(list.items.len() > 3);
    let p2 = cat.search(&SearchQuery { page: 2, ..q.clone() }).await.unwrap();
    println!("  p2: {} items first={:?}", p2.items.len(), p2.items.first().map(|i| i.key.id.clone()));
    assert!(!p2.items.is_empty());
    let first = &list.items[0];
    println!("  first: {} id={} cover={:?}", first.titles.display(), first.key.id, first.cover_url);
    let cover = cat.cover(first.cover_url.as_ref().expect("cover url")).await.unwrap();
    println!("  cover bytes: {}", cover.len());
    let m = cat.details(&first.key.id).await.unwrap();
    println!("  details: {} tags={:?} desc_len={}", m.titles.display(), m.tags, m.description.as_deref().map_or(0, str::len));
    let chapters = ch.chapters(&first.key.id, &[]).await.unwrap();
    println!("  chapters: {} first={:?}", chapters.len(), chapters.first().map(|c| c.label()));
    let pages = ch.pages(&chapters[0].id).await.unwrap();
    println!("  pages: {}", pages.len());
    let bytes = img.page_image(&pages[0]).await.unwrap();
    println!("  page0 bytes: {}", bytes.len());
    if let Ok(dir) = std::env::var("DUMP_PAGE") {
        std::fs::write(format!("{dir}/{}.jpg", name.replace(' ', "_")), &bytes).unwrap();
    }
    let sq = SearchQuery { page: 1, text: search_word.into(), ..Default::default() };
    println!("  search '{}': {} items", sq.text, cat.search(&sq).await.unwrap().items.len());
}

fn http() -> Arc<HttpClient> {
    let browser = Arc::new(BrowserProcess::beside_current_exe().expect("KUMA_BROWSER_EXE"));
    Arc::new(
        HttpClient::new(&["miku-doujin.com", "doujin-lc.net", "hentaithai.net", "mangaclaw.com", "kairew.com"], &[])
            .unwrap()
            .with_browser(Some(browser)),
    )
}

// One test per source, so a failure names the site that broke. The helper browser handles one page at
// a time, so run these with `--test-threads=1`.

#[tokio::test]
#[ignore]
async fn hentaithai() {
    walk(HentaiThai::source(http()).unwrap(), "sekitsui").await;
}

#[tokio::test]
#[ignore]
async fn doujin_lc() {
    walk(DoujinLc::source(http()).unwrap(), "wife").await;
}

#[tokio::test]
#[ignore]
async fn miku_doujin() {
    walk(MikuDoujin::source(http()).unwrap(), "Zen").await;
}

#[tokio::test]
#[ignore]
async fn kairew() {
    let k = Kairew::source(http()).unwrap().catalog.unwrap();
    let list = k.search(&SearchQuery { page: 1, ..Default::default() }).await.unwrap();
    println!("[Kairew] {} items", list.items.len());
    assert!(list.items.len() > 10);
    let f = &list.items[0];
    println!("  first: {} cover bytes: {}", f.titles.display(), k.cover(f.cover_url.as_ref().unwrap()).await.unwrap().len());
    let m = k.details(&f.key.id).await.unwrap();
    println!("  details: {} desc_len={}", m.titles.display(), m.description.as_deref().map_or(0, str::len));
}
