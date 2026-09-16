# Aster

Приватный голосовой + текстовый чат для двух пользователей, на Rust + Tauri, разворачиваемый на недорогом VPS.

- Процесс разработки и стек — см. [`CLAUDE.md`](CLAUDE.md).
- Как вносить изменения (issue-driven + spec-driven workflow) — см. [`CONTRIBUTING.md`](CONTRIBUTING.md).
- Архитектурные решения — см. [`specs/`](specs/README.md).

## Запуск прототипа локально

Нужны Rust (stable), `wasm32-unknown-unknown`, `trunk`, `tauri-cli` и системные
зависимости Tauri (на Linux — `webkit2gtk-4.1`, `libayatana-appindicator3`,
`librsvg2`, `libasound2`).

```bash
# 1. сервер сигнализации: два пользователя, токены от 32 символов и разные
ASTER_USERS="alice:$(openssl rand -hex 24),bob:$(openssl rand -hex 24)" \
  PORT=8080 cargo run -p signaling-server

# 2. клиент (в отдельном терминале)
cd crates/client && cargo tauri dev
```

В окне клиента открывается форма подключения: адрес `ws://127.0.0.1:8080/ws`
(схема `ws://` разрешена только для localhost, см. `specs/0009-client-core.md`)
и токен одного из пользователей. Второй экземпляр клиента с токеном второго
пользователя — второй собеседник; настройки лежат в
`~/.config/dev.gemshrine.aster/settings.json`, так что для двух клиентов на
одной машине нужен разный `XDG_CONFIG_HOME`.

Фронтенд без Tauri (для работы над UI) — `cd crates/client && trunk serve`,
витрина компонентов там же на `#/gallery`.

## Статус

Текстовый чат собран по всей цепочке: сервер сигнализации, ядро клиента
(подключение, переподключение, чат-релей) и UI, связанный с ядром через
команды и события Tauri. Ядро покрыто интеграционными тестами против
настоящего сервера; сквозной прогон в собранном окне — по инструкции выше.

Голос (WebRTC) и локальная история в SQLite — в работе, см. issues.
