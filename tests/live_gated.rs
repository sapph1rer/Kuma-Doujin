//! Hits the real sites through the helper browser:
//! `KUMA_BROWSER_EXE=target/debug/kuma-browser.exe cargo test --test live_gated -- --ignored --nocapture`
use std::sync::Arc;

use kumadoujin::core::models::{SearchQuery, SortOrder};
use kumadoujin::network::browser::BrowserProcess;
use kumadoujin::network::client::HttpClient;
use kumadoujin::sources::adapters::{doujin_lc::DoujinLc, hentaithai::HentaiThai, miku_doujin::MikuDoujin};
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
    let sq = SearchQuery { page: 1, text: search_word.into(), ..Default::default() };
    println!("  search '{}': {} items", sq.text, cat.search(&sq).await.unwrap().items.len());
}

#[tokio::test]
#[ignore]
async fn live() {
    let browser = Arc::new(BrowserProcess::beside_current_exe().expect("KUMA_BROWSER_EXE"));
    let http = Arc::new(
        HttpClient::new(&["miku-doujin.com", "doujin-lc.net", "hentaithai.net", "mangaclaw.com"], &[])
            .unwrap()
            .with_browser(Some(browser)),
    );
    walk(HentaiThai::source(http.clone()).unwrap(), "sekitsui").await;
    walk(DoujinLc::source(http.clone()).unwrap(), "wife").await;
    walk(MikuDoujin::source(http).unwrap(), "mother").await;
}
