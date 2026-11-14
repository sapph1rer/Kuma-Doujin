use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, RichText};
use serde::{Serialize, de::DeserializeOwned};

use crate::cache::disk::DiskCache;
use crate::core::merge;
use crate::core::models::{Category, Chapter, Manga, MangaKey, PageRef, SortOrder, Tag};
use crate::database::Database;
use crate::database::library::{Bookmark, Favorite, Progress, ResumePoint, now_millis};
use crate::discovery::aggregator::{Aggregated, CatalogRef, search_all};
use crate::downloads::{DlStatus, Downloads, Entry, FILE, job_key};
use crate::reader::engine::{self, ReaderSettings};
use crate::reader::image_decoder;
use crate::sources::health::HealthBook;
use crate::sources::registry::SourceRegistry;
use crate::ui::browse::{self, Actions, Alt, BrowseState, Covers, DetailsState, DlInfo, TagsSlot};
use crate::ui::library::{self, LibraryAction};
use crate::ui::reader::{self, PageSlot, ReaderState};
use crate::ui::{View, icons, theme, views};
use crate::update::{self, Release};
use crate::worker::Worker;

const SHOW_ADULT_KEY: &str = "content.show_adult";
const AGE_CONFIRMED_KEY: &str = "content.age_confirmed";
const READER_SETTINGS_KEY: &str = "reader.settings";
const CHAPTER_LANG_KEY: &str = "reader.chapter_lang";
const CATEGORY_KEY: &str = "content.category";
const CHECK_UPDATES_KEY: &str = "updates.check_on_start";
const SEARCH_HISTORY_KEY: &str = "search.history";
const SEARCH_HISTORY_MAX: usize = 12;
/// Seconds for the fade between pages.
const PAGE_FADE: f64 = 0.22;
const PAGE_CACHE_MB_KEY: &str = "cache.pages_mb";
const LISTING_TTL: Duration = Duration::from_secs(10 * 60);
const DETAILS_TTL: Duration = Duration::from_secs(24 * 3600);
const CHAPTERS_TTL: Duration = Duration::from_secs(20 * 60);
const COVER_CACHE_BYTES: u64 = 300 * 1024 * 1024;
const DEFAULT_PAGE_CACHE_MB: u64 = 1024;
const PAGE_CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_COVER_DIM: u32 = 2048;
/// Largest page side accepted for decoding (long strips); output is fit to the GPU limit.
const MAX_PAGE_DIM: u32 = 20_000;
const HISTORY_LIMIT: usize = 200;

/// The user's library as shown in Home, History and Bookmarks; reloaded after each change.
#[derive(Default)]
pub struct Library {
    pub recent: Vec<Progress>,
    pub bookmarks: Vec<Bookmark>,
    pub favorites: Vec<Favorite>,
    /// Recently opened details pages (`added_at` = view time, unix ms).
    pub viewed: Vec<Favorite>,
}

fn load_library(db: &Database) -> Library {
    let log = |e: &rusqlite::Error| tracing::error!(error = %e, "loading library failed");
    Library {
        recent: db
            .recent_progress(HISTORY_LIMIT)
            .inspect_err(log)
            .unwrap_or_default(),
        bookmarks: db.bookmarks().inspect_err(log).unwrap_or_default(),
        favorites: db.favorites().inspect_err(log).unwrap_or_default(),
        viewed: db.recently_viewed(20).inspect_err(log).unwrap_or_default(),
    }
}

/// Results delivered from background tasks to the GUI thread.
pub enum Msg {
    Noop,
    Library(Box<Library>),
    Listing {
        view: View,
        generation: u64,
        result: Aggregated,
        /// false for partial results while slower sources are still loading
        done: bool,
    },
    Details {
        key: MangaKey,
        result: Result<Box<Manga>, String>,
    },
    Chapters {
        key: MangaKey,
        lang: String,
        result: Result<Vec<Chapter>, String>,
    },
    Cover {
        url: String,
        result: Result<ColorImage, String>,
    },
    Tags {
        source: String,
        result: Result<Vec<Tag>, String>,
    },
    Pages {
        generation: u64,
        result: Result<Vec<PageRef>, String>,
    },
    Page {
        generation: u64,
        index: usize,
        result: Result<ColorImage, String>,
    },
    /// The offline library changed (download finished or deleted).
    Downloads(Vec<Entry>),
    Update {
        manual: bool,
        result: Result<Release, String>,
    },
    Installed(Result<PathBuf, String>),
    ReadMarks {
        key: MangaKey,
        read: std::collections::HashSet<String>,
    },
    Alternatives {
        key: MangaKey,
        list: Vec<Alt>,
    },
}

/// Per-source limit when looking for the same work elsewhere.
const ALT_TIMEOUT: Duration = Duration::from_secs(20);

/// Self-update state (Settings › Updates and the banner).
#[derive(Default)]
struct UpdateState {
    check_on_start: bool,
    checking: bool,
    latest: Option<Release>,
    error: Option<String>,
    /// (bytes done, total) while downloading
    progress: Option<Arc<Mutex<(u64, u64)>>>,
    installed: Option<PathBuf>,
    banner_hidden: bool,
}

/// Pages fetched at once per chapter download.
const DOWNLOAD_PARALLEL: usize = 4;

pub struct KumaApp {
    db: Arc<Mutex<Database>>,
    registry: SourceRegistry,
    health: Arc<HealthBook>,
    data_dir: PathBuf,
    worker: Worker<Msg>,
    cover_disk: Arc<DiskCache>,
    page_disk: Arc<DiskCache>,
    page_cache_mb: u64,
    view: View,
    show_adult: bool,
    age_confirmed: bool,
    age_prompt: bool,
    browse: HashMap<View, BrowseState>,
    details: Option<DetailsState>,
    reader: Option<ReaderState>,
    reader_settings: ReaderSettings,
    chapter_lang: String,
    /// What the user wants to read; applied to Home and every browse page.
    category: Category,
    /// When the visible page last changed, for the fade-in, and which page that was.
    page_shown: (f64, (View, bool)),
    /// Reopen this position once the details page has its manga and chapters.
    pending_resume: Option<ResumePoint>,
    library: Library,
    covers: Covers,
    tags: HashMap<String, TagsSlot>,
    logo: Option<egui::TextureHandle>,
    downloads: Arc<Downloads>,
    downloaded: Vec<Entry>,
    /// Chapters queued or downloading (by `job_key`); finished ones are removed.
    dl_status: Arc<Mutex<HashMap<String, DlStatus>>>,
    upd: UpdateState,
    search_history: Vec<String>,
    /// Bookmarks page shelf filter ("" = all).
    fav_shelf: String,
}

fn cache_read<T: DeserializeOwned>(db: &Mutex<Database>, key: &str, ttl: Duration) -> Option<T> {
    let json = db.lock().ok()?.cache_get(key, ttl).ok()??;
    serde_json::from_str(&json).ok()
}

fn cache_write<T: Serialize>(db: &Mutex<Database>, key: &str, source: &str, value: &T) {
    let (Ok(json), Ok(db)) = (serde_json::to_string(value), db.lock()) else {
        return;
    };
    if let Err(e) = db.cache_put(key, source, &json) {
        tracing::warn!(error = %e, "metadata cache write failed");
    }
}

impl KumaApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        db: Database,
        mut registry: SourceRegistry,
        data_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        theme::apply(&cc.egui_ctx);
        // Startup reads happen once, before the first frame; everything later goes through `db_job`.
        let setting = |k: &str| db.setting(k).ok().flatten();
        let show_adult = setting(SHOW_ADULT_KEY).as_deref() == Some("true");
        let age_confirmed = setting(AGE_CONFIRMED_KEY).is_some();
        let reader_settings = setting(READER_SETTINGS_KEY)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let chapter_lang = setting(CHAPTER_LANG_KEY).unwrap_or_else(|| "en".into());
        let category = setting(CATEGORY_KEY)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let page_cache_mb = setting(PAGE_CACHE_MB_KEY)
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_PAGE_CACHE_MB);
        let library = load_library(&db);
        for id in db.disabled_sources().unwrap_or_default() {
            registry.set_enabled(&id, false);
        }
        let browse = [
            (View::Discover, SortOrder::Popular),
            (View::Popular, SortOrder::Popular),
            (View::Latest, SortOrder::Latest),
            (View::Search, SortOrder::Relevance),
        ]
        .into_iter()
        .map(|(v, sort)| (v, BrowseState::new(sort)))
        .collect();
        let check_updates = setting(CHECK_UPDATES_KEY).as_deref() != Some("false");
        let search_history: Vec<String> = setting(SEARCH_HISTORY_KEY)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let downloads = Downloads::new(data_dir.join("downloads"));
        let downloaded = downloads.list();
        let page_disk = DiskCache::new(data_dir.join("pages"), page_cache_mb * 1024 * 1024)
            .with_max_age(PAGE_CACHE_MAX_AGE);
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            registry,
            health: Arc::default(),
            cover_disk: Arc::new(DiskCache::new(data_dir.join("covers"), COVER_CACHE_BYTES)),
            page_disk: Arc::new(page_disk),
            page_cache_mb,
            data_dir,
            worker: Worker::new(cc.egui_ctx.clone())?,
            view: View::Discover,
            show_adult,
            age_confirmed,
            age_prompt: false,
            browse,
            details: None,
            reader: None,
            reader_settings,
            chapter_lang,
            category,
            page_shown: (0.0, (View::Discover, false)),
            pending_resume: None,
            library,
            covers: Covers::default(),
            tags: HashMap::new(),
            logo: image_decoder::decode(include_bytes!("../assets/kuma.png"), 512, 512)
                .ok()
                .map(|img| cc.egui_ctx.load_texture("logo", img, egui::TextureOptions::LINEAR)),
            downloads: Arc::new(downloads),
            downloaded,
            dl_status: Arc::default(),
            upd: UpdateState {
                check_on_start: check_updates,
                ..Default::default()
            },
            search_history,
            fav_shelf: String::new(),
        })
        .map(|mut app| {
            if check_updates && !update::dev_build() {
                app.check_update(false);
            }
            app
        })
    }

    fn check_update(&mut self, manual: bool) {
        if self.upd.checking {
            return;
        }
        self.upd.checking = true;
        self.upd.error = None;
        self.worker.spawn(async move {
            let result = update::latest_release().await.map_err(|e| format!("{e:#}"));
            Msg::Update { manual, result }
        });
    }

    fn start_install(&mut self) {
        let Some(r) = self.upd.latest.clone() else { return };
        let progress = Arc::new(Mutex::new((0u64, 0u64)));
        self.upd.progress = Some(progress.clone());
        self.upd.error = None;
        let work = self.data_dir.join("update");
        self.worker.spawn(async move {
            Msg::Installed(update::install(&r, &work, &progress).await.map_err(|e| format!("{e:#}")))
        });
    }

    /// Starts the installed version and closes this one.
    fn restart_into(&mut self, ctx: &egui::Context, exe: &std::path::Path) {
        match std::process::Command::new(exe).spawn() {
            Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => self.upd.error = Some(format!("Could not start the new version: {e}")),
        }
    }

    /// "A new version is out" strip at the top of the content.
    fn update_banner(&mut self, ui: &mut egui::Ui) {
        let Some(r) = self.upd.latest.clone().filter(|r| update::is_newer(r.version)) else {
            return;
        };
        if self.upd.banner_hidden || self.view == View::Settings || update::dev_build() {
            return;
        }
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}  KumaDoujin {} is available", icons::DOWNLOAD_SIMPLE, r.tag))
                        .strong()
                        .color(theme::ACCENT),
                );
                ui.label(RichText::new(format!("(you have {})", update::VERSION)).color(theme::MUTED));
                if ui.button("Update…").clicked() {
                    self.view = View::Settings;
                    self.details = None;
                }
                if ui.button("Later").clicked() {
                    self.upd.banner_hidden = true;
                }
            });
        });
        ui.add_space(10.0);
    }

    /// Settings › Diagnostics: log folder and a copyable report of every source's state.
    fn diagnostics_card(&self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Diagnostics").strong().size(16.0));
            ui.label(
                RichText::new("When something breaks, attach the report (and the log file) to an issue on GitHub.")
                    .small()
                    .color(theme::MUTED),
            );
            ui.horizontal(|ui| {
                if ui.button(theme::label(icons::FOLDER_OPEN, "Open log folder")).clicked() {
                    // explorer.exe returns 1 even on success, so its status is not checked
                    let _ = std::process::Command::new("explorer").arg(&self.data_dir).spawn();
                }
                if ui
                    .button(theme::label(icons::COPY, "Copy diagnostics"))
                    .on_hover_text("App version and the status / last error of every source")
                    .clicked()
                {
                    ui.ctx().copy_text(self.diagnostics());
                }
                ui.hyperlink_to("Report a problem", format!("https://github.com/{}/issues/new", update::REPO));
            });
        });
        ui.add_space(12.0);
    }

    fn diagnostics(&self) -> String {
        let mut out = format!(
            "KumaDoujin {} · {} · helper browser: {}\n\nSources:\n",
            update::VERSION,
            std::env::consts::OS,
            if std::env::current_exe()
                .ok()
                .and_then(|p| Some(p.parent()?.join("kuma-browser.exe")))
                .is_some_and(|p| p.exists())
            {
                "present"
            } else {
                "missing"
            }
        );
        for s in self.registry.all() {
            let h = self.health.get(&s.info.id);
            let enabled = if self.registry.is_enabled(&s.info.id) { "on" } else { "off" };
            out += &format!(
                "- {} [{enabled}] {:?} · {} ok / {} failed · avg {} ms",
                s.info.name,
                h.status(),
                h.ok,
                h.failed,
                h.avg_latency().map_or(0, |d| d.as_millis())
            );
            if let Some(e) = &h.last_error {
                out += &format!("\n    last error: {e}");
            }
            out += "\n";
        }
        out
    }

    /// Settings › Updates.
    fn updates_card(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Updates").strong().size(16.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("KumaDoujin {}", update::VERSION)).strong());
                ui.label(RichText::new(format!("· from github.com/{}", update::REPO)).color(theme::MUTED));
            });
            if ui
                .checkbox(&mut self.upd.check_on_start, "Look for a new version when the app starts")
                .changed()
            {
                self.save_setting(CHECK_UPDATES_KEY, self.upd.check_on_start.to_string());
            }
            ui.horizontal(|ui| {
                if self.upd.checking {
                    ui.spinner();
                    ui.label(RichText::new("Checking…").color(theme::MUTED));
                } else if ui.button(theme::label(icons::ARROW_CLOCKWISE, "Check now")).clicked() {
                    self.check_update(true);
                }
                match &self.upd.latest {
                    Some(r) if update::is_newer(r.version) => {
                        ui.label(RichText::new(format!("{} is available", r.tag)).strong().color(theme::ACCENT));
                    }
                    Some(r) => {
                        ui.label(RichText::new(format!("Up to date (newest: {})", r.tag)).color(theme::MUTED));
                    }
                    None => {}
                }
            });
            if let Some(e) = &self.upd.error {
                ui.colored_label(egui::Color32::from_rgb(0xE5, 0x6B, 0x6F), e);
            }
            let Some(r) = self.upd.latest.clone().filter(|r| update::is_newer(r.version)) else {
                return;
            };
            if !r.notes.is_empty() {
                egui::CollapsingHeader::new("What's new").id_salt("relnotes").show(ui, |ui| {
                    ui.label(RichText::new(&r.notes).small());
                });
            }
            if let Some(p) = &self.upd.progress {
                let (done, total) = p.lock().map(|g| *g).unwrap_or((0, 0));
                let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                let mb = |b: u64| b as f64 / 1_048_576.0;
                ui.add(egui::ProgressBar::new(frac).text(format!("{:.1} / {:.1} MB", mb(done), mb(total))));
                ui.ctx().request_repaint_after(Duration::from_millis(200));
            } else if let Some(exe) = self.upd.installed.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Installed — restart to use it").color(theme::ACCENT));
                    if ui.button(theme::label(icons::ARROW_CLOCKWISE, "Restart now")).clicked() {
                        self.restart_into(&ui.ctx().clone(), &exe);
                    }
                });
            } else if update::dev_build() {
                ui.label(
                    RichText::new("This copy runs from the source folder (cargo build): update it with git, or download the release:")
                        .color(theme::MUTED),
                );
                ui.hyperlink(&r.page);
            } else if r.asset_url.is_some() {
                if ui
                    .button(theme::label(icons::DOWNLOAD_SIMPLE, &format!("Download and install {}", r.tag)))
                    .clicked()
                {
                    self.start_install();
                }
            } else {
                ui.label(RichText::new("This release has no Windows zip — see its page:").color(theme::MUTED));
                ui.hyperlink(&r.page);
            }
        });
        ui.add_space(12.0);
    }

    /// Runs `f` against the database on a blocking worker thread, never the GUI thread.
    /// With `refresh`, the library lists are reloaded afterwards.
    fn db_job(
        &self,
        refresh: bool,
        f: impl FnOnce(&Database) -> rusqlite::Result<()> + Send + 'static,
    ) {
        let db = self.db.clone();
        self.worker.spawn(async move {
            tokio::task::spawn_blocking(move || {
                let Ok(db) = db.lock() else { return Msg::Noop };
                if let Err(e) = f(&db) {
                    tracing::error!(error = %e, "database write failed");
                }
                if refresh {
                    Msg::Library(Box::new(load_library(&db)))
                } else {
                    Msg::Noop
                }
            })
            .await
            .unwrap_or(Msg::Noop)
        });
    }

    fn save_setting(&self, key: &'static str, value: String) {
        self.db_job(false, move |db| db.set_setting(key, &value));
    }

    fn handle(&mut self, ctx: &egui::Context, msg: Msg) {
        match msg {
            Msg::Noop => {}
            Msg::Library(lib) => self.library = *lib,
            Msg::Listing {
                view,
                generation,
                result,
                done,
            } => {
                if let Some(st) = self.browse.get_mut(&view)
                    && st.generation == generation
                {
                    if done {
                        st.loading = false;
                        st.pending = None;
                    }
                    st.result = Some(result);
                }
            }
            Msg::Details { key, result } => {
                if let Some(d) = &mut self.details
                    && d.key == key
                {
                    if let Ok(m) = &result {
                        let f = Favorite {
                            key: key.clone(),
                            title: m.titles.display().to_string(),
                            cover_url: m.cover_url.clone(),
                            added_at: 0,
                            shelf: String::new(),
                        };
                        self.db_job(true, move |db| db.touch_viewed(&f));
                    }
                    if let Ok(m) = &result {
                        self.find_alternatives(m);
                    }
                    if let Some(d) = &mut self.details {
                        d.result = Some(result.map(|m| *m));
                    }
                    self.try_resume();
                }
            }
            Msg::Alternatives { key, list } => {
                if let Some(d) = &mut self.details
                    && d.key == key
                {
                    d.alternatives = Some(list);
                }
            }
            Msg::ReadMarks { key, read } => {
                if let Some(d) = &mut self.details
                    && d.key == key
                {
                    d.read = read;
                }
            }
            Msg::Chapters { key, lang, result } => {
                if let Some(d) = &mut self.details
                    && d.key == key
                    && d.chapter_lang == lang
                {
                    d.chapters = Some(result);
                    self.try_resume();
                }
            }
            Msg::Cover { url, result } => self.covers.finish(ctx, url, result),
            Msg::Tags { source, result } => {
                let slot = match result {
                    Ok(t) => TagsSlot::Ready(t),
                    Err(e) => TagsSlot::Failed(e),
                };
                self.tags.insert(source, slot);
            }
            Msg::Pages { generation, result } => {
                if let Some(r) = &mut self.reader
                    && r.generation == generation
                {
                    r.pages = Some(result);
                    r.jump(r.current); // clamps "last page" requests
                }
            }
            Msg::Page {
                generation,
                index,
                result,
            } => {
                if let Some(r) = &mut self.reader
                    && r.generation == generation
                {
                    r.set_page(ctx, index, result);
                }
            }
            Msg::Downloads(list) => self.downloaded = list,
            Msg::Update { manual, result } => {
                self.upd.checking = false;
                match result {
                    Ok(r) => self.upd.latest = Some(r),
                    // a failed check at start-up stays quiet; only "Check now" reports it
                    Err(e) if manual => self.upd.error = Some(e),
                    Err(e) => tracing::info!(error = %e, "update check failed"),
                }
            }
            Msg::Installed(result) => {
                self.upd.progress = None;
                match result {
                    Ok(exe) => self.upd.installed = Some(exe),
                    Err(e) => self.upd.error = Some(e),
                }
            }
        }
    }

    /// Catalogs to query: one picked source, or every enabled `in_discover` source.
    fn catalogs_for(&self, source: Option<&str>) -> Vec<CatalogRef> {
        self.registry
            .catalogs()
            .filter(|(info, _)| source.map_or(info.in_discover, |s| s == info.id))
            // 18+ sources only appear under the Doujin category, and only in 18+ mode
            .filter(|(info, _)| !info.adult || (self.show_adult && self.category == Category::Doujin))
            .map(|(info, c)| CatalogRef {
                id: info.id.clone(),
                name: info.name.clone(),
                provider: c.clone(),
            })
            .collect()
    }

    fn load_listing(&mut self, view: View) {
        let catalogs = {
            let st = &self.browse[&view];
            self.catalogs_for(st.source.as_deref())
        };
        // adult-rated works stay inside the Doujin category so the others are always general
        let (show_adult, category) = (self.show_adult && self.category == Category::Doujin, self.category);
        let Some(st) = self.browse.get_mut(&view) else {
            return;
        };
        if let Some(h) = st.pending.take() {
            h.abort(); // cancel the superseded request
        }
        st.generation += 1;
        st.loading = true;
        st.needs_load = false;
        st.result = None; // results of the previous query must not show as partial ones
        let generation = st.generation;
        let mut query = st.query.clone();
        let text = query.text.trim().to_string();
        if view == View::Search && !text.is_empty() {
            self.search_history.retain(|h| !h.eq_ignore_ascii_case(&text));
            self.search_history.insert(0, text);
            self.search_history.truncate(SEARCH_HISTORY_MAX);
            if let Ok(json) = serde_json::to_string(&self.search_history) {
                self.save_setting(SEARCH_HISTORY_KEY, json);
            }
        }
        let Some(st) = self.browse.get_mut(&view) else {
            return;
        };
        query.include_adult = show_adult;
        query.category = category;
        let key = format!(
            "search2:{}:{}",
            st.source.as_deref().unwrap_or("*"),
            serde_json::to_string(&query).unwrap_or_default()
        );
        let db = self.db.clone();
        let health = self.health.clone();
        let notify = self.worker.notifier();
        st.pending = Some(self.worker.spawn(async move {
            let result = match cache_read::<Aggregated>(&db, &key, LISTING_TTL) {
                Some(hit) => hit,
                None => {
                    let partial = |result: &Aggregated| {
                        notify(Msg::Listing {
                            view,
                            generation,
                            result: result.clone(),
                            done: false,
                        })
                    };
                    let fresh = search_all(catalogs, query, health, partial).await;
                    if fresh.errors.is_empty() {
                        cache_write(&db, &key, "*", &fresh);
                    }
                    fresh
                }
            };
            Msg::Listing {
                view,
                generation,
                result,
                done: true,
            }
        }));
    }

    /// Opens the details page; `lang` overrides the chapter-list language (used by resume).
    fn open_details(&mut self, key: MangaKey, lang: Option<String>) {
        self.details = Some(DetailsState {
            key: key.clone(),
            result: None,
            chapters: None,
            chapter_lang: lang.unwrap_or_else(|| self.chapter_lang.clone()),
            read: Default::default(),
            alternatives: None,
        });
        self.load_read_marks(key.clone());
        let Some((can_read, catalog)) = self
            .registry
            .get(&key.source)
            .map(|s| (s.can_read(), s.catalog.clone()))
        else {
            return;
        };
        if can_read {
            self.load_chapters();
        }
        let Some(catalog) = catalog else {
            return;
        };
        let db = self.db.clone();
        let health = self.health.clone();
        // Enabled metadata sources, for filling gaps via the work's cross-source links.
        let enrichers: Vec<(String, Arc<dyn crate::sources::traits::CatalogProvider>)> = self
            .registry
            .catalogs()
            .filter(|(info, _)| info.id != key.source)
            .map(|(info, c)| (info.id.clone(), c.clone()))
            .collect();
        let cache_key = format!("manga2:{}:{}", key.source, key.id);
        self.worker.spawn(async move {
            if let Some(m) = cache_read::<Manga>(&db, &cache_key, DETAILS_TTL) {
                return Msg::Details {
                    key,
                    result: Ok(Box::new(m)),
                };
            }
            let started = Instant::now();
            let res = catalog.details(&key.id).await;
            health.record(&key.source, started, &res);
            let result = match res {
                Ok(mut m) => {
                    merge::record_primary(&mut m);
                    for (src, id) in m.links.clone() {
                        let Some((_, provider)) = enrichers.iter().find(|(e, _)| *e == src) else {
                            continue;
                        };
                        let started = Instant::now();
                        let extra = provider.details(&id).await;
                        health.record(&src, started, &extra);
                        match extra {
                            Ok(extra) => merge::merge(&mut m, &extra),
                            // Enrichment is best-effort; the primary record still shows.
                            Err(e) => {
                                tracing::warn!(source = %src, error = %e, "enrichment failed")
                            }
                        }
                    }
                    cache_write(&db, &cache_key, &key.source, &m);
                    Ok(Box::new(m))
                }
                Err(e) => Err(e.to_string()),
            };
            Msg::Details { key, result }
        });
    }

    fn load_chapters(&mut self) {
        let Some(d) = &mut self.details else { return };
        let Some(provider) = self
            .registry
            .get(&d.key.source)
            .and_then(|s| s.chapters.clone())
        else {
            return;
        };
        d.chapters = None;
        let (key, lang) = (d.key.clone(), d.chapter_lang.clone());
        let health = self.health.clone();
        let db = self.db.clone();
        let cache_key = format!("chapters:{}:{}:{}", key.source, key.id, lang);
        self.worker.spawn(async move {
            if let Some(list) = cache_read::<Vec<Chapter>>(&db, &cache_key, CHAPTERS_TTL) {
                return Msg::Chapters { key, lang, result: Ok(list) };
            }
            let langs: Vec<String> = (!lang.is_empty())
                .then(|| lang.clone())
                .into_iter()
                .collect();
            let started = Instant::now();
            let res = provider.chapters(&key.id, &langs).await;
            health.record(&key.source, started, &res);
            if let Ok(list) = &res {
                cache_write(&db, &cache_key, &key.source, list);
            }
            let result = res.map_err(|e| e.to_string());
            Msg::Chapters { key, lang, result }
        });
    }

    /// Reopens a saved position: details page first (it owns the chapter list), then the reader.
    fn resume(&mut self, at: ResumePoint) {
        let same_list = self
            .details
            .as_ref()
            .is_some_and(|d| d.key == at.key && d.chapter_lang == at.chapter_lang);
        if !same_list {
            if self.details.as_ref().is_some_and(|d| d.key == at.key) {
                if let Some(d) = &mut self.details {
                    d.chapter_lang = at.chapter_lang.clone();
                }
                self.load_chapters();
            } else {
                self.open_details(at.key.clone(), Some(at.chapter_lang.clone()));
            }
        }
        self.pending_resume = Some(at);
        self.try_resume();
    }

    fn try_resume(&mut self) {
        let Some(at) = &self.pending_resume else {
            return;
        };
        let Some(d) = &self.details else { return };
        if d.key != at.key {
            self.pending_resume = None;
            return;
        }
        let (Some(Ok(_)), Some(chapters)) = (&d.result, &d.chapters) else {
            return; // still loading
        };
        let found = chapters
            .as_ref()
            .ok()
            .and_then(|list| list.iter().position(|c| c.id == at.chapter_id));
        let page = at.page;
        self.pending_resume = None;
        match found {
            Some(i) => self.open_reader(i, page),
            // Chapter gone (removed or re-uploaded): stay on details so the user can pick.
            None => tracing::info!("saved chapter not found; showing details"),
        }
    }

    fn open_reader(&mut self, chapter: usize, page: usize) {
        let Some(d) = &self.details else { return };
        let (Some(Ok(manga)), Some(Ok(chapters))) = (&d.result, &d.chapters) else {
            return;
        };
        let source_name = self
            .registry
            .get(&d.key.source)
            .map_or(d.key.source.clone(), |s| s.info.name.clone());
        self.reader = Some(ReaderState::new(
            d.key.clone(),
            manga.titles.display().to_string(),
            source_name,
            chapters.clone(),
            chapter,
            manga.cover_url.clone(),
            d.chapter_lang.clone(),
        ));
        self.open_chapter(chapter, page);
    }

    /// `start == usize::MAX` opens on the last page (paging backwards into a chapter).
    fn open_chapter(&mut self, index: usize, start: usize) {
        let Some(r) = &mut self.reader else { return };
        let Some(chapter_id) = r.chapters.get(index).map(|c| c.id.clone()) else {
            return;
        };
        r.clear_pages();
        r.chapter = index;
        r.generation += 1;
        r.pages = None;
        r.current = start;
        // a downloaded chapter opens from disk, online or not
        if let Some(local) = self.downloads.local_pages(&r.manga, &chapter_id) {
            r.pages = Some(Ok(local));
            r.jump(start);
            return;
        }
        let Some(provider) = self
            .registry
            .get(&r.manga.source)
            .and_then(|s| s.chapters.clone())
        else {
            r.pages = Some(Err("source unavailable".into()));
            return;
        };
        let generation = r.generation;
        let (health, source) = (self.health.clone(), r.manga.source.clone());
        self.worker.spawn(async move {
            let started = Instant::now();
            let res = provider.pages(&chapter_id).await;
            health.record(&source, started, &res);
            let result = res.map_err(|e| e.to_string());
            Msg::Pages { generation, result }
        });
    }

    fn reader_point(r: &ReaderState) -> Option<(ResumePoint, &Chapter)> {
        let chapter = r.chapters.get(r.chapter)?;
        Some((
            ResumePoint {
                key: r.manga.clone(),
                chapter_id: chapter.id.clone(),
                chapter_lang: r.chapter_lang.clone(),
                page: r.current,
            },
            chapter,
        ))
    }

    /// Writes reading progress whenever the visible page changes.
    fn save_progress(&mut self) {
        let Some(r) = &mut self.reader else { return };
        let total = r.page_count();
        if total == 0 || r.last_saved == Some((r.chapter, r.current)) {
            return;
        }
        r.last_saved = Some((r.chapter, r.current));
        let Some((at, chapter)) = Self::reader_point(r) else {
            return;
        };
        let p = Progress {
            at,
            chapter_label: chapter.label(),
            page_count: Some(total as u32),
            title: r.title.clone(),
            cover_url: r.cover_url.clone(),
            updated_at: 0,
        };
        let finished = (r.current + self.reader_settings.layout.visible() >= total)
            .then(|| (r.manga.clone(), chapter.id.clone()));
        self.db_job(false, move |db| db.save_progress(&p));
        if let Some((key, id)) = finished {
            self.mark_read(key, vec![id], true); // reached the last page
        }
    }

    fn is_bookmarked(&self, r: &ReaderState) -> bool {
        let Some((at, _)) = Self::reader_point(r) else {
            return false;
        };
        self.library.bookmarks.iter().any(|b| b.at == at)
    }

    /// Starts loads the engine asks for and releases pages outside its keep window.
    fn sync_pages(&mut self, max_side: u32) {
        let Some(r) = &mut self.reader else { return };
        let Some(Ok(pages)) = &r.pages else { return };
        let source = self.registry.get(&r.manga.source);
        let images = source.and_then(|s| s.images.clone());
        let source_id = r.manga.source.clone();
        let (load, keep) = engine::plan(r.current, &self.reader_settings, pages.len());
        r.slots.retain(|i, slot| {
            let stay = keep.contains(i);
            if !stay && let PageSlot::Loading(h) = slot {
                h.abort();
            }
            stay
        });
        // Current page first, then forward, then the one behind.
        let mut order: Vec<usize> = (r.current.min(load.end)..load.end).collect();
        order.extend(load.start..r.current.min(load.end));
        for index in order {
            if r.slots.contains_key(&index) {
                continue;
            }
            let page = pages[index].clone();
            let local = page.token.strip_prefix(FILE).map(PathBuf::from);
            if local.is_none() && images.is_none() {
                continue; // online page but the source is gone
            }
            let images = images.clone();
            let disk = self.page_disk.clone();
            let generation = r.generation;
            let cache_key = format!("{}:{}:{}", source_id, page.chapter_id, page.token);
            let (health, source_id) = (self.health.clone(), source_id.clone());
            let handle = self.worker.spawn(async move {
                let bytes = match (local, images, disk.get(&cache_key)) {
                    (Some(path), _, _) => tokio::task::spawn_blocking(move || std::fs::read(path))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| format!("downloaded page missing: {e}"))),
                    (None, _, Some(b)) => Ok(b),
                    (None, Some(images), None) => {
                        let started = Instant::now();
                        let res = images.page_image(&page).await;
                        health.record(&source_id, started, &res);
                        res.map_err(|e| e.to_string()).inspect(|b| {
                            if images.may_cache_pages()
                                && let Err(e) = disk.put(&cache_key, b)
                            {
                                tracing::warn!(error = %e, "page cache write failed");
                            }
                        })
                    }
                    (None, None, None) => Err("source unavailable".into()),
                };
                let result = match bytes {
                    Ok(b) => tokio::task::spawn_blocking(move || {
                        image_decoder::decode(&b, MAX_PAGE_DIM, max_side)
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string())),
                    Err(e) => Err(e),
                };
                Msg::Page {
                    generation,
                    index,
                    result,
                }
            });
            r.slots.insert(index, PageSlot::Loading(handle));
        }
    }

    /// Queues chapters (indices into the open details page's list) for offline reading.
    fn download(&mut self, indices: Vec<usize>) {
        let Some(d) = &self.details else { return };
        let (Some(Ok(manga)), Some(Ok(chapters))) = (&d.result, &d.chapters) else {
            return;
        };
        let Some(src) = self.registry.get(&d.key.source) else { return };
        let (Some(provider), Some(images), catalog) =
            (src.chapters.clone(), src.images.clone(), src.catalog.clone())
        else {
            return;
        };
        let have = self.downloaded.iter().find(|e| e.manga.key == d.key);
        let mut status = self.dl_status.lock().expect("download status lock");
        let todo: Vec<Chapter> = indices
            .into_iter()
            .filter_map(|i| chapters.get(i))
            .filter(|c| !have.is_some_and(|e| e.has(&c.id)))
            .filter(|c| {
                !matches!(
                    status.get(&job_key(&d.key, &c.id)),
                    Some(DlStatus::Queued | DlStatus::Running { .. })
                )
            })
            .cloned()
            .collect();
        for c in &todo {
            status.insert(job_key(&d.key, &c.id), DlStatus::Queued);
        }
        drop(status);
        if todo.is_empty() {
            return;
        }
        let (manga, all) = (manga.clone(), chapters.clone());
        let (store, status) = (self.downloads.clone(), self.dl_status.clone());
        self.worker.spawn(async move {
            let cover_file = store.cover_path(&manga.key);
            if let (Some(url), Some(catalog)) = (&manga.cover_url, catalog)
                && !cover_file.exists()
                && let Ok(bytes) = catalog.cover(url).await
            {
                if let Some(dir) = cover_file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&cover_file, bytes);
            }
            for c in todo {
                let key = job_key(&manga.key, &c.id);
                let set = |s: DlStatus| {
                    if let Ok(mut m) = status.lock() {
                        m.insert(key.clone(), s);
                    }
                };
                set(DlStatus::Running { done: 0, total: 0 });
                let res = download_chapter(&store, &manga.key, &c, &*provider, &images, &set).await;
                match res.and_then(|n| {
                    store.record(&manga, &all, &c, n).map_err(|e| e.to_string())
                }) {
                    Ok(()) => {
                        if let Ok(mut m) = status.lock() {
                            m.remove(&key);
                        }
                    }
                    Err(e) => set(DlStatus::Failed(e)),
                }
            }
            let list = tokio::task::spawn_blocking(move || store.list()).await.unwrap_or_default();
            Msg::Downloads(list)
        });
    }

    /// Shows a downloaded work from disk, without asking the source.
    fn open_offline(&mut self, entry: &Entry) {
        self.details = Some(DetailsState {
            key: entry.manga.key.clone(),
            result: Some(Ok(entry.manga.clone())),
            chapters: Some(Ok(entry.chapters.iter().map(|c| c.chapter.clone()).collect())),
            chapter_lang: String::new(),
            read: Default::default(),
            alternatives: Some(vec![]), // offline: no searching
        });
        self.load_read_marks(entry.manga.key.clone());
    }

    /// Searches the other enabled reading sources for this work (best title match on each).
    fn find_alternatives(&mut self, m: &Manga) {
        let key = m.key.clone();
        let title = m.titles.display().to_string();
        let others: Vec<(String, Arc<dyn crate::sources::traits::CatalogProvider>, Arc<dyn crate::sources::traits::ChapterProvider>)> = self
            .registry
            .all()
            .iter()
            .filter(|s| s.info.id != key.source && s.can_read() && self.registry.is_enabled(&s.info.id))
            .filter(|s| self.show_adult || !s.info.adult)
            .filter_map(|s| Some((s.info.name.clone(), s.catalog.clone()?, s.chapters.clone()?)))
            .collect();
        let show_adult = self.show_adult;
        self.worker.spawn(async move {
            let words = crate::discovery::aggregator::words(&title);
            let mut set = tokio::task::JoinSet::new();
            for (source, catalog, chapters) in others {
                let (title, words) = (title.clone(), words.clone());
                set.spawn(async move {
                    let started = Instant::now();
                    let q = crate::core::models::SearchQuery {
                        text: title,
                        page: 1,
                        include_adult: show_adult,
                        ..Default::default()
                    };
                    let found = tokio::time::timeout(ALT_TIMEOUT, catalog.search(&q)).await.ok()?.ok()?;
                    // only a confident match: every word of the title present
                    let best = found
                        .items
                        .into_iter()
                        .map(|m| (crate::discovery::aggregator::relevance(&words, &m), m))
                        .filter(|(score, _)| *score >= 200)
                        .max_by_key(|(score, _)| *score)?
                        .1;
                    let list = tokio::time::timeout(ALT_TIMEOUT, chapters.chapters(&best.key.id, &[]))
                        .await
                        .ok()
                        .and_then(Result::ok);
                    Some(Alt {
                        title: best.titles.display().to_string(),
                        key: best.key,
                        source,
                        chapters: list.as_ref().map(Vec::len),
                        latest: list.as_ref().and_then(|l| l.last()).map(Chapter::label),
                        ms: started.elapsed().as_millis() as u64,
                    })
                });
            }
            let mut list = Vec::new();
            while let Some(r) = set.join_next().await {
                if let Ok(Some(a)) = r {
                    list.push(a);
                }
            }
            list.sort_by_key(|a| (std::cmp::Reverse(a.chapters.unwrap_or(0)), a.ms));
            Msg::Alternatives { key, list }
        });
    }

    fn load_read_marks(&self, key: MangaKey) {
        let db = self.db.clone();
        self.worker.spawn(async move {
            let read = tokio::task::spawn_blocking({
                let key = key.clone();
                move || db.lock().ok().and_then(|db| db.read_chapters(&key).ok()).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            Msg::ReadMarks { key, read }
        });
    }

    /// Marks chapters read/unread on the open details page and in the database.
    fn mark_read(&mut self, key: MangaKey, ids: Vec<String>, read: bool) {
        if let Some(d) = &mut self.details
            && d.key == key
        {
            for id in &ids {
                if read {
                    d.read.insert(id.clone());
                } else {
                    d.read.remove(id);
                }
            }
        }
        self.db_job(false, move |db| db.set_chapters_read(&key, &ids, read));
    }

    fn delete_download(&mut self, key: MangaKey) {
        let store = self.downloads.clone();
        self.worker.spawn(async move {
            let list = tokio::task::spawn_blocking(move || {
                if let Err(e) = store.remove(&key) {
                    tracing::error!(error = %e, "deleting download failed");
                }
                store.list()
            })
            .await
            .unwrap_or_default();
            Msg::Downloads(list)
        });
    }

    fn load_cover(&self, source: String, url: String) {
        if let Some(path) = url.strip_prefix(FILE).map(PathBuf::from) {
            self.worker.spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    std::fs::read(path)
                        .map_err(|e| e.to_string())
                        .and_then(|b| image_decoder::decode(&b, MAX_COVER_DIM, MAX_COVER_DIM))
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
                Msg::Cover { url, result }
            });
            return;
        }
        let Some(catalog) = self.registry.get(&source).and_then(|s| s.catalog.clone()) else {
            return;
        };
        let disk = self.cover_disk.clone();
        let health = self.health.clone();
        self.worker.spawn(async move {
            // shortcut: small sync file I/O + cover decode on the runtime; pages use spawn_blocking.
            let bytes = match disk.get(&url) {
                Some(b) => Ok(b),
                None => {
                    let started = Instant::now();
                    let res = catalog.cover(&url).await;
                    health.record(&source, started, &res);
                    res
                }
                .map_err(|e| e.to_string())
                .inspect(|b| {
                    if let Err(e) = disk.put(&url, b) {
                        tracing::warn!(error = %e, "cover cache write failed");
                    }
                }),
            };
            let result =
                bytes.and_then(|b| image_decoder::decode(&b, MAX_COVER_DIM, MAX_COVER_DIM));
            Msg::Cover { url, result }
        });
    }

    fn load_tags(&mut self, source: String) {
        let Some(catalog) = self.registry.get(&source).and_then(|s| s.catalog.clone()) else {
            return;
        };
        self.tags.insert(source.clone(), TagsSlot::Loading);
        self.worker.spawn(async move {
            let result = catalog.tags().await.map_err(|e| e.to_string());
            Msg::Tags { source, result }
        });
    }

    fn request_adult(&mut self, on: bool) {
        if on && !self.age_confirmed {
            self.age_prompt = true;
        } else {
            self.set_adult(on);
        }
    }

    fn set_adult(&mut self, on: bool) {
        self.show_adult = on;
        self.save_setting(SHOW_ADULT_KEY, on.to_string());
        if !on && self.category == Category::Doujin {
            self.set_category(Category::All);
        }
        self.browse.values_mut().for_each(|b| {
            b.needs_load = true;
            b.source = None; // a picked 18+ source may now be hidden
        });
    }

    fn age_modal(&mut self, ctx: &egui::Context) {
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::unique("age-confirm")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.heading("Adult content (18+)");
            ui.add_space(6.0);
            ui.label(
                "This shows sexually explicit works. Confirm that you are at least 18 \
                 and that viewing such content is legal where you live.",
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("I am 18 or older").strong())
                    .clicked()
                {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(yes) = answer {
            self.age_prompt = false;
            if yes {
                self.age_confirmed = true;
                self.save_setting(AGE_CONFIRMED_KEY, "true".into());
                self.set_adult(true);
            }
        }
    }

    fn clear_caches(&mut self) {
        self.db_job(false, |db| db.cache_clear());
        for disk in [self.cover_disk.clone(), self.page_disk.clone()] {
            self.worker.spawn(async move {
                if let Err(e) = tokio::task::spawn_blocking(move || disk.clear())
                    .await
                    .unwrap_or(Ok(()))
                {
                    tracing::error!(error = %e, "clearing image cache failed");
                }
                Msg::Noop
            });
        }
        self.covers.clear();
    }

    fn library_action(&mut self, a: LibraryAction) {
        if let Some(at) = a.resume {
            self.resume(at);
        } else if let Some(key) = a.open {
            self.open_details(key, None);
        }
        if let Some(key) = a.remove_progress {
            self.db_job(true, move |db| db.delete_progress(&key));
        }
        if a.clear_history {
            self.db_job(true, |db| db.clear_progress());
        }
        if let Some(id) = a.delete_bookmark {
            self.db_job(true, move |db| db.delete_bookmark(id));
        }
        if a.clear_viewed {
            self.db_job(true, |db| db.clear_recently_viewed());
        }
    }

    fn set_category(&mut self, c: Category) {
        if c == Category::Doujin && !self.show_adult {
            self.request_adult(true); // the age check runs first; the category applies either way
        }
        self.category = c;
        if let Ok(json) = serde_json::to_string(&c) {
            self.save_setting(CATEGORY_KEY, json);
        }
        self.browse.values_mut().for_each(|b| {
            b.needs_load = true;
            b.query.page = 1;
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match &self.logo {
                Some(t) => {
                    ui.add(egui::Image::new((t.id(), egui::vec2(34.0, 34.0))).corner_radius(8));
                }
                None => {
                    ui.label(RichText::new(icons::BOOK_BOOKMARK).size(26.0).color(theme::ACCENT));
                }
            }
            ui.label(RichText::new("KumaDoujin").size(20.0).strong());
        });
        ui.add_space(18.0);
        for (i, view) in View::NAV.into_iter().enumerate() {
            if i == 5 || i == 8 {
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
            }
            let selected = self.view == view && self.details.is_none();
            if theme::nav_item(ui, view.icon(), view.label(), selected).clicked() {
                self.view = view;
                self.details = None;
                self.pending_resume = None;
            }
        }
    }

    fn details_page(&mut self, ui: &mut egui::Ui) {
        let Some(d) = &mut self.details else { return };
        let favorite = self
            .library
            .favorites
            .iter()
            .find(|f| f.key == d.key)
            .map(|f| f.shelf.clone());
        let progress = self.library.recent.iter().find(|p| p.at.key == d.key);
        let prefix = job_key(&d.key, "");
        let dl = DlInfo {
            saved: self.downloaded.iter().find(|e| e.manga.key == d.key),
            status: self
                .dl_status
                .lock()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, s)| Some((k.strip_prefix(&prefix)?.to_string(), s.clone())))
                        .collect()
                })
                .unwrap_or_default(),
        };
        let a = browse::details(ui, d, &self.registry, &mut self.covers, favorite.as_deref(), progress, &dl);
        let fav = match (&d.result, a.favorite) {
            (Some(Ok(m)), Some(shelf)) => Some((
                Favorite {
                    key: d.key.clone(),
                    title: m.titles.display().to_string(),
                    cover_url: m.cover_url.clone(),
                    added_at: 0,
                    shelf: shelf.clone().unwrap_or_default(),
                },
                shelf.is_some(),
            )),
            _ => None,
        };
        let lang = d.chapter_lang.clone();
        let key = d.key.clone();
        if let Some((fav, on)) = fav {
            self.db_job(true, move |db| db.set_favorite(&fav, on));
        }
        if let Some((ids, read)) = a.mark_read {
            self.mark_read(key, ids, read);
        }
        if let Some(other) = a.open_other {
            self.open_details(other, None);
            return;
        }
        if a.reload_chapters {
            self.chapter_lang = lang;
            self.save_setting(CHAPTER_LANG_KEY, self.chapter_lang.clone());
            self.load_chapters();
        }
        if let Some(at) = a.resume {
            self.resume(at);
        } else if let Some(i) = a.read {
            self.open_reader(i, 0);
        }
        if !a.download.is_empty() {
            self.download(a.download);
        }
        if a.back {
            self.details = None;
            self.pending_resume = None;
        }
    }

    fn content(&mut self, ui: &mut egui::Ui) -> Actions {
        self.update_banner(ui);
        if self.details.is_some() {
            self.details_page(ui);
            return Actions::default();
        }
        let now = now_millis();
        let titles = match self.view {
            View::Discover => ("Discover", "Browse every connected source in one place"),
            View::Popular => ("Popular", "Most followed across sources"),
            View::Latest => ("Latest", "Recently updated across sources"),
            View::Search => ("Search", "Title, language, tags and source"),
            _ => ("", ""),
        };
        if matches!(
            self.view,
            View::Home | View::Discover | View::Popular | View::Latest | View::Search
        ) {
            ui.horizontal_wrapped(|ui| {
                for c in Category::ALL {
                    if c == Category::Doujin && !self.show_adult {
                        continue;
                    }
                    if theme::pill(ui, &category_label(c), c == self.category).clicked()
                        && c != self.category
                    {
                        self.set_category(c);
                    }
                }
            });
            ui.add_space(12.0);
        }
        let reg = &self.registry;
        match self.view {
            View::Home => {
                let mut actions = Actions::default();
                let mut lib = LibraryAction::default();
                theme::hero(
                    ui,
                    "Welcome back",
                    "Continue where you left off, or find something new",
                );
                library::continue_row(ui, &self.library.recent, &mut self.covers, &mut lib);
                ui.add_space(20.0);
                library::viewed_row(ui, &self.library.viewed, &mut self.covers, now, &mut lib);
                if reg.catalogs().next().is_none() {
                    views::empty_state(ui, "No sources", views::NO_CATALOG);
                } else {
                    for (title, v) in [
                        ("Popular titles", View::Popular),
                        ("Latest updates", View::Latest),
                    ] {
                        browse::row(
                            ui,
                            title,
                            &self.browse[&v],
                            reg,
                            &mut self.covers,
                            &mut actions,
                        );
                    }
                }
                self.library_action(lib);
                actions
            }
            View::Discover | View::Popular | View::Latest | View::Search => {
                let st = self
                    .browse
                    .get_mut(&self.view)
                    .expect("browse state per view");
                browse::browse(
                    ui,
                    st,
                    titles,
                    reg,
                    &self.tags,
                    &mut self.covers,
                    self.show_adult && self.category == Category::Doujin,
                    if self.view == View::Search { &self.search_history } else { &[] },
                )
            }
            View::Bookmarks => {
                let a = library::bookmarks(
                    ui,
                    &self.library.favorites,
                    &self.library.bookmarks,
                    &mut self.covers,
                    now,
                    &mut self.fav_shelf,
                );
                self.library_action(a);
                Actions::default()
            }
            View::History => {
                let a = library::history(ui, &self.library.recent, &mut self.covers, now);
                self.library_action(a);
                Actions::default()
            }
            View::Downloads => {
                let covers: Vec<Option<String>> = self
                    .downloaded
                    .iter()
                    .map(|e| {
                        let file = self.downloads.cover_path(&e.manga.key);
                        if file.exists() {
                            Some(format!("{FILE}{}", file.display()))
                        } else {
                            e.manga.cover_url.clone()
                        }
                    })
                    .collect();
                let progress = self.dl_status.lock().map_or((0, 0, 0), |m| {
                    m.values().fold((0, 0, 0), |(j, d, t), s| match s {
                        DlStatus::Queued => (j + 1, d, t),
                        DlStatus::Running { done, total } => (j + 1, d + done, t + total),
                        DlStatus::Failed(_) => (j, d, t),
                    })
                });
                let a = library::downloads(ui, &self.downloaded, &covers, progress, &mut self.covers);
                if let Some(e) = a.open.and_then(|i| self.downloaded.get(i).cloned()) {
                    self.open_offline(&e);
                }
                if let Some(key) = a.delete.and_then(|i| self.downloaded.get(i)).map(|e| e.manga.key.clone()) {
                    self.delete_download(key);
                }
                Actions::default()
            }
            View::Sources => {
                if let Some((id, on)) = views::sources(ui, reg, &self.health, self.show_adult) {
                    self.registry.set_enabled(&id, on);
                    self.db_job(false, move |db| db.set_source_enabled(&id, on));
                    self.browse.values_mut().for_each(|b| {
                        b.needs_load = true;
                        b.source = None; // the picked source may be gone
                    });
                }
                Actions::default()
            }
            View::Settings => {
                self.updates_card(ui);
                self.diagnostics_card(ui);
                let mut adult = self.show_adult;
                let s = views::settings(
                    ui,
                    &self.data_dir,
                    &mut adult,
                    &mut self.page_cache_mb,
                    &mut self.reader_settings,
                );
                if s.reader_changed
                    && let Ok(json) = serde_json::to_string(&self.reader_settings)
                {
                    self.save_setting(READER_SETTINGS_KEY, json);
                }
                if s.adult_changed {
                    self.request_adult(adult);
                }
                if s.page_cache_changed {
                    self.page_disk
                        .set_max_bytes(self.page_cache_mb * 1024 * 1024);
                    self.save_setting(PAGE_CACHE_MB_KEY, self.page_cache_mb.to_string());
                    let disk = self.page_disk.clone();
                    self.worker.spawn(async move {
                        let _ = tokio::task::spawn_blocking(move || disk.evict()).await;
                        Msg::Noop
                    });
                }
                if s.clear_caches {
                    self.clear_caches();
                }
                Actions::default()
            }
        }
    }

    fn reader_frame(&mut self, ui: &mut egui::Ui) {
        let Some(r) = &self.reader else { return };
        let bookmarked = self.is_bookmarked(r);
        let Some(r) = &mut self.reader else { return };
        let mut settings = self.reader_settings.clone();
        let action = egui::CentralPanel::no_frame()
            .show(ui, |ui| reader::show(ui, r, &mut settings, bookmarked))
            .inner;
        if let Some(i) = action.retry_page {
            r.slots.remove(&i);
        }
        let bookmark = (action.bookmark && !bookmarked)
            .then(|| Self::reader_point(r))
            .flatten()
            .map(|(at, chapter)| Bookmark {
                id: 0,
                at,
                chapter_label: chapter.label(),
                title: r.title.clone(),
                cover_url: r.cover_url.clone(),
                created_at: 0,
            });
        if let Some(b) = bookmark {
            self.db_job(true, move |db| db.add_bookmark(&b).map(|_| ()));
        }
        if action.settings_changed {
            self.reader_settings = settings;
            if let Ok(json) = serde_json::to_string(&self.reader_settings) {
                self.save_setting(READER_SETTINGS_KEY, json);
            }
        }
        if let Some((i, to_end)) = action.open_chapter {
            self.open_chapter(i, if to_end { usize::MAX } else { 0 });
        }
        if action.close {
            if let Some(mut r) = self.reader.take() {
                r.clear_pages();
            }
            // Pick up the progress written while reading.
            self.db_job(true, |_| Ok(()));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }
}

impl eframe::App for KumaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let busy = self
            .dl_status
            .lock()
            .is_ok_and(|m| m.values().any(|s| !matches!(s, DlStatus::Failed(_))));
        if busy {
            ui.ctx().request_repaint_after(Duration::from_millis(300)); // download progress
        }
        let msgs: Vec<Msg> = self.worker.drain().collect();
        for msg in msgs {
            self.handle(ui.ctx(), msg);
        }

        if self.reader.is_some() {
            self.reader_frame(ui);
            let max_side = ui.ctx().input(|i| i.max_texture_side) as u32;
            self.sync_pages(max_side);
            self.save_progress();
            return;
        }

        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(190.0)
            .frame(theme::sidebar_frame())
            .show(ui, |ui| self.sidebar(ui));
        // Fade the content in whenever the page changes.
        let now = ui.input(|i| i.time);
        let page = (self.view, self.details.is_some());
        if self.page_shown.1 != page {
            self.page_shown = (now, page);
        }
        let fade = ((now - self.page_shown.0) / PAGE_FADE).clamp(0.0, 1.0) as f32;
        if fade < 1.0 {
            ui.ctx().request_repaint();
        }
        let actions = egui::CentralPanel::default_margins()
            .show(ui, |ui| {
                ui.set_opacity(fade);
                egui::ScrollArea::vertical()
                    .id_salt((self.view, self.details.is_some()))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        self.content(ui)
                    })
                    .inner
            })
            .inner;
        if self.age_prompt {
            self.age_modal(&ui.ctx().clone());
        }

        // Load only what the visible page needs.
        let mut visible = match self.view {
            View::Home => vec![View::Popular, View::Latest],
            v if self.browse.contains_key(&v) => vec![v],
            _ => vec![],
        };
        if self.details.is_some() || self.registry.catalogs().next().is_none() {
            visible.clear();
        }
        for v in visible {
            if self.browse[&v].needs_load {
                self.load_listing(v);
            }
        }
        if let Some(key) = actions.open {
            self.open_details(key, None);
        }
        if actions.clear_search_history {
            self.search_history.clear();
            self.save_setting(SEARCH_HISTORY_KEY, "[]".into());
        }
        if let Some(src) = actions.load_tags {
            self.load_tags(src);
        }
        if let Some(on) = actions.set_adult {
            self.request_adult(on);
        }
        for (source, url) in std::mem::take(&mut self.covers.wanted) {
            self.load_cover(source, url);
        }
    }
}

fn category_label(c: Category) -> String {
    let icon = match c {
        Category::All => icons::BOOKS,
        Category::Doujin => icons::FIRE,
        Category::Manga => icons::BOOK_OPEN,
        Category::Manhwa => icons::SCROLL,
        Category::Manhua => icons::BOOK_BOOKMARK,
    };
    format!("{icon}  {}", c.label())
}

/// Fetches every page of `chapter` into its folder; pages already on disk are kept (resume).
/// Returns the page count.
async fn download_chapter(
    store: &Downloads,
    key: &MangaKey,
    chapter: &Chapter,
    provider: &dyn crate::sources::traits::ChapterProvider,
    images: &Arc<dyn crate::sources::traits::ImageProvider>,
    set: &impl Fn(DlStatus),
) -> Result<usize, String> {
    let pages = provider.pages(&chapter.id).await.map_err(|e| e.to_string())?;
    let total = pages.len();
    std::fs::create_dir_all(store.chapter_dir(key, &chapter.id)).map_err(|e| e.to_string())?;
    let mut done = 0;
    for batch in pages.chunks(DOWNLOAD_PARALLEL) {
        let mut set_ = tokio::task::JoinSet::new();
        for page in batch {
            let path = store.page_path(key, &chapter.id, page.index as usize);
            if path.exists() {
                continue;
            }
            let (page, images) = (page.clone(), images.clone());
            set_.spawn(async move {
                let bytes = images.page_image(&page).await.map_err(|e| e.to_string())?;
                // write-then-rename: a cancelled download never leaves a truncated page behind
                let tmp = path.with_extension("part");
                std::fs::write(&tmp, bytes)
                    .and_then(|_| std::fs::rename(&tmp, &path))
                    .map_err(|e| e.to_string())
            });
        }
        while let Some(r) = set_.join_next().await {
            r.map_err(|e| e.to_string())??;
        }
        done += batch.len();
        set(DlStatus::Running { done, total });
    }
    Ok(total)
}
