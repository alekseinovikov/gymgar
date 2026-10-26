//! Gymgar's UI shell, built on egui/eframe.
//!
//! L4 and deliberately thin: screens render state and dispatch engine commands; they hold no
//! business logic. Colours, radii and spacing come from the theme, never hard-coded in a widget.
//! Every text the user sees goes through [`tl!`], with its Russian translation in `i18n/ru.tsv`.

mod activities_ui;
mod activity_detail_ui;
mod app;
mod dates;
mod garmin_ui;
pub mod i18n;
pub mod icons;
mod numbers;
mod rail;
mod set_edit_ui;
mod settings_ui;
#[cfg(test)]
mod snapshot_tests;
mod startup_error;
pub mod state;
mod summary_ui;
#[cfg(test)]
mod test_support;
pub mod theme;
pub mod widgets;

pub use app::GymgarApp;
pub use startup_error::StartupErrorApp;
pub use state::{ActivitiesState, ConnectForm, ListFilter, Picker, PickerTarget, Section, SetEdit, UiState};
