//! A manual test that renders the screens to PNG so a person can look at them (CLAUDE.md, section 2, rule 8).
//!
//! It needs a GPU (wgpu) and writes files, so it is `#[ignore]`d. Run it by hand:
//!
//! ```sh
//! GYMGAR_SNAPSHOTS=/tmp/gymgar-shots cargo test -p gymgar-ui-egui -- --ignored render_snapshots
//! ```
//!
//! Without `GYMGAR_SNAPSHOTS` it writes nothing. It draws the real app (its `logic` and `ui`), 1200x800 points at a
//! scale of 2, in English and in Russian: `<state>-en.png` and `<state>-ru.png`.

use std::path::{Path, PathBuf};

use chrono::TimeDelta;
use egui_kittest::Harness;
use gymgar_domain::{ConnectionStatus, GarminConnection};
use gymgar_engine::GarminAuthError;
use gymgar_engine::testing::TestConfig;

use crate::i18n::Lang;
use crate::test_support::{app_connected, app_in, builder, click, pick, tick, type_key, working_connection};
use crate::{GymgarApp, Section, StartupErrorApp};

/// The environment variable that names the directory the PNGs go to.
const DIRECTORY_VARIABLE: &str = "GYMGAR_SNAPSHOTS";

/// Points to pixels, so the text in the PNGs is as sharp as on a retina screen.
const SCALE: f32 = 2.0;

/// The window of the "cannot start" screen, the size the app opens it with.
fn error_window(app: StartupErrorApp) -> Harness<'static, StartupErrorApp> {
    let mut harness: Harness<'static, StartupErrorApp> =
        builder().with_size(egui::vec2(720.0, 400.0)).with_pixels_per_point(SCALE).wgpu().build_eframe(move |_| app);
    harness.run();
    harness
}

/// A made-up key to fill the field with; it is hidden in the picture.
const KEY: &str = "zq-abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz";

type Shot = Harness<'static, GymgarApp>;

/// A window running the real app, the size of the app's start window.
fn window(app: GymgarApp, section: Section) -> Shot {
    let mut harness: Shot = builder().with_pixels_per_point(SCALE).wgpu().build_eframe(move |_| app);
    harness.state_mut().ui.section = section;
    harness.run();
    harness
}

fn save<S>(harness: &mut Harness<'static, S>, directory: &Path, state: &str, lang: Lang) {
    // The pointer, left over a button by the last click, is not part of the screen.
    harness.event(egui::Event::PointerGone);
    harness.step();
    harness.step();
    let image = harness.render().expect("render the window (does this machine have a GPU?)");
    let path = directory.join(format!("{state}-{}.png", pick(lang, "en", "ru")));
    image.save(&path).expect("write the PNG");
    eprintln!("wrote {}", path.display());
}

fn expiring_in(cfg: &TestConfig, days: i64) -> GarminConnection {
    GarminConnection { key_expires_at: cfg.now + TimeDelta::days(days), ..working_connection(cfg) }
}

/// The start-up check of a saved connection never finishes in these windows (the spawner is deferred), so a window
/// shows the saved state as the connection was left.
fn on_hold() -> TestConfig {
    TestConfig { deferred: true, ..TestConfig::default() }
}

#[test]
#[ignore = "renders PNGs with the GPU; run by hand with GYMGAR_SNAPSHOTS=<directory>"]
fn render_snapshots() {
    let Some(directory) = std::env::var_os(DIRECTORY_VARIABLE).map(PathBuf::from) else {
        eprintln!("{DIRECTORY_VARIABLE} is not set: nothing rendered");
        return;
    };
    std::fs::create_dir_all(&directory).expect("create the snapshot directory");

    for lang in Lang::ALL {
        // 1. The first run: an empty Summary with the invitation.
        let (app, _handles) = app_in(lang, on_hold());
        save(&mut window(app, Section::Summary), &directory, "summary-empty", lang);

        // 2. Settings before connecting, with the key typed and Garmin's refusal under the form.
        let cfg = TestConfig { garmin: Err(GarminAuthError::KeyRejected), ..on_hold() };
        let (app, handles) = app_in(lang, cfg);
        let mut harness = window(app, Section::Settings);
        type_key(&mut harness, KEY);
        click(&mut harness, pick(lang, "Connect", "Подключить"));
        save(&mut harness, &directory, "settings-connecting", lang);
        handles.spawner.expect("a deferred spawner").run_pending();
        tick(&mut harness);
        harness.run();
        save(&mut harness, &directory, "settings-error", lang);

        // 3. Connected, with a year left.
        let cfg = on_hold();
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        save(&mut window(app, Section::Settings), &directory, "settings-connected", lang);
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        save(&mut window(app, Section::Summary), &directory, "summary-connected", lang);

        // 4. The key expires in three days, on both screens.
        let connection = expiring_in(&cfg, 3);
        let (app, _handles) = app_connected(lang, cfg.clone(), &connection);
        save(&mut window(app, Section::Summary), &directory, "summary-expiry-warning", lang);
        let (app, _handles) = app_connected(lang, cfg.clone(), &connection);
        save(&mut window(app, Section::Settings), &directory, "settings-expiry-warning", lang);

        // 5. The states of a key that does not work and of one that is gone.
        for (status, name) in [
            (ConnectionStatus::KeyRejected, "settings-key-rejected"),
            (ConnectionStatus::KeyMissing, "settings-key-missing"),
            (ConnectionStatus::Unverified, "settings-unverified"),
        ] {
            let connection = GarminConnection { last_status: status, ..working_connection(&cfg) };
            let (app, _handles) = app_connected(lang, cfg.clone(), &connection);
            save(&mut window(app, Section::Settings), &directory, name, lang);
        }

        // 6. No system password store: the warning that names the file, and the consent box. A path as long as a real one on
        // macOS, and a long Windows one that has no spaces to break at.
        for (path, name) in [
            ("/Users/alexandra/Library/Application Support/Gymgar/secrets/garmin-personal-api-key", "settings-file-consent"),
            (r"C:\Users\alexandra.petrovna-ivanova\AppData\Roaming\Gymgar\data\secrets\garmin-personal-api-key", "settings-file-consent-windows-path"),
        ] {
            let cfg = TestConfig { system_store: false, file_store_location: Some(path.to_owned()), ..on_hold() };
            let (app, _handles) = app_in(lang, cfg);
            save(&mut window(app, Section::Settings), &directory, name, lang);
        }

        // 7. Gymgar cannot start: the database is damaged.
        let locale = Some(pick(lang, "en-US", "ru-RU").to_owned());
        let file = std::path::Path::new("/Users/alexandra/Library/Application Support/Gymgar/gymgar.sqlite3");
        let error = "the database is corrupt: /Users/alexandra/Library/Application Support/Gymgar/gymgar.sqlite3: file is not a database";
        save(&mut error_window(StartupErrorApp::with_system_locale(error, Some(file), locale.clone())), &directory, "cannot-start-database", lang);
        save(
            &mut error_window(StartupErrorApp::with_system_locale("cannot start the background runtime: out of threads", None, locale)),
            &directory,
            "cannot-start-runtime",
            lang,
        );
    }
}
