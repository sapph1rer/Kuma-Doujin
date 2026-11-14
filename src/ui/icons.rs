//! Phosphor icon font (MIT, assets/fonts/Phosphor-LICENSE-MIT.txt), vendored from
//! egui-phosphor 0.14 because that crate targets egui 0.36. Icons are private-use
//! codepoints, so they mix into normal labels: `format!("{} Home", icons::HOUSE)`.

use eframe::egui::{self, FontData, FontFamily};
use eframe::epaint::text::{FontInsert, FontPriority, InsertFontFamily};

pub const HOUSE: &str = "\u{E2C2}";
pub const COMPASS: &str = "\u{E1C8}";
pub const FIRE: &str = "\u{E242}";
pub const SPARKLE: &str = "\u{E6A2}";
pub const MAGNIFYING_GLASS: &str = "\u{E30C}";
pub const BOOKMARK_SIMPLE: &str = "\u{E0EA}";
pub const BOOKMARKS_SIMPLE: &str = "\u{E5F0}";
pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{E1A0}";
pub const PLUGS: &str = "\u{EB56}";
pub const GEAR_SIX: &str = "\u{E272}";
pub const CARET_LEFT: &str = "\u{E138}";
pub const CARET_RIGHT: &str = "\u{E13A}";
pub const CARET_UP: &str = "\u{E13C}";
pub const CARET_DOWN: &str = "\u{E136}";
pub const HEART: &str = "\u{E2A8}";
pub const BOOK_OPEN: &str = "\u{E0E6}";
pub const ARROWS_OUT: &str = "\u{E0A2}";
pub const ARROWS_IN: &str = "\u{E09A}";
pub const MINUS: &str = "\u{E32A}";
pub const PLUS: &str = "\u{E3D4}";
pub const SLIDERS_HORIZONTAL: &str = "\u{E434}";
pub const ARROWS_LEFT_RIGHT: &str = "\u{E0A0}";
pub const STAR: &str = "\u{E46A}";
pub const ARROW_CLOCKWISE: &str = "\u{E036}";
pub const TRASH: &str = "\u{E4A6}";
pub const PLAY: &str = "\u{E3D0}";
pub const X: &str = "\u{E4F6}";
pub const GLOBE: &str = "\u{E288}";
pub const TAG: &str = "\u{E478}";
pub const BOOKS: &str = "\u{E758}";
pub const SCROLL: &str = "\u{EB7A}";
pub const BOOK_BOOKMARK: &str = "\u{E0E4}";
pub const EYE: &str = "\u{E220}";
pub const EYE_SLASH: &str = "\u{E224}";
pub const ROWS: &str = "\u{E5A2}";
pub const COLUMNS: &str = "\u{E546}";
pub const FRAME_CORNERS: &str = "\u{E626}";
pub const TRANSLATE: &str = "\u{E4A2}";
pub const IMAGE_BROKEN: &str = "\u{E7A8}";
pub const CHECK_CIRCLE: &str = "\u{E184}";
pub const WARNING_CIRCLE: &str = "\u{E4E2}";
pub const X_CIRCLE: &str = "\u{E4F8}";
pub const CIRCLE_DASHED: &str = "\u{E602}";
pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";

const FONT: &[u8] = include_bytes!("../../assets/fonts/Phosphor.ttf");

/// Adds the icon font as the last fallback of the proportional family, so it never
/// shadows text glyphs (Phosphor maps a–z to blank ligature parts) and the OS fallback
/// for Thai/CJK keeps working.
pub fn install(ctx: &egui::Context) {
    ctx.add_font(FontInsert::new(
        "phosphor",
        FontData::from_static(FONT),
        vec![InsertFontFamily {
            family: FontFamily::Proportional,
            priority: FontPriority::Lowest,
        }],
    ));
}
