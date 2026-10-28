//! The "Sport" look: an almost black window, a lime accent and rounded cards.
//!
//! Every colour and radius a widget or a screen uses comes from [`Tokens`], never from a literal at the call site, so
//! the look can change (a light theme, say) without touching the screens. [`apply`] hands the tokens to egui's own
//! style; [`Tokens::get`] gives them to the code that paints by hand.

use egui::{Color32, Id, Stroke, Theme, vec2};

/// The colour and radius tokens, with the values of the approved mock-up (spec §6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    /// Window background.
    pub bg: Color32,
    /// The icon rail and the title bar.
    pub chrome: Color32,
    /// Cards.
    pub card: Color32,
    /// Input fields and lists.
    pub field: Color32,
    /// Borders and separators.
    pub border: Color32,
    /// Main text.
    pub text: Color32,
    /// Secondary text.
    pub text_dim: Color32,
    /// The primary button, the active icon and "Connected".
    pub accent: Color32,
    /// Text on the accent button.
    pub accent_text: Color32,
    /// The backing of the active icon.
    pub accent_soft: Color32,
    /// Errors and warnings.
    pub warning: Color32,
    /// The backing of the line of a set being edited and of its buttons, and of a line under the pointer that a click
    /// opens for editing (the approved mock-up `workouts-edit.html`).
    pub edit_row: Color32,
    /// Corner radius of cards, in points.
    pub radius_card: f32,
    /// Corner radius of buttons and fields, in points (the mock-up has 8 to 9).
    pub radius_control: f32,
}

impl Tokens {
    pub const SPORT: Tokens = Tokens {
        bg: Color32::from_rgb(0x0d, 0x0f, 0x0e),
        chrome: Color32::from_rgb(0x12, 0x15, 0x13),
        card: Color32::from_rgb(0x17, 0x1b, 0x18),
        field: Color32::from_rgb(0x0f, 0x12, 0x10),
        border: Color32::from_rgb(0x26, 0x2c, 0x27),
        text: Color32::from_rgb(0xee, 0xf2, 0xec),
        text_dim: Color32::from_rgb(0x8a, 0x95, 0x8c),
        accent: Color32::from_rgb(0xc4, 0xf1, 0x35),
        accent_text: Color32::from_rgb(0x0d, 0x0f, 0x0e),
        accent_soft: Color32::from_rgb(0x2a, 0x34, 0x10),
        warning: Color32::from_rgb(0xff, 0x7a, 0x45),
        edit_row: Color32::from_rgb(0x1d, 0x23, 0x1e),
        radius_card: 12.0,
        radius_control: 8.0,
    };

    /// The tokens [`apply`] last set on `ctx`; the Sport tokens before that.
    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|data| data.get_temp::<Tokens>(tokens_id())).unwrap_or(Tokens::SPORT)
    }
}

/// Sizes and spacing in points. The spec's token table has colours and radii only, so these live next to it.
pub mod metrics {
    /// Width of the icon rail.
    pub const RAIL_WIDTH: f32 = 56.0;
    /// Space between the rail's edge and its buttons.
    pub const RAIL_PADDING: i8 = 8;
    /// The square clickable area of a rail icon.
    pub const RAIL_BUTTON: f32 = 40.0;
    /// The icon inside a rail button.
    pub const RAIL_ICON: f32 = 22.0;
    /// Height of buttons and fields.
    pub const CONTROL_HEIGHT: f32 = 36.0;
    /// Width of a dropdown.
    pub const DROPDOWN_WIDTH: f32 = 220.0;
    /// Diameter of the dot in front of a status line.
    pub const DOT: f32 = 10.0;
    /// Space between neighbouring widgets.
    pub const GAP: f32 = 8.0;
    /// Padding between the window edge and a screen's content.
    pub const SCREEN_MARGIN: i8 = 24;
    /// Padding inside a card.
    pub const CARD_PADDING: i8 = 20;
    /// Padding inside a warning box.
    pub const BOX_PADDING: i8 = 12;
    /// The widest a screen's content grows. A wider window leaves the rest empty rather than stretch the lines.
    pub const CONTENT_MAX_WIDTH: f32 = 760.0;
    /// The width of the card that invites the user to connect Garmin: a small card, not a page-wide one.
    pub const INVITATION_WIDTH: f32 = 460.0;
    /// Width of a text field in a form.
    pub const FIELD_WIDTH: f32 = 420.0;
    /// Space between the border of a search field and its text.
    pub const FIELD_PADDING_X: i8 = 10;
    pub const FIELD_PADDING_Y: i8 = 6;
    /// Diameter of the spinner shown while Garmin is asked.
    pub const SPINNER_SIZE: f32 = 20.0;
    /// Size of the title of a screen.
    pub const PAGE_TITLE_SIZE: f32 = 24.0;
    /// Size of the title of a card, and of the title of the activity list.
    pub const CARD_TITLE_SIZE: f32 = 17.0;
    /// Size of secondary lines in a dense list: the date line of an activity, a chip.
    pub const SMALL_TEXT_SIZE: f32 = 11.5;
    /// The number of new activities on a rail icon: its height and the size of its digits.
    pub const BADGE_HEIGHT: f32 = 15.0;
    pub const BADGE_TEXT_SIZE: f32 = 10.0;
    /// Width of the activity list beside the selected activity (the approved mock-up, variant A).
    pub const LIST_WIDTH: f32 = 270.0;
    /// Space between the edges of the activity list and its header.
    pub const LIST_PADDING: i8 = 12;
    /// Height of one activity in the list: two lines and the space around them.
    pub const LIST_ROW_HEIGHT: f32 = 52.0;
    /// Height of the heading of a week in the list.
    pub const WEEK_HEADING_HEIGHT: f32 = 28.0;
    /// Space between the edge of a list row and its text.
    pub const ROW_PADDING: f32 = 8.0;
    /// Diameter of the dot that marks an activity the user has not opened yet.
    pub const NEW_DOT: f32 = 7.0;
    /// Height of a filter chip, and the space between its border and its text.
    pub const CHIP_HEIGHT: f32 = 24.0;
    pub const CHIP_PADDING: f32 = 10.0;
    /// Space between the numbers in the header of the open activity, and the size of each number.
    pub const STAT_GAP: f32 = 18.0;
    pub const STAT_VALUE_SIZE: f32 = 15.0;
    /// Height of a line of a table of sets.
    pub const SET_ROW_HEIGHT: f32 = 24.0;
    /// The columns of a table of sets, as shares of its width: the number, weight, reps, the plan (only in a card whose
    /// sets were asked for different things), heart rate, time and rest.
    pub const SET_COLUMNS: [f32; 7] = [0.06, 0.2, 0.14, 0.16, 0.12, 0.14, 0.16];
    /// Space between the edge of a table cell and its text.
    pub const CELL_PADDING: f32 = 6.0;
    /// Height of the line of a set being edited: a little taller than the others, for its fields.
    pub const EDIT_ROW_HEIGHT: f32 = 30.0;
    /// A field for a number in a line of a table: its width, the space between its border and its text, and the most
    /// characters it takes (more than any weight or count needs).
    pub const NUMBER_FIELD_WIDTH: f32 = 56.0;
    pub const NUMBER_FIELD_PADDING_X: i8 = 6;
    pub const NUMBER_FIELD_PADDING_Y: i8 = 2;
    pub const NUMBER_FIELD_MAX_CHARS: usize = 10;
    /// Height of a small button (the actions under a line being edited), and the space around its text.
    pub const SMALL_CONTROL_HEIGHT: f32 = 26.0;
    pub const SMALL_BUTTON_PADDING_X: f32 = 10.0;
    pub const SMALL_BUTTON_PADDING_Y: f32 = 4.0;
    /// The exercise picker: its width, how far under the top of the open activity it opens (below the button of the
    /// header), the space inside it, and the most its list of exercises takes before it scrolls.
    pub const POPOVER_WIDTH: f32 = 330.0;
    pub const POPOVER_TOP: f32 = CONTROL_HEIGHT + 2.0 * GAP;
    pub const POPOVER_PADDING: i8 = 10;
    pub const POPOVER_LIST_HEIGHT: f32 = 220.0;
    /// Height of an option in a list to choose from, and the space between its edge and its text.
    pub const OPTION_HEIGHT: f32 = 26.0;
    pub const OPTION_PADDING: f32 = 8.0;
    /// The exercise tiles of Progress: the narrowest a tile gets (the grid has as many columns as fit), the space between
    /// its edge and its content, the size of the numbers it shows, and the height and stroke of the line of its chart.
    pub const TILE_MIN_WIDTH: f32 = 230.0;
    pub const TILE_PADDING: i8 = 12;
    pub const TILE_VALUE_SIZE: f32 = 15.0;
    pub const SPARKLINE_HEIGHT: f32 = 28.0;
    pub const SPARKLINE_STROKE: f32 = 1.8;
    /// The dot of a chart that has one point only.
    pub const SPARKLINE_DOT: f32 = 2.5;
    /// How much of its colour a tile keeps when it is dimmed: an exercise nobody has done for half a year.
    pub const DIMMED_OPACITY: f32 = 0.55;
    /// The status label of a tile: its corner radius and the space between its edge and its text.
    pub const PILL_RADIUS: f32 = 10.0;
    pub const PILL_PADDING_X: i8 = 8;
    pub const PILL_PADDING_Y: i8 = 2;
}

fn tokens_id() -> Id {
    Id::unique("gymgar-theme-tokens")
}

/// Makes `tokens` the look of the whole window: egui's dark and light styles both get it (the app has one look), and
/// the dark theme is chosen so the system's light mode does not override it.
pub fn apply(ctx: &egui::Context, tokens: &Tokens) {
    ctx.data_mut(|data| data.insert_temp(tokens_id(), *tokens));
    ctx.set_theme(Theme::Dark);
    ctx.all_styles_mut(|style| style_with(style, tokens));
}

fn style_with(style: &mut egui::Style, t: &Tokens) {
    style.spacing.item_spacing = vec2(metrics::GAP, metrics::GAP);
    style.spacing.interact_size.y = metrics::CONTROL_HEIGHT;
    style.spacing.button_padding = vec2(14.0, 6.0);

    let visuals = &mut style.visuals;
    *visuals = egui::Visuals::dark();
    visuals.panel_fill = t.bg;
    visuals.window_fill = t.card;
    visuals.window_stroke = Stroke::new(1.0, t.border);
    visuals.window_corner_radius = t.radius_card.into();
    visuals.menu_corner_radius = t.radius_control.into();
    visuals.extreme_bg_color = t.field;
    visuals.faint_bg_color = t.card;
    visuals.code_bg_color = t.field;
    visuals.hyperlink_color = t.accent;
    visuals.warn_fg_color = t.warning;
    visuals.error_fg_color = t.warning;
    visuals.weak_text_color = Some(t.text_dim);
    visuals.selection = egui::style::Selection { bg_fill: t.accent_soft, stroke: Stroke::new(1.0, t.accent) };
    visuals.text_cursor.stroke = Stroke::new(2.0, t.accent);

    let widgets = &mut visuals.widgets;
    let radius = egui::CornerRadius::from(t.radius_control);
    for (state, fill, stroke, text) in [
        (&mut widgets.noninteractive, t.card, t.border, t.text),
        (&mut widgets.inactive, t.field, t.border, t.text),
        (&mut widgets.hovered, t.border, t.text_dim, t.text),
        (&mut widgets.active, t.accent_soft, t.accent, t.accent),
        (&mut widgets.open, t.border, t.text_dim, t.text),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, text);
        state.corner_radius = radius;
    }
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
