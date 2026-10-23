use egui::Color32;

use super::*;

fn hex(rgb: u32) -> Color32 {
    let [_, red, green, blue] = rgb.to_be_bytes();
    Color32::from_rgb(red, green, blue)
}

#[test]
fn sport_tokens_match_the_spec_table() {
    let t = Tokens::SPORT;
    assert_eq!(t.bg, hex(0x0d0f0e));
    assert_eq!(t.chrome, hex(0x121513));
    assert_eq!(t.card, hex(0x171b18));
    assert_eq!(t.field, hex(0x0f1210));
    assert_eq!(t.border, hex(0x262c27));
    assert_eq!(t.text, hex(0xeef2ec));
    assert_eq!(t.text_dim, hex(0x8a958c));
    assert_eq!(t.accent, hex(0xc4f135));
    assert_eq!(t.accent_text, hex(0x0d0f0e));
    assert_eq!(t.accent_soft, hex(0x2a3410));
    assert_eq!(t.warning, hex(0xff7a45));
    assert_eq!(t.radius_card, 12.0);
    // The table says "8-9 px" for buttons and fields.
    assert!((8.0..=9.0).contains(&t.radius_control), "{}", t.radius_control);
}

#[test]
fn without_apply_the_tokens_are_sport() {
    assert_eq!(Tokens::get(&egui::Context::default()), Tokens::SPORT);
}

#[test]
fn apply_publishes_the_tokens_and_restyles_egui() {
    let ctx = egui::Context::default();
    let custom = Tokens { accent: Color32::RED, bg: Color32::BLUE, ..Tokens::SPORT };
    apply(&ctx, &custom);

    assert_eq!(Tokens::get(&ctx), custom);
    let style = ctx.global_style();
    let visuals = &style.visuals;
    assert!(visuals.dark_mode);
    assert_eq!(visuals.panel_fill, custom.bg);
    assert_eq!(visuals.hyperlink_color, custom.accent);
    assert_eq!(visuals.selection.stroke.color, custom.accent);
    assert_eq!(visuals.window_fill, custom.card);
    assert_eq!(visuals.extreme_bg_color, custom.field);
    assert_eq!(visuals.widgets.noninteractive.bg_stroke.color, custom.border);
    assert_eq!(visuals.widgets.noninteractive.fg_stroke.color, custom.text);
    assert_eq!(visuals.warn_fg_color, custom.warning);
    assert_eq!(visuals.window_corner_radius, egui::CornerRadius::from(custom.radius_card));
}

#[test]
fn the_sport_look_wins_whichever_egui_theme_is_active() {
    let ctx = egui::Context::default();
    apply(&ctx, &Tokens::SPORT);
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        assert_eq!(ctx.style_of(theme).visuals.panel_fill, Tokens::SPORT.bg, "{theme:?}");
    }
    assert_eq!(ctx.theme(), egui::Theme::Dark);
}
