use egui::load::TexturePoll;

use super::*;
use crate::test_support::ui_harness;

#[test]
fn the_embedded_icons_are_svgs_that_a_tint_can_colour() {
    for icon in Icon::ALL {
        let bytes = icon.svg();
        let svg = std::str::from_utf8(&bytes).expect("an SVG is text");
        assert!(svg.contains("<svg"), "{icon:?}");
        // `currentColor` renders black outside a browser, and a tint cannot lighten black: the loader gets white.
        assert!(!svg.contains("currentColor"), "{icon:?}");
        assert!(svg.contains("stroke=\"white\""), "{icon:?}");
    }
    assert_ne!(Icon::House.svg(), Icon::Settings.svg());
}

#[test]
fn the_icons_load_and_render_once_the_loaders_are_installed() {
    let mut harness = ui_harness(
        |ui, loaded: &mut Vec<(Icon, bool)>| {
            loaded.clear();
            for icon in Icon::ALL {
                let poll = super::icon(icon).load_for_size(ui.ctx(), egui::vec2(24.0, 24.0));
                loaded.push((icon, matches!(poll, Ok(TexturePoll::Ready { .. }))));
            }
        },
        Vec::new(),
    );
    harness.run();
    assert_eq!(*harness.state(), Icon::ALL.map(|icon| (icon, true)));
}
