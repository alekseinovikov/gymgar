# Загрузка тренировок — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Цель:** Gymgar сам загружает из Garmin все активности: силовые — с подходами из исходного `.FIT`, остальные — сводкой. Активности видны на экране «Тренировки», подходы можно поправить поверх записи часов, агент через MCP ищет, фильтрует и правит.

**Архитектура:** вертикальный срез снизу вверх.

- `domain` — типы и правило пустого подхода;
- `garmin` — запросы данных Personal API и разбор `.FIT` в DTO Garmin;
- `sync` — чистые функции «байты → domain» и правила загрузки;
- `store` — миграция 2, запись загрузки, правки и выборки;
- `engine` — задачи с промежуточными результатами, `garmin.sync`, команды активностей;
- `ui-egui` — экран «Тренировки» и «Сводка»;
- `apps` — адаптер клиента Garmin.

`engine` зависит от `sync`, но не называет ни одного типа `garmin`: сеть — трейт, который отдаёт сырые байты.

**Стек:** как в проекте 1 (Rust 2024, egui 0.37, rusqlite 0.40, reqwest 0.13, tokio 1.54, rmcp, schemars 1.2, chrono, secrecy). Новые зависимости:

- `fitparser = "0.11"` — разбор `.FIT`, в нём же каталог упражнений;
- `zip` с `default-features = false` и только deflate. Версию строитель берёт последнюю стабильную и проверяет имя фичи. `flate2` уже есть в дереве.

**Спека:** [`docs/superpowers/specs/2026-10-23-activity-import-design.md`](../specs/2026-10-23-activity-import-design.md). Читать вместе с этим планом: сценарии, тексты, анти-цели и проверенные факты — оттуда. Макеты: [`2026-10-23-activity-import-mockups/`](../specs/2026-10-23-activity-import-mockups/).

## Global Constraints

- Действуют все правила `CLAUDE.md`:
  - в не-тестовом коде нет `unwrap`, `expect`, `panic!` и индексации `[i]`; `unsafe` запрещён;
  - внешние данные (ответы Garmin, `.FIT`, ZIP, параметры MCP) враждебны: `checked_*`, `get()`, лимиты размеров;
  - каждая мутация — одна транзакция SQLite;
  - перед коммитом `cargo xtask ci` зелёный.
- **Зависимости.** Новые зависимости объявляются в корневом `[workspace.dependencies]` с комментарием «зачем»; крейты подключают их через `{ workspace = true }`. `engine` напрямую не зависит от `garmin` (только dev-зависимость с фичей `testing`).
- **Запросы Personal API.** База — `https://personal.connectapi.garmin.com`, `Authorization: Bearer <токен>`:
  - список — `GET /activitylist-service/activities/search/activities?start=<n>&limit=100`;
  - файл — `GET /download-service/files/activity/{id}` (ZIP с одним `.fit`).
- **Силовая** — `activityType.typeKey == "strength_training"`. Только у неё скачивается файл.
- **Темп:** в каждый момент один запрос; пауза между файлами 2 с; повторы на 429, 5xx и ошибки сети — паузы 30 с, 120 с, 300 с, потом остановка.
- **Пустой подход:** повторов нет или 0, веса нет или 0, длительность меньше 10 000 мс. Не учитывается автоматически. Разминка и кардио учитываются.
- **Ключ подхода** — `(activityId, message_index)` сообщения `set` из `.FIT`. Правки хранятся отдельно и никогда не трогаются разбором.
- **Пометка «новая»** ставится, только если до загрузки в базе были активности и активность начата позже самой новой из них.
- **Агенту закрыты** `garmin.sync`, `activity.reparse`, `activity.markSeen` (вдобавок к закрытым в проекте 1). Разрушительной остаётся только `garmin.disconnect`.
- **В ответах команд и MCP** нет сырого JSON Garmin, файлов, `owner*`-полей, ключа и токена.
- **Строки UI** — английские литералы в `tl!()`, перевод в `crates/ui-egui/i18n/ru.tsv`. Русские тексты — из спеки (§2, «Ошибки загрузки»). Названия упражнений — данные, через `tl!` не идут.
- **Цвета и отступы** — только `theme::Tokens` и `theme::metrics`. Новые общие виджеты — в `widgets.rs`.
- **Тесты** не ходят в настоящий Garmin. В репозиторий попадают только очищенные fixtures (задача 3).
- **Время** хранится в UTC, местное — только при показе. Вес хранится в целых граммах.

## Review Focus

1. **Одна активность на двух страницах списка.** Пока идёт листание, у пользователя появилась новая тренировка, и смещение сдвинулось. Ожидание: одна строка в базе, одна пометка «новая», файл скачан один раз. Тест — в задаче 9.
2. **Правка подхода, пока применяется файл той же активности** (загрузка или повторный разбор). Ожидание: правка цела, числа подходов пересчитаны с её учётом. Тест — в задаче 6.
3. **Силовая без подходов:** ручная активность (`manualActivity`), запись из стороннего приложения, `.FIT` без сообщений `set`. Ожидание: файл «загружен», подходов 0, экран пишет «В записи нет подходов», а не «Не удалось прочитать». Тесты — в задачах 3, 5, 9 и 13.
4. **Часы в фунтах** (`weight_display_unit=pound`). Ожидание: поле `weight` в `.FIT` всё равно в килограммах, Gymgar ничего не пересчитывает. Тест — в задаче 3.
5. **Пустое, `null` или очень длинное `activityName`.** Ожидание: вместо пустого названия — название типа активности; длинное обрезается многоточием в строке списка, а не раздвигает колонку. Тесты — в задачах 5 и 12.

---

## Файловая структура

```text
Cargo.toml                                    # + fitparser, zip
fixtures/garmin/{strength-plan.zip, activities-page.json, README.md}   # очищенные fixtures
xtask/src/{main.rs, scrub.rs}                 # + cargo xtask scrub-fit / scrub-list
crates/domain/src/{lib.rs, error.rs, units.rs, exercise.rs, activity.rs, sets.rs}
crates/garmin/src/{lib.rs, error.rs, personal.rs, personal/client.rs, personal/activities.rs,
                   fit.rs, fit/unzip.rs, fit/parse.rs, fit/catalog.rs, fit/testing.rs}
crates/sync/src/{lib.rs, error.rs, list.rs, strength.rs, exercises.rs, policy.rs}
crates/store/src/{lib.rs, error.rs, migrations.rs, activities.rs, corrections.rs, activity_queries.rs}
crates/engine/src/{lib.rs, error.rs, services.rs, jobs.rs, session.rs, commands.rs, garmin_cmds.rs,
                   sync_cmds.rs, sync_job.rs, activity_cmds.rs, activity_views.rs, exercise_cmds.rs, testing.rs}
crates/ui-egui/src/{state.rs, rail.rs, icons.rs, app.rs, summary_ui.rs, widgets.rs, numbers.rs,
                    activities_ui.rs, activity_detail_ui.rs, set_edit_ui.rs}
crates/ui-egui/i18n/ru.tsv
assets/icons/dumbbell.svg
apps/gymgar/src/{main.rs, services.rs}
apps/gymgar-cli/src/main.rs
CLAUDE.md  docs/architecture.md  README.md  docs/superpowers/plans/2026-10-23-garmin-connect-foundation-followups.md
```

---

### Task 1: доменные типы активностей и подходов

**Files:**
- Create: `crates/domain/src/{error.rs, units.rs, exercise.rs, activity.rs, sets.rs}`
- Modify: `crates/domain/src/lib.rs`, `crates/domain/Cargo.toml` (+ `thiserror`)

**Interfaces:**
- Produces (крейт `gymgar_domain`):
  - `DomainError::OutOfRange { what: &'static str, value: String }` (thiserror).
  - `Mass` (целые граммы):
    - `Mass::from_kg(kg: f64) -> Result<Mass, DomainError>` — конечное, `0.0..=1000.0`, округление до грамма;
    - `Mass::from_grams(g: i64) -> Result<Mass, DomainError>`;
    - `grams(self) -> i64`, `kg(self) -> f64`, `is_zero(self) -> bool`;
    - serde — числом в килограммах.
  - `Reps`: `Reps::new(n: u32) -> Result<Reps, DomainError>` (`0..=1000`), `get(self) -> u16`; serde числом с той же проверкой.
  - `ExerciseId { category: u16, name: Option<u16> }`, `ExerciseId::UNKNOWN_CATEGORY: u16 = 65534`.
  - `ActivityId(pub i64)` (serde transparent), `STRENGTH_TYPE_KEY: &str = "strength_training"`.
  - `ActivitySummary`:
    - поля `{ id: ActivityId, type_key: String, name: Option<String>, start: DateTime<Utc>, duration_ms: Option<u64>, distance_m: Option<f64>, avg_hr: Option<u16>, calories: Option<u32>, manual: bool }`;
    - `is_strength(&self) -> bool`.
  - `FileState { NotNeeded, Pending, Downloaded, Missing, Unreadable }` и `SyncErrorKind { KeyRejected, KeyMissing, RateLimited, Network, Unavailable }` — оба с `as_str`/`parse` в camelCase.
  - `SetRef { Recorded(u16), Added(i64) }` — serde `{"recorded": 12}` / `{"added": 3}`.
  - `PlannedTarget { reps: Option<Reps>, weight: Option<Mass> }`.
  - `RecordedSet { message_index: u16, start: DateTime<Utc>, duration_ms: u32, reps: Option<Reps>, weight: Option<Mass>, exercise: Option<ExerciseId>, step_index: Option<u16>, plan: Option<PlannedTarget>, hr_avg: Option<u16>, hr_max: Option<u16>, rest_after_ms: Option<u32> }`. `exercise: None` — «Без упражнения».
  - `SetCorrection { weight: Option<Mass>, reps: Option<Reps>, exercise: Option<ExerciseId>, counted: Option<bool> }`.
  - `AddedSet { id: i64, after: Option<SetRef>, exercise: ExerciseId, weight: Option<Mass>, reps: Option<Reps>, counted: Option<bool> }`.
  - `EMPTY_SET_MAX_MS: u32 = 10_000`.
  - `is_empty_set(reps: Option<Reps>, weight: Option<Mass>, duration_ms: u32) -> bool`.
  - `CountReason { Counted, ExcludedByUser, Empty, ActivityExcluded }` (serde camelCase).
  - `count_reason(activity_excluded: bool, user: Option<bool>, empty: bool) -> CountReason`. Порядок: исключённая активность → `ActivityExcluded`; `user == Some(false)` → `ExcludedByUser`; `user == Some(true)` → `Counted`; `empty` → `Empty`; иначе `Counted`.
  - `StrengthRecord { workout_name: Option<String>, sets: Vec<RecordedSet>, titles: Vec<(ExerciseId, String)> }`.

- [ ] **Step 1: Написать падающие тесты** в модулях:
  - `mass_bounds`: `from_kg(77.5).grams() == 77_500`; `from_kg(-0.1)`, `from_kg(1000.001)`, `from_kg(f64::NAN)`, `from_kg(f64::INFINITY)` — `Err`; `from_kg(0.0).is_zero()`.
  - `mass_serde_is_kg`: `serde_json::to_value(Mass::from_kg(12.5)) == json!(12.5)`; десериализация `json!(1001)` — ошибка.
  - `reps_bounds`: `Reps::new(1000)` — Ok, `Reps::new(1001)` — `Err`.
  - `empty_set_rule`:
    - `(None, None, 2_100)` — `true`;
    - `(Some(0), Some(0 кг), 9_999)` — `true`;
    - ровно `10_000` мс — `false`;
    - `(Some(10), None, 2_000)` — `false`;
    - `(None, Some(5 кг), 2_000)` — `false`;
    - разминка `(None, None, 278_350)` — `false`.
  - `count_reason_order`: все 12 комбинаций `(activity_excluded, user, empty)` против таблицы из Interfaces.
  - `set_ref_serde`: `SetRef::Recorded(12)` ⇄ `{"recorded":12}`, `SetRef::Added(3)` ⇄ `{"added":3}`; `{"recorded":-1}` — ошибка.
  - `file_state_and_sync_error_round_trip`: `parse(as_str(x)) == Some(x)` для всех вариантов.
- [ ] **Step 2: Запустить и убедиться, что падают:** `cargo test -p gymgar-domain` — ошибки компиляции.
- [ ] **Step 3: Реализовать типы и функции** из Interfaces. `lib.rs` реэкспортирует их; у каждого модуля `//!`.
- [ ] **Step 4: Прогнать:** `cargo test -p gymgar-domain` — PASS.
- [ ] **Step 5: Коммит:** `git commit -m "Add domain types for activities, sets and corrections"`

---

### Task 2: `garmin::personal` — запросы данных с повтором после 401

**Files:**
- Create: `crates/garmin/src/personal/client.rs`, `crates/garmin/src/personal/activities.rs`
- Modify: `crates/garmin/src/personal.rs`, `crates/garmin/src/personal/auth.rs`, `crates/garmin/src/error.rs`

**Interfaces:**
- Consumes: `personal::Auth` (проект 1).
- Produces:
  - `GarminError::NotFound` — новый вариант (HTTP 404).
  - `Auth::token()` меняется: если обновить токен не удалось из-за `Network`, `RateLimited` или `Unavailable`, а в кэше есть токен, у которого `exp` ещё впереди, он и возвращается. При `KeyRejected` кэш очищается, ошибка уходит дальше.
  - `personal::Client`:
    - `Client::new(key: SecretString) -> Result<Client, GarminError>`, `Client::with_base_url(key, base_url: &str)`;
    - `async fn exchange(&self) -> Result<TokenInfo, GarminError>`;
    - `async fn activities_page(&self, start: u32, limit: u32) -> Result<Vec<u8>, GarminError>`;
    - `async fn activity_file(&self, id: i64) -> Result<Vec<u8>, GarminError>`.

    Внутри `Client` — свой `Auth` и `reqwest::Client` (таймаут 30 с, тот же `User-Agent`). Тело читается с лимитом: список — 16 МиБ, файл — 32 МиБ; больше — `UnexpectedResponse("response larger than … bytes")`.
  - Повтор после 401: запрос → 401 → `auth.exchange()` → повтор с новым токеном → снова 401 → `KeyRejected`. Не больше одного обмена на запрос.
  - `personal::ActivityListItem`:
    - поля `{ activity_id: i64, activity_name: Option<String>, activity_type: ActivityTypeDto, start_time_gmt: Option<String>, duration: Option<f64>, distance: Option<f64>, average_hr: Option<f64>, calories: Option<f64>, manual_activity: Option<bool>, #[serde(flatten)] extra: serde_json::Map<String, Value> }` с `rename_all = "camelCase"`; `averageHR` и `startTimeGMT` — явными `rename`;
    - `ActivityTypeDto { type_key: String, #[serde(flatten)] extra }`.
  - `personal::parse_activity_page(body: &[u8]) -> Result<Vec<ActivityListItem>, GarminError>` — не массив или битый JSON → `UnexpectedResponse`.

- [ ] **Step 1: Написать падающие тесты** (wiremock, как в `auth.rs`; хелпер mock на обмен токена):
  - `page_ok_sends_bearer_and_query`: mock на путь списка проверяет `Authorization: Bearer <fake_jwt>`, `start=100`, `limit=100` и отдаёт байты; `activities_page(100, 100)` возвращает их без изменений.
  - `file_ok_returns_bytes` и `file_404_is_not_found`.
  - `data_401_refreshes_once_then_succeeds`: первый GET — 401, второй — 200; обмен токена вызван ровно 2 раза (`.expect(2)`).
  - `data_401_twice_is_key_rejected`: GET дважды 401 → `KeyRejected`; GET вызван 2 раза.
  - `refresh_network_failure_uses_cached_token`: токен с `exp` через 30 мин в кэше, обмен отвечает 503 → `token()` отдаёт кэшированный.
  - `body_over_limit_is_unexpected` — для списка через `#[cfg(test)]` лимит поменьше.
  - `list_item_keeps_unknown_fields`: элемент с `"summarizedExerciseSets": [...]` и `"newField": 1` → `extra` содержит оба; `serde_json::to_value(item)` равен исходному объекту.
  - `list_item_nulls`: `activityName: null`, нет `distance` → `None`, без ошибки.
  - `page_not_array_is_unexpected`.
- [ ] **Step 2: Запустить и убедиться, что падают:** `cargo test -p gymgar-garmin`.
- [ ] **Step 3: Реализовать** `Client`, `ActivityListItem`, `parse_activity_page`, изменения `Auth::token`, `GarminError::NotFound`. Обновить `//!` в `auth.rs`: повтор после 401 теперь есть, живёт в `client.rs`.
- [ ] **Step 4: Прогнать:** `cargo test -p gymgar-garmin` — PASS.
- [ ] **Step 5: Коммит:** `git commit -m "Add Personal API activity list and file requests with 401 retry"`

---

### Task 3: `garmin::fit` — кодировщик для тестов, распаковка и разбор

**Files:**
- Create: `crates/garmin/src/fit.rs`, `crates/garmin/src/fit/{testing.rs, unzip.rs, parse.rs}`
- Modify: `crates/garmin/src/lib.rs` (`//!` упоминает `fit`), `crates/garmin/Cargo.toml` (+ `fitparser`, `zip`, `log`; фича `testing = []`), корневой `Cargo.toml`

**Interfaces:**
- Produces:
  - `garmin::fit::testing::FitWriter` — только с фичей `testing`. Это тестовый код: `expect` в нём допустим под `#[allow]` с причиной.
    - `FitWriter::new(start: DateTime<Utc>) -> FitWriter` — `file_id` с `type = activity` без `serial_number`;
    - `workout(&mut self, name: &str)`;
    - `step(&mut self, s: StepSpec)`, `StepSpec { message_index: u16, reps: Option<u32>, weight_kg: Option<f64>, category: Option<u16>, name: Option<u16>, notes: Option<String> }`;
    - `title(&mut self, category: Option<u16>, name: Option<u16>, title: &str)`;
    - `set(&mut self, s: SetSpec)`, `SetSpec { message_index: u16, active: bool, start: DateTime<Utc>, duration_s: f64, reps: Option<u16>, weight_kg: Option<f64>, category: Option<u16>, subtype: Option<u16>, step_index: Option<u16>, pounds: bool }`;
    - `heart_rate(&mut self, at: DateTime<Utc>, bpm: u8)`;
    - `finish(self) -> Vec<u8>` — заголовок с CRC, сообщения определений и данных, CRC файла.

    Номера сообщений и полей сверяются с `fitparser::profile`: `set` = 225, `exercise_title` = 264, `workout_step` = 27, `workout` = 26, `record` = 20, `file_id` = 0. Масштабы: `weight` ×16, `duration` ×1000, `exercise_weight` ×100.
  - `garmin::fit::testing::zip_single(file_name: &str, bytes: &[u8]) -> Vec<u8>`.
  - `fit::FitError { Zip(String), Fit(String), TooLarge }` (thiserror).
  - `fit::unzip_activity(zip: &[u8]) -> Result<Vec<u8>, FitError>`:
    - ZIP не больше 32 МиБ, распакованный файл не больше 64 МиБ — считается фактически прочитанное через `take`, а не заголовок;
    - внутри ровно одна запись с именем на `.fit` (регистр не важен);
    - иначе `Zip(..)` или `TooLarge`.
  - `fit::parse(fit: &[u8]) -> Result<FitActivity, FitError>` и DTO:
    - `FitActivity { workout_name: Option<String>, sets: Vec<FitSet>, titles: Vec<FitTitle>, steps: Vec<FitStep>, heart_rate: Vec<(DateTime<Utc>, u8)> }`;
    - `FitSet { message_index: u16, kind: FitSetKind, start_time: Option<DateTime<Utc>>, duration_s: Option<f64>, repetitions: Option<u16>, weight_kg: Option<f64>, category: Option<u16>, category_subtype: Option<u16>, wkt_step_index: Option<u16> }`;
    - `FitSetKind { Active, Rest, Other }`;
    - `FitTitle { category: Option<u16>, name: Option<u16>, title: String }`;
    - `FitStep { message_index: u16, duration_reps: Option<u32>, exercise_weight_kg: Option<f64>, category: Option<u16>, name: Option<u16>, notes: Option<String> }`.

    Правила разбора:
    - номер категории берётся, как бы `fitparser` его ни отдал: строкой-именем (`"deadlift"` → `ExerciseCategory::from(&str)` → `u16`), числом или массивом (первый элемент);
    - FIT-«invalid» (`0xFFFF`) — `None`;
    - `Value::Timestamp` переводится в UTC;
    - `set` без `message_index` пропускается с `log::debug!`.

- [ ] **Step 1: Написать падающие тесты** (синтетика через `FitWriter`):
  - `writer_output_parses_with_fitparser`: один активный подход (`reps = 10`, `weight_kg = 25.0`, категория 8, подтип 23, `message_index = 4`), один отдых, три точки пульса. `fitparser::from_bytes` даёт два `Set` и три `Record` с теми же значениями; CRC корректен.
  - `writer_writes_no_serial_number`.
  - `parse_round_trip`: `parse(writer)` возвращает те же подходы, заголовки, шаги, имя программы и пульс.
  - `parse_no_exercise_encodings`: подходы без категории, с 65534 и с `0xFFFF` → `category` равна `None` или `Some(65534)`.
  - `parse_pound_display_keeps_kg` (Review Focus 4): `SetSpec { pounds: true, weight_kg: Some(20.0) }` → `weight_kg == Some(20.0)`.
  - `parse_no_sets_is_ok` (Review Focus 3): файл без `set` → `Ok`, `sets` пуст.
  - `parse_truncated_or_garbage_is_error`: синтетический файл, обрезанный наполовину, и 1 КБ мусора → `Err(Fit(..))`.
  - `unzip_round_trip`: `unzip_activity(zip_single("1_ACTIVITY.fit", b))` возвращает `b`.
  - `unzip_rejects`: не-ZIP, пустой ZIP, две записи `.fit`, только `notes.txt`, распаковка больше лимита (ZIP-бомба из нулей; лимит через `#[cfg(test)]` параметр) → `Err`, без паники.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-garmin --features testing` — FAIL.
- [ ] **Step 3: Реализовать** `FitWriter`, `zip_single`, `unzip_activity`, `parse`.
- [ ] **Step 4: Прогнать** — PASS. `cargo clippy -p gymgar-garmin --all-features -- -D warnings` — чисто.
- [ ] **Step 5: Коммит:** `git commit -m "Unzip and parse strength FIT files; add a FIT writer for tests"`

---

### Task 4: каталог упражнений, очистка и настоящие fixtures

**Files:**
- Create: `crates/garmin/src/fit/catalog.rs`, `xtask/src/scrub.rs`, `fixtures/garmin/{README.md, strength-plan.zip, activities-page.json}`
- Modify: `crates/garmin/src/fit.rs`, `xtask/src/main.rs`, `xtask/Cargo.toml` (+ `gymgar-garmin` с `testing`, `serde_json`, `chrono`)

**Interfaces:**
- Consumes: задача 3.
- Produces:
  - `fit::catalog`:
    - `all() -> &'static [CatalogEntry]`, `CatalogEntry { category: u16, name: Option<u16>, key: String, english: String }`. Строится лениво (`OnceLock`): для каждой категории `ExerciseCategory` — запись без номера и записи для номеров `0..=1023`, у которых per-category enum `fitparser` даёт не `Value(_)`. Категория `Unknown` (65534) в каталог не входит. `match` категории на per-category enum — макросом, а не руками;
    - `exercise_key(category: u16, name: Option<u16>) -> String` — `"deadlift/romanian_deadlift"`, `"cardio"`, для неизвестного номера `"deadlift/77"`, для неизвестной категории `"c900/3"`;
    - `parse_exercise_key(key: &str) -> Option<(u16, Option<u16>)>` — обратное к `exercise_key`, включая числовые формы;
    - `english_name(category: u16, name: Option<u16>) -> String` — `"romanian_deadlift"` → `"Romanian deadlift"`; для неизвестных — ключ.
  - `cargo xtask scrub-fit <in.zip> <out.zip>`: `unzip_activity` → `parse` → заново через `FitWriter` → `zip_single`. В выходе только программа, шаги, заголовки, подходы и пульс. Ни `device_info`, ни `user_profile`, ни позиций, ни неизвестных сообщений.
  - `cargo xtask scrub-list <in.json> <out.json> --take <n>`:
    - берёт до `n` элементов разных типов;
    - удаляет ключи на `owner`, `userProfileId`, `activityUUID` и все строки-ссылки;
    - заменяет `activityName` на `"<typeKey> <порядковый номер>"`;
    - сдвигает `activityId` на постоянное смещение.

- [ ] **Step 1: Написать падающие тесты:**
  - `catalog_keys`:
    - `exercise_key(8, Some(23)) == "deadlift/romanian_deadlift"`, `exercise_key(2, None) == "cardio"`;
    - `parse_exercise_key("deadlift/77") == Some((8, Some(77)))`, `parse_exercise_key("nope/x") == None`;
    - `english_name(8, Some(23)) == "Romanian deadlift"`;
    - `parse(exercise_key(e)) == e` для всех записей `all()`;
    - в `all()` больше 1000 записей, ни одной с категорией 65534.
  - `scrub_list_removes_personal_fields` и `scrub_fit_drops_device_and_profile` — в `xtask`, на маленьких синтетических входах.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-garmin --features testing && cargo test -p xtask` — FAIL.
- [ ] **Step 3: Реализовать** `catalog` и `scrub.rs`.
- [ ] **Step 4: Сделать fixtures** из локальных образцов пользователя (вне репозитория):

```bash
cargo xtask scrub-fit /Users/alekseinovikov/gymgar/temp/garmin-samples/activity-<id>.zip fixtures/garmin/strength-plan.zip
cargo xtask scrub-list /Users/alekseinovikov/gymgar/temp/garmin-samples/activities.json fixtures/garmin/activities-page.json --take 8
```

  Проверить глазами: в `activities-page.json` нет `Aleksei`, `owner`, ссылок `s3.amazonaws.com`; есть `strength_training`, `cycling` и `treadmill_running`. В `fixtures/garmin/README.md` записать, откуда файлы, как очищены и какой командой пересобрать.
- [ ] **Step 5: Тест на настоящих данных** `parse_fixture` (значения проверены независимо по выгрузке `fitparser` 2026-10-23):
  - `workout_name == Some("FB2")`, активных подходов 31;
  - подход `message_index = 4`: `repetitions = 10`, `weight_kg = 25.0`, `category = 8`, `category_subtype = 23`, `wkt_step_index = 4`;
  - отдых `message_index = 1`: `kind = Rest`, `duration_s ≈ 31.765`;
  - заголовок `(Some(8), Some(23))` = «Румынская становая тяга»;
  - разминка `message_index = 2`: `category = 31`, подтипа нет.
- [ ] **Step 6: Прогнать** — PASS; `cargo xtask layers` — OK.
- [ ] **Step 7: Коммит:** `git commit -m "Add the exercise catalog, scrub tools and cleaned Garmin fixtures"`

---

### Task 5: `sync` — байты в domain и правила загрузки

**Files:**
- Create: `crates/sync/src/{error.rs, list.rs, strength.rs, exercises.rs, policy.rs}`
- Modify: `crates/sync/src/lib.rs` (`//!` описывает новое), `crates/sync/Cargo.toml` (+ `gymgar-domain`, `gymgar-garmin`, `chrono`, `serde_json`, `thiserror`, `log`; dev — `gymgar-garmin` с `testing`)

**Interfaces:**
- Consumes: задача 1 (domain), задача 2 (`parse_activity_page`), задача 3 (`unzip_activity`, `parse`, `FitWriter`), задача 4 (`catalog`, fixtures).
- Produces (крейт `gymgar_sync`):
  - `SyncError { Garmin(String), Fit(String) }`.
  - `list::ListedActivity { summary: ActivitySummary, raw_json: String }`.
  - `list::parse_page(body: &[u8]) -> Result<Vec<ListedActivity>, SyncError>`:
    - `start` — из `startTimeGMT` (`%Y-%m-%d %H:%M:%S`, UTC). Элемент без разборчивого времени пропускается с `log::warn!` (только id);
    - `duration_ms` — `duration * 1000`, округление; отрицательное, NaN или inf → `None`;
    - `avg_hr`, `calories` — `None`, если вне `u16`/`u32`;
    - `name` — пустая или из пробелов строка → `None` (Review Focus 5);
    - `manual` — из `manualActivity` или `isManualActivity`;
    - `raw_json` — `serde_json::to_string` элемента со всеми полями.
  - `strength::PARSER_VERSION: u32 = 1`.
  - `strength::parse_strength(zip: &[u8]) -> Result<StrengthRecord, SyncError>`:
    - в `sets` только активные подходы, в порядке `message_index`;
    - `exercise` — `None`, если категории нет или она `65534`, иначе `ExerciseId { category, name: subtype }`;
    - `plan` — из `FitStep` с `message_index == wkt_step_index`: `duration_reps` → `Reps`, `exercise_weight_kg` → `Mass`. Значения вне диапазона отбрасываются;
    - `rest_after_ms` — длительность сообщения `Rest` с `message_index + 1`;
    - `hr_avg`, `hr_max` — по точкам пульса с `start ≤ t < start + duration`, среднее округлено;
    - `titles` — `(ExerciseId, title)` для заголовков с известной категорией и непустым текстом;
    - вес и повторы вне диапазона `domain` превращаются в `None` с `log::warn!`, а не в ошибку файла.
  - `exercises`:
    - `exercise_key(id: ExerciseId) -> String`, `parse_exercise_key(key: &str) -> Option<ExerciseId>`, `english_name(id: ExerciseId) -> String`;
    - `catalog() -> &'static [CatalogExercise]`, `CatalogExercise { id: ExerciseId, key: String, english: String }`.
  - `policy`:
    - `PAGE_SIZE: u32 = 100`, `FILE_PAUSE: Duration = Duration::from_secs(2)`, `RETRY_PAUSES: [Duration; 3]` = 30 с, 120 с, 300 с;
    - `keep_paging(page: &[ActivityId], known: &HashSet<ActivityId>, history_complete: bool) -> bool` — `false` на пустой странице; `false`, если `history_complete` и на странице есть известный id; иначе `true`;
    - `is_new(start: DateTime<Utc>, newest_before: Option<DateTime<Utc>>) -> bool` — `newest_before.is_some_and(|n| start > n)`.

- [ ] **Step 1: Написать падающие тесты:**
  - `page_fixture`:
    - `parse_page(fixtures/garmin/activities-page.json)` даёт 8 сводок;
    - у силовой `is_strength()`, `duration_ms` и `avg_hr` заполнены;
    - у велосипеда `distance_m > 0`;
    - `raw_json` каждого элемента разбирается обратно в JSON с тем же `activityId`.
  - `page_odd_values`: `duration: -1`, `averageHR: 1e9`, `activityName: "   "`, нет `startTimeGMT` у одного элемента → ошибки нет, поля `None`, элемент без времени пропущен.
  - `strength_fixture`:
    - 31 подход, `message_index` по возрастанию;
    - у подхода 4: `reps = 10`, `weight = 25 кг`, `exercise = (8, Some(23))`, `plan.reps = 10`, `plan.weight = 70 кг`, `rest_after_ms ≈ 114_123`;
    - у разминки `exercise = (31, None)`, `reps = None`;
    - у каждого подхода с точками пульса в окне есть `hr_avg`;
    - в `titles` есть `((8, Some(23)), "Румынская становая тяга")`.
  - `strength_synthetic_no_exercise_and_empty`: подходы без категории и с 65534 → `exercise == None`; подход 2 с, без повторов и веса → `is_empty_set(..)` даёт `true`.
  - `strength_no_sets` (Review Focus 3): ZIP с `.fit` без `set` → `Ok`, `sets` пуст.
  - `strength_garbage`: не-ZIP → `Err(Fit(..))`.
  - `keys_round_trip`: `parse_exercise_key(exercise_key(id)) == Some(id)` для 50 случайных записей каталога и для `(8, Some(999))`.
  - `paging_rule`: пустая страница → `false`; история не загружена, есть известный → `true`; загружена, есть известный → `false`; загружена, все новые → `true`.
  - `new_rule`: `None` → `false`; строго позже → `true`; равное время → `false`.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-sync` — FAIL.
- [ ] **Step 3: Реализовать** модули из Interfaces.
- [ ] **Step 4: Прогнать** — PASS; `cargo xtask layers` — OK.
- [ ] **Step 5: Коммит:** `git commit -m "Map Garmin activity lists and strength files to the domain"`

---

### Task 6: `store` — миграция 2 и запись загрузки

**Files:**
- Create: `crates/store/src/activities.rs`
- Modify: `crates/store/src/migrations.rs`, `crates/store/src/error.rs` (+ `NotFound(String)`), `crates/store/src/lib.rs`

**Interfaces:**
- Consumes: задача 1.
- Produces:
  - **Миграция 2** — таблицы из спеки §7 «База»:
    - `activity`: `id`, `type_key`, `name`, `start_at`, `duration_ms`, `distance_m`, `avg_hr`, `calories`, `manual`, `raw_json`, `first_seen_at`, `seen_at`, `excluded`, `file_state`, `parser_version`, `workout_name`, `counted_sets`, `exercise_count`; индекс по `start_at`;
    - `activity_file(activity_id PK, zip BLOB)`;
    - `recorded_set(activity_id, message_index, …, PRIMARY KEY (activity_id, message_index))`;
    - `set_correction(activity_id, message_index, weight_g, reps, exercise_category, exercise_name, counted, updated_at, PK (activity_id, message_index))`;
    - `added_set(id INTEGER PK, activity_id, after_kind, after_value, exercise_category, exercise_name, weight_g, reps, counted, created_at, deleted_at)`;
    - `exercise_title(category, name, title, seen_at, PK (category, name))` — `name` NULL хранится как `-1`, чтобы PK работал;
    - `garmin_sync(id = 1, last_success_at, last_error, history_complete)`.
  - Типы (`gymgar_store`):
    - `NewActivity { summary: ActivitySummary, raw_json: String, is_new: bool }`;
    - `FileOutcome { Parsed { zip: Option<Vec<u8>>, record: StrengthRecord, parser_version: u32 }, Unreadable { zip: Vec<u8> }, Missing }` (`zip: None` — повторный разбор, файл уже лежит);
    - `SyncSnapshot { known: HashSet<ActivityId>, history_complete: bool, newest_start: Option<DateTime<Utc>>, pending_files: Vec<ActivityId> }` (`pending_files` — от новых к старым);
    - `SyncState { last_success_at: Option<DateTime<Utc>>, last_error: Option<SyncErrorKind>, history_complete: bool }`.
  - Методы `Store` — каждый одна транзакция:
    - `upsert_activities(&self, items: &[NewActivity], now: DateTime<Utc>) -> Result<(), StoreError>`:
      - новая строка: `first_seen_at = now`; `seen_at = now`, если `!is_new`, иначе NULL; `file_state = Pending` для силовой не-ручной, `Missing` для ручной силовой, `NotNeeded` для остальных;
      - существующая строка: обновляются только сводка и `raw_json`; `seen_at`, `excluded`, `file_state` не трогаются (Review Focus 1);
    - `sync_snapshot(&self) -> Result<SyncSnapshot, StoreError>`;
    - `apply_file(&self, id: ActivityId, outcome: FileOutcome) -> Result<(), StoreError>`:
      - `Parsed` заменяет `recorded_set` активности целиком, пишет `workout_name`, `parser_version`, `file_state = Downloaded` и `zip`, если он `Some`;
      - заголовки упражнений (`exercise_title`) обновляются upsert'ом;
      - пересчитываются `counted_sets` и `exercise_count` (функция `recount(tx, id)`, используется и задачей 7);
      - `set_correction` и `added_set` не трогаются;
      - `Unreadable` сохраняет `zip` и ставит `Unreadable`; `Missing` ставит `Missing`;
    - `files_to_reparse(&self, version: u32) -> Result<Vec<(ActivityId, Vec<u8>)>, StoreError>` — `Downloaded` и `Unreadable` с `parser_version < version`;
    - `set_history_complete(&self, done: bool)`, `record_sync(&self, success_at: Option<DateTime<Utc>>, error: Option<SyncErrorKind>)`, `sync_state(&self) -> Result<SyncState, StoreError>`.
  - `recount` применяет `domain::is_empty_set` и `count_reason` к итоговым значениям (правка поверх записи). Добавленный подход считается, если `deleted_at IS NULL` и `counted != 0`. `exercise_count` — число различных итоговых упражнений у учитываемых подходов, «Без упражнения» не считается.

- [ ] **Step 1: Написать падающие тесты** (временная база, как в проекте 1):
  - `migrates_v1_database_with_connection`: база схемы 1 с записанным подключением и языком открывается, `user_version == 2`, подключение и язык на месте.
  - `upsert_marks_and_keeps`:
    - первая вставка с `is_new = true` → `seen_at IS NULL`;
    - повторный upsert той же активности с `is_new = false` и другим названием → название новое, `seen_at` всё ещё NULL, `file_state` прежний.
  - `upsert_same_id_twice_in_one_page` (Review Focus 1): две записи с одним id в одном вызове → одна строка.
  - `file_states_on_insert`: силовая → `Pending`, ручная силовая → `Missing`, бег → `NotNeeded`.
  - `snapshot_lists_pending_newest_first`.
  - `apply_parsed_then_reparse_keeps_corrections` (Review Focus 2):
    - `apply_file(Parsed)` с тремя подходами;
    - напрямую в `set_correction` вставлена правка подхода 1 с `counted = 1`;
    - повторный `apply_file(Parsed { zip: None, .. })` с другим набором подходов (подход 1 пустой) → правка на месте, `counted_sets` учитывает `counted = 1`, ZIP не потерян.
  - `apply_unreadable_and_missing`.
  - `reparse_lists_only_old_versions`.
  - `sync_state_round_trip`.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-store` — FAIL.
- [ ] **Step 3: Реализовать** миграцию 2 (новая запись в `MIGRATIONS`, старая не меняется) и методы. Вес — `weight_g INTEGER`, время — RFC 3339 UTC, как в `garmin_connection.rs`.
- [ ] **Step 4: Прогнать** — PASS.
- [ ] **Step 5: Коммит:** `git commit -m "Store activities, strength files and sync state"`

---

### Task 7: `store` — правки и выборки

**Files:**
- Create: `crates/store/src/corrections.rs`, `crates/store/src/activity_queries.rs`
- Modify: `crates/store/src/lib.rs`

**Interfaces:**
- Consumes: задачи 1 и 6 (`recount`).
- Produces:
  - `SetChange { weight: Option<Mass>, reps: Option<Reps>, exercise: Option<ExerciseId>, counted: Option<Option<bool>> }`: `None` — не менять; `counted: Some(None)` — снова по правилу.
  - Методы правок. Каждый — одна транзакция с `recount`. Неизвестная активность или подход → `StoreError::NotFound(..)`, и ничего не меняется:
    - `edit_sets(&self, id: ActivityId, sets: &[SetRef], change: &SetChange, now) -> Result<(), StoreError>` — для `Recorded` — upsert `set_correction`, для `Added` — update `added_set`;
    - `reset_sets(&self, id, indexes: &[u16]) -> Result<(), StoreError>` — удаляет правки;
    - `add_set(&self, id, after: Option<SetRef>, exercise: ExerciseId, weight: Option<Mass>, reps: Option<Reps>, now) -> Result<i64, StoreError>`;
    - `remove_added_set(&self, id, added: i64, now) -> Result<(), StoreError>` — `deleted_at = now`;
    - `set_activity_excluded(&self, id, excluded: bool)`;
    - `mark_seen(&self, id, now)` — идемпотентно.
  - Выборки:
    - `ActivityFilter { from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>, type_keys: Vec<String>, strength_only: bool, exercise: Option<ExerciseId>, only_new: bool, include_excluded: bool, limit: u32, offset: u32 }`;
    - `ActivityRow { summary: ActivitySummary, is_new: bool, excluded: bool, file_state: FileState, workout_name: Option<String>, counted_sets: u32, exercise_count: u32 }`;
    - `list_activities(&self, f: &ActivityFilter) -> Result<Vec<ActivityRow>, StoreError>` — от новых к старым. Фильтр `exercise` — `EXISTS` по итоговому упражнению учитываемых подходов (`COALESCE` правки и записи, плюс неудалённые добавленные);
    - `activity_detail(&self, id) -> Result<Option<ActivityDetail>, StoreError>`, `ActivityDetail { row: ActivityRow, recorded: Vec<RecordedSet>, corrections: BTreeMap<u16, SetCorrection>, added: Vec<AddedSet> }` — удалённые добавленные не входят;
    - `new_count(&self) -> Result<u32, StoreError>`;
    - `exercise_titles(&self) -> Result<HashMap<ExerciseId, String>, StoreError>`;
    - `exercise_usage(&self) -> Result<HashMap<ExerciseId, u32>, StoreError>` — число учитываемых подходов по итоговому упражнению.

- [ ] **Step 1: Написать падающие тесты:**
  - `edit_sets_is_atomic`: правка подходов `[Recorded(4), Recorded(6), Recorded(99)]` → `NotFound`, у 4 и 6 правок нет.
  - `edit_then_reset`: вес 77,5 у подхода 4 → в `activity_detail` есть правка; `reset_sets([4])` → правки нет.
  - `empty_set_counted_by_user` — подход 2 с, без повторов:
    - не учитывается;
    - `counted: Some(Some(true))` → учитывается, `counted_sets` вырос;
    - `Some(None)` → снова не учитывается.
  - `reps_correction_lifts_empty_rule`: пустому подходу правка `reps = 8` → учитывается без флага.
  - `added_set_lifecycle`: `add_set` → в деталях и в `counted_sets`; `remove_added_set` → нет ни там, ни там, а строка в таблице с `deleted_at`.
  - `excluded_activity_counts_nothing`: `set_activity_excluded(true)` → `counted_sets == 0`; в `list_activities` с `include_excluded = false` её нет.
  - `list_filters`:
    - период: граница `from` включительно, `to` исключительно;
    - `type_keys`, `strength_only`, `only_new`;
    - упражнение: подход «Без упражнения», которому правкой поставили `curl/…`, находится по `curl/…`;
    - `limit`, `offset`.
  - `list_large_history_is_fast`: 2500 активностей по 30 подходов → `list_activities(limit 1000)` меньше 200 мс в release и не читает `recorded_set` (проверить через `EXPLAIN QUERY PLAN` или просто время; в debug — порог ×10).
  - `mark_seen_and_new_count`.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-store` — FAIL.
- [ ] **Step 3: Реализовать.** Время из `now`, а не из системных часов.
- [ ] **Step 4: Прогнать** — PASS.
- [ ] **Step 5: Коммит:** `git commit -m "Store set corrections and activity queries"`

---

### Task 8: `engine` — задачи с промежуточными результатами, сон и drop guard

**Files:**
- Modify: `crates/engine/src/jobs.rs`, `crates/engine/src/session.rs`, `crates/engine/src/services.rs`, `crates/engine/src/error.rs`, `crates/engine/src/testing.rs`, `crates/engine/src/lib.rs`, `crates/ui-egui/src/app.rs` (`poll_jobs` → события), `apps/gymgar/src/services.rs`, `apps/gymgar-cli/src/main.rs`

**Interfaces:**
- Produces:
  - `Services.sleep: Arc<dyn Fn(Duration) -> BoxFuture<()> + Send + Sync>`:
    - приложение — `tokio::time::sleep`;
    - CLI — сразу готовый future (задач там нет);
    - `testing` — `RecordingSleep`: возвращается сразу и записывает длительности (`fn slept(&self) -> Vec<Duration>`).
  - `JobCtx` — передаётся в future задачи:
    - `fn send(&self, output: JobOutput)` — промежуточный результат;
    - `fn sleep(&self, d: Duration) -> BoxFuture<()>`.

    `Jobs::start` принимает `FnOnce(JobCtx) -> F`.
  - `EngineError::JobDropped(String)` — спавнер бросил future, не доведя его до конца: guard в обёртке задачи при `Drop` без результата отправляет эту ошибку.
  - `JobEvent { Partial { id: JobId, command: &'static str, result: Result<(), EngineError> }, Finished(JobFinished) }`.
  - `Session::poll_jobs(&mut self) -> Vec<JobEvent>`: промежуточные применяются по порядку, каждый — через `guarded`. Ошибка применения промежуточного не останавливает задачу и приходит в `Partial.result`.
  - `JobOutput` получает варианты для задачи 9 (здесь — только тестовый `#[cfg(test)] Probe(u32)`, чтобы проверить механику).

- [ ] **Step 1: Написать падающие тесты** в `jobs.rs` и `session.rs`:
  - `partials_arrive_in_order_before_finish`: задача отправляет три `Probe`, затем завершается → `poll_jobs` даёт три `Partial`, потом `Finished`. Пока не взят `Finished`, команда числится выполняющейся.
  - `partial_apply_panic_is_error_and_job_continues`.
  - `dropped_job_is_reported_once`: спавнер, который бросает future без запуска → `Finished` с `Err(JobDropped("x.cmd"))`, задача больше не выполняющаяся.
  - `recording_sleep_records`.
  - Существующие тесты задач и `app_tests` проходят после перехода на `JobEvent`.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-engine` — FAIL.
- [ ] **Step 3: Реализовать.** `garmin.connect` и `garmin.verify` переходят на новый `start` без изменений поведения. В `app.rs` любое событие вызывает `refresh`, а `Finished` идёт в прежний `job_finished`.
- [ ] **Step 4: Прогнать:** `cargo test --workspace` — PASS.
- [ ] **Step 5: Коммит:** `git commit -m "Let background jobs send partial results; report dropped jobs"`

---

### Task 9: `garmin.sync`, `activity.reparse` и адаптер клиента

**Files:**
- Create: `crates/engine/src/sync_cmds.rs`, `crates/engine/src/sync_job.rs`
- Modify: `crates/engine/src/services.rs`, `crates/engine/src/jobs.rs`, `crates/engine/src/garmin_cmds.rs` (статус, `enabled` при идущей загрузке), `crates/engine/src/commands.rs` (+ `v.extend(sync_cmds::specs())`, таблица флагов), `crates/engine/src/testing.rs`, `crates/engine/Cargo.toml` (+ `gymgar-sync`; dev — `gymgar-garmin` с `testing`), `apps/gymgar/src/services.rs`

**Interfaces:**
- Consumes: задачи 5–8.
- Produces:
  - Трейт `GarminAuth` получает метод `fn open(&self, key: SecretString) -> Result<Arc<dyn GarminSession>, GarminAuthError>`.
  - Трейт `GarminSession: Send + Sync`:
    - `fn verify(&self) -> BoxFuture<Result<(), GarminAuthError>>`;
    - `fn activities_page(&self, start: u32, limit: u32) -> BoxFuture<Result<Vec<u8>, GarminAuthError>>`;
    - `fn activity_file(&self, id: ActivityId) -> BoxFuture<Result<Vec<u8>, GarminAuthError>>`.
  - `GarminAuthError::NotFound` — новый вариант.
  - Адаптер в `apps/gymgar/src/services.rs`: `GarminPersonalSession(gymgar_garmin::personal::Client)`, `open` строит `Client::new(key)`; `auth_error` дополняется `NotFound`.
  - `testing::FakeGarminSession` — отдаёт страницы и файлы из заданных байтов, ведёт счётчик запросов по виду, умеет отвечать заданной последовательностью ошибок. `TestConfig` получает поле `session: Option<Arc<FakeGarminSession>>`.
  - Хелперы `testing` для задач 10 и 12–15 (через `Store`, без сети):
    - `seed_fixture_activities(session: &Session) -> Vec<ActivityId>` — `activities-page.json` и `strength-plan.zip` из `fixtures/garmin/` (`include_bytes!`) через `sync::list::parse_page` и `sync::strength::parse_strength`;
    - `seed_strength(session: &Session, sets: Vec<RecordedSet>) -> ActivityId` — синтетическая силовая сразу с подходами (для «Без упражнения» и пустых подходов).
  - Команды (`sync_cmds.rs`):

    | Id | Вид | Агенту | Params |
    |---|---|---|---|
    | `garmin.sync` | Job | нет | `NoParams` |
    | `activity.reparse` | Job | нет | `NoParams` |

    - **`enabled` у `garmin.sync`:** подключение записано; есть хранилища, клиент и спавнер; не идёт ни `garmin.sync`, ни `activity.reparse`, ни проверка ключа. Причины — короткие английские строки, например `"Garmin sync already running"`.
    - **`garmin.connect`, `garmin.verify`, `garmin.disconnect`** недоступны, пока идёт загрузка.
    - **`garmin.sync` (run):** на потоке сессии берёт `sync_snapshot` и `had_activities = !known.is_empty()`. Затем запускает `sync_job::run(ctx, input)`, где `input = { snapshot, key_store, cached_session, client }`. `Session` хранит открытую `GarminSession` в памяти (`cached_session`), чтобы не читать ключ и не менять токен на каждую загрузку. `garmin.connect` и `garmin.disconnect` сбрасывают кэш.
    - **`activity.reparse`:** на потоке сессии берёт `files_to_reparse(PARSER_VERSION)`. Задача разбирает каждый файл и шлёт `ActivityFile { id, Parsed { zip: None, .. } }` или `Unreadable`. Недоступна без спавнера; ничего не делает, если список пуст (сразу `Finished`).
  - `sync_job::run` — алгоритм спеки §7 «Фоновые задачи: загрузка»:
    1. Ключ (если нет кэшированной сессии) — `read_key` из хранилища записанного подключения. `open`, `verify`.
    2. Отправить `SyncVerified(Result<(), EngineError>)`; применяется как `apply_verify`. На `KeyRejected`, `KeyMissing`, `Secrets` — завершение.
    3. Листание: `PAGE_SIZE`, `keep_paging`; каждая страница — `ActivityPage { items: Vec<NewActivity> }` с `is_new = had_activities && is_new(start, snapshot.newest_start)`. Уже встреченные в этой загрузке id из следующих страниц выкидываются (Review Focus 1). Пустая страница → `HistoryComplete`.
    4. Файлы: `snapshot.pending_files` плюс новые силовые не-ручные из страниц, от новых к старым, без повторов. Перед каждым, кроме первого, `ctx.sleep(FILE_PAUSE)`. Затем `activity_file`, `parse_strength` в задаче, `ActivityFile { id, outcome }`. `NotFound` → `Missing`; ошибка разбора → `Unreadable { zip }`. После каждого файла — `Progress { done, total }`.
    5. Повторы: на `RateLimited`, `Unavailable`, `Network`, `Unexpected` — до трёх раз с `RETRY_PAUSES`, затем `SyncFinished { result: Err(kind), session }`. `KeyRejected` в середине → `SyncVerified(Err(KeyRejected))` и завершение.
    6. Успех: `SyncFinished { result: Ok, session }`.
  - Применение (`jobs.rs`, `JobOutput::apply`):
    - `ActivityPage` → `upsert_activities`;
    - `HistoryComplete` → `set_history_complete(true)`;
    - `ActivityFile` → `apply_file`;
    - `Progress` → в память `Session` (`sync_progress: Option<(u32, u32)>`);
    - `SyncFinished` → `record_sync(Some(now) | None, error)` и кэш сессии;
    - прогресс сбрасывается при `Finished`.
  - `GarminStatusView` получает `sync: SyncStatusView { running: bool, files_done: u32, files_total: u32, last_success_at: Option<DateTime<Utc>>, last_error: Option<SyncErrorKind>, new_count: u32 }`.

- [ ] **Step 1: Написать падающие тесты** (`sync_cmds.rs`, отдельный `sync_cmds_tests.rs`, fixtures из задачи 3, `DeferredSpawner` и `RecordingSleep`):
  - `first_sync_imports_all_without_new`: страницы `[A(сила), B(бег)]`, затем `[]`. В базе 2 активности, ни одной новой, файл A скачан, `history_complete`, `last_success_at` записан. Запросов: 2 страницы, 1 файл.
  - `second_sync_marks_only_newer`: в базе A (история загружена). Страница `[C(новее A), A]` → C новая; файл C скачан; листание остановилось на первой странице.
  - `duplicate_across_pages` (Review Focus 1): страницы `[C, A]`, `[A, B]`, `[]` → одна строка A, один файл A.
  - `interrupted_sync_resumes`: первая загрузка падает (`Network` трижды) на втором файле из трёх. Повторная загрузка скачивает только оставшиеся два файла, а не все.
  - `retries_pause_then_stop`: файл отвечает `RateLimited` 4 раза → `RecordingSleep` записал 30 с, 120 с, 300 с; `last_error == RateLimited`; уже применённое на месте.
  - `pause_between_files`: три файла → две паузы `FILE_PAUSE`.
  - `key_rejected_stops_and_records_status`: `verify` → `KeyRejected` → статус подключения `KeyRejected`, запросов страниц 0.
  - `missing_key_records_key_missing`.
  - `broken_file_does_not_stop_others`: файл B — мусор → B `Unreadable`, C `Downloaded`.
  - `manual_strength_is_not_downloaded` (Review Focus 3).
  - `sync_disables_key_commands_while_running` и наоборот.
  - `reparse_keeps_corrections`: правка подхода, `PARSER_VERSION` в тесте — через подмену версии в базе (`parser_version = 0`) → после `activity.reparse` правка на месте.
  - `cli_like_session_cannot_sync`: `Services` без клиента → `Disabled` с причиной.
  - Обновлённая таблица флагов в `commands.rs`: `garmin.sync` и `activity.reparse` — `Job`, `agent: false`.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-engine` — FAIL.
- [ ] **Step 3: Реализовать** трейты, адаптер, команды, задачу, применение. Тест адаптера в `apps/gymgar/src/services.rs`: wiremock отвечает на обмен, на список и на файл; `open(key).activities_page(0, 100)` возвращает тело; 404 файла → `GarminAuthError::NotFound`.
- [ ] **Step 4: Прогнать:** `cargo test --workspace` — PASS.
- [ ] **Step 5: Коммит:** `git commit -m "Sync activities and strength files from Garmin in the background"`

---

### Task 10: команды активностей и поиск упражнений

**Files:**
- Create: `crates/engine/src/activity_cmds.rs`, `crates/engine/src/activity_views.rs`, `crates/engine/src/exercise_cmds.rs` (тесты — в `*_tests.rs` рядом, если больше ~200 строк)
- Modify: `crates/engine/src/commands.rs` (+ две строки `v.extend`, таблица флагов), `crates/engine/src/lib.rs` (реэкспорт view-типов), `crates/engine/src/error.rs` (`StoreError::NotFound` → `BadParams`)

**Interfaces:**
- Consumes: задачи 5 (`exercises::*`), 7 (выборки и правки).
- Produces — команды (`agent` — да, кроме `activity.markSeen`; все `Query` или `Mutation`):

  | Id | Params (camelCase, `deny_unknown_fields`) | Ответ |
  |---|---|---|
  | `activity.list` | `from?`, `to?` (RFC 3339), `types?: [String]`, `strengthOnly?`, `exercise?: String` (ключ), `onlyNew?`, `includeExcluded?` (по умолчанию `true`), `limit?` (100, максимум 1000), `offset?` | `ActivityListView { items: Vec<ActivityListItemView> }` |
  | `activity.get` | `id: i64` | `ActivityView` |
  | `activity.markSeen` | `id` | `ActivityListItemView` |
  | `activity.setExcluded` | `id`, `excluded: bool` | `ActivityView` |
  | `activity.editSets` | `id`, `sets: [SetRef]` (1…200), `weightKg?`, `reps?`, `exercise?`, `counted?: bool \| null` | `ActivityView` |
  | `activity.resetSets` | `id`, `sets: [SetRef]` (только `recorded`) | `ActivityView` |
  | `activity.addSet` | `id`, `exercise`, `weightKg?`, `reps?`, `after?: SetRef` | `ActivityView` |
  | `activity.removeAddedSet` | `id`, `set: SetRef` (только `added`) | `ActivityView` |
  | `exercise.search` | `query: String` (≤ 100 символов), `limit?` (20, максимум 100) | `ExerciseSearchView { items: Vec<ExerciseHitView> }` |

  - View-типы (`activity_views.rs`, все `Serialize` camelCase, плюс `Deserialize` — его использует UI):
    - `ActivityListItemView { id, typeKey, name, start, durationMs, distanceM, avgHr, calories, isNew, excluded, fileState, countedSets, exerciseCount, workoutName }`;
    - `ActivityView { …то же, groups: Vec<ExerciseGroupView> }`;
    - `ExerciseGroupView { exercise: Option<ExerciseView>, plan: Option<PlanView>, sets: Vec<SetView> }`;
    - `ExerciseView { id: String, english: String, watchTitle: Option<String> }`;
    - `PlanView { reps: Option<u16>, weightKg: Option<f64> }`;
    - `SetView { set: SetRef, added: bool, start: Option<DateTime<Utc>>, durationMs: Option<u32>, recorded: Option<SetValuesView>, correction: Option<SetValuesView>, effective: SetValuesView, counted: bool, countReason: CountReason, plan: Option<PlanView>, hrAvg, hrMax, restAfterMs }`;
    - `SetValuesView { weightKg: Option<f64>, reps: Option<u16>, exercise: Option<ExerciseView> }`;
    - `ExerciseHitView { exercise: ExerciseView, timesUsed: u32 }`.
  - Сборка `ActivityView` (`activity_views::build(detail, titles) -> ActivityView`):
    - подходы из записи и добавленные идут вместе: добавленный — после подхода из `after`, без `after` — в конце;
    - группы — по итоговому упражнению в порядке первого появления, все «Без упражнения» — одна группа на месте первого такого подхода;
    - `plan` у группы — если у всех её подходов из записи план одинаковый, иначе у подходов;
    - отдых и упражнение правкой не меняются.
  - `exercise.search`:
    - поиск без учёта регистра по `english` и по `watchTitle` (`exercise_titles`);
    - сначала использованные (`exercise_usage`, по убыванию), затем остальной каталог;
    - пустой `query` — только использованные.
  - Плохие параметры → `BadParams`, без паники:
    - нет такой активности или подхода;
    - `weightKg` вне 0…1000, `NaN` не проходит JSON;
    - `reps` вне 0…1000;
    - неизвестный ключ упражнения;
    - `sets` пуст или длиннее 200;
    - `added` в `resetSets`, `recorded` в `removeAddedSet`;
    - `from > to`;
    - правка несиловой активности.

- [ ] **Step 1: Написать падающие тесты** (сессия с `testing::seed_fixture_activities` и `seed_strength`):
  - На каждую команду — поведение, `BadParams` на каждый пункт списка выше, атомарность для `editSets` (третий подход неизвестен → ничего не изменилось), `enabled` — всегда доступны (только база).
  - `get_groups_in_first_appearance_order` — по fixture: первая группа — кардио, затем разминка, затем «Румынская становая тяга»; у тяги `plan = {reps 10, weightKg 70}`.
  - `no_exercise_group_and_assign`: синтетическая активность с тремя подходами без упражнения; `editSets` всех трёх на `curl/incline_dumbbell_biceps_curl` → одна группа «curl»; `exercise.search("incline")` ставит её первой с `timesUsed = 3`.
  - `get_does_not_mark_seen`, `mark_seen_clears_new`.
  - `responses_have_no_personal_fields`: сериализованные ответы `list` и `get` не содержат `owner`, `raw`, `Aleksei`, `s3.amazonaws.com`.
  - `agent_flags`: `activity.markSeen` закрыт агенту, остальные открыты; таблица `SPEC_TABLE` в `commands.rs` дополнена всеми командами.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-engine` — FAIL.
- [ ] **Step 3: Реализовать.** Параметр `SetRef` — свой `SetRefParam` с `JsonSchema` в engine и `From` в domain. `counted: bool | null` — `Option<Option<bool>>` с `#[serde(default, deserialize_with = …)]`, чтобы отличать «нет поля» от `null`.
- [ ] **Step 4: Прогнать:** `cargo test --workspace` — PASS.
- [ ] **Step 5: Коммит:** `git commit -m "Add activity, set correction and exercise search commands"`

---

### Task 11: ключ вне UI-потока

**Files:**
- Modify: `crates/engine/src/garmin_cmds.rs`, `crates/engine/src/jobs.rs`, `crates/engine/src/commands.rs` (флаги), `crates/engine/src/garmin_cmds_tests.rs`, `crates/ui-egui/src/garmin_ui.rs` (отключение ждёт задачу), `apps/gymgar/src/main.rs`, `apps/gymgar/src/services.rs`

**Interfaces:**
- Produces:
  - **`garmin.connect`:** задача после успешной проверки сама сохраняет ключ (`store.set`) и возвращает `ConnectChecked { …, saved: Result<Option<SecretString>, EngineError> }`, где внутри `Ok` — прежний ключ для отката. `apply_connect` на потоке сессии только пишет запись в базу. Если запись в базу упала, откат ключа (`restore_key`) запускается отдельным future на спавнере, ошибки — в лог. Удаление ключа прежнего подключения из другого хранилища — тоже future на спавнере после успешной записи.
  - **`garmin.disconnect`** становится `CommandKind::Job` (по-прежнему `destructive`, агенту закрыта): задача удаляет ключ, `apply` удаляет запись из базы.
  - **Проба системного хранилища** в релизной сборке — `SystemStore::is_available` на отдельном потоке, запущенном в начале `main` и присоединённом перед созданием окна (`run_native`). Окна, которое могло бы замереть, в это время ещё нет. Это уточнение формулировки спеки (§2, п. 22). Строитель отмечает его в `docs/architecture.md`.

- [ ] **Step 1: Написать падающие тесты:**
  - `connect_writes_key_off_the_session_thread`: `MemorySecretStore` запоминает `ThreadId` вызовов `set`; при `DeferredSpawner`, запущенном в другом потоке, `set` вызван не в потоке теста.
  - `connect_db_failure_restores_key_in_background`.
  - `disconnect_is_a_job`: `execute` → `Started`; после `poll_jobs` ключа нет и записи нет; при ошибке удаления ключа запись остаётся.
  - Обновить таблицу флагов: `garmin.disconnect` — `Job`, `destructive: true`, `agent: false`.
  - В `app_tests`/`garmin_ui_tests`: «Отключить» показывает индикатор, пока задача идёт, и снова доступна после ошибки.
- [ ] **Step 2: Запустить** — FAIL.
- [ ] **Step 3: Реализовать.** Пробу в `main` покрыть тестом-функцией: `probe_before_window(probe: impl FnOnce() -> bool + Send) -> bool` выполняет `probe` не в вызывающем потоке.
- [ ] **Step 4: Прогнать:** `cargo test --workspace` — PASS.
- [ ] **Step 5: Коммит:** `git commit -m "Move Garmin key writes and the keychain probe off the UI thread"`

---

### Task 12: интерфейс — полоса, список «Тренировки», загрузка при старте, «Сводка»

**Files:**
- Create: `crates/ui-egui/src/activities_ui.rs`, `crates/ui-egui/src/numbers.rs`, `assets/icons/dumbbell.svg`
- Modify: `crates/ui-egui/src/{state.rs, rail.rs, icons.rs, app.rs, summary_ui.rs, widgets.rs, lib.rs, snapshot_tests.rs}`, `crates/ui-egui/i18n/ru.tsv`, `ATTRIBUTION.md`

**Interfaces:**
- Consumes: задачи 9 (`garmin.sync`, `activity.reparse`, `SyncStatusView`) и 10 (`activity.list`, `ActivityListItemView`).
- Produces:
  - `Section::Activities`; `UiState.activities: ActivitiesState { selected: Option<i64>, filter: ListFilter }`, `ListFilter { All, Strength, New }` — сохраняются.
  - `Icon::Dumbbell` (Lucide `dumbbell`, ISC, строка в `ATTRIBUTION.md`). Полоса: «Сводка», «Тренировки» с числом новых (`SyncStatusView.new_count`, кружок `accent`), «Настройки» внизу.
  - **`GymgarApp`** держит `activities: Vec<ActivityListItemView>` и перечитывает список (страницами по 1000, пока приходит полная) после мутаций, событий задач и изменения `data_version`.
  - **При старте** вместо `verify_once` — `start_once`: если подключено — `activity.reparse` (если нечего, задача сразу завершится), затем `garmin.sync`. Ошибка старта — в лог, не пользователю.
  - **`numbers.rs`:**
    - `fn kg(weight_kg: f64) -> String` — «77,5 кг» / «77.5 kg», без лишних нулей;
    - `fn duration(ms: u64) -> String` — `1:39` (часы:минуты) от часа и больше, иначе `28:41` (минуты:секунды);
    - `fn distance(m: f64) -> String` — «5,2 км» / «5.2 km».
  - **Шапка списка:** заголовок, кнопка «Обновить» (`garmin.sync`, недоступна, пока идёт загрузка, с индикатором), строка состояния по `SyncStatusView`. Тексты — из спеки §2 п. 6 и §7 «Ошибки загрузки»; для ключа — ссылка в «Настройки».
  - **Чипы** «Все» / «Силовые» / «Новые · N» — новый виджет `widgets::chip(ui, text, selected) -> Response`.
  - **Группы по неделям** — понедельник по местному времени; «Эта неделя», иначе «13–19 октября».
  - **Строка списка:** точка `accent` у новой, название (или название типа, если `name == None`; длинное обрезается многоточием — Review Focus 5), дата, длительность, `trn(countedSets, …)` подходов у силовой или дистанция у остальных. Несиловая и исключённая приглушены (`text_dim`). Список рисуется через `ScrollArea::show_rows`.
  - **Названия типов:** `strength_training`, `running`, `treadmill_running`, `cycling`, `indoor_cycling`, `pilates`, `walking`, `swimming`, `lap_swimming`, `hiking`, `yoga`, `cardio`, `hiit`, `indoor_rowing`. Остальные — ключ по-человечески.
  - **«Сводка»** (подключено): карточка «N новых тренировок» с кнопкой (раздел «Тренировки», фильтр «Новые»), карточка последней силовой с «Открыть», предупреждение о ключе как раньше. Фраза про «следующее обновление» удалена.
  - **Пустые состояния** из спеки §2 п. 16 для списка: не подключено, ещё ничего не загружено.
  - Правая часть экрана в этой задаче — заглушка «Выберите тренировку»: её заполнит задача 13.

- [ ] **Step 1: Написать падающие тесты** (kittest, `test_support`, сессия с fixtures):
  - `rail_shows_activities_with_badge`: после загрузки с двумя новыми на кнопке «Activities» есть `2`.
  - `startup_runs_reparse_then_sync_once`: при подключении в `tick` стартуют обе задачи, повторный `tick` не стартует новых.
  - `refresh_disabled_while_syncing`.
  - `list_groups_by_week_and_filters`: «Новые» показывает только новые; «Силовые» — без бега.
  - `long_and_missing_names` (Review Focus 5): название из 300 символов не расширяет колонку списка дальше её ширины; `name == None` → «Running».
  - `sync_error_texts`: для каждого `SyncErrorKind` в шапке — свой текст (en и ru).
  - `summary_new_card_opens_filtered_list`.
  - `numbers_format`: `kg(77.5)` — «77,5 кг» в ru и «77.5 kg» в en; `kg(80.0)` — «80 кг»; `duration(5_940_528)` — «1:39»; `duration(1_721_000)` — «28:41».
  - `ru_catalog_covers_new_literals` — существующий тест i18n проходит.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-ui-egui` — FAIL.
- [ ] **Step 3: Реализовать.** Русские строки — в `ru.tsv` в том же коммите.
- [ ] **Step 4: Прогнать тесты, затем снапшоты** `GYMGAR_SNAPSHOTS=/tmp/gymgar-shots cargo test -p gymgar-ui-egui -- --ignored render_snapshots`. Добавить в снапшот-тест состояния: «Тренировки» со списком, загрузка идёт, ошибка загрузки, «Сводка» с новыми — 1200×800 и 800×500, en и ru. Открыть PNG и посмотреть глазами.
- [ ] **Step 5: Коммит:** `git commit -m "Add the Activities list, the rail entry and sync on start"`

---

### Task 13: интерфейс — тренировка справа

**Files:**
- Create: `crates/ui-egui/src/activity_detail_ui.rs`
- Modify: `crates/ui-egui/src/{activities_ui.rs, app.rs, widgets.rs, snapshot_tests.rs}`, `crates/ui-egui/i18n/ru.tsv`

**Interfaces:**
- Consumes: задача 10 (`activity.get`, `activity.markSeen`, `activity.setExcluded`, `ActivityView`).
- Produces:
  - `GymgarApp` держит `detail: Option<ActivityView>`; выбор строки → `activity.get`; если активность новая — `activity.markSeen`.
  - **Силовая:**
    - заголовок: название, «‹день недели, дата› · ‹начало›–‹конец›», «по программе «‹workoutName›»», если есть;
    - цифры: время, подходы, средний пульс, калории;
    - действие «Не учитывать тренировку» / «Учитывать тренировку» (`activity.setExcluded`);
    - карточки групп: название упражнения (ru-интерфейс и `watchTitle` с кириллицей → `watchTitle`, иначе `english`), план в заголовке;
    - таблица: №, вес, повторы, пульс, время, отдых;
    - правка — `accent`, рядом зачёркнутое значение часов;
    - неучтённый — перечёркнут, с «не учитывается · вернуть» или «пустой, не учитывается · учитывать» по `countReason`;
    - добавленный помечен «добавлен»;
    - группа без упражнения — рамка `warning`, кнопка «Указать упражнение» (откроет задача 14; пока неактивна).
  - **Несиловая:** только сводка.
  - **Пустые состояния** (спека §2 п. 16): `Pending` — «Подходы загружаются…»; `Missing` или `Downloaded` без подходов — «В записи нет подходов» (Review Focus 3); `Unreadable` — «Не удалось прочитать запись часов».
  - Общие виджеты в `widgets.rs`: `set_value(ui, effective: &str, recorded: Option<&str>)` (значение с зачёркнутым исходным), `card_with_border(ui, color, add)`.

- [ ] **Step 1: Написать падающие тесты** (kittest):
  - `opening_marks_seen`: клик по новой строке → точка пропала, счётчик на полосе уменьшился.
  - `strength_detail_shows_groups_and_corrections`: после `activity.editSets` веса подхода в тесте на экране есть «77,5 кг» и зачёркнутое «80».
  - `empty_set_line_and_count_again`: пустой подход показан с «учитывать» — клик → подход учитывается, число подходов в заголовке выросло.
  - `file_states_texts`: три текста для `Pending`, `Missing`, `Unreadable`.
  - `exclude_activity_toggle`.
  - `russian_watch_title_used_only_in_ru`.
- [ ] **Step 2: Запустить** — FAIL.
- [ ] **Step 3: Реализовать.**
- [ ] **Step 4: Прогнать тесты и снапшоты** (силовая с правками и «Без упражнения», несиловая, три пустых состояния; 1200×800 и 800×500; en и ru). Открыть PNG и сравнить с макетом `workouts-layout.html`.
- [ ] **Step 5: Коммит:** `git commit -m "Show strength sets, corrections and activity summaries"`

---

### Task 14: интерфейс — правка подхода, выбор упражнения, добавление

**Files:**
- Create: `crates/ui-egui/src/set_edit_ui.rs`
- Modify: `crates/ui-egui/src/{activity_detail_ui.rs, state.rs, widgets.rs, snapshot_tests.rs}`, `crates/ui-egui/i18n/ru.tsv`

**Interfaces:**
- Consumes: задача 10 (`activity.editSets`, `resetSets`, `addSet`, `removeAddedSet`, `exercise.search`).
- Produces:
  - `ActivitiesState` получает поля минуты (`#[serde(skip)]`):
    - `editing: Option<SetEdit { set: SetRef, weight: String, reps: String, error: Option<String> }>`;
    - `picker: Option<Picker { target: PickerTarget, query: String, whole_group: bool, hits: Vec<ExerciseHitView> }>`, `PickerTarget { Sets(Vec<SetRef>), NewSet { after: Option<SetRef> } }`.
  - **Правка в строке** (макет `workouts-edit.html`):
    - клик по подходу — поля веса и повторов;
    - кнопки «Сохранить» (`editSets` только с изменёнными полями), «Отмена», «Вернуть как на часах» (`resetSets`; у добавленного вместо неё «Убрать» — `removeAddedSet`), «Не учитывать подход» / «Учитывать» (`editSets counted`), «Другое упражнение…» (поиск);
    - ввод веса принимает и запятую, и точку; неверный ввод — текст ошибки под строкой, команда не вызывается.
  - **Поиск упражнения:**
    - поле поиска; разделы «Вы делали» (`timesUsed > 0`) и «Каталог Garmin»;
    - флажок «Для всех подходов карточки» (по умолчанию включён для «Указать упражнение», выключен для «Другое упражнение…»);
    - «Выбрать» → `editSets { exercise }`;
    - запрос `exercise.search` — при изменении текста, не каждый кадр.
  - **«+ Добавить подход»** под карточкой: открывает поиск с упражнением карточки по умолчанию, затем `addSet` после последнего подхода карточки.

- [ ] **Step 1: Написать падающие тесты** (kittest):
  - `inline_edit_saves_only_changed_fields`: правка только повторов → в базе у подхода правка повторов, вес не тронут.
  - `comma_and_dot_both_parse`; `bad_input_shows_error_and_sends_nothing`.
  - `reset_returns_watch_values`.
  - `assign_exercise_to_whole_group`: «Без упражнения» из трёх подходов → «Указать упражнение» → выбор → оранжевая карточка исчезла, появилась карточка упражнения с тремя подходами.
  - `assign_to_one_set_only`.
  - `add_and_remove_set`.
  - `edit_state_not_persisted`: `UiState` после сериализации не содержит `editing` и `picker`.
- [ ] **Step 2: Запустить** — FAIL.
- [ ] **Step 3: Реализовать.**
- [ ] **Step 4: Прогнать тесты и снапшоты** (правка в строке, поиск упражнения; en и ru; 800×500). Открыть PNG и сравнить с `workouts-edit.html`.
- [ ] **Step 5: Коммит:** `git commit -m "Edit sets inline, pick exercises and add missing sets"`

---

### Task 15: MCP — проверка того, что видит агент

**Files:**
- Modify: `crates/automation/src/server.rs` (тесты; код — только если тест покажет нужду), `apps/gymgar-cli/src/main.rs` (если `Services` требует новых полей — уже сделано в задаче 8)

**Interfaces:**
- Consumes: задачи 9 и 10.

- [ ] **Step 1: Написать падающие тесты** в `server.rs` (сессия с fixture-активностями):
  - `command_list_shows_activity_commands_but_not_sync_or_mark_seen`;
  - `command_run_activity_list_with_filters` — по упражнению и периоду;
  - `command_run_edit_sets_then_get_shows_correction`;
  - `bad_set_ref_is_tool_error_not_crash` — `isError: true`;
  - `garmin_sync_is_refused_for_agent`.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-automation` — тесты, проверяющие реестр, могут сразу пройти: это нормально, они фиксируют поведение. Падение — повод чинить код.
- [ ] **Step 3: Вручную:** собрать `cargo build -p gymgar-cli`, подключить к MCP-клиенту (Claude Code) на копии отладочной базы с fixtures. `command_list`, `activity.list` с фильтром, `activity.editSets`, затем `activity.get`. Открытое отладочное окно на той же базе показывает правку в течение секунды.
- [ ] **Step 4: Коммит:** `git commit -m "Cover the agent's view of activities over MCP"`

---

### Task 16: документы

**Files:**
- Modify: `CLAUDE.md`, `docs/architecture.md`, `README.md`, `docs/superpowers/plans/2026-10-23-garmin-connect-foundation-followups.md`

- [ ] **Step 1: Обновить по спеке §10:**
  - **`docs/architecture.md`:**
    - §4 — `sync` и зависимость `engine → sync`;
    - §5 — промежуточные результаты задач, ключ пишут задачи, проба до окна;
    - §6.1 — как идёт загрузка;
    - §7 — что видит агент;
    - решения 16–18; уточнение к решению 1 (bridge отложен);
    - §9 и история изменений.
  - **`CLAUDE.md`:**
    - карта воркспейса (`sync`, `garmin::fit`, `fixtures/`, `xtask scrub-*`);
    - «`engine` не зависит от `garmin` напрямую»;
    - список команд агента в §4;
    - правило «fixtures только через `xtask scrub-*`».
  - **`README.md`:** раздел «Тренировки» (en и ru).
  - **followups:** отметить закрытое (ключ вне UI-потока, 401, drop guard), bridge — в «будущие проекты».
- [ ] **Step 2: Проверить:** `cargo xtask ci` — зелёный.
- [ ] **Step 3: Коммит:** `git commit -m "Document activity import in CLAUDE.md, architecture and README"`

---

### Task 17: финальная проверка с пользователем

Последний шаг, как в проекте 1. Строитель не трогает настоящий ключ пользователя.

- [ ] **Step 1: Пользователь выгружает оригиналы** в Garmin Connect (тренировка → ⚙ → «Экспортировать оригинал») в `/Users/alekseinovikov/gymgar/temp/garmin-samples/`:
  - 16.06.2026 и 17.10.2026 — подходы сверх программы;
  - 29.08.2026 и 04.07.2026 — пустые подходы.
- [ ] **Step 2: Строитель сверяет разбор:**
  - прогоняет `cargo xtask scrub-fit` для каждого файла;
  - смотрит, как в настоящих файлах закодированы подходы без упражнения и пустые подходы;
  - если иначе, чем в синтетике, — правит `garmin::fit::parse` или `sync::strength` с тестом, который падал до правки;
  - добавляет очищенные копии в `fixtures/garmin/` как регрессионные (`no-exercise.zip`, `empty-set.zip`) с тестами в `sync`.
- [ ] **Step 3: Пользователь проходит чеклист на релизной сборке** (`cargo run --release -p gymgar`, macOS):
  1. первая загрузка всей истории: окно не замирает, видно «Загружаю: N из M», «новых» нет;
  2. пустые подходы не учитываются, разминка учитывается, «учитывать» возвращает пустой подход;
  3. после новой тренировки — пометка и число на иконке, снятие при открытии;
  4. правки (вес, повторы, «не учитывать», «Без упражнения» → упражнение, добавленный подход) переживают «Обновить» и перезапуск;
  5. агент через MCP находит тренировки по упражнению и периоду, правит подход, открытое окно это видит.
- [ ] **Step 4: Записать в `docs/architecture.md` §6.1:** глубину истории (дату самой старой активности) и замеченное о лимитах (были ли 429). Коммит: `git commit -m "Record history depth and rate-limit behaviour from the live account"`
- [ ] **Step 5: CI зелёный на трёх ОС**, `cargo xtask ci` — локально.
