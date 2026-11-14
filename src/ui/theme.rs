//! Dark theme, palette and the few animated widgets the views share.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Id, Rect, Response, RichText, Sense, Stroke, TextStyle,
    epaint::Vertex, vec2,
};

pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x5C, 0x8A);
pub const ACCENT_2: Color32 = Color32::from_rgb(0x8B, 0x5C, 0xF6);
pub const MUTED: Color32 = Color32::from_rgb(0x9A, 0x9C, 0xA8);
pub const TEXT: Color32 = Color32::from_rgb(0xE8, 0xE9, 0xEE);
pub const BG: Color32 = Color32::from_rgb(0x0E, 0x0F, 0x13);
pub const PANEL: Color32 = Color32::from_rgb(0x16, 0x17, 0x1D);
pub const SURFACE: Color32 = Color32::from_rgb(0x20, 0x22, 0x2A);
const SURFACE_HI: Color32 = Color32::from_rgb(0x2A, 0x2D, 0x37);

/// Seconds for hover/selection transitions.
pub const ANIM: f32 = 0.16;

pub fn apply(ctx: &egui::Context) {
    super::icons::install(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(0x0A, 0x0B, 0x0E);
    v.faint_bg_color = SURFACE;
    v.hyperlink_color = ACCENT;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.selection.stroke.color = ACCENT;
    v.window_corner_radius = CornerRadius::same(14);
    v.window_stroke = Stroke::new(1.0, SURFACE_HI);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.weak_bg_fill = SURFACE;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.weak_bg_fill = SURFACE_HI;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.linear_multiply(0.5));
    v.widgets.active.weak_bg_fill = ACCENT.linear_multiply(0.45);
    v.widgets.open.weak_bg_fill = SURFACE_HI;
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.set_theme(egui::ThemePreference::Dark);

    ctx.global_style_mut(|s| {
        s.spacing.button_padding = vec2(10.0, 6.0);
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.interact_size.y = 30.0;
        s.animation_time = ANIM;
        for (style, size) in [
            (TextStyle::Body, 14.5),
            (TextStyle::Button, 14.0),
            (TextStyle::Heading, 26.0),
            (TextStyle::Small, 11.5),
        ] {
            if let Some(f) = s.text_styles.get_mut(&style) {
                f.size = size;
            }
        }
    });
}

pub fn sidebar_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .inner_margin(egui::Margin::symmetric(12, 16))
}

/// Rounded card used for empty states and sections.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(12)
        .inner_margin(16)
}

/// Horizontal gradient fill (egui has no gradient shapes, so a 4-vertex mesh).
pub fn gradient(ui: &egui::Ui, rect: Rect, left: Color32, right: Color32) {
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rect.left_top(), left),
        (rect.right_top(), right),
        (rect.right_bottom(), right),
        (rect.left_bottom(), left),
    ] {
        mesh.vertices.push(Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color,
        });
    }
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    ui.painter().add(mesh);
}

/// Smoothly animated 0..1 for `on`, keyed by `id`.
pub fn anim(ui: &egui::Ui, id: Id, on: bool) -> f32 {
    ui.ctx().animate_bool_with_time(id, on, ANIM)
}

/// Sidebar entry: icon + label, animated highlight and accent bar when selected.
pub fn nav_item(ui: &mut egui::Ui, icon: &str, label: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let sel = anim(ui, resp.id.with("sel"), selected);
    let hov = anim(ui, resp.id.with("hov"), resp.hovered());
    let p = ui.painter();
    let bg = SURFACE
        .gamma_multiply(hov * 0.8)
        .blend(ACCENT.gamma_multiply(0.18 * sel));
    p.rect_filled(rect, 10, bg);
    if sel > 0.0 {
        let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 9.0), vec2(3.0 * sel, 20.0));
        p.rect_filled(bar, 2, ACCENT);
    }
    let color = MUTED.lerp_to_gamma(TEXT, sel.max(hov));
    let x = rect.left() + 14.0 + 2.0 * hov;
    p.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon,
        FontId::proportional(18.0),
        if selected { ACCENT } else { color },
    );
    p.text(
        egui::pos2(x + 28.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        color,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded pill tab with an animated selected state.
pub fn pill(ui: &mut egui::Ui, text: &str, selected: bool) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(14.0), TEXT);
    let size = galley.size() + vec2(28.0, 14.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let sel = anim(ui, resp.id.with("sel"), selected);
    let hov = anim(ui, resp.id.with("hov"), resp.hovered());
    let bg = SURFACE
        .lerp_to_gamma(SURFACE_HI, hov)
        .lerp_to_gamma(ACCENT, sel);
    ui.painter().rect_filled(rect, 16, bg);
    let color = MUTED.lerp_to_gamma(Color32::WHITE, sel.max(hov));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pulsing placeholder used while covers or lists load.
pub fn skeleton(ui: &egui::Ui, rect: Rect, radius: u8) {
    let t = ui.input(|i| i.time) as f32;
    let pulse = 0.55 + 0.45 * (t * 3.0).sin().abs();
    ui.painter()
        .rect_filled(rect, radius, SURFACE.gamma_multiply(pulse));
    ui.ctx().request_repaint();
}

/// Big gradient banner at the top of browse pages.
pub fn hero(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::hover());
    gradient(
        ui,
        rect,
        ACCENT.gamma_multiply(0.35),
        ACCENT_2.gamma_multiply(0.20),
    );
    // Accent stripe on the left edge; the banner is square on purpose (meshes can't round).
    ui.painter().rect_filled(
        Rect::from_min_size(rect.left_top(), vec2(4.0, rect.height())),
        0,
        ACCENT,
    );
    ui.painter().text(
        rect.left_center() + vec2(24.0, -12.0),
        egui::Align2::LEFT_CENTER,
        title,
        FontId::proportional(28.0),
        Color32::WHITE,
    );
    ui.painter().text(
        rect.left_center() + vec2(24.0, 20.0),
        egui::Align2::LEFT_CENTER,
        subtitle,
        FontId::proportional(14.0),
        TEXT.gamma_multiply(0.8),
    );
    ui.add_space(14.0);
}

/// Icon + text for buttons: `ui.button(label(icons::HEART, "Favorite"))`.
pub fn label(icon: &str, text: &str) -> RichText {
    RichText::new(format!("{icon}  {text}"))
}
