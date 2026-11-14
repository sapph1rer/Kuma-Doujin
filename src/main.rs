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

    // miku-doujin, doujin-lc, hentaithai and kairew are blocked by Cloudflare for non-browser
    // clients, so they are not registered; the three below are reachable.
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

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("KumaDoujin")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "KumaDoujin",
        options,
        Box::new(|cc| Ok(Box::new(app::KumaApp::new(cc, db, registry, data_dir)?))),
    )
    .map_err(|e| anyhow!("UI failed: {e}"))
}
