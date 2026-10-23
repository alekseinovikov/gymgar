//! The window shown when Gymgar cannot start: the database cannot be opened, or the background runtime cannot be built.
//!
//! One screen and nothing else, so a damaged or locked file is a message the user can act on instead of a crash. It has
//! no engine session, so the interface language cannot come from the settings (they live in the database that failed to
//! open): it follows the system language. The error text is the one the failing code gave, untranslated, because it
//! is what to quote when asking for help; the title and the advice around it are translated.

use std::path::Path;

use egui::{CentralPanel, Frame, Margin, Ui};
use gymgar_domain::Language;

use crate::i18n::{self, Lang};
use crate::theme::{Tokens, metrics};
use crate::{tl, widgets};

/// The "Gymgar cannot start" screen.
pub struct StartupErrorApp {
    error: String,
    /// The file the error is about (the database); `None` for an error that is not about a file.
    file: Option<String>,
    lang: Lang,
}

impl StartupErrorApp {
    /// A screen for `error`, in the language of the operating system. `file` is the file the error is about, if any.
    pub fn new(error: &str, file: Option<&Path>) -> StartupErrorApp {
        StartupErrorApp::with_system_locale(error, file, sys_locale::get_locale())
    }

    /// Like [`StartupErrorApp::new`], with the locale the operating system is taken to have: `None` when it is unknown.
    /// For tests, which must not depend on the machine they run on.
    pub fn with_system_locale(error: &str, file: Option<&Path>, system_locale: Option<String>) -> StartupErrorApp {
        let lang = i18n::resolve(Language::System, system_locale.as_deref());
        StartupErrorApp { error: error.to_owned(), file: file.map(|path| path.display().to_string()), lang }
    }

    /// Draws the screen.
    pub fn show(&self, ui: &mut Ui) {
        // Set on every frame: the language is per thread, and this is the thread that draws.
        i18n::set_current(self.lang);
        let t = Tokens::get(ui.ctx());
        CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::same(metrics::SCREEN_MARGIN))).show(ui, |ui| {
            // `set_max_width` would also widen the content past a window narrower than the maximum.
            ui.set_max_width(ui.available_width().min(metrics::CONTENT_MAX_WIDTH));
            let advice = match self.file {
                Some(_) => tl!("Gymgar could not open its data. Check that the disk is available and the file is not damaged, then start Gymgar again."),
                None => tl!("Start Gymgar again. If the problem remains, report the error below."),
            };
            widgets::page_title(ui, tl!("Gymgar cannot start"), Some(advice));
            ui.add_space(metrics::GAP);
            widgets::card(ui, |ui| {
                widgets::hint(ui, tl!("Details"));
                widgets::warning_box(ui, &self.error);
                if let Some(file) = &self.file {
                    ui.add_space(metrics::GAP);
                    widgets::hint(ui, tl!("File"));
                    ui.label(file);
                }
            });
        });
    }
}

impl eframe::App for StartupErrorApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    /// This window is not the app: what it would save (the window state of an error screen) must not replace what the real
    /// window saved.
    fn persist_egui_memory(&self) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "startup_error_tests.rs"]
mod tests;
