//! The shared widgets. A screen never invents its own button or card: it uses these, and they take every colour and
//! radius from the theme tokens.
//!
//! Two things a widget cannot do for the caller: disable it (wrap it in `ui.add_enabled_ui`) and give a text field its
//! accessible name (`Response::labelled_by` on the visible label).

use egui::{
    Align2, Area, Button, Color32, ComboBox, Context, CursorIcon, Frame, Id, Label, Link, Margin, OpenUrl, Order, Pos2, Response, RichText, Role, Sense, Shape,
    Stroke, StrokeKind, TextEdit, TextStyle, TextWrapMode, Ui, UiKind, Vec2, WidgetInfo, WidgetText, pos2, vec2,
};

use crate::theme::{Tokens, metrics};
use crate::tl;

/// The main action of a screen: the accent colour.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    ui.scope(|ui| {
        let widgets = &mut ui.style_mut().visuals.widgets;
        for (state, fill) in [
            (&mut widgets.inactive, t.accent),
            (&mut widgets.hovered, t.accent.lerp_to_gamma(t.text, 0.25)),
            (&mut widgets.active, t.accent.gamma_multiply(0.8)),
        ] {
            state.weak_bg_fill = fill;
            state.bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.0, t.accent_text);
        }
        ui.add(Button::new(text).min_size(vec2(0.0, ui.spacing().interact_size.y)))
    })
    .inner
}

/// A quieter action next to the main one: a field-coloured button with a border.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(Button::new(text).min_size(vec2(0.0, ui.spacing().interact_size.y)))
}

/// A row of small buttons, the actions on the line of a table above them ([`primary_button`] and [`secondary_button`]
/// inside are small too). It wraps in a narrow window.
pub fn toolbar<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let spacing = ui.spacing_mut();
        spacing.interact_size.y = metrics::SMALL_CONTROL_HEIGHT;
        spacing.button_padding = vec2(metrics::SMALL_BUTTON_PADDING_X, metrics::SMALL_BUTTON_PADDING_Y);
        spacing.item_spacing = vec2(metrics::GAP / 2.0, metrics::GAP / 2.0);
        ui.horizontal_wrapped(add).inner
    })
    .inner
}

/// A quiet action under a card, such as "+ Add set": small dimmed text that brightens under the pointer.
pub fn quiet_button(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let galley = WidgetText::from(RichText::new(text).size(metrics::SMALL_TEXT_SIZE)).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        ui.available_width(),
        TextStyle::Button,
    );
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), text));
    if ui.is_rect_visible(rect) {
        let color = if response.hovered() || response.has_focus() { t.text } else { t.text_dim };
        ui.painter().galley(rect.min, galley, color);
        if response.has_focus() {
            ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, t.accent));
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A rounded panel on the window background, as wide as the space it is given.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    card_with_border(ui, t.border, add)
}

/// A [`card`] whose border is `color`, to make it stand out: the sets without an exercise have a warning-coloured one.
pub fn card_with_border<R>(ui: &mut Ui, color: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, color))
        .corner_radius(t.radius_card)
        .inner_margin(Margin::same(metrics::CARD_PADDING))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A value of a set (activity import spec §2, item 8): `effective` as it is; when the user corrected it, in the accent
/// colour, with what the watch `recorded` struck through after it ([`recorded_value`]).
pub fn set_value(ui: &mut Ui, effective: &str, recorded: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let Some(recorded) = recorded else {
        ui.add(Label::new(RichText::new(effective).color(t.text)).selectable(false));
        return;
    };
    ui.add(Label::new(RichText::new(effective).color(t.accent).strong()).selectable(false));
    recorded_value(ui, recorded);
}

/// What the watch recorded for a value the user corrected: small, dimmed and struck through. Screen readers, which do
/// not see the line through a number, hear it as "was …".
pub fn recorded_value(ui: &mut Ui, recorded: &str) {
    let t = Tokens::get(ui.ctx());
    let was = ui.add(Label::new(RichText::new(recorded).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim).strikethrough()).selectable(false));
    was.widget_info(|| WidgetInfo::labeled(Role::Label, ui.is_enabled(), tl!("was {}").replace("{}", recorded)));
}

/// A short field for a number in the line of a table (the weight or the reps of a set being corrected): the field
/// colour with an accent border, as in the approved mock-up `workouts-edit.html`. It takes at most
/// [`metrics::NUMBER_FIELD_MAX_CHARS`] characters, which no weight or count needs; `name` is what screen readers (and
/// tests) call it, since the column's heading is far above.
pub fn number_field(ui: &mut Ui, buf: &mut String, name: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let frame = Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.accent))
        .corner_radius(t.radius_control)
        .inner_margin(Margin::symmetric(metrics::NUMBER_FIELD_PADDING_X, metrics::NUMBER_FIELD_PADDING_Y));
    let field = TextEdit::singleline(buf).frame(frame).desired_width(metrics::NUMBER_FIELD_WIDTH).char_limit(metrics::NUMBER_FIELD_MAX_CHARS);
    ui.scope(|ui| {
        // As high as its text, to fit the line of a table: not the height of a form's controls.
        ui.spacing_mut().interact_size.y = 0.0;
        ui.add(field)
    })
    .inner
    .accessible_name(name)
}

/// A checkbox on a line of its own, as high as a small button: an option under a list rather than a form's control.
pub fn small_checkbox(ui: &mut Ui, checked: &mut bool, text: &str) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = metrics::SMALL_CONTROL_HEIGHT;
        ui.checkbox(checked, text)
    })
    .inner
}

/// A field to search with: as wide as the space it is given, `hint` in it while it is empty, and named `hint` for screen
/// readers. It takes at most `max_chars` characters.
pub fn search_field(ui: &mut Ui, buf: &mut String, hint: &str, max_chars: usize) -> Response {
    let field = TextEdit::singleline(buf)
        .hint_text(hint)
        .desired_width(f32::INFINITY)
        .char_limit(max_chars)
        .margin(Margin::symmetric(metrics::FIELD_PADDING_X, metrics::FIELD_PADDING_Y));
    ui.add(field).accessible_name(hint)
}

/// A panel over the screen with its top right corner at `corner` (kept inside the window), `width` wide: card-coloured,
/// with an accent border and a shadow, as the exercise picker of `workouts-edit.html`. It shows while the caller draws
/// it each frame; closing it is the caller's to decide. `name` is what screen readers call it.
pub fn popover<R>(ctx: &Context, id: Id, corner: Pos2, width: f32, name: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get(ctx);
    let shadow = ctx.global_style().visuals.popup_shadow;
    Area::new(id)
        .kind(UiKind::Popup)
        .order(Order::Foreground)
        .fixed_pos(corner)
        .pivot(Align2::RIGHT_TOP)
        .constrain(true)
        .movable(false)
        .fade_in(false)
        .accessible_name(name)
        .show(ctx, |ui| {
            Frame::new()
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.accent))
                .corner_radius(t.radius_card)
                .shadow(shadow)
                .inner_margin(Margin::same(metrics::POPOVER_PADDING))
                .show(ui, |ui| {
                    ui.set_width(width);
                    add(ui)
                })
                .inner
        })
        .inner
}

/// The heading of a part of a list, such as "Done before" in the exercise picker: small, dimmed, in capitals. Screen
/// readers hear it as written.
pub fn list_heading(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    let response = ui.add(Label::new(RichText::new(text.to_uppercase()).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim)).selectable(false));
    response.widget_info(|| WidgetInfo::labeled(Role::Heading, ui.is_enabled(), text));
}

/// One option of a list to choose from: `text`, then `detail` dimmed after it, on the accent's backing when it is
/// `selected`. Screen readers hear a toggle named `text`, described by `detail`. Clicking it is the caller's to act on.
pub fn option_row(ui: &mut Ui, text: &str, detail: Option<&str>, selected: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), metrics::OPTION_HEIGHT), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, text));
    if let Some(detail) = detail {
        ui.ctx().accesskit_node_builder(response.id, |node| node.set_description(detail));
    }
    if ui.is_rect_visible(rect) {
        let backing = match (selected, response.hovered()) {
            (true, _) => Some(t.accent_soft),
            (false, true) => Some(t.border),
            (false, false) => None,
        };
        if let Some(fill) = backing {
            ui.painter().rect_filled(rect, t.radius_control, fill);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(rect, t.radius_control, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        }
        let inner = rect.shrink2(vec2(metrics::OPTION_PADDING, 0.0));
        let detail = detail.map(|detail| {
            WidgetText::from(RichText::new(detail).size(metrics::SMALL_TEXT_SIZE)).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Body)
        });
        let detail_width = detail.as_ref().map_or(0.0, |galley| galley.size().x + metrics::GAP);
        // The name gives way to the detail, which is short: a long name is cut.
        let name =
            WidgetText::from(RichText::new(text)).into_galley(ui, Some(TextWrapMode::Truncate), (inner.width() - detail_width).max(0.0), TextStyle::Body);
        let name_width = name.size().x;
        ui.painter().galley(pos2(inner.left(), rect.center().y - name.size().y / 2.0), name, t.text);
        if let Some(galley) = detail {
            let at = pos2(inner.left() + name_width + metrics::GAP, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(at, galley, t.text_dim);
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A one-line field that hides what is typed (the API key). It takes text of any length: there is no character limit,
/// because real keys are longer than the 128 characters some inputs cut at.
///
/// Name it for screen readers and tests with `.labelled_by(label.id)`.
pub fn secret_field(ui: &mut Ui, id: Id, buf: &mut String, width: f32) -> Response {
    ui.add(TextEdit::singleline(buf).id(id).password(true).desired_width(width).margin(Margin::symmetric(10, 8)))
}

/// A list to choose one of `items` from, each a value and its label. Writes the choice to `current` and returns
/// whether the user changed it this frame.
pub fn dropdown<T: PartialEq + Copy>(ui: &mut Ui, id: Id, current: &mut T, items: &[(T, &str)]) -> bool {
    let selected = items.iter().find(|(value, _)| value == current).map_or("", |(_, label)| *label);
    let mut changed = false;
    let response = ComboBox::from_id_salt(id).width(metrics::DROPDOWN_WIDTH).selected_text(selected).show_ui(ui, |ui| {
        for &(value, label) in items {
            changed |= ui.selectable_value(current, value, label).changed();
        }
    });
    // Without a visible label of its own, the list is named by what it shows.
    response.response.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::ComboBox, ui.is_enabled(), selected);
        info.current_text_value = Some(selected.to_owned());
        info
    });
    changed
}

/// A coloured dot and a line of text in the same colour, such as "Connected" in the accent colour.
pub fn status_dot(ui: &mut Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(metrics::DOT, metrics::DOT), Sense::hover());
        ui.painter().circle_filled(rect.center(), metrics::DOT / 2.0, color);
        ui.label(RichText::new(text).color(color));
    });
}

/// An error or a warning, in a box tinted with the warning colour.
pub fn warning_box(ui: &mut Ui, text: &str) {
    warning_box_with(ui, text, |_| {});
}

/// A [`warning_box`] with more under its text, such as a link or a button that deals with the warning.
pub fn warning_box_with<R>(ui: &mut Ui, text: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    Frame::new()
        .fill(t.warning.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, t.warning))
        .corner_radius(t.radius_control)
        .inner_margin(Margin::same(metrics::BOX_PADDING))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(t.warning));
            add(ui)
        })
        .inner
}

/// The title of a screen, with an optional line of explanation under it.
pub fn page_title(ui: &mut Ui, title: &str, subtitle: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let response = ui.add(Label::new(RichText::new(title).size(metrics::PAGE_TITLE_SIZE).color(t.text)));
    // A heading for screen readers, which can jump from one to the next.
    response.widget_info(|| WidgetInfo::labeled(Role::Heading, ui.is_enabled(), title));
    if let Some(subtitle) = subtitle {
        hint(ui, subtitle);
    }
}

/// The title of a part of a screen that has no card around it, such as the activity list: the size of a card's title,
/// and a heading for screen readers.
pub fn heading(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    let response = ui.add(Label::new(RichText::new(text).size(metrics::CARD_TITLE_SIZE).color(t.text)).selectable(false));
    response.widget_info(|| WidgetInfo::labeled(Role::Heading, ui.is_enabled(), text));
}

/// A filter chip: a rounded outline with a short text, in the accent colour when `selected`. Clicking it is the caller's
/// to act on; screen readers hear a toggle that is on or off.
pub fn chip(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let galley =
        WidgetText::from(RichText::new(text).size(metrics::SMALL_TEXT_SIZE)).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Button);
    let size = vec2(galley.size().x + 2.0 * metrics::CHIP_PADDING, metrics::CHIP_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, text));
    if ui.is_rect_visible(rect) {
        let (outline, color) = match (selected, response.hovered()) {
            (true, _) => (t.accent, t.accent),
            (false, true) => (t.text_dim, t.text),
            (false, false) => (t.border, t.text_dim),
        };
        let radius = metrics::CHIP_HEIGHT / 2.0;
        ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, outline), StrokeKind::Inside);
        if response.has_focus() {
            ui.painter().rect_stroke(rect.expand(2.0), radius + 2.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        }
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A clickable card, as wide as the space it is given: an exercise of Progress. `accent` gives it the accent border (the
/// advice is to add weight), and `dim` fades all of it (something nobody has done for a long time). The caller names
/// it for screen readers with `Response::widget_info` and acts on the click.
pub fn tile(ui: &mut Ui, accent: bool, dim: bool, add: impl FnOnce(&mut Ui)) -> Response {
    let t = Tokens::get(ui.ctx());
    ui.scope(|ui| {
        if dim {
            ui.multiply_opacity(metrics::DIMMED_OPACITY);
        }
        let border = if accent { t.accent } else { t.border };
        let frame = Frame::new().fill(t.card).stroke(Stroke::new(1.0, border)).corner_radius(t.radius_card).inner_margin(Margin::same(metrics::TILE_PADDING));
        let mut tile = frame.begin(ui);
        tile.content_ui.set_width(tile.content_ui.available_width());
        add(&mut tile.content_ui);
        // The click is felt over what the tile holds, so the frame is painted once the pointer is known.
        let space = tile.allocate_space(ui);
        let response = ui.interact(space.rect, space.id.with("tile"), Sense::click());
        if response.hovered() {
            tile.frame.fill = t.edit_row;
        }
        tile.paint(ui);
        if response.has_focus() {
            ui.painter().rect_stroke(space.rect.expand(2.0), t.radius_card + 2.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        }
        response.on_hover_cursor(CursorIcon::PointingHand)
    })
    .inner
}

/// How a status label looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Something to act on: the accent colour on its backing.
    Accent,
    /// Nothing to act on: dimmed, in a plain outline.
    Dim,
    /// Something is off: the warning colour.
    Warning,
}

/// A short status in a rounded label, such as "can add 85 kg" on a tile. The accent one has a small up-chevron before its
/// text, drawn with the painter: the bundled fonts have no arrow. It is as wide as its text, and a text longer than the
/// space it is given is cut with an ellipsis.
pub fn advice_label(ui: &mut Ui, text: &str, tone: Tone) {
    let t = Tokens::get(ui.ctx());
    let (fill, outline, color) = match tone {
        Tone::Accent => (t.accent_soft, t.accent_soft, t.accent),
        Tone::Dim => (Color32::TRANSPARENT, t.border, t.text_dim),
        Tone::Warning => (Color32::TRANSPARENT, t.warning.gamma_multiply(0.4), t.warning),
    };
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, outline))
        .corner_radius(metrics::PILL_RADIUS)
        .inner_margin(Margin::symmetric(metrics::PILL_PADDING_X, metrics::PILL_PADDING_Y))
        .show(ui, |ui| {
            let mut label = RichText::new(text).size(metrics::SMALL_TEXT_SIZE).color(color);
            if tone == Tone::Accent {
                label = label.strong();
            }
            // A line of text, not a row of controls: as high as its text, which the chevron is centred on.
            let line = WidgetText::from(label.clone()).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Body).size().y;
            ui.spacing_mut().interact_size.y = line;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = metrics::CHEVRON_GAP;
                if tone == Tone::Accent {
                    up_chevron(ui, color);
                }
                ui.add(Label::new(label).truncate().selectable(false));
            });
        });
}

/// A chevron pointing up: two strokes meeting at its apex, the width and height of the metrics.
fn up_chevron(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(metrics::CHEVRON_WIDTH, metrics::CHEVRON_HEIGHT), Sense::hover());
    if ui.is_rect_visible(rect) {
        // Room for the thickness of the line, so it does not touch the edge of its rectangle.
        let rect = rect.shrink(metrics::CHEVRON_STROKE / 2.0);
        let points = vec![rect.left_bottom(), pos2(rect.center().x, rect.top()), rect.right_bottom()];
        ui.painter().add(Shape::line(points, Stroke::new(metrics::CHEVRON_STROKE, color)));
    }
}

/// A line through `values`, old to new, spread over `size` from its left edge to its right and from the lowest value at
/// its foot to the highest at its top: the small chart of a tile. A flat line runs through the middle, one value is a dot,
/// and what is not a number (NaN, an infinity) is left out, so a series that is nothing draws nothing.
pub fn sparkline(ui: &mut Ui, values: &[f32], color: Color32, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let values: Vec<f32> = values.iter().copied().filter(|value| value.is_finite()).collect();
    // Room for the thickness of the line, so it does not touch the edge.
    let inner = rect.shrink(metrics::SPARKLINE_STROKE);
    let (low, high) = values.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), value| (low.min(*value), high.max(*value)));
    let span = high - low;
    let y_of = |value: f32| if span > 0.0 && span.is_finite() { inner.bottom() - (value - low) / span * inner.height() } else { inner.center().y };
    let last = values.len().saturating_sub(1);
    let x_of = |index: usize| if last == 0 { inner.right() } else { inner.left() + index as f32 / last as f32 * inner.width() };
    let points: Vec<Pos2> = values.iter().enumerate().map(|(index, value)| pos2(x_of(index), y_of(*value))).collect();
    match points.as_slice() {
        [] => {}
        [only] => {
            ui.painter().circle_filled(*only, metrics::SPARKLINE_DOT, color);
        }
        _ => {
            ui.painter().add(Shape::line(points, Stroke::new(metrics::SPARKLINE_STROKE, color)));
        }
    }
}

/// The title of a card.
pub fn card_title(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(text).size(metrics::CARD_TITLE_SIZE).color(t.text));
}

/// A line of secondary text: explanations, dates, notes.
pub fn hint(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(text).color(t.text_dim));
}

/// A link that opens `url` in the system browser, in a new tab. Hovering shows where it leads.
pub fn link(ui: &mut Ui, text: &str, url: &str) -> Response {
    let response = ui.add(Link::new(text));
    if response.clicked() {
        ui.ctx().open_url(OpenUrl::new_tab(url));
    }
    response.on_hover_text(url)
}

#[cfg(test)]
#[path = "widgets_tests.rs"]
mod tests;
