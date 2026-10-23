//! Logging for the desktop app: every `log` record goes to stderr and to `logs/gymgar.log` in the data directory.
//!
//! A small logger of our own instead of `env_logger` or `fern`: none of them is in the dependency tree yet, and what the
//! app needs fits in this file (levels per module from `RUST_LOG`, two sinks, one rotation).
//!
//! `RUST_LOG` is a comma-separated list of `module=level` directives and at most one bare level, as in `env_logger`
//! (`debug`, `gymgar_engine=trace`, `warn,gymgar_engine::jobs=debug`); a name ends at a module boundary, and a trailing
//! star (`gymgar*`) makes it a prefix. It replaces the default, which is `info` for the Gymgar crates and `warn` for
//! everything else; modules a `RUST_LOG` without a bare level does not name stay at `warn`.
//!
//! Nothing here can stop the app: a log that cannot be written to is skipped, and a log file that cannot be opened
//! leaves stderr alone. Messages never carry the Garmin key or a token, because the code that logs never has them in a
//! form that prints (`SecretString`); this module also keeps every record on one line, so a message cannot fake another.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use chrono::{SecondsFormat, Utc};
use log::{LevelFilter, Log, Metadata, Record};

const LOGS_DIR: &str = "logs";
const LOG_FILE: &str = "gymgar.log";
const PREVIOUS_LOG_FILE: &str = "gymgar.1.log";

/// `info` for the Gymgar crates (`gymgar`, `gymgar_engine`, ...), `warn` for everything else.
const DEFAULT_SPEC: &str = "warn,gymgar*=info";

/// The level of modules that no directive and no bare level in `RUST_LOG` reaches.
const FALLBACK_LEVEL: LevelFilter = LevelFilter::Warn;

/// Starts logging to stderr and to `<data_dir>/logs/gymgar.log`. The log of the previous run is kept as `gymgar.1.log`.
/// Never fails: what cannot be set up is reported through the log itself.
pub fn install(data_dir: &Path) {
    install_with(Some(&data_dir.join(LOGS_DIR)), std::env::var("RUST_LOG").ok().as_deref());
}

/// Starts logging to stderr only: for a run that has no data directory to keep a log in.
pub fn install_stderr_only() {
    install_with(None, std::env::var("RUST_LOG").ok().as_deref());
}

fn install_with(logs_dir: Option<&Path>, rust_log: Option<&str>) {
    let Setup { filter, file, notes } = setup(logs_dir, rust_log);
    let mut sinks: Vec<Sink> = vec![Box::new(io::stderr())];
    sinks.extend(file.map(|file| Box::new(file) as Sink));
    if install_logger(Logger::new(filter, sinks)).is_err() {
        // Another logger got there first, so there is nowhere to log to but the terminal.
        let _ = writeln!(io::stderr(), "gymgar: a logger is already installed");
        return;
    }
    for note in notes {
        log::warn!("{note}");
    }
}

fn install_logger(logger: Logger) -> Result<(), log::SetLoggerError> {
    let max_level = logger.filter.max_level();
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(max_level);
    Ok(())
}

/// What `install` found out before the logger exists: the filter, the log file if it could be opened, and what is worth a
/// warning once there is a log to put it in.
struct Setup {
    filter: Filter,
    file: Option<File>,
    notes: Vec<String>,
}

fn setup(logs_dir: Option<&Path>, rust_log: Option<&str>) -> Setup {
    let (filter, ignored) = Filter::from_rust_log(rust_log);
    let mut notes: Vec<String> = ignored.into_iter().map(|directive| format!("ignoring the invalid RUST_LOG directive `{directive}`")).collect();
    let mut file = None;
    if let Some(dir) = logs_dir {
        match open_log_file(dir) {
            Ok((opened, rotation)) => {
                file = Some(opened);
                notes.extend(rotation.map(|error| format!("cannot keep the previous log as {PREVIOUS_LOG_FILE}: {error}")));
            }
            Err(error) => notes.push(format!("cannot write the log file in {}: {error}; logging to stderr only", dir.display())),
        }
    }
    Setup { filter, file, notes }
}

/// Opens `gymgar.log` in `logs_dir` for appending, creating the directory and first moving the log of the previous run
/// to `gymgar.1.log` (replacing the one before it, so the last two runs are kept). Also returns why that move failed,
/// if it did: the new run then appends to the existing file, which beats having no log.
fn open_log_file(logs_dir: &Path) -> io::Result<(File, Option<io::Error>)> {
    fs::create_dir_all(logs_dir)?;
    let rotation = rotate(logs_dir).err();
    let file = OpenOptions::new().create(true).append(true).open(logs_dir.join(LOG_FILE))?;
    Ok((file, rotation))
}

fn rotate(logs_dir: &Path) -> io::Result<()> {
    let current = logs_dir.join(LOG_FILE);
    if !current.try_exists()? {
        return Ok(());
    }
    let previous = logs_dir.join(PREVIOUS_LOG_FILE);
    // Windows does not rename over an existing file.
    match fs::remove_file(&previous) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(&current, &previous)
}

/// Where a line goes. Write errors are ignored: a log cannot report that it cannot be written.
type Sink = Box<dyn Write + Send>;

struct Logger {
    filter: Filter,
    sinks: Mutex<Vec<Sink>>,
}

impl Logger {
    fn new(filter: Filter, sinks: Vec<Sink>) -> Logger {
        Logger { filter, sinks: Mutex::new(sinks) }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.filter.level_for(metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        // `log!` only checks the global maximum level, which is the highest of all directives.
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_line(record);
        // A sink that panicked must not silence the others for the rest of the run.
        for sink in self.sinks.lock().unwrap_or_else(PoisonError::into_inner).iter_mut() {
            let _ = sink.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        for sink in self.sinks.lock().unwrap_or_else(PoisonError::into_inner).iter_mut() {
            let _ = sink.flush();
        }
    }
}

/// `2026-10-23T12:00:00.123Z WARN  gymgar_engine::jobs: message`, one line. Line breaks in the message are written as `\n`
/// and `\r`, so that a message (an error text from the OS, say) cannot pass for several records.
fn format_line(record: &Record<'_>) -> String {
    let message = record.args().to_string().replace('\r', "\\r").replace('\n', "\\n");
    format!("{} {:<5} {}: {message}\n", Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true), record.level(), record.target())
}

/// How much to log for which modules.
#[derive(Debug)]
struct Filter {
    /// The level of a module no directive names.
    base: LevelFilter,
    directives: Vec<Directive>,
}

#[derive(Debug)]
struct Directive {
    /// The module path, without the star of a family.
    name: String,
    /// `gymgar*`: every target that starts with the name, not only the module and its children.
    family: bool,
    level: LevelFilter,
}

impl Directive {
    fn matches(&self, target: &str) -> bool {
        if self.family {
            target.starts_with(&self.name)
        } else {
            target.strip_prefix(self.name.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
        }
    }
}

impl Default for Filter {
    fn default() -> Filter {
        Filter::parse(DEFAULT_SPEC).0
    }
}

impl Filter {
    /// The filter for the value of `RUST_LOG`: the default when it is missing or blank. Also returns the directives that were
    /// skipped because they make no sense.
    fn from_rust_log(spec: Option<&str>) -> (Filter, Vec<String>) {
        match spec {
            Some(spec) if !spec.trim().is_empty() => Filter::parse(spec),
            _ => (Filter::default(), Vec::new()),
        }
    }

    /// Reads a spec such as `warn,gymgar*=info`. Returns the filter and the directives that were skipped.
    fn parse(spec: &str) -> (Filter, Vec<String>) {
        let mut filter = Filter { base: FALLBACK_LEVEL, directives: Vec::new() };
        let mut ignored = Vec::new();
        for part in spec.split(',').map(str::trim).filter(|part| !part.is_empty()) {
            let directive = match part.split_once('=') {
                Some((name, level)) => match level.trim().parse::<LevelFilter>() {
                    Ok(level) => Directive::named(name.trim(), level),
                    Err(_) => None,
                },
                // A bare level is the level of everything; any other bare word is a module that logs everything.
                None => match part.parse::<LevelFilter>() {
                    Ok(level) => {
                        filter.base = level;
                        continue;
                    }
                    Err(_) => Directive::named(part, LevelFilter::Trace),
                },
            };
            match directive {
                Some(directive) => filter.directives.push(directive),
                None => ignored.push(part.to_owned()),
            }
        }
        (filter, ignored)
    }

    /// The level for `target`: that of the longest directive that matches it (the later one when they are as long), else
    /// the base level.
    fn level_for(&self, target: &str) -> LevelFilter {
        self.directives
            .iter()
            .filter(|directive| directive.matches(target))
            .max_by_key(|directive| directive.name.len())
            .map_or(self.base, |directive| directive.level)
    }

    /// The highest level any module is logged at: what `log` is told, so that records nobody wants are not even built.
    fn max_level(&self) -> LevelFilter {
        self.directives.iter().map(|directive| directive.level).fold(self.base, std::cmp::max)
    }
}

impl Directive {
    /// A directive for `name`, which may end in a star; `None` for a name with nothing in it.
    fn named(name: &str, level: LevelFilter) -> Option<Directive> {
        let (name, family) = match name.strip_suffix('*') {
            Some(prefix) => (prefix, true),
            None => (name, false),
        };
        if name.is_empty() && !family {
            return None;
        }
        Some(Directive { name: name.to_owned(), family, level })
    }
}

/// Test helpers shared with the tests of the other modules.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, OnceLock};

    use super::*;

    /// A sink that writes to a buffer.
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn reader(buffer: &Arc<Mutex<Vec<u8>>>) -> impl Fn() -> String + use<> {
        let buffer = buffer.clone();
        move || String::from_utf8_lossy(&buffer.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }

    /// A sink that keeps what is written to it, and a function that reads that.
    pub(crate) fn memory_sink() -> (Sink, impl Fn() -> String) {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        (Box::new(Shared(buffer.clone())), reader(&buffer))
    }

    /// Installs the default logger once for the whole test process, writing to memory, and returns a reader of everything
    /// logged so far (by every test). Tests look for markers of their own in it.
    pub(crate) fn capture() -> impl Fn() -> String {
        static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
        let buffer = BUFFER.get_or_init(|| {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            // Only one logger can exist per process; if another one got there first, the markers never show up and the
            // tests that look for them fail loudly.
            let _ = install_logger(Logger::new(Filter::default(), vec![Box::new(Shared(buffer.clone()))]));
            buffer
        });
        reader(buffer)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use log::{Level, LevelFilter};
    use tempfile::TempDir;

    use super::testing::{capture, memory_sink};
    use super::*;

    fn filter(spec: &str) -> Filter {
        Filter::parse(spec).0
    }

    /// Asks `logger` to log `message` from `target` at `level`. One statement, so that the temporaries `format_args!` makes
    /// live as long as the record that borrows them.
    fn log_to(logger: &Logger, level: Level, target: &str, message: &str) {
        log::Log::log(logger, &log::Record::builder().args(format_args!("{message}")).level(level).target(target).build());
    }

    /// The line `format_line` makes of a record.
    fn line_of(level: Level, target: &str, message: &str) -> String {
        format_line(&log::Record::builder().args(format_args!("{message}")).level(level).target(target).build())
    }

    /// Whether a record of `level` from `target` passes `filter`.
    fn passes(filter: &Filter, target: &str, level: Level) -> bool {
        level <= filter.level_for(target)
    }

    #[test]
    fn default_is_info_for_gymgar_and_warn_for_the_rest() {
        let filter = Filter::default();
        for target in ["gymgar", "gymgar_engine", "gymgar_ui_egui::app", "gymgar_garmin::personal::auth"] {
            assert!(passes(&filter, target, Level::Info), "{target}");
            assert!(!passes(&filter, target, Level::Debug), "{target}");
        }
        for target in ["eframe", "eframe::native::run", "wgpu_core", "reqwest", "egui"] {
            assert!(passes(&filter, target, Level::Warn), "{target}");
            assert!(!passes(&filter, target, Level::Info), "{target}");
        }
        assert_eq!(filter.max_level(), LevelFilter::Info);
    }

    #[test]
    fn rust_log_replaces_the_default() {
        // A bare level is the level of everything, Gymgar included.
        let all = filter("debug");
        assert!(passes(&all, "gymgar_engine", Level::Debug) && passes(&all, "eframe", Level::Debug));
        assert!(!passes(&all, "eframe", Level::Trace));
        assert_eq!(all.max_level(), LevelFilter::Debug);
        // Directives for some modules: the others keep a quiet `warn` rather than going silent or falling back to `info`.
        let one = filter("gymgar_engine=trace");
        assert!(passes(&one, "gymgar_engine::jobs", Level::Trace));
        assert!(!passes(&one, "gymgar_store", Level::Info) && passes(&one, "gymgar_store", Level::Warn));
        assert!(!passes(&one, "eframe", Level::Info));
        assert_eq!(one.max_level(), LevelFilter::Trace);
        // `off` is a level too.
        let off = filter("off");
        assert!(!passes(&off, "gymgar_engine", Level::Error));
        assert_eq!(off.max_level(), LevelFilter::Off);
    }

    #[test]
    fn the_most_specific_directive_wins_and_names_end_at_a_module_boundary() {
        let filter = filter("error,gymgar_engine=warn,gymgar_engine::jobs=debug");
        assert!(passes(&filter, "gymgar_engine::jobs", Level::Debug));
        assert!(passes(&filter, "gymgar_engine::jobs::tests", Level::Debug));
        assert!(!passes(&filter, "gymgar_engine::session", Level::Info) && passes(&filter, "gymgar_engine::session", Level::Warn));
        // `gymgar_engine` is not a prefix of `gymgar_engine_extra`: that is another crate.
        assert!(!passes(&filter, "gymgar_engine_extra", Level::Warn) && passes(&filter, "gymgar_engine_extra", Level::Error));
        // A star makes the name a prefix: all of `gymgar*`, as the default has it.
        let family = self::filter("warn,gymgar*=debug,gymgar_store=error");
        assert!(passes(&family, "gymgar", Level::Debug) && passes(&family, "gymgar_ui_egui::app", Level::Debug));
        assert!(!passes(&family, "gymgar_store", Level::Warn), "the longer name wins over the star");
        assert!(!passes(&family, "eframe", Level::Info));
    }

    #[test]
    fn a_module_named_without_a_level_logs_everything() {
        let filter = filter("warn,gymgar_engine");
        assert!(passes(&filter, "gymgar_engine::session", Level::Trace));
        assert!(!passes(&filter, "eframe", Level::Info));
    }

    #[test]
    fn levels_are_case_insensitive_and_spaces_are_ignored() {
        let (filter, ignored) = Filter::parse(" INFO , gymgar_engine = Debug ,");
        assert!(ignored.is_empty(), "{ignored:?}");
        assert!(passes(&filter, "gymgar_engine", Level::Debug));
        assert!(passes(&filter, "eframe", Level::Info) && !passes(&filter, "eframe", Level::Debug));
    }

    #[test]
    fn invalid_directives_are_reported_and_skipped() {
        let (filter, ignored) = Filter::parse("gymgar_engine=loud,info,=debug,a=b=c,gymgar_store=trace");
        assert_eq!(ignored, ["gymgar_engine=loud", "=debug", "a=b=c"]);
        assert!(passes(&filter, "gymgar_store", Level::Trace));
        assert!(passes(&filter, "eframe", Level::Info) && !passes(&filter, "eframe", Level::Debug));
        let (_, ignored) = Filter::parse("");
        assert!(ignored.is_empty());
    }

    #[test]
    fn an_empty_or_missing_rust_log_means_the_default() {
        for spec in [None, Some(""), Some("  ")] {
            let filter = Filter::from_rust_log(spec).0;
            assert!(passes(&filter, "gymgar_engine", Level::Info), "{spec:?}");
            assert!(!passes(&filter, "eframe", Level::Info), "{spec:?}");
        }
        assert!(passes(&Filter::from_rust_log(Some("trace")).0, "eframe", Level::Trace));
    }

    #[test]
    fn a_line_has_time_level_target_and_message() {
        let line = line_of(Level::Warn, "gymgar_engine::jobs", "connected for 365 days");
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "{line:?}");
        let (time, rest) = line.split_once(' ').unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(time).is_ok(), "{time}");
        assert_eq!(rest, "WARN  gymgar_engine::jobs: connected for 365 days\n");
    }

    #[test]
    fn a_message_with_line_breaks_stays_one_line() {
        let line = line_of(Level::Error, "gymgar", "first\nsecond\r\nthird");
        assert_eq!(line.matches('\n').count(), 1, "{line:?}");
        assert!(line.ends_with("gymgar: first\\nsecond\\r\\nthird\n"), "{line:?}");
    }

    #[test]
    fn the_logger_writes_what_passes_the_filter_to_every_sink() {
        let (first, first_text) = memory_sink();
        let (second, second_text) = memory_sink();
        let logger = Logger::new(filter("warn,gymgar*=info"), vec![first, second]);
        for (level, target) in [(Level::Info, "gymgar_engine"), (Level::Debug, "gymgar_engine"), (Level::Info, "eframe"), (Level::Warn, "eframe")] {
            // The logger decides by itself: `log!` only checks the global maximum level.
            log_to(&logger, level, target, "a message");
        }
        for text in [first_text(), second_text()] {
            let lines: Vec<&str> = text.lines().map(|line| line.split_once(' ').unwrap().1).collect();
            assert_eq!(lines, ["INFO  gymgar_engine: a message", "WARN  eframe: a message"]);
        }
    }

    #[test]
    fn a_broken_sink_does_not_stop_the_others_or_the_app() {
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("the disk is full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("the disk is full"))
            }
        }
        let (memory, text) = memory_sink();
        let logger = Logger::new(Filter::default(), vec![Box::new(Broken), memory]);
        log_to(&logger, Level::Error, "gymgar", "still here");
        log::Log::flush(&logger);
        assert!(text().contains("still here"));
    }

    #[test]
    fn the_previous_log_becomes_the_first_old_one() {
        let dir = TempDir::new().unwrap();
        let logs = dir.path().join("logs");
        // The first run: no previous log, the directory is created.
        let file = open_log_file(&logs).unwrap();
        drop(file);
        assert!(logs.join("gymgar.log").is_file());
        assert!(!logs.join("gymgar.1.log").exists());

        fs::write(logs.join("gymgar.log"), "first run\n").unwrap();
        drop(open_log_file(&logs).unwrap());
        assert_eq!(fs::read_to_string(logs.join("gymgar.1.log")).unwrap(), "first run\n");
        assert_eq!(fs::read_to_string(logs.join("gymgar.log")).unwrap(), "", "a new run starts a new log");

        // A third run replaces the old one: only the last two runs are kept.
        fs::write(logs.join("gymgar.log"), "second run\n").unwrap();
        drop(open_log_file(&logs).unwrap());
        assert_eq!(fs::read_to_string(logs.join("gymgar.1.log")).unwrap(), "second run\n");
        let names: Vec<String> = fs::read_dir(&logs).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn a_log_directory_that_cannot_be_made_is_an_error_not_a_panic() {
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("logs");
        fs::write(&blocker, "a file where the directory should be").unwrap();
        assert!(open_log_file(&blocker).is_err());
    }

    #[test]
    fn setup_opens_the_log_file_and_reads_rust_log() {
        let dir = TempDir::new().unwrap();
        let logs = dir.path().join("logs");
        let setup = setup(Some(&logs), Some("gymgar_engine=debug,nonsense=loud"));
        assert!(setup.file.is_some() && logs.join("gymgar.log").is_file());
        assert!(passes(&setup.filter, "gymgar_engine", Level::Debug));
        assert_eq!(setup.notes, ["ignoring the invalid RUST_LOG directive `nonsense=loud`"]);
        // Nothing to say when all is well.
        let quiet = self::setup(Some(&logs), None);
        assert!(quiet.notes.is_empty() && quiet.file.is_some(), "{:?}", quiet.notes);
    }

    #[test]
    fn setup_falls_back_to_stderr_when_the_log_file_cannot_be_opened() {
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("logs");
        fs::write(&blocker, "a file where the directory should be").unwrap();
        let setup = setup(Some(&blocker), None);
        assert!(setup.file.is_none());
        assert_eq!(setup.notes.len(), 1, "{:?}", setup.notes);
        assert!(setup.notes.iter().all(|note| note.contains("logging to stderr only") && note.contains("logs")), "{:?}", setup.notes);
        // No data directory at all (the system does not say where it is): the same, without a complaint about a file.
        let none = self::setup(None, None);
        assert!(none.file.is_none() && none.notes.is_empty());
    }

    #[test]
    fn the_installed_logger_writes_the_default_levels() {
        let captured = capture();
        log::info!(target: "gymgar_engine", "marker-visible-info");
        log::debug!(target: "gymgar_engine", "marker-hidden-debug");
        log::info!(target: "eframe", "marker-hidden-foreign-info");
        log::warn!(target: "eframe", "marker-visible-foreign-warn");
        let text = captured();
        assert!(text.contains("marker-visible-info") && text.contains("marker-visible-foreign-warn"), "{text}");
        assert!(!text.contains("marker-hidden-debug") && !text.contains("marker-hidden-foreign-info"), "{text}");
    }
}
