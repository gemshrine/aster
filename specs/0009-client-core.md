# 0009 — Ядро клиента: соединение, команды и события Tauri

- **Status:** Accepted
- **Issue:** #27

## Проблема

По `0008` UI (shell, чат, звонок) пишется в wasm-фронтенде `crates/client/src/ui`. Но соединение с signaling-server, переподключение, хранение адреса сервера и токена, а позже WebRTC и аудио (`0005`) — не задача wasm-фронтенда: это долгоживущие задачи, сокеты и секреты, которые должны жить в Rust-процессе Tauri (`crates/client/src-tauri`). Нужен зафиксированный контракт «ядро ↔ UI», чтобы их можно было делать параллельно.

## Предлагаемое решение

### Где живёт

`crates/client/src-tauri/src/core/`:

- `settings` — чтение/запись настроек;
- `signaling` — WS-клиент, handshake, состояния, переподключение. Не зависит от Tauri (общается через каналы), тестируется против настоящего `signaling-server`;
- тонкий слой `commands`/`events` в `src-tauri` пробрасывает ядро в UI.

Сигналы WebRTC (`Signal`) в UI не выходят — их потребляет голосовой модуль ядра (#7).

### Настройки

Файл `settings.json` в `app_config_dir` Tauri (Linux: `~/.config/dev.gemshrine.aster/`), права `0600`:

```json
{ "server_url": "wss://aster.example.com/ws", "token": "…" }
```

- `server_url` — `wss://…`; `ws://` разрешён только для `localhost` / `127.0.0.1` / `[::1]` (локальная разработка), чтобы токен не ушёл по сети открытым текстом.
- Токен в UI обратно не отдаётся: UI узнаёт только, задан ли он.
- **Решение по открытому вопросу `0006`:** в v1 — файл с `0600`, не `keyring`. На Linux `keyring` требует Secret Service (gnome-keyring/KWallet), которого часто нет на тайлинговых WM; файл в домашнем каталоге пользователя — та же граница доверия. Переход на `keyring` не меняет контракт с UI.

При старте приложения, если настройки есть, ядро сразу подключается.

### Состояния соединения

```
not_configured ──save_settings──► connecting ──Welcome──► connected
                                      ▲   │                   │
                          backoff ────┘   │ сеть/таймаут      │ сеть/таймаут/close
                                          ▼                   ▼
                                     reconnecting ◄───────────┘
connecting/connected ──Rejected──► failed{invalid_token|unsupported_protocol_version|other}
connected ──Error{session_replaced}──► failed{session_replaced}
любое ──disconnect──► disconnected
```

Сериализация (`serde(tag = "state", rename_all = "snake_case")`):

| `state` | Поля |
|---|---|
| `not_configured` | — |
| `connecting` | `attempt: u32` |
| `connected` | `user_id: String`, `peer_online: bool` |
| `reconnecting` | `attempt: u32`, `retry_in_ms: u64` |
| `disconnected` | — |
| `failed` | `reason: "invalid_token" \| "unsupported_protocol_version" \| "session_replaced" \| "other"` |

- **Backoff:** 1s, удвоение до 30s, jitter ±20%; счётчик сбрасывается после `Welcome`.
- **`other`** — причина отказа, которую эта версия клиента не знает (`RejectReason`/`ErrorCode` в `protocol` десериализуют незнакомые значения в `Unknown`). UI должен так же относиться к незнакомым значениям `reason` — не падать, а показывать общий текст (#31).
- **Таймауты:** TCP/TLS/WS-подключение и ожидание ответа на `Hello` — 10s. Сервер шлёт WS ping каждые 20s (`0003`), поэтому 60s без единого фрейма → соединение считается мёртвым → `reconnecting`.
- **Keepalive в обе стороны (#33):** пока `connected`, ядро непрерывно поллит WS-стрим (ответные pong на ping сервера) и само шлёт WS ping каждые 20s — серверный idle-таймаут 60s не зависит от того, успели ли уйти pong.
- **`failed` не переподключается сам.** `invalid_token`/`unsupported_protocol_version` без действий пользователя не исправятся, а автопереподключение после `session_replaced` превратило бы два запущенных клиента в бесконечную взаимную дуэль. Выход из `failed` — команда `connect` или `save_settings`.
- `ice_servers` из последнего `Welcome` хранятся в ядре для голосового модуля и в UI не отдаются.

### Команды Tauri (UI → ядро)

| Команда | Аргументы | Результат |
|---|---|---|
| `get_settings` | — | `{ server_url: string \| null, has_token: bool }` |
| `save_settings` | `{ server_url: string, token: string }` | `()`; сохраняет и переподключается |
| `connect` | — | `()`; из `disconnected`/`failed` |
| `disconnect` | — | `()` |
| `connection_state` | — | `ConnectionState` (текущее, для первого рендера) |
| `send_chat` | `{ id: uuid, body: string, sent_at: i64 }` | `()` |

Ошибка команды — `{ code, message }`, `code`: `invalid_settings`, `not_configured`, `not_connected`, `io`.

`send_chat` не ретраит и не хранит сообщения: при `not_connected` UI/чат (#6) оставляет сообщение в статусе `Sending` и досылает после `connected`.

### События Tauri (ядро → UI)

Все события исходят из одного актора и приходят в UI в порядке возникновения.

| Событие | Payload |
|---|---|
| `connection-state` | `ConnectionState` — при каждой смене |
| `peer-status` | `{ online: bool }` |
| `chat-message` | `{ id, from, body, sent_at }` |
| `chat-ack` | `{ id, status: "delivered" \| "queued" }` |
| `chat-rejected` | `{ id: uuid \| null, code, message }` — `queue_full` и прочие `Error` сервера, относящиеся к сообщениям |

**Presence (#33).** Каждое `connection-state { state: "connected" }` несёт актуальный `peer_online`: при `PeerStatus` от сервера ядро сначала эмитит обновлённое `connection-state`, затем `peer-status` с тем же значением. Устаревшего `connected` после свежего `peer-status` не бывает, UI может брать presence из любого из двух событий; `peer-status` — удобство для подписчиков, которым не нужно всё состояние.

### Статусы сообщений и дедупликация (#32)

Соответствие сигналов ядра статусам из `0004`:

| Сигнал | Статус |
|---|---|
| сообщение создано, `send_chat` ещё не вернулся или вернул `not_connected` / `io` | `Sending` |
| `send_chat` вернул `Ok` | `Sent` |
| `chat-ack { status: "queued" }` | `Queued` |
| `chat-ack { status: "delivered" }` (сразу или позже, из очереди) | `Delivered` |
| `chat-rejected { id }` (`queue_full`) | `Failed` — сервер отбросил, повторная отправка только по действию пользователя |

- Сообщения в `Sending` после перехода в `connected` досылаются слоем чата (#6) тем же `id`. Если первая попытка всё-таки дошла до сервера, получатель увидит дубль по `id` — это нормально.
- **Дедупликация — в слое истории (#6):** запись сообщения — UPSERT по `id`, и для входящих, и для своих. Ядро состояния не хранит и после рестарта не знает, что уже было показано.
- Статус не откатывается назад: `Delivered` не меняется на `Queued`/`Sent` при позднем или повторном ack.

## Альтернативы

- **WS-клиент во wasm-фронтенде (браузерный `WebSocket`)** — отклонено: соединение рвалось бы вместе с перезагрузкой webview, токен жил бы в JS-контексте, а WebRTC всё равно в Rust (`0005`) — сигналинг пришлось бы гонять через UI туда-обратно.
- **`keyring` для токена** — см. «Настройки».

## Затронутые компоненты

- `crates/client/src-tauri` — `core/`, команды, события.
- `crates/client/src` — UI вызывает команды и подписывается на события (#5, #6).
- `crates/protocol` — `Unknown` в `RejectReason`/`ErrorCode`.

## Открытые вопросы

- Несколько окон/трей — вне v1; события рассылаются всем окнам.
