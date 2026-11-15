// A GUI app on Windows: no console window. Logs go to `<data dir>/kumadoujin.log` instead.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::sync::Mutex;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use kumadoujin::{app, core, database, sources, network};

fn main() -> anyhow::Result<()> {
    let data_dir = core::config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    // Truncated each launch so the file never grows unbounded.
    let log = std::fs::File::create(data_dir.join("kumadoujin.log"))?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(log))
        .with_ansi(false)
        .init();

    kumadoujin::update::clean_old();
    let result = run(data_dir);
    if let Err(e) = &result {
        tracing::error!(error = %e, "fatal error");
    }
    result
}

fn run(data_dir: std::path::PathBuf) -> anyhow::Result<()> {
    let db = database::Database::open(&core::config::database_path())?;
    // Each adapter is added only after its terms are verified (ROADMAP.md).
    let mut registry = sources::registry::SourceRegistry::default();
    registry.register(sources::adapters::mangadex::MangaDex::source()?)?;
    registry.register(sources::adapters::anilist::AniList::source()?)?;

    let theme_http = Arc::new(network::client::HttpClient::new(
        &["dark-manga.com", "go-manga.com", "up-manga.com"],
        &[
            ("www.dark-manga.com", Duration::from_millis(400)),
            ("www.go-manga.com", Duration::from_millis(400)),
            ("www.up-manga.com", Duration::from_millis(400)),
        ],
    )?);
    registry.register(sources::adapters::dark_manga::DarkManga::source(theme_http.clone())?)?;
    registry.register(sources::adapters::go_manga::GoManga::source(theme_http.clone())?)?;
    registry.register(sources::adapters::up_manga::UpManga::source(theme_http)?)?;

    // These sites answer plain HTTP with 403 (browser check); the helper browser next to the exe
    // (kuma-browser.exe) loads their pages instead. Kairew is listing-only: its reader is
    // protected, so the details page links to the site.
    let gated_http = Arc::new(
        network::client::HttpClient::new(
            &["miku-doujin.com", "doujin-lc.net", "hentaithai.net", "mangaclaw.com", "manga-gg.com", "images-delivery.com", "kairew.com"],
            &[
                ("miku-doujin.com", Duration::from_millis(400)),
                ("doujin-lc.net", Duration::from_millis(400)),
                ("hentaithai.net", Duration::from_millis(400)),
                ("kairew.com", Duration::from_millis(400)),
            ],
        )?
        .with_browser(network::browser::BrowserProcess::beside_current_exe().map(Arc::new)),
    );
    registry.register(sources::adapters::miku_doujin::MikuDoujin::source(gated_http.clone())?)?;
    registry.register(sources::adapters::doujin_lc::DoujinLc::source(gated_http.clone())?)?;
    registry.register(sources::adapters::hentaithai::HentaiThai::source(gated_http.clone())?)?;
    registry.register(sources::adapters::kairew::Kairew::source(gated_http)?)?;

    // nhentai.xxx answers plain HTTP; images come from i*.nhentaimg.com.
    let nh_http = Arc::new(network::client::HttpClient::new(
        &["nhentai.xxx", "nhentaimg.com"],
        &[("nhentai.xxx", Duration::from_millis(300))],
    )?);
    registry.register(sources::adapters::nhentai_xxx::NhentaiXxx::source(nh_http)?)?;

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("KumaDoujin")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 520.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/kuma.png"))
                    .unwrap_or_default(),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "KumaDoujin",
        options,
        Box::new(|cc| Ok(Box::new(app::KumaApp::new(cc, db, registry, data_dir)?))),
    )
    .map_err(|e| anyhow!("UI failed: {e}"))
}
