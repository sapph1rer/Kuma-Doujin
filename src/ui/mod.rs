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
    Sources,
    Settings,
}

impl View {
    pub const NAV: [View; 9] = [
        View::Home,
        View::Discover,
        View::Popular,
        View::Latest,
        View::Search,
        View::Bookmarks,
        View::History,
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
            View::Sources => "Sources",
            View::Settings => "Settings",
        }
    }
}
