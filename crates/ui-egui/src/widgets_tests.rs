use egui::{CornerRadius, Id, OutputCommand, Role, Shape};
use egui_kittest::kittest::{NodeT, Queryable};

use super::*;
use crate::test_support::ui_harness;
use crate::theme::Tokens;

#[test]
fn buttons_are_labelled_and_clickable() {
    let mut harness = ui_harness(
        |ui, clicks: &mut (u32, u32)| {
            clicks.0 += u32::from(primary_button(ui, "Connect").clicked());
            clicks.1 += u32::from(secondary_button(ui, "Cancel").clicked());
        },
        (0, 0),
    );
    harness.get_by_role_and_label(Role::Button, "Connect").click();
    harness.run();
    assert_eq!(*harness.state(), (1, 0));
    harness.get_by_role_and_label(Role::Button, "Cancel").click();
    harness.run();
    assert_eq!(*harness.state(), (1, 1));
}

#[test]
fn a_disabled_button_is_disabled_and_ignores_clicks() {
    let mut harness = ui_harness(
        |ui, clicks: &mut u32| {
            ui.add_enabled_ui(false, |ui| {
                *clicks += u32::from(primary_button(ui, "Connect").clicked());
                *clicks += u32::from(secondary_button(ui, "Cancel").clicked());
            });
        },
        0,
    );
    for label in ["Connect", "Cancel"] {
        assert!(harness.get_by_label(label).accesskit_node().is_disabled(), "{label}");
        harness.get_by_label(label).click();
    }
    harness.run();
    assert_eq!(*harness.state(), 0);
}

#[test]
fn a_card_holds_its_content_and_passes_the_result_out() {
    let harness = ui_harness(
        |ui, answer: &mut Option<u32>| {
            *answer = Some(card(ui, |ui| {
                ui.label("Inside the card");
                42
            }));
        },
        None,
    );
    harness.get_by_label("Inside the card");
    assert_eq!(*harness.state(), Some(42));
}

#[test]
fn a_card_is_painted_with_the_card_tokens() {
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            card(ui, |ui| ui.label("x"));
        },
        (),
    );
    harness.run();
    let t = Tokens::SPORT;
    // A rounded rectangle filled with the card colour and radius: both come from the tokens, not from the widget.
    let painted = harness
        .output()
        .shapes
        .iter()
        .any(|clipped| matches!(&clipped.shape, Shape::Rect(rect) if rect.fill == t.card && rect.corner_radius == CornerRadius::from(t.radius_card)));
    assert!(painted, "no rectangle with the card fill and the card radius was painted");
}

fn long_key() -> String {
    (b'a'..=b'z').cycle().take(300).map(char::from).collect()
}

#[test]
fn a_secret_field_takes_a_key_of_any_length_and_never_shows_it() {
    let key = long_key();
    let mut harness = ui_harness(
        |ui, buf: &mut String| {
            let label = ui.label("Key");
            secret_field(ui, Id::unique("key"), buf, 320.0).labelled_by(label.id);
        },
        String::new(),
    );
    harness.get_by_role(Role::PasswordInput).focus();
    harness.run();
    harness.get_by_role(Role::PasswordInput).type_text(&key);
    harness.run();

    // Not cut at 128 characters, or at any other length.
    assert_eq!(harness.state().len(), 300);
    assert_eq!(*harness.state(), key);

    // No node of the accessibility tree (what a screen reader or a test reads) carries the key or a part of it.
    let probe = key.get(..16).expect("the key is ASCII");
    for node in harness.root().children_recursive() {
        let node = node.accesskit_node();
        for text in [node.label(), node.value()].into_iter().flatten() {
            assert!(!text.contains(probe), "{:?} exposes the key: {text}", node.role());
        }
    }
}

#[test]
fn a_secret_field_is_found_by_its_label() {
    let mut harness = ui_harness(
        |ui, buf: &mut String| {
            let label = ui.label("Key");
            secret_field(ui, Id::unique("key"), buf, 200.0).labelled_by(label.id);
        },
        String::new(),
    );
    harness.run();
    assert_eq!(harness.get_by_label("Key").accesskit_node().role(), Role::PasswordInput);
}

#[test]
fn a_dropdown_applies_the_choice_and_reports_only_a_real_change() {
    struct State {
        value: u8,
        changed: bool,
    }
    let mut harness = ui_harness(
        |ui, state: &mut State| {
            state.changed |= dropdown(ui, Id::unique("number"), &mut state.value, &[(1, "One"), (2, "Two"), (3, "Three")]);
        },
        State { value: 1, changed: false },
    );
    assert_eq!(harness.get_by_role(Role::ComboBox).value().as_deref(), Some("One"));

    // Choosing what is already chosen changes nothing.
    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    // (The list is named by what it shows, so "One" is also its name: pick the entry by its role.)
    harness.get_by_role_and_label(Role::Button, "One").click();
    harness.run();
    assert_eq!((harness.state().value, harness.state().changed), (1, false));

    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Two").click();
    harness.run();
    assert_eq!((harness.state().value, harness.state().changed), (2, true));
    assert_eq!(harness.get_by_role(Role::ComboBox).value().as_deref(), Some("Two"));
}

#[test]
fn status_and_warning_texts_are_visible() {
    let harness = ui_harness(
        |ui, _: &mut ()| {
            status_dot(ui, Tokens::SPORT.accent, "Connected");
            warning_box(ui, "The key expires soon");
        },
        (),
    );
    harness.get_by_label("Connected");
    harness.get_by_label("The key expires soon");
}

#[test]
fn a_link_opens_its_address_in_a_new_browser_tab() {
    let mut harness = ui_harness(
        |ui, clicked: &mut bool| {
            *clicked |= link(ui, "Open Garmin Connect", "https://connect.garmin.com/app/settings/personalApiToken").clicked();
        },
        false,
    );
    harness.get_by_role_and_label(Role::Link, "Open Garmin Connect").click();
    harness.step();
    let opened: Vec<_> = harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            OutputCommand::OpenUrl(open) => Some((open.url.as_str(), open.new_tab)),
            _ => None,
        })
        .collect();
    assert_eq!(opened, [("https://connect.garmin.com/app/settings/personalApiToken", true)]);
    assert!(*harness.state());
}

/// The colour the text `text` was painted in, read from the shapes of the last frame.
fn text_colour(harness: &egui_kittest::Harness<'_, impl Sized>, text: &str) -> Option<egui::Color32> {
    harness.output().shapes.iter().find_map(|clipped| match &clipped.shape {
        Shape::Text(shape) if shape.galley.text() == text => shape.galley.job.sections.first().map(|section| section.format.color),
        _ => None,
    })
}

#[test]
fn titles_and_hints_are_visible_and_drawn_from_the_tokens() {
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            page_title(ui, "A page", Some("What the page is for"));
            page_title(ui, "Another page", None);
            card_title(ui, "A card");
            hint(ui, "A note");
        },
        (),
    );
    harness.run();
    for text in ["A page", "What the page is for", "Another page", "A card", "A note"] {
        harness.get_by_label(text);
    }
    // The title of a screen is a heading for screen readers; the title of a card is not.
    harness.get_by_role_and_label(Role::Heading, "A page");
    harness.get_by_role_and_label(Role::Heading, "Another page");
    assert!(harness.query_by_role_and_label(Role::Heading, "A card").is_none());
    let t = Tokens::SPORT;
    assert_eq!(text_colour(&harness, "A note"), Some(t.text_dim), "a hint is secondary text");
    assert_eq!(text_colour(&harness, "What the page is for"), Some(t.text_dim), "so is the line under a title");
    assert_eq!(text_colour(&harness, "A page"), Some(t.text));
    assert_eq!(text_colour(&harness, "A card"), Some(t.text));
}

#[test]
fn a_page_title_is_larger_than_a_card_title_and_both_than_body_text() {
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            page_title(ui, "A page", None);
            card_title(ui, "A card");
            ui.label("Body");
        },
        (),
    );
    harness.run();
    let height = |label: &str| harness.get_by_label(label).rect().height();
    assert!(height("A page") > height("A card"), "{} vs {}", height("A page"), height("A card"));
    assert!(height("A card") > height("Body"), "{} vs {}", height("A card"), height("Body"));
}

#[test]
fn a_warning_box_can_hold_more_than_its_text() {
    let mut harness = ui_harness(
        |ui, clicks: &mut u32| {
            let clicked = warning_box_with(ui, "The key expires soon", |ui| secondary_button(ui, "Replace key").clicked());
            *clicks += u32::from(clicked);
        },
        0,
    );
    harness.get_by_label("The key expires soon");
    harness.get_by_role_and_label(Role::Button, "Replace key").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
}
