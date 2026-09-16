# Aster

Приватный голосовой + текстовый чат для двух пользователей (владелец репозитория и Кент), рассчитанный на развёртывание на недорогом VPS. Аналог "своего мини-Discord/Mumble" без лишней инфраструктуры.

## Стек

- **Язык:** Rust (workspace, edition 2021+)
- **Клиент:** Tauri 2, фронтенд на Leptos (Rust → wasm), без Node/React-стека
- **Сервер сигнализации:** `axum` + `tokio` + `tokio-tungstenite` (WebSocket)
- **Голос/видео:** WebRTC, P2P между клиентами (`webrtc-rs`), сервер участвует только в сигналинге (SDP/ICE) и как TURN-фолбэк (`coturn`) для NAT traversal
- **Текстовый чат:** сообщения передаются через тот же WebSocket-канал сигнализации; история хранится локально у каждого клиента (SQLite через `rusqlite`/`sqlx`)
- **Кодек аудио:** Opus
- **Деплой:** Docker Compose на VPS, `Caddy` как reverse proxy + автоматический TLS, `coturn` как TURN-сервер

Все архитектурные решения фиксируются в `specs/` до реализации — это не гайдлайн, а обязательный процесс (см. ниже).

## Процесс разработки: issue-driven + spec-driven

Работаем вдвоём (владелец + Кент), поэтому весь процесс идёт через GitHub, а не через прямые пуши в `main`.

### Spec-driven

- Любая нетривиальная фича или архитектурное решение сначала описывается в `specs/NNNN-slug.md`.
- Спека описывает: проблему, предлагаемое решение, альтернативы (кратко), затронутые компоненты, открытые вопросы.
- Реализация не начинается, пока спека не смержена (через свой issue/PR) или явно не согласована в issue.
- Спеки нумеруются по возрастанию (`0001`, `0002`, ...), не переиспользуются и не удаляются задним числом — если решение устарело, спека помечается как Superseded со ссылкой на новую.

### Issue-driven

Каждая единица работы — это GitHub issue. Без issue код не пишется (кроме тривиальных правок вроде опечаток).

GH workflow строго такой:

1. **Issue** — завести issue с чётким описанием (что и зачем), при необходимости — ссылка на spec.
2. **Assign** — назначить issue на себя (`gh issue edit <n> --add-assignee @me`) перед началом работы.
3. **Branch** — создать ветку от `main`: `issue-<n>-<короткий-slug>` (например `issue-12-webrtc-signaling`).
4. **PR** — открыть PR из ветки в `main`, в описании — `Closes #<n>`.
5. **Merge** — смержить PR (squash) после ревью/прохождения CI.
6. **Close PR** — PR закрывается автоматически при мерже.
7. **Close issue** — issue закрывается автоматически через `Closes #<n>`; если авто-закрытие не сработало — закрыть вручную.

Ветки-исключения: не делаем долгоживущих feature-веток дольше одного issue.

## Структура репозитория

```
aster/
├── CLAUDE.md              # этот файл
├── README.md
├── specs/                 # спеки (spec-driven development)
├── crates/
│   ├── protocol/          # общие типы: сигналинг-сообщения, чат-протокол
│   ├── signaling-server/  # сервер сигнализации (axum + WebSocket)
│   └── client/            # Tauri-приложение (src-tauri + Leptos frontend)
├── deploy/                # docker-compose, Caddyfile, coturn config
└── .github/               # issue/PR templates, CI workflows
```

## Команды

```bash
# сборка всего workspace (protocol, signaling-server, client/src-tauri)
cargo build --workspace

# тесты
cargo test --workspace

# линт
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# запуск сервера сигнализации локально
cargo run -p signaling-server

# запуск клиента (Tauri dev) — фронтенд (crates/client) не входит в workspace,
# т.к. это wasm-крейт, собираемый отдельно через trunk
cd crates/client
cargo fmt -- --check && cargo clippy --all-targets -- -D warnings
cargo tauri dev
```

### Первоначальная настройка окружения для клиента

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cargo install tauri-cli --version "^2" --locked
```

## Конвенции

- Коммиты: `<scope>: <короткое summary в повелительном наклонении>` (например `signaling: add ICE candidate relay`).
- Форматирование — `rustfmt` (дефолтный конфиг), линт — `clippy` без warnings в CI.
- PR должен проходить CI (build + test + clippy + fmt) прежде чем мержится.
- Секреты (токены авторизации между двумя клиентами, TURN-креды) никогда не коммитятся — только через `.env`/секреты деплоя, см. `deploy/`.

## Модель угроз (кратко)

Сервис приватный для двух конкретных людей, не публичный сервис. Тем не менее:
- Весь трафик (сигналинг, чат, TURN relay) должен идти через TLS/DTLS.
- Доступ к серверу сигнализации — по преролированным токенам/паролю, не anonymous.
- Подробности — `specs/0006-auth-security.md`.
