pub mod browse;
pub mod icons;
pub mod library;
pub mod reader;
pub mod theme;
pub mod views;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Home,
    Discover,
    Popular,
    Latest,
    Search,
    Bookmarks,
    History,
    Downloads,
    Sources,
    Settings,
}

impl View {
    pub const NAV: [View; 10] = [
        View::Home,
        View::Discover,
        View::Popular,
        View::Latest,
        View::Search,
        View::Bookmarks,
        View::History,
        View::Downloads,
        View::Sources,
        View::Settings,
    ];

    pub fn icon(self) -> &'static str {
        match self {
            View::Home => icons::HOUSE,
            View::Discover => icons::COMPASS,
            View::Popular => icons::FIRE,
            View::Latest => icons::SPARKLE,
            View::Search => icons::MAGNIFYING_GLASS,
            View::Bookmarks => icons::BOOKMARKS_SIMPLE,
            View::History => icons::CLOCK_COUNTER_CLOCKWISE,
            View::Downloads => icons::DOWNLOAD_SIMPLE,
            View::Sources => icons::PLUGS,
            View::Settings => icons::GEAR_SIX,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            View::Home => "Home",
            View::Discover => "Discover",
            View::Popular => "Popular",
            View::Latest => "Latest",
            View::Search => "Search",
            View::Bookmarks => "Bookmarks",
            View::History => "History",
            View::Downloads => "Downloads",
            View::Sources => "Sources",
            View::Settings => "Settings",
        }
    }
}

/// Spinner centred in the space left in the current panel.
pub fn center_spinner(ui: &mut eframe::egui::Ui) {
    let r = ui.available_rect_before_wrap();
    ui.put(
        eframe::egui::Rect::from_center_size(r.center(), eframe::egui::vec2(32.0, 32.0)),
        eframe::egui::Spinner::new(),
    );
}
