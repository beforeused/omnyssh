<div align="center">

# OmnySSH — форк от beforeused

### Десктопный SSH-клиент: дашборд, терминал, SFTP, сниппеты — с быстрым фоновым SFTP и докачкой, полноценным файловым менеджером, панелью Docker, VPN через Tunnelblick и русским интерфейсом

<img src="assets/gui.webp" alt="OmnySSH" width="900">

</div>

> Это форк [**timhartmann7/omnyssh**](https://github.com/timhartmann7/omnyssh) (автор — Tim Hartmann, лицензия Apache 2.0).
> Форк основан на коммите [`07726c0`](https://github.com/timhartmann7/omnyssh/commit/07726c06560d75acd4d33d4a8c2cc46495b261fd) оригинала; вся его история сохранена.
> Разработчик форка — **beforeused**, Telegram: [@beforeused](https://t.me/beforeused).
>
> Все изменения касаются десктопного приложения (`crates/omnyssh-gui`) и общего ядра (`crates/omnyssh-core`). Терминальная версия (`omny`) работает как раньше.

---

## Что нового в форке

### SFTP: передачи в фоне и в разы быстрее

В оригинале загрузка и скачивание шли через то же соединение, что и просмотр папок, строго по одному файлу, с ожиданием ответа сервера на каждые 64 КБ. На сервере с пингом 50 мс это давало около 1 МБ/с, а открыть папку во время загрузки было нельзя. В форке написан новый движок передачи ([`crates/omnyssh-core/src/ssh/transfer.rs`](crates/omnyssh-core/src/ssh/transfer.rs)):

- **Отдельные соединения для передач.** 4 по умолчанию, от 1 до 8 в Настройки → Файлы. На каждом соединении по 2 SFTP-канала. Просмотр папок идёт по своему соединению и никогда не ждёт передачу.
- **Конвейер запросов.** В полёте держится до ~8 МБ запросов вместо одного; размер запроса берётся из расширения `limits@openssh.com`.
- **Большие файлы (от 16 МБ)** делятся на части, которые качаются параллельно через разные соединения.
- **Мелкие файлы** идут пачками, до 16 одновременно на соединение.
- **Папки целиком**, в обе стороны, с сохранением прав доступа файлов.
- **Безопасная запись.** Новые большие файлы пишутся во временный `.имя.omnyssh-part` и переименовываются только после полной загрузки. Мелкие при сбое удаляются. При замене существующего файла на сервере данные пишутся в него на месте, поэтому владелец, права и симлинки сохраняются.
- **Панель передач** под файлами: прогресс каждого файла, скорость, общий прогресс и оставшееся время, отмена, повтор, очистка. Закрыть вкладку с активными передачами можно только после подтверждения.

Замеры на OpenSSH с задержкой 50 мс туда-обратно (4 соединения):

| Операция | Оригинал | Форк |
|---|---|---|
| Загрузка файла 300 МБ | ~1,1 МБ/с | **~173 МБ/с** |
| Скачивание файла 300 МБ | — | **~420 МБ/с** |
| Загрузка 301 мелкого файла в 14 папках | — | **1,4 с** |

### Drag & drop

- Файлы и папки из Finder, Проводника или файлового менеджера перетаскиваются прямо в окно, и начинается загрузка на сервер. Если бросить на папку, файлы окажутся в ней.
- Файлы перетаскиваются между локальной и серверной панелями в обе стороны.
- Выделение работает как в файловом менеджере: клик, Shift+клик для диапазона, Cmd/Ctrl+клик, Cmd/Ctrl+A.
- Правый клик: открыть, быстрый просмотр, загрузить/скачать, переименовать, удалить, новая папка, открыть терминал здесь.

### Конфликты имён

Если файл с таким именем уже есть, приложение спрашивает: **Заменить**, **Пропустить** или **Оставить оба** (новый файл сохранится как `имя (1).ext`). Галочка «Применить ко всем» отвечает сразу на все конфликты в пачке.

### Свой редактор

- В Настройки → Файлы выбирается, чем открывать файлы: системным приложением, найденным редактором (VS Code, Cursor, Zed, Sublime Text, BBEdit, Nova и другие), любым приложением через «Выбрать приложение…» или своей командой, например `code --wait {file}`.
- Двойной клик открывает файл.
- Файл с сервера скачивается и открывается в редакторе. После каждого сохранения появляется вопрос **«Файл изменён. Загрузить новую версию на сервер?»**.
- Если файл за это время изменили на сервере, приложение ничего не перезаписывает и предлагает взять версию с сервера или перезаписать её.

### Терминал рядом с файлами

Кнопка терминала на панели сервера или **Ctrl+`** открывает шелл того же сервера под файловым менеджером, как в IDE.
- Терминал стартует в папке, которую вы смотрите; кнопка `cd` возвращает его в текущую папку, а в контекстном меню есть «Открыть терминал здесь».
- Высота меняется перетаскиванием верхнего края. Если терминал скрыть, сессия продолжает работать.

### SSH-ключи

В оригинале кнопка ключа сразу генерировала ключ, ставила его на сервер и отключала вход по паролю. В форке:

- **Кнопка ключа есть на каждой карточке** и открывает диалог.
- **Существующий ключ.** Ключи из `~/.ssh` находятся сами (по содержимому файла, а не по имени) и показываются карточками. Для ключа из другого места есть кнопка «Выбрать другой файл ключа…». Если рядом нет `.pub`, публичный ключ вычисляется из приватного.
- **Новый ключ** создаётся с **вашим именем**, например `~/.ssh/deploy_prod`. Имя проверяется: без путей, служебных имён и совпадений с существующими ключами.
- **Режим входа.** «Пароль и ключ» добавляет ключ и оставляет вход по паролю; если пароль был отключён, он включается обратно. «Только ключ» отключает вход по паролю, но только после того, как проверено, что ключ работает. Режим можно менять в любой момент.
- **Ключ по умолчанию** (Настройки → SSH-ключи) используется для серверов без своего ключа и предлагается при установке.
- **Форма сервера.** Ключ выбирается из найденных, рядом кнопка «Обзор» для файла в любой папке. Пароль необязателен.
- **Серверы из `~/.ssh/config`** тоже можно настраивать: при установке ключа сохраняется ваша копия в `hosts.toml`, сам `~/.ssh/config` не меняется.

### Докачка прерванных передач

Если соединение с сервером рвётся посреди передачи, в очереди появляется «Переподключение…». Приложение само переподключается с нарастающей паузой (1, 2, 4… до 30 секунд) и продолжает каждый файл с последнего надёжно записанного байта, а не с нуля. Разрыв замечается за доли секунды, а не через двухминутный таймаут. Передача сдаётся, только если 12 переподключений подряд не удались. Проверено прокси, который рвал соединение каждые несколько секунд: все файлы дошли целиком, контрольные суммы совпали.

### VPN через Tunnelblick (macOS)

- Для отдельного сервера можно указать конфигурацию OpenVPN из Tunnelblick: выбрать её в форме сервера или импортировать там `.ovpn`-файл.
- Перед подключением (терминал, SFTP, Docker, установка ключа) приложение поднимает VPN и ждёт, пока он подключится. Фоновый мониторинг VPN сам не поднимает.
- VPN, которые подняло приложение, отключаются при выходе из него. На карточке сервера есть значок VPN.
- Если Tunnelblick не установлен, при запуске появляется баннер с кнопкой «Установить». Приложение скачивает Tunnelblick 9.0.1 с tunnelblick.net (запасной источник — GitHub), сверяет SHA-256 с опубликованной суммой, копирует его в «Программы» и открывает, чтобы тот завершил свою настройку. «Больше не спрашивать» скрывает баннер; в форме сервера установка всё равно доступна.

### Панель Docker

Если на сервере найден Docker, на карточке появляется кнопка `docker`. Она открывает вкладку со списком контейнеров:

- состояние, образ, порты, CPU и память; список обновляется каждые 5 секунд;
- запуск, остановка, перезапуск, пауза и удаление (с подтверждением) в один клик;
- логи с выбором числа строк и режимом «Следить»;
- «Шелл» открывает терминал сразу внутри контейнера (`bash`, а если его нет — `sh`).

Если пользователю нельзя работать с Docker напрямую, приложение использует `sudo`, когда тот не спрашивает пароль.

### SFTP как файловый менеджер

- Колонки «Имя», «Размер», «Изменён». Сортировка по клику на заголовок, папки всегда сверху.
- Путь в виде «хлебных крошек»: по ним можно кликать или ввести путь вручную (Cmd+L). Кнопка «Наверх» и Backspace.
- Фильтр по текущей папке, переключатель скрытых файлов, закладки на папки (свои для каждого сервера и для компьютера).
- На сервере: новый файл, «Копировать в…» и «Переместить в…», удаление (с подтверждением, оно необратимо), права доступа (галочки или число, при желании рекурсивно), сжатие в `.tar.gz` или `.zip`, распаковка архивов на месте (`.tar` с любым сжатием, `.zip`, `.gz`), копирование пути.
- На компьютере: новый файл и папка, переименование, «Показать в Finder». Удаление отправляет файлы в Корзину.
- Клавиши: Delete, F2 (переименовать), F5 (обновить), Enter (открыть), Cmd+A (выделить всё).

### Русский интерфейс

Настройки → Внешний вид → Язык: **English / Русский**. Переведён весь интерфейс, с правильными русскими формами множественного числа («1 передача», «3 передачи», «5 передач»). По умолчанию выбирается язык системы.

### Исправления

- **Проверка ключа при настройке теперь честная.** В оригинале проверочное подключение могло пройти через ssh-agent или другой ключ по умолчанию, а не через устанавливаемый. В режиме «только ключ» пароль мог отключиться, хотя новый ключ не работал. Теперь проверка идёт строго установленным ключом.
- **Повторная установка ключа не дублирует строку** в `authorized_keys`.
- **Исправлен устаревший e2e-тест терминала**, который падал ещё в оригинале (искал кнопку закрытия по неверной подписи).

---

## Что изменилось в поведении по сравнению с оригиналом

| Было в оригинале | Стало в форке |
|---|---|
| Один клик по папке открывает её, по файлу — просмотр | Клик выделяет, двойной клик открывает (папку или файл в редакторе); быстрый просмотр в контекстном меню |
| Передачи блокируют просмотр, одна за другой | Передачи в фоне, параллельно, по отдельным соединениям |
| Передаются только файлы | Передаются и папки целиком |
| Файл с тем же именем молча перезаписывается | Вопрос: заменить / пропустить / оставить оба |
| Кнопка ключа только у серверов без ключа, всё делает сразу | Кнопка у всех серверов, диалог с выбором ключа, имени и режима входа |
| Поле «Identity file» — путь вручную | Выбор из найденных ключей + «Обзор»; «ключ по умолчанию» |
| Автопроверка обновлений на GitHub оригинала, её настройки в Settings | Выключена: лента релизов оригинала предлагала бы заменить форк оригиналом. Внизу настроек — блок «О приложении» |
| Только английский | Английский и русский |
| Оборванная передача завершается ошибкой, файл нужно передавать заново | Переподключение и докачка с места обрыва |
| Нет VPN | Для сервера можно указать конфигурацию Tunnelblick; при запуске предлагается установить Tunnelblick |
| Docker виден только как найденный сервис | Вкладка Docker: контейнеры, действия, логи, шелл |
| SFTP: просмотр, загрузка/скачивание, переименование, удаление | Сортировка, фильтр, скрытые файлы, закладки, ввод пути, права, архивы, копирование и перемещение на сервере, Корзина локально |

Что ещё стоит знать:
- **Дополнительные SSH-соединения.** Передачи открывают до 8 соединений к серверу (по умолчанию 4) и закрывают их через минуту простоя. Если на сервере жёстко ограничено число подключений (`MaxStartups`, fail2ban), уменьшите «Параллельные соединения» в настройках.
- **Новый параметр `config.toml`.** В разделе `[general]` появился `default_identity_file` (ключ по умолчанию). Терминальная версия тоже его учитывает.
- **Отдельные копии файлов для редактирования.** Файлы, открытые в редакторе, скачиваются в кэш приложения и удаляются при закрытии вкладки SFTP.
- **Tunnelblick управляется через AppleScript.** При первом подключении с VPN macOS спросит, разрешить ли OmnySSH управлять Tunnelblick. Без этого разрешения VPN не поднимется.
- **Операции над файлами на сервере — это shell-команды** (`rm`, `cp`, `mv`, `chmod`, `tar`, `zip`/`unzip`) с экранированием путей. Им нужна обычная POSIX-оболочка на сервере; для `.zip` на сервере должны быть `zip`/`unzip`.
- **Новые разрешения Tauri.** Добавлены `dialog:allow-open` и `dialog:allow-ask` для выбора файлов и подтверждений, а также ссылка `https://t.me/beforeused` для открытия во внешнем браузере.

---

## Сборка из исходников

Готовые сборки форка публикуются в [Releases этого репозитория](https://github.com/beforeused/omnyssh/releases) (если они там есть). Команды установки из описания оригинала ниже (`install.sh`, Homebrew, `cargo install`, `nix run`) ставят **оригинальный** OmnySSH, а не форк.

Нужны [Rust](https://rustup.rs) (stable), Node.js 20+ и npm. На Linux также понадобятся системные пакеты Tauri (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`).

```bash
git clone https://github.com/beforeused/omnyssh
cd omnyssh/crates/omnyssh-gui/ui && npm ci && cd ..
npx @tauri-apps/cli@^2 build            # macOS: .app и .dmg в target/release/bundle/
```

Запуск в режиме разработки: `npx @tauri-apps/cli@^2 dev` из `crates/omnyssh-gui`.

Сборка не подписана сертификатом Apple Developer. Если macOS не даёт открыть скачанное приложение, откройте его через правый клик → «Открыть» или выполните `xattr -dr com.apple.quarantine /Applications/OmnySSH.app`.

### Тесты

```bash
cargo test --workspace && cargo test -p omnyssh-gui       # Rust
cd crates/omnyssh-gui/ui
npx svelte-check && npx vitest run                          # фронтенд
npx playwright test                                         # e2e (нужен браузер Playwright)
```

> Под Node 26 часть старых unit-тестов (тема, сайдбар) спотыкается о встроенный в Node `localStorage`. Запускайте с `NODE_OPTIONS=--no-experimental-webstorage`.

### Где что лежит

| Что | Где |
|---|---|
| Движок передачи файлов | `crates/omnyssh-core/src/ssh/transfer.rs` |
| Поиск ключей в `~/.ssh` | `crates/omnyssh-core/src/ssh/keys.rs` |
| Установка ключа: режимы, имя, строгая проверка | `crates/omnyssh-core/src/ssh/key_setup.rs`, `session.rs` |
| Tauri-команды передач, редактора, ключей | `crates/omnyssh-gui/src/commands/{transfer,editor,keysetup}.rs` |
| Синхронизация файлов из редактора | `crates/omnyssh-gui/src/edit.rs`, `editor.rs` |
| SFTP-экран, панель передач, терминал | `crates/omnyssh-gui/ui/src/lib/screens/{SftpView,SftpPane,TransferPanel,TerminalPane}.svelte` |
| Диалог ключа, выбор ключа | `.../screens/KeySetupDialog.svelte`, `.../components/KeyPicker.svelte` |
| Операции над файлами на сервере (удаление, права, архивы, копирование) | `crates/omnyssh-core/src/ssh/remote_fs.rs` |
| Docker: список, действия, логи | `crates/omnyssh-core/src/ssh/docker.rs`, `.../screens/DockerView.svelte` |
| Tunnelblick: установка, подключение, импорт | `crates/omnyssh-core/src/vpn.rs`, `.../components/TunnelblickBanner.svelte` |
| Переводы | `crates/omnyssh-gui/ui/src/lib/i18n/{en,ru}.ts` |

Полный список изменений — в [CHANGELOG.md](CHANGELOG.md): разделы «1.1.2-fork.1» и «Unreleased».

---

# Об оригинальном OmnySSH

Ниже — описание оригинального приложения из репозитория [timhartmann7/omnyssh](https://github.com/timhartmann7/omnyssh). Всё это в форке тоже есть.

## What it does

You add a server once. After that it sits on the dashboard as a card with live CPU, RAM and disk, uptime, distro, the top processes eating your CPU, and a badge for what runs on it. One click on `sh` drops you into a real PTY terminal. One click on `files` opens a two panel SFTP browser. Ten servers fit on one screen and refresh on their own.

### Live dashboard
Cards for every host with CPU, RAM and disk bars, uptime, OS version, top processes, and a Docker badge showing how many containers are up. Bars turn yellow, then red, so a sick server is obvious from across the room.

### Real terminals
Full PTY sessions in tabs. Open as many servers as you need, switch between them from the sidebar, and keep them running while you work in the dashboard.

### Two panel SFTP
Local on the left, remote on the right. Drag files between the panes, or straight from Finder onto the server. Whole folders move too, several connections at once, in the background: the queue at the bottom shows progress while you keep browsing. Double-click a file to open it in your own editor, and every save goes back to the server. Nobody remembers `scp -r` syntax anyway.

### Snippets
Save the commands you paste every week. Pick a snippet, tick the hosts to send it to, and it runs on all of them at once. Snippets take parameters, so `sudo systemctl restart {{service}}` asks you for the name.

### Search everything
Hit ⌘K and start typing. Every host you have, plus every session already open. Enter drops you into a terminal on the host you picked, or back into the session you left.

### Streamer mode
Swaps every real IP on screen for a fake one. Record a demo or share your screen without leaking client infrastructure.

### Light and dark themes
Both ship in the app. Switch from the sidebar.

### Small
Around 130 MB of RAM with several sessions open, on a 20 MB download. Termius on the same machine, doing nothing, sat at 649 MB across nine processes. Full numbers in the [comparison](#comparison).

---

## SSH key setup

Password auth on a fresh VPS is the thing you always mean to fix and never do. OmnySSH does it from the server card.

Hit the key button on any server, pick a key you already have (the app lists the ones in `~/.ssh`, and a folder button finds any other) or create a new Ed25519 one, and choose how the server should accept logins: **password and key**, or **key only**. The app appends the public half to `authorized_keys`, switches the host over to that key, and opens a fresh connection with that key alone to prove it works. Only after that does key-only mode turn password login off. Settings → SSH keys holds a default key for servers that have none of their own.

Before touching `sshd_config` it saves a backup on the server. If any step fails, it restores the backup and leaves your access exactly as it was. Your private key never leaves your machine, and nothing gets sent anywhere except the server you chose.

The code lives in [`crates/omnyssh-core/src/ssh/key_setup.rs`](crates/omnyssh-core/src/ssh/key_setup.rs). Read it before you point this at production. That is the whole point of shipping it open source.

---

## Comparison

Memory and CPU measured on an M4 Mac with both apps open and idle.

| | OmnySSH | Termius | tmux + ssh |
|---|---|---|---|
| RAM at idle | ~130 MB | ~649 MB | tiny |
| Processes | 4 | 9 | 1 |
| Live metrics dashboard | ✅ | ✅ | ❌ |
| Two panel SFTP | ✅ | ✅ | ❌ |
| Snippets and broadcast | ✅ | ✅ | ❌ |
| One click key setup | ✅ | ❌ | ❌ |
| Account required | ❌ | ✅ | ❌ |
| Telemetry | ❌ | ✅ | ❌ |
| Open source | ✅ | ❌ | ✅ |
| Price | free | 💰 | free |

tmux stays in the table because it is what most of us actually use. It wins on weight and loses on everything visual.

---

## The TUI version

OmnySSH started in the terminal, and the TUI is still here, still maintained, still gets releases.

![Demo](assets/demo.gif)

Same engine underneath: the repo is a cargo workspace where `crates/omnyssh-core` holds the logic and the frontends sit on top. Dashboard, SFTP, snippets, multi session tabs, fuzzy search, plus four themes (`default`, `dracula`, `nord`, `gruvbox`) and remappable keys in `config.toml`.

[![Crates.io](https://img.shields.io/crates/v/omnyssh.svg)](https://crates.io/crates/omnyssh)
[![Crates downloads](https://img.shields.io/crates/d/omnyssh.svg)](https://crates.io/crates/omnyssh)

```bash
# cargo
cargo install omnyssh

# homebrew
brew install timhartmann7/tap/omnyssh

# nix
nix run github:timhartmann7/omnyssh
```

Then run `omny`. Press `a` to add a host, `/` to search, `?` for help, `Shift+K` to set up keys on the selected host.

Prebuilt TUI binaries for Linux, macOS, Windows and Termux live on the [Releases](https://github.com/timhartmann7/omnyssh/releases) page under the `omny-*` files. Config sits in `~/.config/omnyssh/` on Linux, `~/Library/Application Support/omnyssh/` on macOS, `%APPDATA%\omnyssh\` on Windows. Your `~/.ssh/config` is read at startup and never written to.

Full options, keybindings and config examples: `man omny`.

---

## Dev notes

I write about what I am building on Telegram. Release notes, work in progress screenshots, benchmarks, and the things that broke on the way there. Usually before they show up anywhere else.

### 👉 [**t.me/timhartmanndev**](https://t.me/timhartmanndev)

---

## Contributing

Pull requests welcome. [CONTRIBUTING.md](CONTRIBUTING.md) has the setup, the conventions and the checklist. Open an issue first if you plan something big, so we do not both build it.

Workspace layout:

```
crates/omnyssh-core   engine, frontend agnostic
crates/omnyssh        TUI application (binary: omny)
crates/omnyssh-gui    Tauri desktop application
```

## License

Apache 2.0. See [LICENSE](LICENSE).

<div align="center">

### ⭐ Star the repo if OmnySSH saved you a terminal tab

[Report a bug](https://github.com/timhartmann7/omnyssh/issues) •
[Request a feature](https://github.com/timhartmann7/omnyssh/issues) •
[Discussions](https://github.com/timhartmann7/omnyssh/discussions)

</div>
