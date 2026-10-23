# Подключение Garmin и фундамент приложения — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Цель:** Gymgar подключается к Garmin Connect по личному ключу Personal API, хранит ключ безопасно на трёх ОС, показывает статус и предупреждает об истечении ключа. Вокруг этого строится фундамент: реестр команд, фоновые задачи, SQLite, переводы en/ru, тема «Спорт», MCP и CI.

**Архитектура:** вертикальный срез по слоям из `CLAUDE.md`, снизу вверх:
- `domain` — типы;
- `secrets`, `garmin`, `store` — хранилище ключа, клиент Garmin, база;
- `engine` — команды, задачи, трейты сервисов;
- `ui-egui` и `automation` — экраны и MCP;
- `apps/*` — сборка и внедрение сервисов.

`engine` не знает о `garmin` и `secrets`: он объявляет трейты, а приложение подключает адаптеры.

**Стек:** Rust 2024 и rustc 1.99. Библиотеки:
- UI — eframe и egui 0.37, `egui_extras` 0.37 с фичей `svg`, `egui_kittest` 0.37;
- база — `rusqlite` 0.40 с фичей `bundled`;
- секреты — `keyring` 4.2 в режиме `v1` по умолчанию, `secrecy` 0.10;
- сеть и асинхронность — `reqwest` 0.13 с фичей `json`, `tokio` 1.54, `futures` 0.3;
- MCP — `rmcp` 3.7;
- прочее — `schemars` 1.2, `chrono` 0.4.45, `directories` 6, `sys-locale` 0.3, `base64` 0.23, `thiserror` 2, `serde`, `serde_json`, `log`;
- только в тестах — `wiremock` 0.6, `tempfile` 3.

**Спека:** [`docs/superpowers/specs/2026-10-23-garmin-connect-foundation-design.md`](../specs/2026-10-23-garmin-connect-foundation-design.md). Читать вместе с этим планом: тексты интерфейса, статусы и сценарии берутся оттуда.

## Global Constraints

- Действуют все правила `CLAUDE.md`, главное из них:
  - в не-тестовом коде нет `unwrap`, `expect`, `panic!` и индексации `[i]`; это проверяют `[workspace.lints]` и `clippy.toml`;
  - `unsafe` запрещён;
  - перед коммитом `cargo xtask ci` должен быть зелёным.
- Новые крейты: `crates/secrets` (класс `Standalone`) и `apps/gymgar-cli` (класс `Exempt`). Оба регистрируются в `TABLE` в `xtask/src/layers.rs`.
- В каждом новом `Cargo.toml`:
  - `version`, `edition`, `license`, `rust-version`, `publish` наследуются из воркспейса (`.workspace = true`);
  - стоит `[lints] workspace = true`;
  - внешние зависимости объявлены в корневом `[workspace.dependencies]` и подключаются через `{ workspace = true }`.
- Базовый адрес Garmin — `https://personal.connectapi.garmin.com`.
- Обмен ключа на токен — `POST /personalapitoken-service/personal-api/personal-api-key/token`:
  - заголовки `Authorization: ApiKey <ключ>` и `Accept: */*`, тело пустое;
  - ответ — JWT обычным текстом;
  - срок жизни токена берётся из claim `exp`.
- Ключ может быть длиннее 128 символов (настоящий — 132). Нигде не обрезать.
- Секрет — всегда `secrecy::SecretString`. Он никогда не попадает в `Debug`, `Display`, `Serialize`, логи, ответы команд, тексты ошибок и argv.
- Имя в системном хранилище секретов: service `gymgar`, account `garmin-personal-api-key`. Файловое хранилище: `<data_dir>/secrets/garmin-personal-api-key`, права `0600` на Unix.
- Каталог данных:
  - релиз — `directories::ProjectDirs::from("", "", "Gymgar")`;
  - отладочная сборка (`cfg(debug_assertions)`) — `ProjectDirs::from("", "", "Gymgar Dev")`;
  - база — `<data_dir>/gymgar.sqlite3`;
  - логи — `<data_dir>/logs/gymgar.log`.
- SQLite: `journal_mode=WAL`, `busy_timeout=5000`, миграции через `PRAGMA user_version`, время хранится строками RFC 3339 в UTC.
- Срок ключа — `KeyTerm`: 30, 90, 180 или 365 дней, по умолчанию 365. Предупреждение показывается, когда до окончания ключа осталось 7 дней и меньше.
- Команды проекта 1 и их флаги — строго по таблице спеки, §7 «Команды»:
  - агенту закрыты `garmin.connect`, `garmin.verify`, `garmin.disconnect`;
  - `garmin.disconnect` — разрушительная.
- Параметры команд — structs с `#[serde(deny_unknown_fields, rename_all = "camelCase")]` и `schemars::JsonSchema`. Поле с `SecretString` получает `#[schemars(with = "String")]`.
- Все строки UI пишутся по-английски и обёрнуты в `tl!()`. Русские тексты — из спеки, §2.
- Стиль «Спорт»: токены и значения — из таблицы спеки, §6.
- Тесты не ходят в настоящий Garmin и в настоящее системное хранилище секретов. Исключение — тесты с `#[ignore]`, которые запускаются вручную.

## Review Focus

1. **Ключ, вставленный с пробелами или переводом строки по краям.** Перед обменом и сохранением ключ обрезается по краям (`trim`). Сохраняется уже обрезанный ключ. Тест в задаче 6.
2. **Ключ с не-ASCII или управляющими символами** (кириллица, `\n` в середине). Без сетевого запроса и без паники возвращается `GarminError::InvalidKey`, в UI это то же «Garmin не принял ключ». Тест в задаче 3.
3. **Двойной клик «Подключить» или повторный `garmin.connect`, пока задача ещё идёт.** Вторая команда отказывает: `enabled` возвращает причину «connection check already running». В UI кнопка неактивна. Тест в задаче 6.
4. **База заблокирована другим процессом дольше `busy_timeout`.** Команда возвращает `StoreError::Busy`, UI показывает ошибку, паники нет. Тест в задаче 4 (короткий таймаут в тесте).
5. **Каталог данных недоступен для записи или файл базы повреждён при первом запуске.** `Store::open` возвращает `Err` с путём, приложение показывает экран ошибки вместо паники. Тесты в задачах 4 и 10.

---

## Файловая структура

```text
Cargo.toml                                   # + workspace.dependencies
xtask/src/layers.rs                          # + secrets, gymgar-cli
crates/domain/src/{lib.rs, language.rs, garmin.rs}
crates/secrets/{Cargo.toml, src/{lib.rs, file.rs, system.rs}}
crates/garmin/src/{lib.rs, error.rs, personal.rs, personal/auth.rs, personal/jwt.rs}
crates/store/src/{lib.rs, error.rs, paths.rs, migrations.rs, settings.rs, garmin_connection.rs}
crates/engine/src/{lib.rs, error.rs, services.rs, commands.rs, params.rs, session.rs, jobs.rs,
                   settings_cmds.rs, garmin_cmds.rs, testing.rs}
crates/automation/src/{lib.rs, server.rs}
crates/ui-egui/src/{lib.rs, app.rs, state.rs, theme.rs, widgets.rs, icons.rs, i18n.rs,
                    rail.rs, summary_ui.rs, settings_ui.rs, garmin_ui.rs}
crates/ui-egui/i18n/ru.tsv
assets/icons/{house.svg, settings.svg, LICENSE-lucide.txt}
apps/gymgar/src/{main.rs, services.rs, logging.rs}
apps/gymgar-cli/{Cargo.toml, src/main.rs}
.github/workflows/ci.yml
README.md  ATTRIBUTION.md  CLAUDE.md  docs/architecture.md
```

---

### Task 1: доменные типы

**Files:**
- Create: `crates/domain/src/language.rs`, `crates/domain/src/garmin.rs`
- Modify: `crates/domain/src/lib.rs`, `crates/domain/Cargo.toml`, корневой `Cargo.toml` (добавить в `[workspace.dependencies]` `chrono = { version = "0.4.45", default-features = false, features = ["std", "clock"] }`)

**Interfaces:**
- Produces:
  - `domain::Language { System, En, Ru }`:
    - `as_str(&self) -> &'static str` — `"system"`, `"en"`, `"ru"`;
    - `parse(&str) -> Option<Language>`;
    - serde в виде строк в нижнем регистре.
  - `domain::KeyTerm { Days30, Days90, Days180, Days365 }`:
    - `days(&self) -> u16`;
    - `from_days(u16) -> Option<KeyTerm>`;
    - `ALL: [KeyTerm; 4]`;
    - `Default` — `Days365`.
  - `domain::ConnectionStatus { NotConnected, Connected, KeyRejected, KeyMissing, Unverified }` с `as_str` и `parse`. Значения: `"notConnected"`, `"connected"`, `"keyRejected"`, `"keyMissing"`, `"unverified"`.
  - `domain::SecretBackend { System, File }` с `as_str` и `parse` (`"system"`, `"file"`).
  - `domain::GarminConnection { connected_at: DateTime<Utc>, key_term: KeyTerm, key_expires_at: DateTime<Utc>, last_verified_at: Option<DateTime<Utc>>, last_status: ConnectionStatus, secret_backend: SecretBackend }`.
  - `domain::key_expires_at(connected_at: DateTime<Utc>, term: KeyTerm) -> DateTime<Utc>` — `connected_at + term.days()` дней.
  - `domain::expiry_warning(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool` — `true`, когда `expires_at - now <= 7 дней` (в том числе если срок уже прошёл).

- [ ] **Step 1: Написать падающие тесты** в `#[cfg(test)] mod tests` каждого модуля:
  - `key_term_days_and_default`: `KeyTerm::default() == Days365`, `Days30.days() == 30`, `from_days(90) == Some(Days90)`, `from_days(31) == None`.
  - `expires_after_term`: для `connected_at = 2026-10-23T12:00:00Z` и `Days365` получается `2027-10-23T12:00:00Z`.
  - `warning_threshold`: при `expires - now` = 8 дней — `false`; ровно 7 дней — `true`; 1 час — `true`; минус 1 день (уже истёк) — `true`.
  - `language_and_status_round_trip`: `parse(as_str(x)) == Some(x)` для всех вариантов `Language`, `ConnectionStatus`, `SecretBackend`; `Language::parse("de") == None`.
- [ ] **Step 2: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-domain`. Ожидается ошибка компиляции: типов ещё нет.
- [ ] **Step 3: Реализовать типы и функции** из блока Interfaces. Все вычисления — в `chrono`, без ручной календарной арифметики.
- [ ] **Step 4: Убедиться, что тесты проходят:** `cargo test -p gymgar-domain`.
- [ ] **Step 5: Коммит:** `git commit -m "Add domain types for Garmin connection and language"`

---

### Task 2: крейт `secrets` — хранилище ключа

**Files:**
- Create: `crates/secrets/Cargo.toml` (зависимости: `keyring`, `secrecy`, `thiserror`; dev — `tempfile`), `crates/secrets/src/lib.rs`, `crates/secrets/src/file.rs`, `crates/secrets/src/system.rs`
- Modify: корневой `Cargo.toml` — добавить `gymgar-secrets = { path = "crates/secrets" }`, `keyring = "4.2"`, `secrecy = { version = "0.10", features = ["serde"] }`, `tempfile = "3"`; `xtask/src/layers.rs` — строка `("secrets", Class::Standalone)` с комментарием

**Interfaces:**
- Produces (крейт `gymgar_secrets`):
  - `SecretsError { Unavailable(String), Io(String) }` (thiserror). В тексте ошибки никогда нет значения секрета.
  - `FileStore`:
    - `FileStore::new(path: PathBuf) -> FileStore`;
    - `get(&self) -> Result<Option<SecretString>, SecretsError>`;
    - `set(&self, secret: &SecretString) -> Result<(), SecretsError>`;
    - `delete(&self) -> Result<(), SecretsError>` — удаление отсутствующего файла даёт `Ok`.
  - `SystemStore` — тот же набор `get`/`set`/`delete`:
    - `SystemStore::new(service: &str, account: &str) -> SystemStore`;
    - `SystemStore::is_available() -> bool` — `keyring::Entry::store_status().is_ok()` плюс пробная запись, чтение и удаление записи с account `availability-probe`.
  - Константы `SERVICE = "gymgar"`, `GARMIN_ACCOUNT = "garmin-personal-api-key"`.

- [ ] **Step 1: Написать падающие тесты** в `file.rs`:
  - `file_round_trip`: `set`, затем `get` возвращает то же значение. Сравнивать через `expose_secret()`.
  - `file_get_missing_is_none`: `get` на несуществующем пути возвращает `Ok(None)`.
  - `file_delete_missing_is_ok`.
  - `file_keeps_long_keys`: ключ из 300 символов `[A-Za-z0-9._-]` сохраняется без обрезки.
  - `file_creates_parent_dirs`: путь `<tmp>/a/b/key` работает.
  - `#[cfg(unix)] file_mode_is_0600`: после `set` права файла `0o600`.
  - `debug_does_not_leak`: `format!("{:?}", SecretsError::Io(..))` не содержит значения секрета, а `format!("{:?}", secret)` не содержит его текста.
- [ ] **Step 2: Написать ручной тест** в `system.rs`: `#[ignore] system_store_round_trip_with_dummy`. Он работает с service `gymgar-test`, account `dummy` и значением `"dummy-" + 140×'x'`: `set`, `get` (сравнение), `delete`, затем `get == None`.
- [ ] **Step 3: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-secrets`.
- [ ] **Step 4: Реализовать `FileStore` и `SystemStore`.** Решения:
  - `FileStore::set` пишет во временный файл рядом и переименовывает (`rename`). На Unix права `0600` выставляются при создании через `OpenOptions` и `std::os::unix::fs::OpenOptionsExt::mode`.
  - Ошибка `keyring::Error::NoEntry` в `get` превращается в `Ok(None)`, в `delete` — в `Ok(())`.
- [ ] **Step 5: Прогнать тесты и проверки:** `cargo test -p gymgar-secrets`, затем `cargo xtask layers`. Ожидается PASS и `layers: … OK`.
- [ ] **Step 6: Запустить ручной тест на macOS** (фиктивный секрет, не ключ пользователя): `cargo test -p gymgar-secrets -- --ignored system_store_round_trip_with_dummy`. Ожидается PASS. Если macOS покажет диалог доступа, разрешить.
- [ ] **Step 7: Коммит:** `git commit -m "Add secrets crate: system keyring and file stores"`

---

### Task 3: `garmin::personal` — обмен ключа на токен

**Files:**
- Create: `crates/garmin/src/error.rs`, `crates/garmin/src/personal.rs`, `crates/garmin/src/personal/auth.rs`, `crates/garmin/src/personal/jwt.rs`
- Modify: `crates/garmin/Cargo.toml` (зависимости: `reqwest` с фичей `json`, `secrecy`, `chrono`, `base64`, `serde`, `serde_json`, `thiserror`, `tokio` с фичей `sync`; dev — `wiremock`, `tokio` с фичами `rt-multi-thread` и `macros`), `crates/garmin/src/lib.rs`, корневой `Cargo.toml`

**Interfaces:**
- Produces (крейт `gymgar_garmin`):
  - `GarminError { InvalidKey, KeyRejected, RateLimited, Unavailable { status: u16 }, Network(String), UnexpectedResponse(String) }` (thiserror). Ни в одном варианте нет ключа или токена.
  - `personal::DEFAULT_BASE_URL: &str = "https://personal.connectapi.garmin.com"`.
  - `personal::Auth`:
    - `Auth::new(key: SecretString) -> Result<Auth, GarminError>`;
    - `Auth::with_base_url(key: SecretString, base_url: &str) -> Result<Auth, GarminError>`;
    - `async fn exchange(&self) -> Result<TokenInfo, GarminError>` — всегда новый обмен; кэширует токен;
    - `async fn token(&self) -> Result<SecretString, GarminError>` — кэшированный токен, если до `exp` больше часа, иначе новый `exchange`.
  - `personal::TokenInfo { pub expires_at: DateTime<Utc> }`.
  - `personal::jwt::expires_at(token: &str) -> Result<DateTime<Utc>, GarminError>` — base64url-декодирование payload и чтение `exp`, без проверки подписи.

- [ ] **Step 1: Написать падающие тесты** в `auth.rs`. Используется `wiremock::MockServer`. Хелпер `fake_jwt(exp: i64) -> String` собирает `header.payload.sig` в base64url.
  - `exchange_ok_returns_expiry`: mock на `POST /personalapitoken-service/personal-api/personal-api-key/token` проверяет заголовки `Authorization: ApiKey k-123` и `Accept: */*` и отвечает 200 с `fake_jwt(T)`. `exchange()` возвращает `expires_at == T`.
  - `exchange_401_is_key_rejected`, `exchange_429_is_rate_limited`, `exchange_503_is_unavailable` (проверить `status == 503`).
  - `exchange_garbage_body_is_unexpected`: 200 с телом `not-a-jwt` возвращает `UnexpectedResponse`.
  - `exchange_timeout_is_network`: mock с `set_delay(2s)`, у `Auth` в тесте таймаут 1 с. Для этого есть `#[cfg(test)]` конструктор с таймаутом.
  - `long_key_is_sent_whole`: ключ из 300 символов доходит до mock целиком (matcher на полный заголовок).
  - `non_ascii_key_is_invalid_key_without_request`: ключ `"ключ\nabc"` возвращает `InvalidKey`; mock ожидает 0 запросов (`.expect(0)`).
  - `token_is_cached_until_one_hour_left`: при `exp = now + 24h` два вызова `token()` дают один запрос к mock (`.expect(1)`); при `exp = now + 30m` — два запроса.
  - `errors_do_not_contain_key`: для каждого варианта ошибки `format!("{e:?}")` и `format!("{e}")` не содержат `"k-123"`.
- [ ] **Step 2: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-garmin`.
- [ ] **Step 3: Реализовать `Auth`, `TokenInfo`, `jwt::expires_at`.** Решения:
  - один `reqwest::Client`: таймаут 30 с, `User-Agent: Gymgar/<CARGO_PKG_VERSION>`;
  - заголовок собирается через `HeaderValue::from_str`; ошибка превращается в `InvalidKey`;
  - кэш токена — `tokio::sync::Mutex<Option<(SecretString, DateTime<Utc>)>>`;
  - соответствие статусов: 401 → `KeyRejected`, 429 → `RateLimited`, 5xx → `Unavailable`, прочие не-2xx → `UnexpectedResponse("HTTP <код>")`, ошибки транспорта и таймаут → `Network`.

  Повтор после 401 на запросах данных из спеки (§7, «Garmin: авторизация») сознательно не делается: в проекте 1 запросов данных нет. Он добавится вместе с первым таким запросом в проекте 2.

  `reqwest` берётся с фичами по умолчанию (rustls и aws-lc-rs). Если aws-lc-rs не соберётся в CI на Windows, решение принимается в задаче 11: перейти на провайдер `ring` через `rustls-no-provider` и явную установку провайдера.
- [ ] **Step 4: Убедиться, что тесты проходят:** `cargo test -p gymgar-garmin`.
- [ ] **Step 5: Коммит:** `git commit -m "Add Garmin Personal API key-to-token exchange"`

---

### Task 4: `store` — база SQLite

**Files:**
- Create: `crates/store/src/error.rs`, `crates/store/src/paths.rs`, `crates/store/src/migrations.rs`, `crates/store/src/settings.rs`, `crates/store/src/garmin_connection.rs`
- Modify: `crates/store/Cargo.toml` (зависимости: `gymgar-domain`, `rusqlite` с фичей `bundled`, `chrono`, `directories`, `thiserror`; dev — `tempfile`), `crates/store/src/lib.rs`, корневой `Cargo.toml`

**Interfaces:**
- Consumes: из задачи 1 — `Language`, `GarminConnection`, `ConnectionStatus`, `KeyTerm`, `SecretBackend`.
- Produces (крейт `gymgar_store`):
  - `StoreError { Open { path: PathBuf, reason: String }, Busy, Corrupt(String), Sql(String) }` (thiserror).
  - `Store`:
    - `Store::open(path: &Path) -> Result<Store, StoreError>` — создаёт родительские каталоги, включает WAL и `busy_timeout` 5000, применяет миграции;
    - `Store::open_in_memory() -> Result<Store, StoreError>`;
    - `#[cfg(test)] Store::open_with_timeout(path, Duration)`.
  - Методы `Store`:
    - `language(&self) -> Result<Language, StoreError>` — по умолчанию `System`;
    - `set_language(&self, l: Language) -> Result<(), StoreError>`;
    - `garmin_connection(&self) -> Result<Option<GarminConnection>, StoreError>`;
    - `save_garmin_connection(&self, c: &GarminConnection) -> Result<(), StoreError>` — upsert единственной строки `id = 1`;
    - `delete_garmin_connection(&self) -> Result<(), StoreError>`;
    - `data_version(&self) -> Result<i64, StoreError>` — `PRAGMA data_version`.
  - `paths::data_dir() -> Result<PathBuf, StoreError>` и `paths::db_path(data_dir: &Path) -> PathBuf`. Имена каталогов — по Global Constraints.
- Схема миграции 1:
  - `app_settings(key TEXT PRIMARY KEY, value TEXT NOT NULL)`;
  - `garmin_connection(id INTEGER PRIMARY KEY CHECK (id = 1), connected_at TEXT NOT NULL, key_term_days INTEGER NOT NULL, key_expires_at TEXT NOT NULL, last_verified_at TEXT, last_status TEXT NOT NULL, secret_backend TEXT NOT NULL)`.

- [ ] **Step 1: Написать падающие тесты:**
  - `migrates_fresh_db_and_reopens`: `open` на пути в `tempdir` даёт `user_version == 1`; повторный `open` на том же пути возвращает `Ok`.
  - `language_defaults_to_system_and_persists`.
  - `garmin_connection_round_trip_and_delete`: `save`, затем `get` возвращает то же; `delete`, затем `get` возвращает `None`.
  - `second_connection_sees_writes_and_version_changes`: две `Store` на одном файле; запись через первую меняет `data_version` у второй, и вторая читает новое значение.
  - `busy_is_reported_not_panicked` (Review Focus 4): первая соединение держит `BEGIN IMMEDIATE`; вторая, открытая с таймаутом 100 мс, при `set_language` получает `StoreError::Busy`.
  - `corrupt_file_is_error` (Review Focus 5): файл с мусорными байтами — `open` возвращает `Err(Corrupt | Open)`.
  - `#[cfg(unix)] unwritable_dir_is_error` (Review Focus 5): каталог `0o500` — `open` возвращает `Err(Open { path, .. })` с этим путём.
  - `bad_stored_values_are_errors`: строка в `garmin_connection` с `last_status = 'xyz'` даёт `Err(Corrupt)`, не панику.
  - `debug_build_uses_dev_dir`: тесты собираются с `debug_assertions`, поэтому `paths::data_dir()` (если ОС отдаёт домашний каталог) оканчивается на `Gymgar Dev` (macOS и Windows) или `gymgar-dev` (Linux).
- [ ] **Step 2: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-store`.
- [ ] **Step 3: Реализовать.** Миграции — `const MIGRATIONS: &[&str]`, номер `i` означает `user_version = i + 1`. Каждая выполняется в своей транзакции. Коды ошибок `SQLITE_BUSY` и `SQLITE_LOCKED` превращаются в `Busy`, `SQLITE_NOTADB` и `SQLITE_CORRUPT` — в `Corrupt`.
- [ ] **Step 4: Убедиться, что тесты проходят:** `cargo test -p gymgar-store`.
- [ ] **Step 5: Коммит:** `git commit -m "Add SQLite store with migrations, settings and Garmin connection"`

---

### Task 5: `engine` — сервисы, реестр команд, синхронные команды

**Files:**
- Create: `crates/engine/src/error.rs`, `services.rs`, `commands.rs`, `params.rs`, `session.rs`, `settings_cmds.rs`, `garmin_cmds.rs`, `testing.rs`
- Modify: `crates/engine/Cargo.toml` (зависимости: `gymgar-domain`, `gymgar-store`, `serde`, `serde_json`, `schemars`, `secrecy`, `chrono`, `thiserror`, `futures`; фича `testing = []`), `crates/engine/src/lib.rs`, корневой `Cargo.toml` (`schemars = "1.2"`, `futures = "0.3"`)

**Interfaces:**
- Consumes: задачи 1 и 4.
- Produces (крейт `gymgar_engine`):
  - `type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>`.
  - `trait SecretStore: Send + Sync`:
    - `backend(&self) -> SecretBackend`;
    - `get(&self) -> Result<Option<SecretString>, SecretStoreError>`;
    - `set(&self, key: &SecretString) -> Result<(), SecretStoreError>`;
    - `delete(&self) -> Result<(), SecretStoreError>`.
  - `SecretStoreError(pub String)` — сообщение без секрета.
  - `SecretStores { pub system: Option<Arc<dyn SecretStore>>, pub file: Arc<dyn SecretStore>, pub prefer_file: bool }`. В отладочной сборке `prefer_file = true`.
  - `trait GarminAuth: Send + Sync { fn check_key(&self, key: SecretString) -> BoxFuture<Result<(), GarminAuthError>>; }`.
  - `GarminAuthError { KeyRejected, RateLimited, Unavailable, Network, Unexpected }`.
  - `trait Spawner: Send + Sync { fn spawn(&self, fut: BoxFuture<()>); }`.
  - `Services`, все поля публичные:
    - `secrets: Option<SecretStores>`;
    - `garmin: Option<Arc<dyn GarminAuth>>`;
    - `spawner: Option<Arc<dyn Spawner>>`;
    - `notify: Arc<dyn Fn() + Send + Sync>`;
    - `clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>`.

    В `gymgar-cli` поля `secrets`, `garmin` и `spawner` равны `None`.
  - `Caller { User, Agent }`.
  - `CommandKind { Query, Mutation, Job }`.
  - `Outcome { Done(Value), Started(JobId) }`, `JobId(pub u64)`.
  - `CommandSpec`:
    - `id: &'static str`;
    - `description: &'static str` — английский текст для MCP;
    - `kind: CommandKind`;
    - `agent: bool`;
    - `destructive: bool`;
    - `params_schema: fn() -> schemars::Schema`;
    - `enabled: fn(&Session) -> Result<(), String>`;
    - `run: fn(&mut Session, Value) -> Result<Outcome, EngineError>`.
  - `commands::all() -> Vec<&'static CommandSpec>` и `commands::find(id: &str) -> Option<&'static CommandSpec>`.
  - `params::parse<P: DeserializeOwned>(cmd: &str, v: Value) -> Result<P, EngineError>`. `Value::Null` трактуется как `{}`.
  - `EngineError` (thiserror):
    - `UnknownCommand(String)`;
    - `BadParams { cmd: String, msg: String }`;
    - `AgentNotAllowed(String)`;
    - `Disabled { cmd: String, reason: String }`;
    - `Store(String)`;
    - `Secrets(String)`;
    - `Garmin(GarminAuthError)`;
    - `FileStoreConsentRequired`;
    - `NotConnected`;
    - `KeyMissing`;
    - `JobPanicked(String)`.
  - `Session`:
    - `Session::new(store: Store, services: Services) -> Session`;
    - `execute(&mut self, id: &str, params: Value, caller: Caller) -> Result<Outcome, EngineError>`. Порядок проверок: `find` → `agent` (для `Caller::Agent`) → `enabled` → `run`;
    - `store_changed(&mut self) -> bool` — сравнивает `data_version` с прошлым значением;
    - `language(&self) -> Result<Language, EngineError>`.
  - Ответ `garmin.status` — `GarminStatusView`, сериализуется в camelCase:
    - `status: ConnectionStatus`;
    - `keyExpiresAt: Option<DateTime<Utc>>`;
    - `lastVerifiedAt: Option<DateTime<Utc>>`;
    - `expiryWarning: bool`;
    - `secretBackend: Option<SecretBackend>`;
    - `systemStoreAvailable: bool`;
    - `checkRunning: bool`.
  - Команды этой задачи — `settings.get`, `settings.setLanguage { language }`, `garmin.status`, `garmin.disconnect`. Флаги — по таблице спеки.
  - `#[cfg(feature = "testing")] testing`:
    - `MemorySecretStore::new(backend)`;
    - `FakeGarminAuth::returning(Result<(), GarminAuthError>)` и `FakeGarminAuth::panicking()`; `last_key(&self) -> Option<String>` возвращает ключ, с которым был последний вызов;
    - `InlineSpawner` — выполняет future сразу через `futures::executor::block_on`;
    - `DeferredSpawner` — копит future до явного `run_pending(&self)`;
    - `fixed_clock(DateTime<Utc>)`;
    - `TestConfig` (`Default`): `garmin: Result<(), GarminAuthError>` или `panic`, `system_store: bool` (по умолчанию `true`), `prefer_file: bool` (по умолчанию `false`), `deferred: bool`, `now: DateTime<Utc>` (по умолчанию `2026-10-23T12:00:00Z`);
    - `test_session_with(cfg: TestConfig) -> (Session, TestHandles)`, где `TestHandles` даёт доступ к `system: Option<Arc<MemorySecretStore>>`, `file: Arc<MemorySecretStore>`, `garmin: Arc<FakeGarminAuth>` и `spawner: Option<Arc<DeferredSpawner>>`;
    - `test_session() -> Session` — то же, что `test_session_with(TestConfig::default()).0`.

- [ ] **Step 1: Написать падающие тесты** (`session.rs`, `settings_cmds.rs`, `garmin_cmds.rs`):
  - `unknown_command_is_error`.
  - `bad_params_are_errors`: `settings.setLanguage` с `{"language":"de"}`, с `{"lang":"ru"}` (неизвестный ключ) и с `42` возвращает `BadParams` во всех трёх случаях.
  - `set_language_persists`: после `setLanguage ru` `settings.get` возвращает `{"language":"ru"}`.
  - `status_not_connected_by_default`: `status == notConnected`, `expiryWarning == false`.
  - `status_reports_expiry_warning`: соединение сохранено с `key_expires_at = clock + 5 дней`, `expiryWarning == true`.
  - `agent_cannot_disconnect`: `execute("garmin.disconnect", Null, Caller::Agent)` возвращает `AgentNotAllowed`; соединение и ключ остаются на месте.
  - `disconnect_removes_key_and_row`: после `disconnect` хранилище, записанное в `secret_backend`, пусто, а `garmin_connection == None`.
  - `disconnect_when_not_connected_is_disabled`: `Disabled` с причиной.
  - `registry_flags_match_spec`: `agent == false` у `garmin.connect`, `garmin.verify`, `garmin.disconnect`; `destructive` только у `disconnect`; у каждой команды `params_schema()` — объект.
- [ ] **Step 2: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-engine --features testing`.
- [ ] **Step 3: Реализовать** типы и команды из блока Interfaces. Каждый модуль `<area>_cmds.rs` отдаёт `specs() -> Vec<CommandSpec>`. В `commands::all()` они собираются через `v.extend(...)`.
- [ ] **Step 4: Убедиться, что тесты проходят:** `cargo test -p gymgar-engine --features testing`, затем `cargo xtask layers`.
- [ ] **Step 5: Коммит:** `git commit -m "Add engine session, typed command registry and settings/status commands"`

---

### Task 6: `engine` — фоновые задачи, `garmin.connect` и `garmin.verify`

**Files:**
- Create: `crates/engine/src/jobs.rs`
- Modify: `crates/engine/src/session.rs`, `crates/engine/src/garmin_cmds.rs`

**Interfaces:**
- Consumes: задача 5.
- Produces:
  - `Session::poll_jobs(&mut self) -> Vec<JobFinished>`. Применяет готовые результаты на вызывающем потоке.
  - `JobFinished { pub id: JobId, pub command: &'static str, pub result: Result<Value, EngineError> }`.
  - Параметры `garmin.connect`: `ConnectParams { api_key: SecretString, key_term: KeyTerm, allow_file_store: bool }`. В JSON — `apiKey`, `keyTerm` (число дней), `allowFileStore` (по умолчанию `false`).
  - Ход задачи. Future вызывает `GarminAuth::check_key` и возвращает внутренний `JobOutput`:
    - `ConnectChecked { key, term, result }`;
    - `VerifyChecked { result }`.

    `poll_jobs` применяет его на UI-потоке.
  - Подключение (`connect`) при успехе:
    - пишет ключ в выбранное хранилище;
    - сохраняет `GarminConnection` со статусом `Connected` и `last_verified_at = clock`.
  - Подключение при ошибке ничего не сохраняет.
  - Проверка (`verify`) обновляет `last_status` и `last_verified_at`:
    - `KeyRejected` → `keyRejected`;
    - `Network` или `Unavailable` → `unverified` (время последней проверки не меняется);
    - ключа нет в хранилище из `secret_backend` → `keyMissing` без сетевого запроса.
  - Пока идёт задача Garmin, `enabled` у `garmin.connect`, `garmin.verify` и `garmin.disconnect` возвращает причину `"connection check already running"`. В `garmin.status` при этом `checkRunning == true`.
  - Выбор хранилища для `connect`:
    - `prefer_file` или нет `system` → файл;
    - иначе системное, если оно доступно;
    - если системное недоступно и `allow_file_store == false` → `Err(FileStoreConsentRequired)` до сетевого запроса.

- [ ] **Step 1: Написать падающие тесты** (через `testing::test_session` и `InlineSpawner`):
  - `connect_success_stores_key_and_row`: `FakeGarminAuth` возвращает `Ok`. `execute` даёт `Started`, `poll_jobs` возвращает один `Ok`. Ключ в памяти совпадает, `status == connected`, `keyExpiresAt == clock + 365 дней`.
  - `connect_rejected_saves_nothing`: `KeyRejected` даёт `Err(Garmin(KeyRejected))`; хранилище пусто, строки в базе нет.
  - `connect_trims_whitespace` (Review Focus 1): `apiKey = "  k-123\n"` — в хранилище `"k-123"`, и `check_key` получил `"k-123"` (`FakeGarminAuth` записывает полученный ключ).
  - `connect_twice_is_disabled` (Review Focus 3): без `poll_jobs` между вызовами второй `connect` возвращает `Disabled` с `"connection check already running"`.
  - `connect_requires_file_consent`: `system = None`, `prefer_file = false`, `allowFileStore = false` → `FileStoreConsentRequired`, сетевого вызова нет. С `allowFileStore = true` → успех и `secretBackend == file`.
  - `verify_key_missing`: строка есть, хранилище пусто → `status == keyMissing`, `check_key` не вызывался.
  - `verify_reads_recorded_backend`: строка записана с `secret_backend = file`, ключ лежит только в системном хранилище → `keyMissing`. Хранилище выбирается по базе, а не заново.
  - `verify_network_is_unverified`: `Network` → `status == unverified`, `last_verified_at` прежнее.
  - `verify_rejected`: `KeyRejected` → `status == keyRejected`, ключ не удалён.
  - `job_panic_is_error`: `FakeGarminAuth::panicking()` → `JobFinished.result == Err(JobPanicked)`, сессия продолжает работать (следующий `garmin.status` → `Ok`).
  - `connect_result_has_no_key`: сериализованный результат `connect` и `format!("{:?}", params)` не содержат `"k-123"`.
- [ ] **Step 2: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-engine --features testing`.
- [ ] **Step 3: Реализовать `jobs.rs`.** Решения:
  - `std::sync::mpsc` канал от задач к сессии;
  - future оборачивается в `AssertUnwindSafe(..).catch_unwind()` из `futures::FutureExt`;
  - после отправки результата вызывается `services.notify`.

  Затем реализовать команды из блока Interfaces.

  `Session::wait_job` для `gymgar-cli` из спеки (§7, «Фоновые задачи») сознательно не делается: в проекте 1 ни одна команда-задача агенту не доступна. Если `gymgar-cli` получит `Outcome::Started`, это внутренняя ошибка, а не ожидание.
- [ ] **Step 4: Убедиться, что тесты проходят:** `cargo test -p gymgar-engine --features testing`.
- [ ] **Step 5: Коммит:** `git commit -m "Add background jobs and garmin.connect / garmin.verify"`

---

### Task 7: MCP — `automation` и `apps/gymgar-cli`

**Files:**
- Create: `crates/automation/src/server.rs`, `apps/gymgar-cli/Cargo.toml`, `apps/gymgar-cli/src/main.rs`
- Modify: `crates/automation/Cargo.toml` (зависимости: `gymgar-engine`, `rmcp` с фичами `server`, `macros`, `transport-io`, `tokio` с фичами `rt-multi-thread`, `macros`, `io-std`, `sync`, `serde_json`, `thiserror`; dev — `rmcp` с фичей `client`, `gymgar-engine` с фичей `testing`), `crates/automation/src/lib.rs`, `xtask/src/layers.rs` (строка `("gymgar-cli", Class::Exempt)`), корневой `Cargo.toml` (`rmcp = { version = "3.7", default-features = false }`, `tokio = "1.54"`, `gymgar-automation`)

**Interfaces:**
- Consumes: `Session`, `Caller::Agent`, `commands::all()` из задач 5 и 6.
- Produces:
  - `gymgar_automation::GymgarMcp::new(session: Session) -> GymgarMcp` — реализует `rmcp::ServerHandler`.
    - `list_tools` возвращает ровно `command_list` и `command_run`.
    - `command_list` принимает `{ filter?: string }` и возвращает массив `{ id, description, kind, destructive, enabled, disabledReason, paramsSchema }`, только команды с `agent == true`.
    - `command_run` принимает `{ id: string, params?: object }`. Ошибка `EngineError` возвращается как `CallToolResult` с `is_error = true`.
  - `gymgar_automation::serve_stdio(session: Session) -> Result<(), AutomationError>`.
  - Бинарь `gymgar-cli`:
    - `gymgar-cli mcp` — открывает `Store::open(&paths::db_path(&paths::data_dir()?))`, `Services` без секретов, Garmin и spawner, затем `serve_stdio`;
    - `gymgar-cli --help` или неизвестная подкоманда — usage в stderr и код выхода 2.

- [ ] **Step 1: Написать падающие тесты** в `server.rs`. Используется in-process транспорт: `tokio::io::duplex` и клиент `rmcp`, сессия из `testing::test_session()`.
  - `lists_two_tools`.
  - `command_list_hides_agent_forbidden`: в списке есть `garmin.status`, `settings.get`, `settings.setLanguage` и нет `garmin.connect`, `garmin.verify`, `garmin.disconnect`.
  - `command_run_forbidden_is_tool_error`: `command_run` для `garmin.disconnect` даёт `is_error == true`, соединение не тронуто.
  - `unknown_tool_argument_is_invalid_params`: `command_run` с `{ "id": "settings.get", "extra": 1 }` возвращает JSON-RPC ошибку с кодом `-32602`.
  - `set_language_via_mcp`: `command_run settings.setLanguage {"language":"ru"}`, затем `settings.get` → `"ru"`.
  - `engine_bad_params_is_tool_error`: `settings.setLanguage {"language":"de"}` даёт `is_error == true` и текст с `BadParams`.
- [ ] **Step 2: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-automation`.
- [ ] **Step 3: Реализовать.** Решения:
  - `GymgarMcp` держит `Arc<std::sync::Mutex<Session>>`;
  - `execute` выполняется внутри `tokio::task::spawn_blocking`;
  - паника превращается в ошибку инструмента;
  - отравленный mutex восстанавливается через `PoisonError::into_inner`;
  - аргументы инструментов разбираются в structs с `deny_unknown_fields`, ошибка разбора → `ErrorData::invalid_params`.
- [ ] **Step 4: Убедиться, что тесты проходят:** `cargo test -p gymgar-automation`, затем `cargo xtask layers`.
- [ ] **Step 5: Ручная проверка через stdio:**

  ```bash
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' '{"jsonrpc":"2.0","method":"notifications/initialized"}' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | cargo run -q -p gymgar-cli -- mcp
  ```

  Ожидается ответ с `id: 2`, в котором есть `command_list` и `command_run`. Если `rmcp` не принимает эту версию протокола, взять ту, что он объявляет в ответе на `initialize`.
- [ ] **Step 6: Коммит:** `git commit -m "Add MCP server with command_list/command_run and gymgar-cli mcp"`

---

### Task 8: `ui-egui` — тема, виджеты, переводы, иконки, каркас окна

**Files:**
- Create: `crates/ui-egui/src/{app.rs, state.rs, theme.rs, widgets.rs, icons.rs, i18n.rs, rail.rs}`, `crates/ui-egui/i18n/ru.tsv`, `assets/icons/house.svg`, `assets/icons/settings.svg`, `assets/icons/LICENSE-lucide.txt`, `ATTRIBUTION.md`
- Modify: `crates/ui-egui/src/lib.rs` (весь текущий заглушечный код переезжает в `app.rs`), `crates/ui-egui/Cargo.toml` (зависимости: `gymgar-engine`, `gymgar-domain`, `egui_extras` с фичей `svg`, `serde`, `serde_json`, `sys-locale`, `chrono`, `log`; dev — `egui_kittest` с фичами `wgpu` и `snapshot`, `gymgar-engine` с фичей `testing`), корневой `Cargo.toml`

**Interfaces:**
- Consumes: `Session`, `Caller::User`, `Outcome`, `Services` из задач 5 и 6.
- Produces:
  - `theme::Tokens` — поля и значения из таблицы спеки, §6, тип `egui::Color32` и `f32`. Также `Tokens::SPORT: Tokens`, `theme::apply(ctx: &egui::Context, t: &Tokens)` и `Tokens::get(ctx) -> Tokens`.
  - `i18n`:
    - `Lang { En, Ru }`;
    - `i18n::resolve(pref: Language, system_locale: Option<&str>) -> Lang` — `System` смотрит на префикс `ru` в локали, всё остальное даёт `En`;
    - `i18n::set_current(Lang)`;
    - `i18n::t(&'static str) -> &'static str`;
    - макрос `tl!("…")`;
    - `trn(n: u64, one, few, many) -> String` — для `En`: `n == 1` → `one`, иначе `many`; для `Ru` — правило one, few, many.
  - Каталог `ru.tsv`: строка `английский<TAB>русский`, комментарии через `#`. Встраивается через `include_str!` и разбирается в `OnceLock<HashMap>`.
  - `widgets`, каждая функция принимает `&mut egui::Ui`:
    - `primary_button(ui, &str) -> Response`;
    - `secondary_button(ui, &str) -> Response`;
    - `card<R>(ui, add: impl FnOnce(&mut Ui) -> R) -> R`;
    - `secret_field(ui, id: egui::Id, buf: &mut String, width: f32) -> Response` — `TextEdit` с `password(true)` и без ограничения длины;
    - `dropdown<T: PartialEq + Copy>(ui, id, current: &mut T, items: &[(T, &str)]) -> bool`;
    - `status_dot(ui, color, &str)`;
    - `warning_box(ui, &str)`;
    - `link(ui, &str, url: &str)` — открывает URL через `ui.ctx().open_url`.
  - `icons::install(ctx)` — загрузчики `egui_extras`. `icons::icon(Icon::House | Icon::Settings) -> egui::Image`.
  - `state::UiState { section: Section, connect: ConnectForm }` (serde):
    - `Section { Summary, Settings }`;
    - поля `ConnectForm`:
      - `key: String` с `#[serde(skip)]`;
      - `term: KeyTerm`;
      - `allow_file_store: bool`;
      - `error: Option<String>` с `#[serde(skip)]`;
      - `pending: Option<JobId>` с `#[serde(skip)]`;
      - `replacing: bool`;
      - `confirm_disconnect: bool`.
  - `GymgarApp`:
    - `GymgarApp::new(session: Session, ui: UiState) -> GymgarApp`;
    - `show(&mut self, ui: &mut egui::Ui)` — полоса иконок и текущий раздел;
    - `run(&mut self, id: &str, params: Value) -> Result<Outcome, String>` — единственная точка вызова команд из UI; ошибка превращается в переведённый текст по варианту `EngineError`;
    - `tick(&mut self, ctx: &egui::Context)` — вызывается из `eframe::App::logic`:
      - `poll_jobs`;
      - раз в секунду `store_changed()`, при изменении — перечитать язык и статус;
      - один раз при старте `garmin.verify`, если статус не `notConnected`.

- [ ] **Step 1: Написать падающие тесты:**
  - `i18n_resolve`: `resolve(System, Some("ru-RU")) == Ru`; `resolve(System, Some("de-DE")) == En`; `resolve(Ru, Some("en-US")) == Ru`; `resolve(System, None) == En`.
  - `i18n_plural_ru`: `trn(1)` → `one`, `trn(3)` → `few`, `trn(5)` → `many`, `trn(11)` → `many`, `trn(21)` → `one`, `trn(22)` → `few`.
  - `i18n_every_tl_literal_is_translated`: обходит `CARGO_MANIFEST_DIR/src/**/*.rs`, вынимает литералы `tl!("…")` (простой сканер по подстроке `tl!("` до закрывающей неэкранированной кавычки) и проверяет, что каждый есть в `ru.tsv`. Ещё проверяет, что в `ru.tsv` нет дублей.
  - `rail_shows_summary_and_settings` (kittest, `Harness::new_ui_state` с `GymgarApp`, построенным из `testing::test_session()`): есть узлы с подписями (tooltip или accesskit-label) `Summary` и `Settings`. Клик по `Settings` переключает `ui.section`.
  - `language_switch_relabels`: после `run("settings.setLanguage", {"language":"ru"})` и `tick` есть узел `Сводка`.
- [ ] **Step 2: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-ui-egui`.
- [ ] **Step 3: Реализовать.** Иконки `house.svg` и `settings.svg` взять из репозитория Lucide (ISC), лицензию положить в `assets/icons/LICENSE-lucide.txt`. В `ATTRIBUTION.md` добавить таблицу: путь, название, автор, источник, лицензия. Заглушки эмодзи из макетов не использовать. Шрифт — по умолчанию egui. Если кириллица в нём не отображается (проверить снапшотом в задаче 9), подключить Inter (OFL) и добавить строку в `ATTRIBUTION.md`.
- [ ] **Step 4: Убедиться, что тесты проходят:** `cargo test -p gymgar-ui-egui`.
- [ ] **Step 5: Коммит:** `git commit -m "Add UI shell: Sport theme, widgets, i18n, icon rail"`

---

### Task 9: `ui-egui` — экраны «Сводка» и «Настройки → Garmin»

**Files:**
- Create: `crates/ui-egui/src/{summary_ui.rs, settings_ui.rs, garmin_ui.rs}`
- Modify: `crates/ui-egui/src/app.rs`, `crates/ui-egui/i18n/ru.tsv`

**Interfaces:**
- Consumes: всё из задачи 8, `GarminStatusView` из задачи 5, `ConnectParams` из задачи 6.
- Produces: экраны строго по спеке, §2, пункты 1–10, с текстами оттуда же:
  - `summary_ui::show(app, ui)`: карточка «Подключите Garmin Connect», текст «Garmin подключён…», предупреждение о сроке;
  - `garmin_ui::show(app, ui)`:
    - инструкция из четырёх шагов со ссылкой `https://connect.garmin.com/app/settings/personalApiToken`;
    - поле ключа;
    - выбор срока;
    - «Подключить» с индикатором, пока `pending`;
    - строка «где хранится ключ» по `secretBackend` или `systemStoreAvailable`;
    - предупреждение о файле с согласием (галочка, которая выставляет `allow_file_store`), если системное хранилище недоступно;
    - ошибки по вариантам `GarminAuthError`;
    - состояния `connected`, `keyRejected`, `keyMissing`, `unverified`;
    - «Заменить ключ»;
    - «Отключить» с подтверждением;
  - `settings_ui::show(app, ui)`: карточка Garmin и карточка «Язык интерфейса» с вариантами «Системный», «English», «Русский».

- [ ] **Step 1: Написать падающие тесты** (kittest, без GPU). Каждый — в обоих языках, через цикл по `Lang`:
  - `first_run_summary_has_connect_card`: есть кнопка `Connect Garmin` (`Подключить Garmin`), клик переключает раздел на Settings.
  - `connect_flow_success`: ввод ключа в поле, клик `Connect`, `tick`. Видно `Connected`, нет текста ключа ни в одном узле accesskit.
  - `connect_flow_rejected_shows_error`: `FakeGarminAuth` → `KeyRejected`. Виден текст ошибки из спеки, §2, пункт 3, состояние остаётся «не подключено».
  - `connect_button_disabled_while_pending`: `TestConfig { deferred: true, .. }`, задача не выполняется до `run_pending()`. Кнопка `Connect` неактивна.
  - `connected_summary_text`: при статусе `connected` на «Сводке» есть текст из спеки, §2, пункт 9.
  - `startup_verify_runs_once_when_connected`: соединение сохранено, `deferred: true`. После трёх `tick` в очереди `DeferredSpawner` ровно одна задача (`garmin.verify`). При `notConnected` очередь пуста.
  - `expiry_warning_on_summary`: соединение с окончанием через 3 дня, на «Сводке» есть текст предупреждения.
  - `key_missing_and_rejected_states_render`: для статусов `keyMissing`, `keyRejected`, `unverified` есть соответствующие тексты.
  - `disconnect_asks_confirmation`: первый клик `Disconnect` показывает подтверждение, ключ ещё на месте. Подтверждение — соединение удалено.
  - `file_store_consent_required`: `system = None`, `prefer_file = false`. Без галочки после клика `Connect` видно предупреждение о файле. С галочкой — `Connected`.
- [ ] **Step 2: Написать ручной тест снапшотов** `#[ignore] render_snapshots`. Если задана переменная `GYMGAR_SNAPSHOTS=<dir>`, тест рендерит через `egui_kittest` с `wgpu` в PNG при размере 1200×800 и масштабе 2, для `en` и `ru`, состояния: пустая сводка, форма с ошибкой, подключено, предупреждение о сроке.
- [ ] **Step 3: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar-ui-egui`.
- [ ] **Step 4: Реализовать экраны.** Русские строки добавить в `ru.tsv`. Тест покрытия переводов из задачи 8 должен проходить.
- [ ] **Step 5: Убедиться, что тесты проходят:** `cargo test -p gymgar-ui-egui`.
- [ ] **Step 6: Отрендерить и посмотреть снапшоты:** `GYMGAR_SNAPSHOTS=/tmp/gymgar-shots cargo test -p gymgar-ui-egui -- --ignored render_snapshots`. Открыть PNG и сверить с макетами из спеки (стиль «Спорт», раскладка карточек). Проверить, что кириллица отображается без «квадратиков». Поправить, если не так.
- [ ] **Step 7: Коммит:** `git commit -m "Add Summary and Settings/Garmin screens"`

---

### Task 10: `apps/gymgar` — сборка сервисов, tokio, логи

**Files:**
- Create: `apps/gymgar/src/services.rs`, `apps/gymgar/src/logging.rs`
- Modify: `apps/gymgar/src/main.rs`, `apps/gymgar/Cargo.toml` (зависимости: `gymgar-engine`, `gymgar-store`, `gymgar-secrets`, `gymgar-garmin`, `tokio` с фичей `rt-multi-thread`, `secrecy`, `chrono`, `log`, `sys-locale`)

**Interfaces:**
- Consumes: трейты из задачи 5, `secrets` (задача 2), `garmin::personal::Auth` (задача 3), `Store` и `paths` (задача 4), `GymgarApp` (задача 8).
- Produces:
  - `services::SystemSecretStore(gymgar_secrets::SystemStore)` и `services::FileSecretStore(gymgar_secrets::FileStore)` — реализуют `engine::SecretStore`.
  - `services::GarminPersonalAuth` — реализует `engine::GarminAuth` через `Auth::new(key)?.exchange()`. Соответствие ошибок: `InvalidKey` → `KeyRejected`, `Network` → `Network`, остальные варианты — один к одному или в `Unexpected`.
  - `services::TokioSpawner(tokio::runtime::Handle)` — реализует `engine::Spawner`.
  - `services::build(data_dir: &Path, ctx: egui::Context, rt: &tokio::runtime::Runtime) -> Services`. `notify` вызывает `ctx.request_repaint()`. `prefer_file = cfg!(debug_assertions)`.
  - `logging::install(data_dir: &Path)`:
    - пишет в stderr и в `logs/gymgar.log`;
    - при старте переименовывает прежний лог в `gymgar.1.log`;
    - уровень по умолчанию — `info` для `gymgar*` и `warn` для остальных;
    - `RUST_LOG` переопределяет уровень;
    - крейт логгера выбирает исполнитель по правилам CLAUDE.md.
  - Поведение `main`:
    1. Найти `data_dir`.
    2. Установить логгер.
    3. Создать tokio runtime на 2 рабочих потока.
    4. Открыть `Store`. Если не получилось, запустить окно с одним экраном ошибки: текст `StoreError` и путь к файлу. Без паники (Review Focus 5).
    5. Собрать `Session` и `GymgarApp`, применить тему и `icons::install`, запустить `run_native`.

- [ ] **Step 1: Написать падающие тесты** в `services.rs`:
  - `garmin_error_mapping`: каждый вариант `GarminError` превращается в ожидаемый `GarminAuthError`.
  - `file_adapter_round_trip`: `FileSecretStore` во `tempdir` — `set`, `get`, `delete`, `backend() == File`.
- [ ] **Step 2: Запустить тесты и убедиться, что падают:** `cargo test -p gymgar`.
- [ ] **Step 3: Реализовать** по блоку Interfaces.
- [ ] **Step 4: Убедиться, что тесты проходят:** `cargo test -p gymgar`.
- [ ] **Step 5: Ручная проверка отладочной сборки** (без настоящего ключа): `cargo run -p gymgar`.
  - На пустой сводке есть карточка подключения.
  - В «Настройках» ввести `not-a-real-key` и нажать «Подключить». Должна появиться ошибка «Garmin не принял ключ…», окно при этом не замирает.
  - Переключить язык.
  - Перезапустить приложение: язык сохранился.
  - Из второго терминала выполнить `gymgar-cli mcp`, вызвать `command_run settings.setLanguage`. Открытое окно должно переключить язык примерно за секунду.
  - В логе `<data_dir>/logs/gymgar.log` нет `not-a-real-key`. Проверить командой `grep -c not-a-real-key <log>`, ожидается `0`.
- [ ] **Step 6: Коммит:** `git commit -m "Wire desktop app: services, tokio runtime, logging, store errors"`

---

### Task 11: CI, README, документация, чеклист для пользователя

**Files:**
- Create: `.github/workflows/ci.yml`, `README.md`
- Modify: `CLAUDE.md`, `docs/architecture.md`

**Interfaces:**
- Consumes: всё предыдущее.
- Produces:
  - **CI** — workflow `CI` на `push` и `pull_request`:
    - матрица `ubuntu-latest`, `windows-latest`, `macos-latest`;
    - шаги: `actions/checkout`, `dtolnay/rust-toolchain@stable` с `components: rustfmt, clippy`, `Swatinem/rust-cache`;
    - на Linux: `sudo apt-get install -y libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libdbus-1-dev pkg-config` (точный список — по ошибкам первого прогона);
    - затем `cargo xtask ci`.
  - **README.md**:
    - что такое Gymgar (EN и RU);
    - сборка: `cargo run --release -p gymgar`;
    - MCP: `gymgar-cli mcp` и пример регистрации в Claude Code: `claude mcp add gymgar -- <путь>/gymgar-cli mcp`;
    - получение ключа Garmin: четыре шага из спеки со ссылкой;
    - предупреждение: в терминале macOS не вводить ключ через `security` с интерактивным запросом, потому что он обрезает строку до 128 символов;
    - лицензия.
  - **CLAUDE.md:**
    - карта воркспейса (`secrets`, `gymgar-cli`);
    - правило «секрет — только `secrecy::SecretString`, в параметрах команд с `#[schemars(with = "String")]`»;
    - запасное файловое хранилище и `Gymgar Dev` для отладочных сборок;
    - CI на трёх ОС;
    - строка в таблице библиотек про `secrecy`.
  - **docs/architecture.md:**
    - платформы — Windows, Linux, macOS;
    - решения 13–15 (окно — вариант C, стиль «Спорт», хранение ключа с запасным файлом);
    - уточнение к решению 1 про параллельную запись в проекте 1;
    - `secrets` и `gymgar-cli` в §4 «Слои»;
    - факты Personal API из спеки, §5;
    - запись в истории изменений.

- [ ] **Step 1: Создать workflow, README и обновить документы** по блоку Produces.
- [ ] **Step 2: Прогнать полную проверку:** `cargo xtask ci`. Ожидается всё зелёное и `layers: 11 crates OK`.
- [ ] **Step 3: Коммит:** `git commit -m "Add CI on three platforms, README and docs for project 1"`
- [ ] **Step 4: Пуш и проверка CI.** Только с разрешения пользователя: пушит ветку исполнитель или сам пользователь. Убедиться, что все три job зелёные, иначе поправить список пакетов для Linux и прочее, что упало.
- [ ] **Step 5: Передать пользователю чеклист релизной сборки на macOS** (спека, §10). Строитель его не выполняет:
  1. `cargo run --release -p gymgar`.
  2. Подключить настоящим ключом со сроком, который выбран на сайте. Должно появиться «Подключено», «Ключ действует до…» и «Проверен…».
  3. Перезапустить приложение. Статус сохранился, появилась новая отметка «Проверен…».
  4. «Заменить ключ», вставить тот же ключ: снова «Подключено».
  5. Переключить язык на «Русский», затем на «Системный».
  6. При открытом окне сменить язык агентом через `gymgar-cli mcp` (в Claude Code: `command_run settings.setLanguage`). Окно переключилось.
  7. «Отключить» и подтвердить. Статус «не подключено», а в «Связке ключей» нет записи `gymgar`.
