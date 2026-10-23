//! Gymgar desktop app: opens the window, wires the platform services to the engine and hands everything to the UI shell.
//!
//! What goes wrong before the window exists (no data directory, a database that cannot be opened, no runtime) is shown
//! in a window of its own with the reason, not as a panic or a silent exit: the user's workouts live in that database.

mod logging;
mod services;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::NativeOptions;
use egui::ViewportBuilder;
use gymgar_engine::Session;
use gymgar_store::{Store, StoreError, paths};
use gymgar_ui_egui::{GymgarApp, StartupErrorApp, UiState, icons, theme};

/// Threads of the runtime that drives the Garmin requests. A request is short and rare, so two are plenty.
const WORKER_THREADS: usize = 2;

/// How long closing the window waits for a job that is still running (a keychain prompt nobody answered, say) before it
/// gives up on it. Dropping the runtime would wait for ever.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

fn main() -> eframe::Result {
    let data_dir = paths::data_dir();
    match &data_dir {
        Ok(dir) => logging::install(dir),
        // Nowhere to keep a log file; the error window below says why.
        Err(_) => logging::install_stderr_only(),
    }
    log::info!("Gymgar {} starting", env!("CARGO_PKG_VERSION"));

    // Owned here, so it lives as long as the app: the jobs of the engine run on it.
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(error) => return show_startup_error(&StartupFailure { text: format!("cannot start the background runtime: {error}"), file: None }),
    };
    let (data_dir, store) = match open_store(data_dir) {
        Ok(opened) => opened,
        Err(failure) => return show_startup_error(&failure),
    };
    log::info!("data directory: {}", data_dir.display());

    let result = run_app(&data_dir, store, &runtime);
    runtime.shutdown_timeout(SHUTDOWN_WAIT);
    result
}

fn build_runtime() -> io::Result<tokio::runtime::Runtime> {
    // Built by hand: `#[tokio::main]` panics when the runtime cannot be created.
    tokio::runtime::Builder::new_multi_thread().worker_threads(WORKER_THREADS).thread_name("gymgar-worker").enable_all().build()
}

/// Why Gymgar cannot start, for the error window.
#[derive(Debug)]
struct StartupFailure {
    /// The text of the error, as the failing code gave it.
    text: String,
    /// The file the error is about, if it is about one.
    file: Option<PathBuf>,
}

/// Opens the database in the data directory (`data_dir` is the result of looking for it), creating it on the first run.
fn open_store(data_dir: Result<PathBuf, StoreError>) -> Result<(PathBuf, Store), StartupFailure> {
    let data_dir = data_dir.map_err(|error| StartupFailure { text: error.to_string(), file: None })?;
    let db_path = paths::db_path(&data_dir);
    match Store::open(&db_path) {
        Ok(store) => Ok((data_dir, store)),
        Err(error) => Err(StartupFailure { text: error.to_string(), file: Some(db_path) }),
    }
}

/// The main window: the engine session on the database, with the platform services.
fn run_app(data_dir: &Path, store: Store, runtime: &tokio::runtime::Runtime) -> eframe::Result {
    let options = NativeOptions {
        viewport: ViewportBuilder::default().with_title("Gymgar").with_app_id("gymgar").with_inner_size([1200.0, 800.0]).with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Gymgar",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx, &theme::Tokens::SPORT);
            icons::install(&cc.egui_ctx);
            let services = services::build(data_dir, cc.egui_ctx.clone(), runtime);
            let session = Session::new(store, services);
            Ok(Box::new(GymgarApp::new(session, UiState::load(cc.storage))))
        }),
    )
}

/// A window with one screen: why Gymgar cannot start.
fn show_startup_error(failure: &StartupFailure) -> eframe::Result {
    log::error!("Gymgar cannot start: {}", failure.text);
    let options = NativeOptions {
        viewport: ViewportBuilder::default().with_title("Gymgar").with_app_id("gymgar").with_inner_size([720.0, 400.0]).with_min_inner_size([480.0, 260.0]),
        // Not the app's window: its size must not become the size the real window opens with next time.
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "Gymgar",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx, &theme::Tokens::SPORT);
            Ok(Box::new(StartupErrorApp::new(&failure.text, failure.file.as_deref())))
        }),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn the_runtime_has_two_worker_threads() {
        let runtime = build_runtime().expect("a runtime");
        assert_eq!(runtime.metrics().num_workers(), 2);
    }

    #[test]
    fn the_first_run_creates_the_database_in_the_data_directory() {
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("Gymgar");
        let (opened_dir, _store) = open_store(Ok(data_dir.clone())).expect("the database opens");
        assert_eq!(opened_dir, data_dir);
        assert!(data_dir.join("gymgar.sqlite3").is_file());
    }

    #[test]
    fn a_damaged_database_is_an_error_screen_with_its_text_and_path() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("gymgar.sqlite3");
        fs::write(&db_path, "This is not a SQLite database, it is just a long enough line of text to be mistaken for a damaged one.".repeat(20)).unwrap();
        let failure = open_store(Ok(dir.path().to_path_buf())).expect_err("a damaged file is not a database");
        assert_eq!(failure.file.as_deref(), Some(db_path.as_path()));
        assert!(failure.text.contains(&db_path.display().to_string()), "the text names the file: {}", failure.text);
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().filter(|entry| entry.as_ref().unwrap().file_name() == "gymgar.sqlite3").count(),
            1,
            "the file is left alone"
        );
    }

    #[test]
    fn a_data_directory_that_cannot_be_created_is_an_error_screen() {
        let dir = TempDir::new().unwrap();
        // A file where the directory should be.
        let blocker = dir.path().join("Gymgar");
        fs::write(&blocker, "a file").unwrap();
        let failure = open_store(Ok(blocker.clone())).expect_err("nothing can be created inside a file");
        assert_eq!(failure.file, Some(blocker.join("gymgar.sqlite3")));
        assert!(failure.text.contains("gymgar.sqlite3") || failure.text.contains("Gymgar"), "{}", failure.text);
    }

    #[test]
    fn no_data_directory_is_an_error_screen_without_a_file() {
        let error = StoreError::Open { path: PathBuf::from("Gymgar"), reason: "the operating system reports no home directory".to_owned() };
        let failure = open_store(Err(error.clone())).expect_err("there is nowhere to open the database");
        assert_eq!((failure.text, failure.file), (error.to_string(), None));
    }
}
