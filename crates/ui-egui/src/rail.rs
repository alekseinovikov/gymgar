//! The icon rail: a narrow column on the left with Summary at the top and Settings at the bottom (spec §6, window
//! variant C). It draws the sections and reports a click by changing the section; it knows nothing else.

use egui::{Align, CursorIcon, Frame, Layout, Margin, Panel, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, WidgetInfo, vec2};

use crate::icons::{self, Icon};
use crate::state::Section;
use crate::theme::{Tokens, metrics};
use crate::tl;

/// Draws the rail as the left panel of `ui`; a click on an icon makes its section the current one.
pub(crate) fn show(section: &mut Section, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    Panel::left("rail")
        .exact_size(metrics::RAIL_WIDTH)
        .resizable(false)
        .frame(Frame::new().fill(t.chrome).inner_margin(Margin::symmetric(metrics::RAIL_PADDING, metrics::RAIL_PADDING + 4)))
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                item(ui, &t, section, Section::Summary, Icon::House, tl!("Summary"));
            });
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                item(ui, &t, section, Section::Settings, Icon::Settings, tl!("Settings"));
            });
        });
}

fn item(ui: &mut Ui, t: &Tokens, current: &mut Section, target: Section, icon: Icon, label: &str) {
    if button(ui, t, icon, label, *current == target).clicked() {
        *current = target;
    }
}

/// One icon button. It has no text, so its name for screen readers and tests is `label`, which is also its tooltip.
fn button(ui: &mut Ui, t: &Tokens, icon: Icon, label: &str, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(metrics::RAIL_BUTTON, metrics::RAIL_BUTTON), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), active, label));
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let (backing, tint) = match (active, hovered) {
            (true, _) => (t.accent_soft, t.accent),
            (false, true) => (t.border, t.text),
            (false, false) => (egui::Color32::TRANSPARENT, t.text_dim),
        };
        ui.painter().rect_filled(rect, t.radius_control, backing);
        if response.has_focus() {
            ui.painter().rect_stroke(rect, t.radius_control, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        }
        icons::icon(icon).tint(tint).paint_at(ui, Rect::from_center_size(rect.center(), vec2(metrics::RAIL_ICON, metrics::RAIL_ICON)));
    }
    response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label)
}
