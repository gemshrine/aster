# 0010 — История чата в ядре клиента

- **Status:** Accepted
- **Issue:** #41

## Проблема

`0004` требует локальную историю в SQLite, `0009` — статусы `Sending / Sent / Queued / Delivered / Failed`, досылку после переподключения и дедупликацию по `id` «в слое истории». Этого слоя нет: ядро отдаёт сырые `chat-message` / `chat-ack` / `chat-rejected`, и UI пришлось бы самому хранить сообщения, сопоставлять ack'и и досылать. Это та же логика с таймингами и БД, которую `0009` уже вынесла из wasm в Rust.

## Предлагаемое решение

### Кто владеет историей

Модуль `core::chat` в `crates/client/src-tauri`. Он единственный пишет в БД, подписан на события сигналинга и отдаёт UI готовые сообщения. Сырые `chat-message` / `chat-ack` / `chat-rejected` из `0009` становятся внутренними событиями ядра и в UI не эмитятся; команда `send_chat` из `0009` заменяется на `send_message`.

### Хранилище

Файл `history.sqlite3` в `app_data_dir` Tauri (Linux: `~/.local/share/dev.gemshrine.aster/`), `rusqlite` с фичей `bundled` (не зависим от системной libsqlite). WAL-режим.

```sql
CREATE TABLE messages (
    id          TEXT PRIMARY KEY,           -- UUID v4 от отправителя
    direction   TEXT NOT NULL CHECK (direction IN ('out', 'in')),
    body        TEXT NOT NULL,
    sent_at     INTEGER NOT NULL,           -- мс, часы отправителя
    received_at INTEGER NOT NULL,           -- мс, момент локальной записи
    status      TEXT CHECK (status IN ('sending', 'sent', 'queued', 'delivered', 'failed'))
                                            -- только для 'out', для 'in' NULL
);
CREATE INDEX messages_order ON messages (sent_at, received_at);
```

- Версия схемы — `PRAGMA user_version`; миграции применяются при открытии по порядку.
- Порядок в ленте — `(sent_at, received_at, id)` (`0003`: сортировка по `sent_at` + порядок вставки).
- Автор хранится как направление, а не `user_id`: своё сообщение можно создать до первого `Welcome`, когда ядро ещё не знает свой `user_id`. Имена сторон UI берёт из `connection-state`.

### Отправка и досылка

1. `send_message { body }`: ядро проверяет тело (после trim не пустое, ≤ 16 000 символов), генерирует `id` и `sent_at`, пишет строку `out / sending`, эмитит `message-upserted` и возвращает сообщение.
2. Сразу пытается отправить через сигналинг. `Ok` → `sent`. `not_connected` / `io` → остаётся `sending`.
3. При каждом переходе в `connected` ядро досылает все свои `sending` **и `sent`** в порядке `(sent_at, received_at)` тем же `id`. `sent` без ack означает, что фрейм ушёл в сокет, но неизвестно, дошёл ли до сервера. Возможный дубль у получателя гасится UPSERT'ом по `id`.
4. `retry_message { id }` — только для `failed`: статус → `sending`, отправка как в п. 2.

### Переходы статуса

Статус своего сообщения только растёт по рангу `sending < sent < queued < delivered`. Ack с меньшим или равным рангом игнорируется: поздний `queued` не откатывает `delivered`, повторный ack не эмитит событие.

`failed` ставится по `chat-rejected { id }` из любого статуса, кроме `delivered`. Выход из `failed` — `retry_message` или реальная доставка: `chat-ack { delivered }` переводит `failed` → `delivered`. Так бывает, когда сообщение ушло дважды (досылка `sent`): одна копия легла в офлайн-очередь, вторая получила `queue_full`, а потом первая всё-таки доставлена.

### Входящие

`INSERT ... ON CONFLICT (id) DO NOTHING`. Событие эмитится, только если строка действительно вставлена: повтор из офлайн-очереди или досылка отправителя не дают дубля ни в БД, ни в UI.

### Команды Tauri

| Команда | Аргументы | Результат |
|---|---|---|
| `load_history` | `{ before: uuid \| null, limit: u32 }` | `Message[]` по возрастанию; до 200 сообщений строго раньше `before` (или последние, если `null`) |
| `send_message` | `{ body: string }` | `Message` |
| `retry_message` | `{ id: uuid }` | `Message` |

Ошибки: `invalid_message` (пустое или слишком длинное тело), `not_found`, `not_failed` (retry не для `failed`), `io`.

### События Tauri

| Событие | Payload |
|---|---|
| `message-upserted` | `Message` — новое сообщение (своё или входящее) или смена статуса своего |

```json
{ "id": "…", "direction": "out", "body": "привет", "sent_at": 1726500000000, "status": "delivered" }
```

`status` у входящих — `null`. UI хранит сообщения в памяти по `id` и заменяет запись целиком при каждом `message-upserted`.

## Альтернативы

- **История во wasm-фронтенде (IndexedDB/localStorage)** — отклонено: webview-хранилище привязано к origin dev-сервера и теряется при смене `devUrl`/сборки; досылка и сопоставление ack'ов всё равно нужны рядом с соединением (`0009`).
- **`sqlx`** — отклонено для v1: асинхронный пул и compile-time проверка запросов избыточны для одного файла, одного писателя и десятка запросов.

## Затронутые компоненты

- `crates/client/src-tauri` — `core::chat`, команды, события.
- `specs/0009-client-core.md` — `send_chat` и `chat-*` события становятся внутренними.
- `crates/client/src` — чат-UI (#36/#38) подписывается на `message-upserted` вместо трёх событий.

## Открытые вопросы

- Шифрование БД на диске — остаётся открытым вопросом `0004`, не блокирует.
