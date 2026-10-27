//! The icon rail: a narrow column on the left with Summary and Activities at the top and Settings at the bottom (spec
//! §6, window variant C; activity import spec §2, item 4). It draws the sections, the number of activities the user has
//! not opened on the Activities icon, and reports a click by changing the section; it knows nothing else.

use egui::{Align, CursorIcon, FontId, Frame, Layout, Margin, Panel, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, WidgetInfo, pos2, vec2};

use crate::icons::{self, Icon};
use crate::state::Section;
use crate::theme::{Tokens, metrics};
use crate::tl;

/// The badge shows at most this number; more is `99+`.
const BADGE_MAX: u32 = 99;

/// Draws the rail as the left panel of `ui`; a click on an icon makes its section the current one. `new_count` is the
/// number of activities not opened yet, shown on the Activities icon when there are any.
pub(crate) fn show(section: &mut Section, new_count: u32, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    Panel::left("rail")
        .exact_size(metrics::RAIL_WIDTH)
        .resizable(false)
        .frame(Frame::new().fill(t.chrome).inner_margin(Margin::symmetric(metrics::RAIL_PADDING, metrics::RAIL_PADDING + 4)))
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                item(ui, &t, section, Section::Summary, Icon::House, tl!("Summary"), 0);
                item(ui, &t, section, Section::Activities, Icon::Dumbbell, tl!("Activities"), new_count);
            });
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                item(ui, &t, section, Section::Settings, Icon::Settings, tl!("Settings"), 0);
            });
        });
}

fn item(ui: &mut Ui, t: &Tokens, current: &mut Section, target: Section, icon: Icon, label: &str, badge: u32) {
    if button(ui, t, icon, label, *current == target, badge).clicked() {
        *current = target;
    }
}

/// One icon button. It has no text, so its name for screen readers and tests is `label`, which is also its tooltip; a
/// `badge` above zero is drawn on its corner and is its value.
fn button(ui: &mut Ui, t: &Tokens, icon: Icon, label: &str, active: bool, badge: u32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(metrics::RAIL_BUTTON, metrics::RAIL_BUTTON), Sense::click());
    let count = (badge > 0).then(|| if badge > BADGE_MAX { format!("{BADGE_MAX}+") } else { badge.to_string() });
    response.widget_info(|| {
        let mut info = WidgetInfo::selected(Role::Button, ui.is_enabled(), active, label);
        info.current_text_value = count.clone();
        info
    });
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
        if let Some(count) = count {
            paint_badge(ui, t, rect, count);
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label)
}

/// The number on the top right corner of a button: accent-coloured, as wide as its digits need.
fn paint_badge(ui: &Ui, t: &Tokens, button: Rect, count: String) {
    let galley = ui.painter().layout_no_wrap(count, FontId::proportional(metrics::BADGE_TEXT_SIZE), t.accent_text);
    let width = (galley.size().x + metrics::BADGE_HEIGHT / 2.0).max(metrics::BADGE_HEIGHT);
    // Over the corner of the button, as in the mock-up, and still inside the rail.
    let right = button.right() + metrics::BADGE_HEIGHT / 3.0;
    let badge =
        Rect::from_min_max(pos2(right - width, button.top() - metrics::BADGE_HEIGHT / 3.0), pos2(right, button.top() + metrics::BADGE_HEIGHT * 2.0 / 3.0));
    ui.painter().rect_filled(badge, metrics::BADGE_HEIGHT / 2.0, t.accent);
    ui.painter().galley(badge.center() - galley.size() / 2.0, galley, t.accent_text);
}
