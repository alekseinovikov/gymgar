# Gymgar

English · [Русский](#русский)

## English

Gymgar is a native desktop app for strength training, written in Rust (egui/eframe, no webview). Its main value is analytics that Garmin Connect does not have: weight progress per exercise over time, muscle-group load by week and month, and how the body responds (sleep, HRV) in relation to training. All data comes from your Garmin account. The interface is in English and Russian, and an MCP server lets AI agents do what you do in the window.

**Status: early.** This is the first of several projects. What works today: connecting Gymgar to Garmin Connect with your personal API key, showing the state of the connection (with a warning a week before the key expires), switching the interface language, and an MCP server that reports the connection state and can change the language. Loading activities, charts, reports and the workout editor come in later projects.

### Build and run

You need a stable Rust toolchain (1.99 or newer). On Linux you also need the X11/Wayland development packages that egui builds against; the list CI installs is in [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

```sh
cargo run --release -p gymgar    # the app
cargo run -p gymgar              # a debug build, see below
cargo xtask ci                   # fmt, clippy, tests and the layering check: what CI runs
```

A debug build never mixes with an installed one: it uses its own data directory (`Gymgar Dev`, `gymgar-dev` on Linux) and keeps the Garmin key in a file instead of the system keychain, so macOS does not ask for keychain access after every rebuild.

Gymgar keeps its data in one folder per user:

| System | Release build | Debug build |
|---|---|---|
| macOS | `~/Library/Application Support/Gymgar/` | `~/Library/Application Support/Gymgar Dev/` |
| Windows | `%APPDATA%\Gymgar\data\` | `%APPDATA%\Gymgar Dev\data\` |
| Linux | `~/.local/share/gymgar/` | `~/.local/share/gymgar-dev/` |

In it: the database `gymgar.sqlite3`, the logs `logs/gymgar.log` (the previous run is kept as `gymgar.1.log`; set `RUST_LOG` to change the level) and, only if you agree to it, `secrets/garmin-personal-api-key`.

### Connect Garmin

Gymgar reads your data through Garmin's official Personal API. You issue a personal key yourself, without applying for anything:

1. Open the [Personal API Key](https://connect.garmin.com/app/settings/personalApiToken) page in Garmin Connect and sign in.
2. Click **Create Key** and accept the terms.
3. Choose how long the key is valid: 30, 90, 180 days or 1 year (1 year is the most convenient).
4. Click **Create** and copy the key right away: Garmin shows it only once.

Then open **Settings** in Gymgar, paste the key, choose the same term and click **Connect**. Gymgar checks the key with Garmin and saves it only if Garmin accepts it. The key does not carry its own expiry date, so Gymgar asks for the term you chose and warns you a week before it ends. You can delete the key on the same Garmin page at any time.

Where the key is kept: in the macOS Keychain, the Windows Credential Manager or the system password store (Secret Service) on Linux. If a release build finds no system store, Gymgar offers to save the key to a file readable only by you and names that file; it does so only after you agree. The key is never written to the database, the logs or an error message.

A real key is 132 characters long. **On macOS, do not enter it into a terminal prompt, for example through the `security` tool asking for the password interactively: the terminal cuts the line at 128 characters, and Garmin then answers 401.** Paste the key into Gymgar's field, which takes any length.

### Use Gymgar from an AI agent (MCP)

`gymgar-cli mcp` is an MCP server over stdio on the same database as the app. Build it and register it, for example in Claude Code:

```sh
cargo build --release -p gymgar-cli
claude mcp add gymgar -- <path>/gymgar-cli mcp    # e.g. <path> = /path/to/gymgar/target/release
```

Build the app and the CLI in the same profile (both release or both debug), or they will look at different data directories. The server offers two tools: `command_list` shows the commands an agent may run, with their parameters, and `command_run` runs one. Today an agent can read the connection state (`garmin.status`) and the settings (`settings.get`), and change the language (`settings.setLanguage`); an open Gymgar window switches within a second. An agent cannot enter, replace or delete the key, and `gymgar-cli` never opens the keychain.

### Licence

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Bundled third-party resources are listed in [`ATTRIBUTION.md`](ATTRIBUTION.md). For contributors: [`CLAUDE.md`](CLAUDE.md) (rules, in Russian) and [`docs/architecture.md`](docs/architecture.md).

## Русский

Gymgar — нативное desktop-приложение на Rust для силовых тренировок (egui/eframe, без webview). Главная ценность — аналитика, которой нет в Garmin Connect: прогресс весов по каждому упражнению во времени, нагрузка на группы мышц по неделям и месяцам, реакция тела (сон, ВСР) в связке с тренировками. Все данные приходят из вашего аккаунта Garmin. Интерфейс на английском и русском, а MCP-сервер позволяет агентам делать то же, что вы делаете в окне.

**Статус: ранний.** Это первый из нескольких проектов. Сегодня работает: подключение Gymgar к Garmin Connect личным ключом API, статус подключения (с предупреждением за неделю до конца срока ключа), переключение языка интерфейса и MCP-сервер, который показывает статус подключения и умеет менять язык. Загрузка активностей, графики, отчёты и редактор тренировок появятся в следующих проектах.

### Сборка и запуск

Нужен stable Rust (1.99 или новее). На Linux ещё нужны пакеты разработки X11/Wayland, от которых зависит egui; список, который ставит CI, лежит в [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

```sh
cargo run --release -p gymgar    # приложение
cargo run -p gymgar              # отладочная сборка, см. ниже
cargo xtask ci                   # fmt, clippy, тесты и проверка слоёв: то, что гоняет CI
```

Отладочная сборка не пересекается с установленной: у неё свой каталог данных (`Gymgar Dev`, на Linux `gymgar-dev`), а ключ Garmin она хранит в файле, а не в системной связке ключей, поэтому macOS не спрашивает доступ к ней после каждой пересборки.

Данные Gymgar лежат в одном каталоге на пользователя:

| Система | Релизная сборка | Отладочная сборка |
|---|---|---|
| macOS | `~/Library/Application Support/Gymgar/` | `~/Library/Application Support/Gymgar Dev/` |
| Windows | `%APPDATA%\Gymgar\data\` | `%APPDATA%\Gymgar Dev\data\` |
| Linux | `~/.local/share/gymgar/` | `~/.local/share/gymgar-dev/` |

В нём: база `gymgar.sqlite3`, логи `logs/gymgar.log` (лог предыдущего запуска хранится как `gymgar.1.log`; уровень меняет `RUST_LOG`) и, только если вы на это согласились, `secrets/garmin-personal-api-key`.

### Как подключить Garmin

Gymgar читает ваши данные через официальный Personal API Garmin. Личный ключ вы выпускаете сами, заявок подавать не нужно:

1. Откройте страницу [Personal API Key](https://connect.garmin.com/app/settings/personalApiToken) в Garmin Connect и войдите в аккаунт.
2. Нажмите **Create Key** и примите условия.
3. Выберите срок действия: 30, 90, 180 дней или 1 год (удобнее всего 1 год).
4. Нажмите **Create** и сразу скопируйте ключ: Garmin показывает его один раз.

Затем откройте в Gymgar **Настройки**, вставьте ключ, выберите тот же срок и нажмите **Подключить**. Gymgar проверит ключ у Garmin и сохранит его, только если Garmin его принял. Срока действия в самом ключе нет, поэтому Gymgar спрашивает выбранный вами срок и предупреждает за неделю до конца. Удалить ключ можно в любой момент на той же странице Garmin.

Где хранится ключ: в связке ключей macOS, в диспетчере учётных данных Windows или в хранилище паролей системы (Secret Service) на Linux. Если релизная сборка не находит системного хранилища, Gymgar предлагает сохранить ключ в файл, который читает только владелец, и называет этот файл; без вашего согласия он этого не делает. Ключ никогда не попадает в базу, в логи и в тексты ошибок.

Настоящий ключ — 132 символа. **В терминале macOS не вводите его в интерактивный запрос, например через утилиту `security`, которая спрашивает пароль сама: терминал обрезает строку до 128 символов, и Garmin отвечает 401.** Вставляйте ключ в поле Gymgar: оно принимает любую длину.

### Gymgar для ИИ-агента (MCP)

`gymgar-cli mcp` — MCP-сервер по stdio на той же базе, что и приложение. Соберите его и зарегистрируйте, например, в Claude Code:

```sh
cargo build --release -p gymgar-cli
claude mcp add gymgar -- <путь>/gymgar-cli mcp    # например, <путь> = /путь/к/gymgar/target/release
```

Приложение и CLI собирайте в одном профиле (оба release или оба debug), иначе они смотрят в разные каталоги данных. Сервер даёт два инструмента: `command_list` показывает команды, которые агенту можно запускать, вместе с их параметрами, а `command_run` запускает одну из них. Сейчас агент может прочитать статус подключения (`garmin.status`) и настройки (`settings.get`) и сменить язык (`settings.setLanguage`); открытое окно Gymgar переключится за секунду. Агент не может ввести, заменить или удалить ключ, а `gymgar-cli` никогда не открывает связку ключей.

### Лицензия

[MIT](LICENSE-MIT) или [Apache-2.0](LICENSE-APACHE) на ваш выбор. Сторонние ресурсы перечислены в [`ATTRIBUTION.md`](ATTRIBUTION.md). Контрибьюторам: [`CLAUDE.md`](CLAUDE.md) (правила) и [`docs/architecture.md`](docs/architecture.md).
