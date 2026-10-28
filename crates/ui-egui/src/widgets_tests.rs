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

#[test]
fn a_chip_is_a_toggle_that_reports_its_clicks() {
    let mut harness = ui_harness(
        |ui, chosen: &mut usize| {
            ui.horizontal(|ui| {
                for (index, label) in ["All", "Strength"].into_iter().enumerate() {
                    if chip(ui, label, *chosen == index).clicked() {
                        *chosen = index;
                    }
                }
            });
        },
        0,
    );
    let toggled = |harness: &egui_kittest::Harness<'_, usize>, label: &str| {
        format!("{:?}", harness.get_by_role_and_label(Role::Button, label).accesskit_node().toggled())
    };
    assert_eq!((toggled(&harness, "All").as_str(), toggled(&harness, "Strength").as_str()), ("Some(True)", "Some(False)"));
    harness.get_by_role_and_label(Role::Button, "Strength").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
    assert_eq!((toggled(&harness, "All").as_str(), toggled(&harness, "Strength").as_str()), ("Some(False)", "Some(True)"));
    // As high as the metric says, and wider than its text.
    let rect = harness.get_by_role_and_label(Role::Button, "Strength").rect();
    assert_eq!(rect.height(), metrics::CHIP_HEIGHT);
    assert!(rect.width() > 2.0 * metrics::CHIP_PADDING);
}

#[test]
fn a_heading_is_a_heading_for_screen_readers() {
    let harness = ui_harness(|ui, (): &mut ()| heading(ui, "Activities"), ());
    harness.get_by_role_and_label(Role::Heading, "Activities");
}

/// Whether a text with `text` was painted, and how: its colour and whether it is struck through.
fn painted_text(harness: &egui_kittest::Harness<'_, ()>, text: &str) -> Option<(egui::Color32, bool)> {
    harness.output().shapes.iter().find_map(|clipped| match &clipped.shape {
        Shape::Text(shape) if shape.galley.job.text == text => {
            let format = &shape.galley.job.sections.first()?.format;
            Some((format.color, format.strikethrough.width > 0.0))
        }
        _ => None,
    })
}

#[test]
fn a_corrected_set_value_is_accent_with_the_recorded_one_struck_through() {
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            set_value(ui, "77.5 kg", Some("80"));
            set_value(ui, "8", None);
        },
        (),
    );
    harness.run();
    let t = Tokens::SPORT;
    assert_eq!(painted_text(&harness, "77.5 kg"), Some((t.accent, false)));
    assert_eq!(painted_text(&harness, "80"), Some((t.text_dim, true)));
    assert_eq!(painted_text(&harness, "8"), Some((t.text, false)), "a value as recorded is plain");
    // Screen readers cannot see a line through a number: they hear what it was.
    harness.get_by_label("was 80");
    harness.get_by_label("77.5 kg");
}

#[test]
fn a_card_with_a_border_takes_its_colour() {
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            card_with_border(ui, Tokens::SPORT.warning, |ui| ui.label("x"));
        },
        (),
    );
    harness.run();
    let t = Tokens::SPORT;
    let painted = harness.output().shapes.iter().any(|clipped| {
        matches!(&clipped.shape, Shape::Rect(rect) if rect.fill == t.card && rect.stroke.color == t.warning && rect.corner_radius == CornerRadius::from(t.radius_card))
    });
    assert!(painted, "no card with the warning border was painted");
}

#[test]
fn a_number_field_is_named_fits_a_table_line_and_takes_a_few_characters() {
    let mut harness = ui_harness(
        |ui, buf: &mut String| {
            number_field(ui, buf, "Weight");
        },
        "80".to_owned(),
    );
    let field = harness.get_by_role_and_label(Role::TextInput, "Weight");
    assert_eq!(field.value().as_deref(), Some("80"));
    // As high as its text, to fit the line of a table (not a form's 36 points).
    assert!(field.rect().height() <= crate::theme::metrics::EDIT_ROW_HEIGHT, "{}", field.rect().height());
    field.set_text_value_accesskit("123456789012345");
    harness.run();
    assert_eq!(harness.state().chars().count(), crate::theme::metrics::NUMBER_FIELD_MAX_CHARS);
}

#[test]
fn an_option_is_a_toggle_named_by_its_text_and_described_by_its_detail() {
    let mut harness = ui_harness(
        |ui, clicks: &mut u32| {
            *clicks += u32::from(option_row(ui, "Romanian deadlift", Some("14 times"), true).clicked());
            *clicks += u32::from(option_row(ui, "Bench press", None, false).clicked());
        },
        0,
    );
    let chosen = harness.get_by_role_and_label(Role::Button, "Romanian deadlift");
    assert_eq!(format!("{:?}", chosen.accesskit_node().toggled()), "Some(True)");
    assert_eq!(chosen.accesskit_node().description().as_deref(), Some("14 times"));
    let other = harness.get_by_role_and_label(Role::Button, "Bench press");
    assert_eq!(format!("{:?}", other.accesskit_node().toggled()), "Some(False)");
    other.click();
    harness.run();
    assert_eq!(*harness.state(), 1);
    // The chosen one is on the accent's backing.
    let t = Tokens::SPORT;
    let backed = harness.output().shapes.iter().any(|clipped| matches!(&clipped.shape, Shape::Rect(rect) if rect.fill == t.accent_soft));
    assert!(backed, "no option was painted on the accent's backing");
}

#[test]
fn a_list_heading_is_drawn_in_capitals_and_read_as_written() {
    let harness = ui_harness(|ui, _: &mut ()| list_heading(ui, "Done before"), ());
    assert_eq!(harness.get_by_label("Done before").accesskit_node().role(), Role::Heading);
    assert!(painted_text(&harness, "DONE BEFORE").is_some(), "the heading is painted in capitals");
}

#[test]
fn a_quiet_button_and_a_toolbar_take_clicks() {
    let mut harness = ui_harness(
        |ui, clicks: &mut (u32, u32)| {
            clicks.0 += u32::from(quiet_button(ui, "+ Add set").clicked());
            toolbar(ui, |ui| clicks.1 += u32::from(secondary_button(ui, "Cancel").clicked()));
        },
        (0, 0),
    );
    harness.get_by_role_and_label(Role::Button, "+ Add set").click();
    harness.run();
    let cancel = harness.get_by_role_and_label(Role::Button, "Cancel");
    // The toolbar's buttons are small.
    assert!(cancel.rect().height() <= crate::theme::metrics::SMALL_CONTROL_HEIGHT + 0.5, "{}", cancel.rect().height());
    cancel.click();
    harness.run();
    assert_eq!(*harness.state(), (1, 1));
}

#[test]
fn a_popover_floats_over_the_screen_with_its_corner_where_asked() {
    let corner = egui::pos2(600.0, 100.0);
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            popover(ui.ctx(), Id::unique("test-popover"), corner, 200.0, "Choose exercise", |ui| ui.label("Inside"));
        },
        (),
    );
    harness.run();
    let inside = harness.get_by_label("Inside").rect();
    assert!(inside.right() <= corner.x && inside.top() >= corner.y && inside.left() >= corner.x - 240.0, "{inside:?}");
    harness.get_by_label("Choose exercise");
}

/// The rectangles painted in the last frame, as (fill, stroke colour, corner radius).
fn painted_rects(harness: &egui_kittest::Harness<'_, impl Sized>) -> Vec<(egui::Color32, egui::Color32, CornerRadius)> {
    harness
        .output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Rect(rect) => Some((rect.fill, rect.stroke.color, rect.corner_radius)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_tile_holds_its_content_takes_clicks_and_is_a_card() {
    let mut harness = ui_harness(
        |ui, clicks: &mut u32| {
            let response = tile(ui, false, false, |ui| {
                ui.label("Inside the tile");
            });
            response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "A tile"));
            *clicks += u32::from(response.clicked());
        },
        0,
    );
    harness.get_by_label("Inside the tile");
    let t = Tokens::SPORT;
    assert!(painted_rects(&harness).contains(&(t.card, t.border, CornerRadius::from(t.radius_card))), "a tile is a card with a border");
    harness.get_by_role_and_label(Role::Button, "A tile").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
}

#[test]
fn a_tile_that_asks_to_add_has_the_accent_border_and_a_dimmed_one_is_faded() {
    let t = Tokens::SPORT;
    let drawn = |accent: bool, dim: bool| {
        let mut harness = ui_harness(
            |ui, _: &mut ()| {
                tile(ui, accent, dim, |ui| {
                    ui.label("x");
                });
            },
            (),
        );
        harness.run();
        painted_rects(&harness)
    };
    let radius = CornerRadius::from(t.radius_card);
    assert!(drawn(true, false).contains(&(t.card, t.accent, radius)));
    assert!(!drawn(false, false).iter().any(|(_, stroke, _)| *stroke == t.accent));
    // Dimmed: what the tile paints is the same colours, less opaque.
    let faded = |color: egui::Color32| color.gamma_multiply(metrics::DIMMED_OPACITY);
    assert!(drawn(false, true).contains(&(faded(t.card), faded(t.border), radius)));
    assert!(drawn(true, true).contains(&(faded(t.card), faded(t.accent), radius)));
}

#[test]
fn a_status_label_is_drawn_in_the_colour_of_its_tone() {
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            advice_label(ui, "can add 85 kg", Tone::Accent);
            advice_label(ui, "hold · 6 of 10", Tone::Dim);
            advice_label(ui, "break · 10 wk", Tone::Warning);
        },
        (),
    );
    harness.run();
    let t = Tokens::SPORT;
    for (text, color) in [("can add 85 kg", t.accent), ("hold · 6 of 10", t.text_dim), ("break · 10 wk", t.warning)] {
        harness.get_by_label(text);
        assert_eq!(text_colour(&harness, text), Some(color), "{text}");
    }
    // Only the one that asks for attention is filled; every label is a rounded outline.
    let rects = painted_rects(&harness);
    assert!(rects.iter().any(|(fill, _, _)| *fill == t.accent_soft));
    assert!(rects.iter().filter(|(_, _, radius)| *radius == CornerRadius::from(metrics::PILL_RADIUS)).count() >= 3);
}

/// The points of every three-point line painted in `color` in the last frame: what the chevron of a status label is.
fn chevrons(harness: &egui_kittest::Harness<'_, impl Sized>, color: egui::Color32) -> Vec<Vec<egui::Pos2>> {
    harness
        .output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Path(path) if path.points.len() == 3 && path.stroke.color == egui::epaint::ColorMode::Solid(color) => Some(path.points.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_status_label_that_asks_to_add_has_an_up_chevron_before_its_text() {
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            advice_label(ui, "can add 85 kg", Tone::Accent);
            advice_label(ui, "hold · 6 of 10", Tone::Dim);
            advice_label(ui, "break · 10 wk", Tone::Warning);
        },
        (),
    );
    harness.run();
    let t = Tokens::SPORT;
    let text = harness.get_by_label("can add 85 kg").rect();
    // One chevron, in the label's colour, drawn with the painter: an apex over two feet, to the left of the text and as
    // high as it is.
    let drawn = chevrons(&harness, t.accent);
    let [chevron] = drawn.as_slice() else { panic!("one chevron expected, got {drawn:?}") };
    let [left, apex, right] = chevron.as_slice() else { panic!("three points") };
    assert!(apex.y < left.y && apex.y < right.y && left.x < apex.x && apex.x < right.x, "an up-chevron: {chevron:?}");
    assert!(right.x < text.left() && left.y < text.bottom() && apex.y > text.top(), "before the text, at its height: {chevron:?} vs {text:?}");
    // The other tones have no chevron.
    assert!(chevrons(&harness, t.text_dim).is_empty() && chevrons(&harness, t.warning).is_empty());
}

#[test]
fn the_title_of_the_advice_is_a_heading_in_the_colour_of_its_tone_and_the_accent_one_has_a_chevron() {
    let mut harness = ui_harness(
        |ui, _: &mut ()| {
            advice_title(ui, "Can add: 85 kg", Tone::Accent);
            advice_title(ui, "Hold 90 kg", Tone::Dim);
            advice_title(ui, "Break 10 weeks", Tone::Warning);
        },
        (),
    );
    harness.run();
    let t = Tokens::SPORT;
    for (text, color) in [("Can add: 85 kg", t.accent), ("Hold 90 kg", t.text), ("Break 10 weeks", t.warning)] {
        harness.get_by_role_and_label(Role::Heading, text);
        assert_eq!(text_colour(&harness, text), Some(color), "{text}");
    }
    // The chevron is before the text of the one that asks to add, and bigger than the one of a status label.
    let text = harness.get_by_label("Can add: 85 kg").rect();
    let drawn = chevrons(&harness, t.accent);
    let [chevron] = drawn.as_slice() else { panic!("one chevron expected, got {drawn:?}") };
    let [left, apex, right] = chevron.as_slice() else { panic!("three points") };
    assert!(right.x < text.left() && apex.y > text.top() && left.y < text.bottom(), "before the text, at its height: {chevron:?} vs {text:?}");
    assert!(right.x - left.x > metrics::CHEVRON_WIDTH, "larger than a label's: {chevron:?}");
    assert!(chevrons(&harness, t.text).is_empty() && chevrons(&harness, t.warning).is_empty());
}

#[test]
fn an_advice_link_is_a_link_with_an_up_chevron_in_the_accent_colour_and_takes_clicks() {
    let text = "Can add: 85 kg · open progress";
    let mut harness = ui_harness(|ui, clicks: &mut u32| *clicks += u32::from(advice_link(ui, text).clicked()), 0);
    harness.run();
    let t = Tokens::SPORT;
    // A link for screen readers, as wide as what it says rather than as its space, with the text and the chevron in the accent.
    let link = harness.get_by_role_and_label(Role::Link, text).rect();
    assert!(link.width() < 400.0, "as wide as its text: {link:?}");
    assert_eq!(text_colour(&harness, text), Some(t.accent));
    let drawn = chevrons(&harness, t.accent);
    let [chevron] = drawn.as_slice() else { panic!("one chevron expected, got {drawn:?}") };
    let [left, apex, right] = chevron.as_slice() else { panic!("three points") };
    // The text itself (the link too is labelled with it for screen readers).
    let label = harness.get_by(|node| node.role() == Role::Label && node.value().as_deref() == Some(text)).rect();
    assert!(apex.y < left.y && apex.y < right.y && right.x < label.left() && left.y < label.bottom() && apex.y > label.top(), "{chevron:?} before {label:?}");
    // Quiet until the pointer is over it: then the text is underlined, and the pointer is a hand.
    let underlined = |harness: &egui_kittest::Harness<'_, u32>| {
        harness.output().shapes.iter().any(|clipped| matches!(&clipped.shape, Shape::LineSegment { points, .. } if (points[0].y - label.bottom()).abs() < 1.0))
    };
    assert!(!underlined(&harness));
    harness.hover_at(link.center());
    harness.run();
    assert!(underlined(&harness), "the text is underlined under the pointer");
    assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    harness.get_by_role_and_label(Role::Link, text).click();
    harness.run();
    assert_eq!(*harness.state(), 1);
}

#[test]
fn a_clickable_row_is_as_wide_as_its_space_takes_clicks_and_lights_under_the_pointer() {
    let t = Tokens::SPORT;
    let mut harness = ui_harness(
        |ui, clicks: &mut u32| {
            let response = clickable_row(ui, 30.0, |ui| {
                ui.label("Inside the row");
            });
            response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "A row"));
            *clicks += u32::from(response.clicked());
        },
        0,
    );
    harness.get_by_label("Inside the row");
    let row = harness.get_by_role_and_label(Role::Button, "A row").rect();
    assert_eq!(row.height(), 30.0);
    assert!(row.width() > 1000.0, "the width of the window, less its margins: {row:?}");
    // Quiet until the pointer is over it.
    let lit = |harness: &egui_kittest::Harness<'_, u32>| painted_rects(harness).iter().any(|(fill, _, _)| *fill == t.edit_row);
    assert!(!lit(&harness));
    harness.hover_at(row.center());
    harness.run();
    assert!(lit(&harness), "the backing shows under the pointer");
    // A click anywhere on it, over its content too, is a click on the row.
    harness.get_by_role_and_label(Role::Button, "A row").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
}

/// The size the sparkline tests give the chart.
const SPARK: egui::Vec2 = egui::vec2(120.0, metrics::SPARKLINE_HEIGHT);

/// What `sparkline` painted for `values`: the polyline's points, and the number of dots, and the rectangle it took.
fn sparkline_of(values: &[f32]) -> (Vec<egui::Pos2>, usize, egui::Rect) {
    let mut harness = ui_harness(
        |ui, (values, rect): &mut (Vec<f32>, egui::Rect)| {
            ui.set_max_width(400.0);
            let before = ui.cursor().min;
            sparkline(ui, values, Tokens::SPORT.accent, SPARK);
            *rect = egui::Rect::from_min_size(before, SPARK);
        },
        (values.to_vec(), egui::Rect::NOTHING),
    );
    harness.run();
    let rect = harness.state().1;
    let mut lines = Vec::new();
    let mut dots = 0;
    for clipped in &harness.output().shapes {
        match &clipped.shape {
            Shape::Path(path) if path.stroke.color == egui::epaint::ColorMode::Solid(Tokens::SPORT.accent) => lines.extend(path.points.iter().copied()),
            Shape::Circle(circle) if circle.fill == Tokens::SPORT.accent => dots += 1,
            _ => {}
        }
    }
    (lines, dots, rect)
}

#[test]
fn a_sparkline_with_no_points_one_point_or_twelve_does_not_panic_and_keeps_its_size() {
    // Nothing to draw: nothing is drawn.
    let (line, dots, _) = sparkline_of(&[]);
    assert_eq!((line.len(), dots), (0, 0));
    // One point is a dot, not a line.
    let (line, dots, rect) = sparkline_of(&[80.0]);
    assert_eq!((line.len(), dots), (0, 1));
    // Twelve points are a line of twelve, inside its rectangle, going up with the values.
    let values: Vec<f32> = (0..12).map(|month| 60.0 + month as f32).collect();
    let (line, dots, rect12) = sparkline_of(&values);
    assert_eq!((line.len(), dots), (12, 0));
    assert!(line.iter().all(|point| rect12.contains(*point) && point.is_finite()), "{line:?} not in {rect12:?}");
    assert!(line.windows(2).all(|pair| pair[0].x < pair[1].x && pair[0].y > pair[1].y), "left to right and up: {line:?}");
    assert_eq!(rect.size(), rect12.size());
}

#[test]
fn a_sparkline_survives_flat_missing_and_extreme_values() {
    // A flat series is a level line in the middle.
    let (line, _, rect) = sparkline_of(&[50.0, 50.0, 50.0]);
    assert_eq!(line.len(), 3);
    assert!(line.iter().all(|point| (point.y - rect.center().y).abs() < 1.0), "{line:?}");
    // What is not a number is left out, and the rest still draws.
    let (line, dots, _) = sparkline_of(&[1.0, f32::NAN, 3.0, f32::INFINITY, f32::NEG_INFINITY]);
    assert_eq!((line.len(), dots), (2, 0));
    assert!(line.iter().all(|point| point.is_finite()));
    // Nothing but those is nothing; the extremes of the type do not overflow.
    assert_eq!(sparkline_of(&[f32::NAN, f32::INFINITY]).0.len(), 0);
    let (line, _, _) = sparkline_of(&[f32::MIN, f32::MAX]);
    assert!(line.iter().all(|point| point.is_finite()), "{line:?}");
}
