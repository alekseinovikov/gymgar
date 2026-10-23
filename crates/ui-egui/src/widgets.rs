//! The shared widgets. A screen never invents its own button or card: it uses these, and they take every colour and
//! radius from the theme tokens.
//!
//! Two things a widget cannot do for the caller: disable it (wrap it in `ui.add_enabled_ui`) and give a text field its
//! accessible name (`Response::labelled_by` on the visible label).

use egui::{Button, Color32, ComboBox, Frame, Id, Link, Margin, OpenUrl, Response, RichText, Role, Sense, Stroke, TextEdit, Ui, WidgetInfo, vec2};

use crate::theme::{Tokens, metrics};

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
        ui.add(Button::new(text).min_size(vec2(0.0, metrics::CONTROL_HEIGHT)))
    })
    .inner
}

/// A quieter action next to the main one: a field-coloured button with a border.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(Button::new(text).min_size(vec2(0.0, metrics::CONTROL_HEIGHT)))
}

/// A rounded panel on the window background, as wide as the space it is given.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.radius_card)
        .inner_margin(Margin::same(metrics::CARD_PADDING))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
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
    let t = Tokens::get(ui.ctx());
    Frame::new()
        .fill(t.warning.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, t.warning))
        .corner_radius(t.radius_control)
        .inner_margin(Margin::same(metrics::BOX_PADDING))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(t.warning));
        });
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
