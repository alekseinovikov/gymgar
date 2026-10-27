# Прогресс по упражнению и подсказка — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Цель:** раздел «Прогресс»: плитки упражнений с подсказкой «держим или добавляем», упражнение целиком с графиком «вес на N повторов» по месяцам, карточка на «Сводке» и строка в «Тренировках»; агент видит то же через `progress.list` и `progress.get`.

**Архитектура:** вертикальный срез снизу вверх. Правило подсказки и ряд графика — чистые функции `domain::progress` над занятиями упражнения. `store` отдаёт плоскую историю учитываемых подходов, `engine` группирует её в занятия, зовёт `domain` и отдаёт сериализуемые ответы. `ui-egui` только рисует: плитки, страницу упражнения (`egui_plot`), карточку и строку. Записи нет нигде: обе команды только читают.

**Стек:** как в проектах 1–2 (Rust 2024, egui 0.37, rusqlite, schemars, chrono). Новая зависимость — `egui_plot = "0.38"` (зависит от egui 0.37, проверено `cargo info`), только в `ui-egui`.

**Спека:** [`docs/superpowers/specs/2026-10-27-exercise-progress-design.md`](../specs/2026-10-27-exercise-progress-design.md). Читать вместе с планом: правило (§7), тексты подсказки (§7), экраны (§2), анти-цели (§4). Макеты: [`2026-10-27-exercise-progress-mockups/`](../specs/2026-10-27-exercise-progress-mockups/) — плитки (вариант B) и график (вариант A).

## Global Constraints

- Действуют все правила `CLAUDE.md`: в не-тестовом коде нет `unwrap`, `expect`, `panic!`, индексации `[i]`; `unsafe` запрещён; арифметика над данными — `checked_*`/`saturating_*`; перед коммитом `cargo xtask ci` зелёный.
- Веса сравниваются и считаются в целых граммах (`Mass`), без `f64`. `f64` — только в ответах (`weightKg`) и на графике.
- Константы `domain::progress`: `BREAK_DAYS = 28` (ровно 28 дней — ещё не перерыв), `DORMANT_DAYS = 182`, `ADD_PERCENT = 5`, шаг `1 000..=5 000` г, шаг по умолчанию `2 500` г, `SPARKLINE_MONTHS = 12`.
- **Повторы занятия** — наименьшие среди подходов на рабочем весе: цель взята, только если её сделали все они (решение пользователя).
- **Новый вес** — наименьшее кратное шага, строго больше W и не меньше `ceil(W × 105 / 100)`; больше 1000 кг — «держим» без нового веса.
- Не упражнения для прогресса: «без упражнения», категория 65534 и не силовые категории FIT (список в задаче 3). Упражнения, где ни в одном подходе нет повторов, в прогресс не входят.
- Месяц и день занятия — по местному времени начала из Garmin (`startTimeLocal` в `activity.raw_json`, формат `YYYY-MM-DD HH:MM:SS`); нет или не читается — по UTC. Две активности одного местного дня — одно занятие.
- `progress.list` и `progress.get` — `Query`, `agent: true`, `destructive: false`, `enabled: always`. Разрушительной по-прежнему остаётся только `garmin.disconnect`; агенту видно 13 команд.
- В ответах нет сырого JSON Garmin, файлов, `owner*`-полей.
- Строки UI — английские литералы в `tl!()`, перевод в `crates/ui-egui/i18n/ru.tsv`; русские тексты — из спеки (§2, §7 «Тексты подсказки»). Названия упражнений — данные, не `tl!`. Значения подставляются через `{}` и `.replace`, как в `activities_ui.rs`.
- Цвета и размеры — только `theme::Tokens` и `theme::metrics`. Новые общие виджеты — в `widgets.rs`.
- Fixtures и тестовые данные — синтетические. Копия базы пользователя (задача 5) и вывод по ней остаются вне репозитория.

## Review Focus

1. **План с нулём повторов** (шаг «до отказа» или пустой план Garmin: `plan_reps = 0`). Ожидание: такой план не считается целью, иначе «R ≥ 0» давало бы «можно добавлять» после любых двух занятий. Тест — в задаче 1.
2. **Занятие «из будущего»** (часы с неверной датой: начало позже «сегодня»). Ожидание: не перерыв, не паника, отрицательные дни нигде не показываются. Тест — в задаче 1.
3. **Тренировка около полуночи на границе месяца** (23:30 местного 31 октября = 21:30 UTC, или 00:30 местного 1 ноября = 22:30 UTC 31 октября). Ожидание: месяц и день — местные. Тесты — в задачах 1 и 2.
4. **Открытое упражнение исчезло:** пользователь перенёс все его подходы в другое упражнение правкой (или агент сделал это, пока окно открыто). Ожидание: страница упражнения закрывается, показываются плитки, без ошибки в баннере. Тест — в задаче 8.
5. **Дробные и «странные» веса** (40,5 кг, 22,5 кг, рабочие 40 и 40,5). Ожидание: шаг не меньше 1 кг, новый вес кратен шагу и больше текущего (40,5 → 43). Тест — в задаче 1.

---

## Файловая структура

```text
Cargo.toml                                        # + egui_plot в [workspace.dependencies]
assets/icons/chart-line.svg  ATTRIBUTION.md       # иконка раздела (Lucide, ISC)
crates/domain/src/{lib.rs, progress.rs, progress_tests.rs}
crates/store/src/{lib.rs, activity_queries.rs, activity_queries_tests.rs}
crates/sync/src/exercises.rs                      # is_strength
crates/engine/src/{lib.rs, commands.rs, testing.rs, progress_views.rs, progress_views_tests.rs,
                   progress_cmds.rs, progress_cmds_tests.rs}
crates/automation/src/server/tests/activity.rs    # видимость progress.* агенту
crates/ui-egui/Cargo.toml                         # + egui_plot
crates/ui-egui/src/{state.rs, icons.rs, rail.rs, app.rs, widgets.rs, summary_ui.rs, activity_detail_ui.rs,
                    progress_ui.rs, progress_ui_tests.rs, progress_detail_ui.rs, progress_detail_ui_tests.rs,
                    progress_chart.rs, snapshot_tests.rs, test_support.rs}
crates/ui-egui/i18n/ru.tsv
CLAUDE.md  docs/architecture.md  README.md
```

---

### Task 1: правило подсказки и ряд графика в `domain`

**Files:**
- Create: `crates/domain/src/progress.rs`, `crates/domain/src/progress_tests.rs`
- Modify: `crates/domain/src/lib.rs` (`pub mod progress;`)

**Interfaces:**
- Consumes: `Mass`, `Reps`, `ActivityId`, `SetRef` (проект 2).
- Produces (модуль `gymgar_domain::progress`):
  - константы из Global Constraints: `BREAK_DAYS: i64`, `DORMANT_DAYS: i64`, `ADD_PERCENT: i64`, `STEP_MIN_G: i64`, `STEP_MAX_G: i64`, `STEP_DEFAULT_G: i64`, `SPARKLINE_MONTHS: u32`;
  - `ProgressSet { activity: ActivityId, set: SetRef, weight: Option<Mass>, reps: Option<Reps>, plan_reps: Option<Reps> }` (`Debug, Clone, Copy, PartialEq`);
  - `ExerciseSession { start: DateTime<Utc>, day: NaiveDate, workout_name: Option<String>, sets: Vec<ProgressSet> }` — учитываемые подходы одного упражнения за местный день, в порядке тренировки; `start` — начало первой активности дня;
  - `WorkingSets { weight: Mass, reps: u16, all_reps: Vec<u16>, set: ProgressSet }` — рабочий вес (нет веса = 0), наименьшие повторы на нём, повторы всех подходов на нём по порядку, первый подход с наименьшими повторами;
  - `working_sets(session: &ExerciseSession) -> Option<WorkingSets>` — `None` только для пустого занятия;
  - `target_reps(sessions: &[ExerciseSession]) -> Option<u16>` — `sessions` от старых к новым; повторы плана рабочего подхода последнего занятия с планом, иначе самые частые повторы плана других подходов того же занятия (при равенстве — большие); **план с 0 повторов — не план**;
  - `weight_step(sessions: &[ExerciseSession]) -> Mass` — наименьшая положительная разница различных ненулевых рабочих весов, ограниченная `STEP_MIN_G..=STEP_MAX_G`; меньше двух весов — `STEP_DEFAULT_G`;
  - `next_weight(weight: Mass, step: Mass) -> Option<Mass>` — наименьшее кратное `step`, строго больше `weight` и не меньше `ceil(weight × (100 + ADD_PERCENT) / 100)`; `None`, если не помещается в `Mass` или `step` — 0;
  - `AfterBreak { days: i64, before: usize }`;
  - `Advice` (`Debug, Clone, PartialEq`), индексы — в срезе `sessions`:
    - `Add { weight: Mass, to: Mass, target: u16, previous: usize, last: usize }`;
    - `Hold { weight: Mass, reps: u16, target: u16, sessions_at_weight: u32, met_once: bool, after_break: Option<AfterBreak> }`;
    - `Break { days: i64, last: usize }`;
    - `Bodyweight { last: usize }`;
    - `NoTarget { last: usize }`;
  - `advise(sessions: &[ExerciseSession], today: DateTime<Utc>) -> Option<Advice>` — `None` без занятий; порядок проверок спеки §7: перерыв → без веса → нет цели → можно добавлять → держим;
  - `chart_reps(sessions: &[ExerciseSession]) -> u16` — цель, а без неё самые частые повторы занятий (при равенстве — большие);
  - `SeriesMode { Weight { reps: u16 }, Reps }`;
  - `MonthPoint { month: NaiveDate /* 1-е число */, weight: Mass, reps: u16, session: usize, set: ProgressSet }`;
  - `Gap { from: usize, to: Option<usize>, days: i64 }` — между занятиями `from` и `to`; `to: None` — перерыв идёт до сегодня;
  - `Series { mode: SeriesMode, points: Vec<MonthPoint>, heavier: Vec<MonthPoint>, gaps: Vec<Gap> }`;
  - `series(sessions: &[ExerciseSession], today: DateTime<Utc>) -> Series` — правила спеки §7 «Ряд графика»; режим `Reps`, только если у всех занятий рабочий вес 0;
  - `days_between(from: DateTime<Utc>, to: DateTime<Utc>) -> i64` — целые сутки, не меньше 0 (занятие «из будущего» даёт 0).

- [ ] **Step 1: Написать падающие табличные тесты** в `progress_tests.rs` (помощник `session(day: &str, sets: &[(f64, u32, Option<u32>)]) -> ExerciseSession`, где вес 0.0 — «без веса»):
  - `working_sets_take_the_fewest_reps_at_the_top_weight`: `[(40,10),(60,10),(80,10),(90,6)]` → вес 90, повторы 6; `[(100,8),(100,8),(100,6)]` → 100, повторы 6, `all_reps == [8,8,6]`, `set` — третий подход; `[(0,12),(0,10)]` → вес 0, повторы 10; подход без повторов считается 0.
  - `target_comes_from_the_plan`: план 10 у рабочего → 10; у рабочего плана нет, у соседей 12, 12, 10 → 12; плана нет ни в одном занятии → `None`; **план 0 → `None`**; в последнем занятии плана нет, в предыдущем 8 → 8.
  - `step_and_next_weight` (примеры спеки): рабочие 100/105/90/80 → шаг 5 000, `next(80) == 85`; 24/25/28/35 → шаг 1 000, `next(24) == 26`; 48/50/52/56 → 2 000, `next(52) == 56`; только 80 → 2 500, `next(80) == 85`; **40 и 40,5 → шаг 1 000, `next(40,5) == 43`**; разницы 10 и 20 → шаг 5 000; `next_weight(1000 кг, 5 кг) == None`; `next_weight(x, 0 г) == None`.
  - `advice_table` (сегодня = 2026-10-27 12:00 UTC):
    - два занятия 80×10 и 80×10 с промежутком 16 дней, цель 10 → `Add { weight: 80, to: 85, previous: 0, last: 1 }`;
    - 80×[10,10,9] и 80×10 → `Hold { reps: 9, met_once: false }`; 80×10 после 80×9 → `Hold { met_once: true, sessions_at_weight: 2 }`;
    - 85×10 после 80×10 (вес вырос) → `Hold { sessions_at_weight: 1, met_once: true }`;
    - промежуток ровно 28 дней между 80×10 и 80×10 → `Add`; 29 дней → `Hold { after_break: Some(AfterBreak { days: 29, before: 0 }) }`;
    - последнее занятие 28 дней назад → не `Break`; 29 дней назад → `Break { days: 29 }`;
    - рабочий вес последнего занятия 0, раньше 75 → `Bodyweight` (регрессия находки из спеки: не «добавить до 5 кг»);
    - плана нет нигде → `NoTarget`;
    - одно занятие 80×10 → `Hold { met_once: true, sessions_at_weight: 1 }`;
    - **занятие на 3 дня позже «сегодня»** → не `Break`, `days_between` даёт 0;
    - вес 1000 кг × 10 дважды → `Hold` (нового веса нет).
  - `series_rules`: в мае `[(40,10),(80,10),(90,10),(100,6)]` при N=10 → точка 90×10, кружок 100×6; месяц без подхода ≥ N → без точки, с кружком; кружок не выше точки — не попадает; два занятия в одном месяце → одна точка (тяжелейшая); подходы с весом 0 в режиме веса не дают точек; все рабочие веса 0 → `SeriesMode::Reps`, точка — наибольшие повторы занятия, кружков нет; промежуток 66 дней → `Gap { days: 66 }`; последнее занятие 40 дней назад → `Gap { to: None }`.
  - `month_is_the_local_day_month`: занятие с `day = 2026-11-01` и `start = 2026-10-31T22:30Z` → точка месяца 2026-11-01.
- [ ] **Step 2: Запустить и убедиться, что падают:** `cargo test -p gymgar-domain progress` — ошибки компиляции «not found».
- [ ] **Step 3: Реализовать `progress.rs`** с интерфейсами выше. Модуль начинается с `//!` (за что отвечает: правило подсказки и ряд графика; чего не делает: не читает хранилище и не знает о UI). Целочисленная арифметика с `checked_*`; `ceil` в граммах — `(w * 105 + 99) / 100` через `checked_mul`/`checked_add`.
- [ ] **Step 4: Запустить:** `cargo test -p gymgar-domain` — PASS; `cargo clippy -p gymgar-domain --all-targets -- -D warnings` — чисто.
- [ ] **Step 5: Commit** — `git commit -m "Add the progress advice rule and chart series to domain"`.

### Task 2: история учитываемых подходов в `store`

**Files:**
- Modify: `crates/store/src/activity_queries.rs`, `crates/store/src/activity_queries_tests.rs`, `crates/store/src/lib.rs` (экспорт `HistorySet`)

**Interfaces:**
- Consumes: SQL-фрагменты учёта `RECORDED_FROM`, `RECORDED_CATEGORY`, `RECORDED_NAME`, `recorded_counts()`, `ADDED_COUNTS` того же файла.
- Produces:
  - `HistorySet { activity: ActivityId, start: DateTime<Utc>, local_start: Option<NaiveDateTime>, workout_name: Option<String>, set: SetRef, exercise: ExerciseId, weight: Option<Mass>, reps: Option<Reps>, plan_reps: Option<Reps> }` (`Debug, Clone, PartialEq`);
  - `Store::strength_history(&self) -> Result<Vec<HistorySet>, StoreError>` — все учитываемые подходы неисключённых активностей с итоговым упражнением (не NULL и не 65534), итоговыми весом и повторами; у подходов из записи `plan_reps` из `recorded_set`, у добавленных — `None`. Порядок: по `start_at`, затем подходы из записи по `message_index`, затем добавленные по `id`. `local_start` — `json_extract(a.raw_json, '$.startTimeLocal')`, разобранный форматом `%Y-%m-%d %H:%M:%S`; не разобрался — `None` (без ошибки). Миграции нет.

- [ ] **Step 1: Написать падающие тесты** (помощники `test_support`):
  - `history_holds_counted_sets_with_corrections`: правка веса, повторов и упражнения видна в итоговых значениях; подход с `counted = false` не попадает; пустой подход (правило) не попадает, но с `counted = true` попадает; «без упражнения» и 65534 не попадают; исключённая активность не попадает; убранный добавленный подход не попадает, добавленный — попадает с `plan_reps: None`.
  - `history_order_and_plan`: две активности в обратном порядке вставки → по времени начала; внутри — по `message_index`, добавленные в конце; `plan_reps` из записи.
  - `history_local_start`: `raw_json` с `"startTimeLocal":"2026-11-01 00:30:00"` → `local_start` 2026-11-01T00:30; без поля или с мусором → `None`.
  - В существующий `the_stored_counts_the_filter_and_the_usage_agree` добавить: число строк `strength_history()` равно сумме `counted_sets` по активностям за вычетом подходов «без упражнения».
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-store history` — FAIL (нет метода).
- [ ] **Step 3: Реализовать `strength_history`** одним запросом `UNION ALL` (записанные с правками и добавленные) на тех же фрагментах учёта, без второй копии логики; чтение колонок через `read_mass`, `read_reps`, `read_exercise`, `parse_time`.
- [ ] **Step 4: Запустить:** `cargo test -p gymgar-store` — PASS.
- [ ] **Step 5: Commit** — `git commit -m "Read the counted strength history from the store"`.

### Task 3: представления прогресса в `engine`

**Files:**
- Create: `crates/engine/src/progress_views.rs`, `crates/engine/src/progress_views_tests.rs`
- Modify: `crates/sync/src/exercises.rs` (+ `is_strength`), `crates/engine/src/lib.rs` (`mod progress_views;` и `pub use` представлений)

**Interfaces:**
- Consumes: `Store::strength_history`, `HistorySet` (задача 2); всё из `gymgar_domain::progress` (задача 1); `exercise_view(id, titles) -> ExerciseView` из `activity_views.rs`.
- Produces:
  - `gymgar_sync::exercises::is_strength(id: ExerciseId) -> bool` — `false` для категории 65534 и категорий с ключами `warm_up`, `cardio`, `run`, `bike`, `cardio_sensors`, `move`, `pose`, `elliptical`, `floor_climb`, `indoor_bike`, `indoor_row`, `stair_stepper`, `run_indoor`, `bike_outdoor` (сравнение по `exercise_key(ExerciseId { category, name: None })`);
  - `AdviceKind { Add, Hold, Break, Bodyweight, NoTarget }` (serde camelCase, `JsonSchema` — параметр фильтра);
  - `WorkingView { weight_kg: f64, reps: Vec<u16>, date: NaiveDate, activity_id: ActivityId }` — рабочий вес и повторы всех подходов на нём;
  - `AdviceView { kind: AdviceKind, target: Option<u16>, next_weight_kg: Option<f64>, sessions_at_weight: Option<u32>, met_once: bool, basis: Vec<WorkingView> /* Add: [предыдущее, последнее] */, break_days: Option<i64> /* Break; Hold после перерыва — его длина */, before_break: Option<WorkingView> }`;
  - `PointView { month: String /* "2026-08" */, weight_kg: f64, reps: u16, date: NaiveDate, activity_id: ActivityId }`;
  - `ProgressItemView { exercise: ExerciseView, advice: AdviceView, last: WorkingView, sessions: u32, first_date: NaiveDate, dormant: bool, chart_reps: Option<u16> /* None — режим повторов */, sparkline: Vec<PointView> }`;
  - `ProgressListView { items: Vec<ProgressItemView> }`;
  - `GapView { from: NaiveDate, to: Option<NaiveDate>, days: i64 }`;
  - `SessionView { date: NaiveDate, start: DateTime<Utc>, activity_id: ActivityId, workout_name: Option<String>, working: WorkingView, sets: Vec<SetBriefView> }`, `SetBriefView { weight_kg: Option<f64>, reps: Option<u16> }`;
  - `ProgressView { exercise: ExerciseView, summary: Option<ProgressItemView>, step_kg: Option<f64>, points: Vec<PointView>, heavier: Vec<PointView>, gaps: Vec<GapView>, sessions: Vec<SessionView> /* новые сверху */ }`;
  - `pub(crate) fn sessions_by_exercise(history: Vec<HistorySet>) -> BTreeMap<ExerciseId, Vec<ExerciseSession>>` — фильтр `is_strength`, отбрасывает упражнения, где ни у одного подхода нет повторов > 0; день — `local_start.date()` или UTC-дата `start`; один упражнение-день — одно занятие;
  - `pub(crate) fn list_view(by_exercise: &BTreeMap<..>, titles: &HashMap<ExerciseId, String>, today: DateTime<Utc>) -> Vec<ProgressItemView>` — порядок плиток спеки §2: `add`, затем `hold`/`bodyweight`/`noTarget`, затем `break`, затем `dormant`; внутри — по дате последнего занятия, новые выше; при равенстве — по ключу упражнения;
  - `pub(crate) fn full_view(exercise: ExerciseId, sessions: Option<&[ExerciseSession]>, titles: &HashMap<..>, today: DateTime<Utc>) -> ProgressView`.

- [ ] **Step 1: Написать падающие тесты** в `progress_views_tests.rs` на построенных вручную `HistorySet`:
  - `is_strength_drops_warm_up_cardio_and_run` (в `sync`): `warm_up`, `cardio`, `run`, `indoor_row`, 65534 → `false`; `deadlift/romanian_deadlift`, `curl` (категория без номера), `plank` → `true`.
  - `same_local_day_is_one_session`: две активности 2026-10-20 (UTC 06:00 и 18:00, местные даты одинаковые) → одно занятие с подходами обеих; `start` — ранний.
  - `exercises_without_reps_are_left_out`: упражнение, у которого все подходы без повторов, не попадает в `list_view`.
  - `list_order_follows_the_tiles`: набор из пяти упражнений со статусами add, hold, noTarget, break (40 дней), dormant (200 дней) → именно такой порядок; `dormant == true` только у последнего.
  - `views_carry_the_basis`: для `add` — `basis` из двух `WorkingView` с датами занятий и `next_weight_kg == Some(85.0)`; для `hold` после перерыва — `break_days` и `before_break`.
  - `sparkline_is_the_last_twelve_months`: точки за 14 месяцев → в `sparkline` только последние 12 месяцев до сегодня.
  - `full_view_of_an_exercise_never_done`: `summary: None`, пустые ряды и занятия.
  - `sessions_are_newest_first_with_all_sets`.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-engine progress_views` и `cargo test -p gymgar-sync is_strength` — FAIL.
- [ ] **Step 3: Реализовать** `is_strength` и `progress_views.rs` (`//!` в начале: собирает занятия и ответы, ничего не читает и не пишет сам). Вес в ответах — `Mass::kg()`.
- [ ] **Step 4: Запустить:** `cargo test -p gymgar-engine -p gymgar-sync` — PASS.
- [ ] **Step 5: Commit** — `git commit -m "Build the progress views from the strength history"`.

### Task 4: команды `progress.list` и `progress.get`

**Files:**
- Create: `crates/engine/src/progress_cmds.rs`, `crates/engine/src/progress_cmds_tests.rs`
- Modify: `crates/engine/src/commands.rs` (`v.extend(progress_cmds::specs())`, две строки в `SPEC_TABLE`), `crates/engine/src/lib.rs`, `crates/engine/src/testing.rs`, `crates/automation/src/server/tests/activity.rs`

**Interfaces:**
- Consumes: задача 3; `Session::store()`, `Session::now()`, `params::{parse, schema, bad_params}`, `commands::{always, done}`; `parse_exercise_key`, `catalog()` из `gymgar_sync::exercises`.
- Produces:
  - `ListParams { status: Option<Vec<AdviceKind>>, limit: Option<u32> /* 1..=1000, по умолчанию 100 */, offset: Option<u32> }` и `GetParams { exercise: String }` — `deny_unknown_fields`, camelCase, `JsonSchema` с описаниями полей;
  - команды по таблице спеки §7 «Команды»; `progress.get` с ключом, который не разбирается или не называет ни упражнение каталога, ни упражнение из данных, — `BadParams`;
  - в `engine::testing`:
    - `strength_set(index: u16, exercise: ExerciseId, weight_kg: f64, reps: u32, plan_reps: Option<u32>) -> RecordedSet` (`weight_kg == 0.0` — `weight: Some(0)`);
    - `seed_workout(store: &Store, id: i64, local_start: &str /* "2026-10-20 07:55:00" */, utc_offset_hours: i32, workout_name: Option<&str>, sets: Vec<RecordedSet>) -> ActivityId` — силовая с `raw_json`, где есть `startTimeLocal`, и подходами (через `upsert_activities` и `apply_file`); времена подходов идут от начала.

- [ ] **Step 1: Написать падающие тесты** в `progress_cmds_tests.rs`:
  - `list_answers_what_the_store_holds`: три тренировки через `seed_workout` → `progress.list` с `Caller::Agent` даёт ожидаемые статусы и порядок; `status: ["add"]` оставляет только `add`; `limit`/`offset` режут список.
  - `get_answers_one_exercise`: `progress.get {"exercise": "deadlift/romanian_deadlift"}` — точки, кружки, занятия, основание подсказки; упражнение каталога, которого нет в данных, — `summary: null`.
  - `bad_params_are_errors_not_panics`: `{"exercise": "nonsense/!!"}`, `{"exercise": 5}`, `{"limit": 0}`, `{"limit": 1001}`, `{"status": ["maybe"]}`, `{"unknown": 1}`, `[]` → `EngineError::BadParams`.
  - `a_correction_moves_sessions_between_exercises`: `activity.editSets` меняет упражнение всех подходов занятия → занятие ушло из одного упражнения и пришло в другое; правка веса меняет статус с `add` на `hold`; `activity.setExcluded` убирает занятие.
  - `answers_carry_no_raw_garmin_data`: в JSON ответов нет `raw`, `owner`, `zip`.
  - `progress_on_a_copy_of_the_users_database` — `#[ignore]`: путь из `GYMGAR_PROGRESS_DB`, без переменной тест ничего не делает; открывает копию `Store::open`, печатает таблицу `progress.list` (название, рабочий вес и повторы последнего занятия, статус, новый вес). Нужен задаче 5.
  - В `commands.rs`: две строки `SPEC_TABLE` (`("progress.get", Query, true, false)`, `("progress.list", Query, true, false)`).
  - В `automation`: `progress.list` и `progress.get` есть в `command_list` агента и выполняются через `command_run`.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-engine progress` и `cargo test -p gymgar-automation` — FAIL.
- [ ] **Step 3: Реализовать `progress_cmds.rs`** по образцу `exercise_cmds.rs`: описания команд для агента (английские, говорят, что такое рабочий вес, цель и статусы); `run` читает `strength_history` и `exercise_titles`, зовёт `sessions_by_exercise` и `list_view`/`full_view`.
- [ ] **Step 4: Запустить:** `cargo test -p gymgar-engine -p gymgar-automation` — PASS.
- [ ] **Step 5: Проверить скорость:** `#[ignore]`-тест `progress_list_on_a_long_history` (300 тренировок по 40 подходов через `seed_workout`) печатает время `progress.list`; `cargo test --release -p gymgar-engine -- --ignored progress_list_on_a_long_history --nocapture`. Ориентир — меньше 16 мс (кадр); если больше — кэш ответа в `Session`, сбрасываемый по `store_changed` и мутациям, с тестом.
- [ ] **Step 6: Commit** — `git commit -m "Add progress.list and progress.get"`.

### Task 5: сверка правила на копии базы пользователя (контрольная точка)

Пользователь разрешил (спека §6). Исполнитель этой задачи — тот, кто ведёт план, а не подагент: нужен разговор с пользователем.

**Files:** нет изменений в репозитории (копия и вывод — во временной папке сессии).

- [ ] **Step 1: Скопировать базу** релизной сборки (`~/Library/Application Support/Gymgar/gymgar.sqlite3` вместе с `-wal` и `-shm`, если они есть) во временную папку сессии. Если у пользователя открыто окно Gymgar, попросить закрыть его перед копированием или скопировать все три файла разом.
- [ ] **Step 2: Прогнать** `GYMGAR_PROGRESS_DB=<копия> cargo test -p gymgar-engine -- --ignored progress_on_a_copy_of_the_users_database --nocapture`. Ожидание: таблица без паники; статусы сходятся с прогоном из спеки §5 (почти всё «держим, после перерыва», FB5–FB6 — «перерыв»), если пользователь с тех пор не тренировался и не догружал историю.
- [ ] **Step 3: Показать пользователю** (по-русски) подсказки по 5–6 основным упражнениям — рабочий вес, повторы, цель, статус, новый вес и основание — и спросить, совпадают ли они с его собственным решением.
- [ ] **Step 4: Если не совпадают** — понять почему, поправить правило в спеке (§7) с согласия пользователя, добавить регрессионный тест в задачу 1 и повторить шаги 2–3. Если совпадают — записать в журнал выполнения плана, что сверка пройдена.

### Task 6: раздел «Прогресс» в окне: состояние, полоса, чтение данных

**Files:**
- Create: `assets/icons/chart-line.svg` (Lucide `chart-line`, ISC — как остальные иконки)
- Modify: `ATTRIBUTION.md`, `crates/ui-egui/src/{state.rs, icons.rs, rail.rs, app.rs}`, `crates/ui-egui/src/state_tests.rs`, `crates/ui-egui/src/app_tests.rs`, `crates/ui-egui/i18n/ru.tsv`

**Interfaces:**
- Consumes: `ProgressListView`, `ProgressItemView`, `ProgressView` (задача 3); команды (задача 4).
- Produces:
  - `Section::Progress` (serde `"progress"`), в полосе между «Тренировками» и «Настройками»: `tl!("Progress")`, `Icon::ChartLine`;
  - `UiState.progress: ProgressState { filter: ProgressFilter, open: Option<String> }`, `ProgressFilter { #[default] All, Add, Break }` — сохраняются; несуществующий ключ — ничего не открыто;
  - в `GymgarApp`: поля прогресса; `pub fn progress(&self) -> &[ProgressItemView]`; `pub(crate) fn progress_detail(&self) -> Option<&ProgressView>`; `pub(crate) fn open_exercise(&mut self, key: Option<String>)`; `pub(crate) fn open_activity(&mut self, id: i64)` — раздел «Тренировки», фильтр «Все», выбранная активность `id`;
  - `refresh()` читает `progress.list` (`limit: 1000`; полный ответ — предупреждение в лог) по тем же поводам, что список активностей; открытое упражнение перечитывается `progress.get` так же, как открытая активность в `refresh_detail(force)`: при смене ключа и когда `tick` пометил данные устаревшими (команда, конец задачи, запись другого процесса); если открытого ключа больше нет в `progress()`, `ui.progress.open` сбрасывается.
- [ ] **Step 1: Написать падающие тесты:**
  - `state_tests`: `Section::Progress` и `ProgressState` переживают сохранение и загрузку; старое состояние без `progress` загружается с умолчаниями.
  - `app_tests`: `progress_is_read_with_the_activities` — после `seed_workout` и `tick` `app.progress()` не пуст; `a_correction_refreshes_progress` — `app.run("activity.editSets", ..)` меняет статус в `app.progress()`; `open_exercise_reads_its_detail`; `open_activity_switches_to_activities`.
  - rail: в дереве доступности есть кнопка `Progress` (и `Прогресс` в русском), клик делает раздел текущим.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-ui-egui` — FAIL.
- [ ] **Step 3: Реализовать.** Пока раздел рисует только заголовок `tl!("Progress")` (экраны — задачи 7–8). Строка в `ATTRIBUTION.md` и SVG — в этом же коммите. Перевод «Прогресс» в `ru.tsv`.
- [ ] **Step 4: Запустить:** `cargo test -p gymgar-ui-egui` — PASS.
- [ ] **Step 5: Commit** — `git commit -m "Add the Progress section and read progress into the window"`.

### Task 7: плитки упражнений

**Files:**
- Create: `crates/ui-egui/src/progress_ui.rs`, `crates/ui-egui/src/progress_ui_tests.rs`
- Modify: `crates/ui-egui/src/{widgets.rs, widgets_tests.rs, app.rs, lib.rs, test_support.rs}`, `crates/ui-egui/i18n/ru.tsv`

**Interfaces:**
- Consumes: задача 6; `exercise_name(&ExerciseView) -> &str` из `activity_detail_ui.rs`; `numbers::kg`, `dates`.
- Produces:
  - `progress_ui::show(app: &mut GymgarApp, ui: &mut Ui)` — плитки или, если `ui.progress.open` задан, `progress_detail_ui::show` (задача 8);
  - `progress_ui::advice_short(item: &ProgressItemView) -> String` — статус плитки: `↑ can add {}` (вес), `hold · {} of {}`, `hold · one more session`, `after a break`, `break · {} wk`, `bodyweight`, `no target`;
  - `progress_ui::working_text(last: &WorkingView, chart_reps: Option<u16>) -> String` — «80 kg × 10», «100 kg × 8, 8, 6», без веса — `trn` «{} rep / reps»;
  - в `widgets.rs`: `tile(ui: &mut Ui, accent: bool, dim: bool, add: impl FnOnce(&mut Ui)) -> Response` (кликабельная карточка; рамка `accent` у `add`); `advice_label(ui: &mut Ui, text: &str, tone: Tone)` с `Tone { Accent, Dim, Warning }`; `sparkline(ui: &mut Ui, values: &[f32], color: Color32, size: Vec2)` — ломаная `Painter`, пустой или одноточечный ряд не паникует;
  - в `ui-egui::test_support`: `app_with_workouts(lang: Lang, workouts: &[(&str /* local start */, Option<&str>, Vec<RecordedSet>)]) -> (GymgarApp, TestHandles)` поверх `seed_workout` (местное время UTC+2, Garmin подключён), часы сессии — `2026-10-27T12:00Z`.
- [ ] **Step 1: Написать падающие тесты** (`egui_kittest`, оба языка, где есть текст):
  - `tiles_show_every_status`: набор из задачи 3 `list_order_follows_the_tiles` → видны тексты всех статусов в нужном порядке (порядок узлов дерева доступности); у `add` — новый вес.
  - `chips_count_and_filter`: «Можно добавлять · 1», «Перерыв · 2»; клик по чипу оставляет только эти плитки; фильтр сохраняется в `UiState`.
  - `a_tile_opens_its_exercise`: клик по плитке задаёт `ui.progress.open` = ключ упражнения.
  - `empty_progress`: без силовых — «Progress will appear here once strength workouts are loaded»; Garmin не подключён — ещё и ссылка в «Настройки».
  - `working_text` и `advice_short` — табличный тест строк (en и ru, `trn` у недель и повторов).
  - `widgets_tests`: `sparkline` с 0, 1 и 12 точками не паникует.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-ui-egui progress` — FAIL.
- [ ] **Step 3: Реализовать** сетку плиток (`ui.columns` или `Grid`, число колонок = ширина / ~230 pt, не меньше 1), чипы через `widgets::chip`, приглушённые `dormant`. Плитки считаются из `app.progress()` без пересчёта каждый кадр (только фильтрация). Все строки — в `ru.tsv`.
- [ ] **Step 4: Запустить:** `cargo test -p gymgar-ui-egui` — PASS.
- [ ] **Step 5: Commit** — `git commit -m "Show exercises as tiles with their advice"`.

### Task 8: упражнение целиком и график

**Files:**
- Create: `crates/ui-egui/src/progress_detail_ui.rs`, `crates/ui-egui/src/progress_detail_ui_tests.rs`, `crates/ui-egui/src/progress_chart.rs`
- Modify: `Cargo.toml` (`egui_plot = "0.38"` с комментарием «графики прогресса; версия под egui 0.37»), `crates/ui-egui/Cargo.toml`, `crates/ui-egui/src/lib.rs`, `crates/ui-egui/i18n/ru.tsv`

**Interfaces:**
- Consumes: задачи 6–7; `ProgressView`.
- Produces:
  - `progress_detail_ui::show(app: &mut GymgarApp, ui: &mut Ui)` — «← Back», название, строка «{} sessions since {} · target — {} reps (program {})», карточка подсказки, график, список занятий (клик → `app.open_activity(id)`);
  - `progress_detail_ui::advice_texts(view: &ProgressView) -> Option<(String, String)>` — заголовок и объяснение по таблице спеки §7 «Тексты подсказки» (с вариантом для нескольких рабочих подходов и добавкой «после перерыва»);
  - `progress_chart::show(ui: &mut Ui, view: &ProgressView, height: f32)` — `egui_plot::Plot`: линия точек месяцев (`accent`), пунктир через перерывы, кружки `heavier` (`text_dim`, без заливки), закрашенные перерывы (`warning` с низкой непрозрачностью), подписи месяцев по языку, подсказка при наведении «{date} · {kg} × {reps}»; в режиме повторов ось — повторы. Перетаскивание и масштаб выключены.
- [ ] **Step 1: Написать падающие тесты:**
  - `advice_texts` — табличный: `add` («Can add: 85 kg» / «Можно добавлять: 85 кг» и даты основания), `hold` с одним и с несколькими рабочими подходами, `met_once`, после перерыва (длина и «до перерыва»), `break`, `bodyweight`, `noTarget`; оба языка.
  - `detail_shows_advice_chart_and_sessions`: kittest — заголовок подсказки, строки занятий; клик по занятию открывает «Тренировки» с этой активностью.
  - `back_returns_to_the_tiles`.
  - `the_open_exercise_that_vanished_closes` (Review Focus 4): открыто упражнение, все его подходы перенесены `activity.editSets` в другое → после `tick` открыты плитки, баннера ошибки нет.
  - `chart_draws_odd_series`: одна точка; только кружки; режим повторов; перерыв до сегодня — рендер без паники.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-ui-egui progress_detail` — FAIL.
- [ ] **Step 3: Реализовать.** Проверить `cargo tree -i egui_plot` и что `cargo xtask layers` пропускает `egui_plot` только в `ui-egui` (список UI-тулкита в `xtask/src/layers.rs` его уже называет — если нет, добавить).
- [ ] **Step 4: Запустить:** `cargo test -p gymgar-ui-egui` и `cargo xtask layers` — PASS.
- [ ] **Step 5: Commit** — `git commit -m "Show one exercise with its advice, chart and sessions"`.

### Task 9: подсказка на «Сводке» и в «Тренировках»

**Files:**
- Modify: `crates/ui-egui/src/{summary_ui.rs, summary_ui_tests.rs, activity_detail_ui.rs, activity_detail_ui_tests.rs}`, `crates/ui-egui/i18n/ru.tsv`

**Interfaces:**
- Consumes: `app.progress()`, `app.open_exercise`, `ProgressFilter::Add` (задачи 6–7).
- Produces:
  - карточка «Сводки» «Can add weight: {} exercise(s)» (`trn`) со строками «{name} {from} → {to} kg» (до пяти, дальше «and {} more»), кнопка «Open progress» — раздел «Прогресс», фильтр `Add`, ничего не открыто; карточки нет, если `add` нет;
  - строка в заголовке карточки упражнения открытой активности «↑ Can add: {} kg · open progress» — только если эта активность — последнее занятие упражнения (`item.last.activity_id`) и статус `add`; клик — `open_exercise(key)` в разделе «Прогресс».
- [ ] **Step 1: Написать падающие тесты:** карточка есть с двумя `add` и её нет без них; шесть `add` → пять строк и «и ещё 1»; кнопка ведёт в «Прогресс» с фильтром; строка в карточке упражнения есть у последнего занятия и нет у предыдущего того же упражнения; клик по ней открывает упражнение. Оба языка для текстов.
- [ ] **Step 2: Запустить:** `cargo test -p gymgar-ui-egui summary activity_detail` — FAIL.
- [ ] **Step 3: Реализовать.**
- [ ] **Step 4: Запустить:** `cargo test -p gymgar-ui-egui` — PASS.
- [ ] **Step 5: Commit** — `git commit -m "Point to the advice from the Summary and the open workout"`.

### Task 10: снапшоты и проверка глазами

**Files:**
- Modify: `crates/ui-egui/src/snapshot_tests.rs`

- [ ] **Step 1: Добавить состояния** в `render_snapshots` (1200×800 и 800×500, en и ru): `progress-tiles` (все статусы), `progress-add`, `progress-after-break` (с перерывом на графике и кружками), `progress-break`, `progress-bodyweight`, `progress-empty`, `summary-can-add`, `detail-can-add` (строка в карточке упражнения). Данные — синтетические, похожие на макеты.
- [ ] **Step 2: Отрендерить:** `GYMGAR_SNAPSHOTS=<временная папка> cargo test -p gymgar-ui-egui -- --ignored render_snapshots`.
- [ ] **Step 3: Открыть каждый PNG и сверить** с макетами и спекой §2: ничего не обрезано на 800×500, длинные русские названия упражнений не ломают плитку, цвета статусов из токенов, подписи осей читаются, перерыв закрашен. Найденное — исправить и перерендерить.
- [ ] **Step 4: Commit** — `git commit -m "Render the progress screens in the snapshot test"`.

### Task 11: документы и итоговая проверка

**Files:**
- Modify: `docs/architecture.md`, `CLAUDE.md`, `README.md`, `docs/superpowers/specs/2026-10-27-exercise-progress-design.md` (статус)

- [ ] **Step 1: `docs/architecture.md`** по спеке §11: решения 19–21, уточнение к решению 9, номера проектов в §6.1 (группы мышц — 5, здоровье — 6), §7 — 13 команд агента с `progress.list` и `progress.get`, строка истории изменений.
- [ ] **Step 2: `CLAUDE.md`:** карта воркспейса (`domain` — прогресс, `ui-egui` — экран «Прогресс» и файлы `progress_*`), список команд агента в §4, `egui_plot` подключён. **`README.md`:** раздел «Прогресс» (en и ru).
- [ ] **Step 3: Проверка:** `cargo xtask ci` — зелёный; запушить ветку, CI на трёх ОС зелёный.
- [ ] **Step 4: Commit** — `git commit -m "Document the progress project"`.
- [ ] **Step 5: Финальное ревью ветки** (superpowers:requesting-code-review) и правки по нему.
- [ ] **Step 6: Приёмка пользователем** на `cargo run --release -p gymgar` по спеке §12: догрузить историю («Обновить»), подсказки по 5–6 упражнениям совпадают с его решением, правка подхода меняет график и подсказку, агент через `gymgar-cli mcp` отвечает тем же списком «можно добавлять».
