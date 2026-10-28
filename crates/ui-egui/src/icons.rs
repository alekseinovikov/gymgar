//! The icons: Lucide SVGs (ISC licence, see `ATTRIBUTION.md`) embedded in the binary and drawn through `egui_extras`.
//!
//! Lucide draws with `currentColor`, which a renderer outside a browser takes as black, and a tint cannot lighten black.
//! So the icons are handed to the loader in white, and the caller tints them with a theme colour.

use std::sync::{Arc, LazyLock};

/// An icon of the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    House,
    Dumbbell,
    ChartLine,
    Settings,
}

impl Icon {
    pub const ALL: [Icon; 4] = [Icon::House, Icon::Dumbbell, Icon::ChartLine, Icon::Settings];

    /// The address the image loaders know this icon by; it ends in `.svg`, which is how they pick the SVG loader.
    fn uri(self) -> &'static str {
        match self {
            Icon::House => "bytes://gymgar/icons/house.svg",
            Icon::Dumbbell => "bytes://gymgar/icons/dumbbell.svg",
            Icon::ChartLine => "bytes://gymgar/icons/chart-line.svg",
            Icon::Settings => "bytes://gymgar/icons/settings.svg",
        }
    }

    /// The SVG, white where Lucide says `currentColor`.
    pub(crate) fn svg(self) -> Arc<[u8]> {
        static HOUSE: LazyLock<Arc<[u8]>> = LazyLock::new(|| white(include_str!("../../../assets/icons/house.svg")));
        static DUMBBELL: LazyLock<Arc<[u8]>> = LazyLock::new(|| white(include_str!("../../../assets/icons/dumbbell.svg")));
        static CHART_LINE: LazyLock<Arc<[u8]>> = LazyLock::new(|| white(include_str!("../../../assets/icons/chart-line.svg")));
        static SETTINGS: LazyLock<Arc<[u8]>> = LazyLock::new(|| white(include_str!("../../../assets/icons/settings.svg")));
        match self {
            Icon::House => Arc::clone(&HOUSE),
            Icon::Dumbbell => Arc::clone(&DUMBBELL),
            Icon::ChartLine => Arc::clone(&CHART_LINE),
            Icon::Settings => Arc::clone(&SETTINGS),
        }
    }
}

fn white(svg: &str) -> Arc<[u8]> {
    Arc::from(svg.replace("currentColor", "white").into_bytes())
}

/// Lets `egui::Image` load SVGs. Call once when the window is created.
pub fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
}

/// The icon as an image: white, to be tinted (`.tint(colour)`) and sized (`.fit_to_exact_size(..)`) by the caller.
pub fn icon(icon: Icon) -> egui::Image<'static> {
    egui::Image::from_bytes(icon.uri(), icon.svg())
}

#[cfg(test)]
#[path = "icons_tests.rs"]
mod tests;
