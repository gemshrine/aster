# 0003 — Протокол сигналинга

- **Status:** Accepted
- **Issue:** #2

## Проблема

Клиентам нужен согласованный протокол поверх WebSocket для: аутентификации, обмена WebRTC SDP/ICE, передачи чат-сообщений и presence. Протокол должен жить в отдельном крейте `protocol`, чтобы клиент и сервер не расходились.

## Предлагаемое решение

### Транспорт

Один WebSocket (WSS) на клиента к `signaling-server`. Сообщения — JSON (читаемость и простота отладки важнее компактности при таком мизерном объёме трафика — только сигналинг и текст, не медиа).

### Версионирование

Каждое сообщение — enum `ClientMessage` / `ServerMessage` в `crates/protocol`, сериализуется как `{"type": "...", ...}` через `serde(tag = "type")`. Имена типов и enum-значений — snake_case (`chat_delivered`, `ice_candidate`, `invalid_token`). `SignalPayload` внутри `payload` использует тот же дискриминатор `type`. Несовместимые изменения протокола — bump `PROTOCOL_VERSION: u32` константы, сервер отклоняет клиентов с несовпадающей версией на этапе `Hello`.

### Handshake / auth

```
Client -> Server: Hello { protocol_version, auth_token }
Server -> Client: Welcome { user_id, peer_online: bool, ice_servers: Vec<IceServer> }
                  | Rejected { reason: RejectReason }

enum RejectReason { InvalidToken, UnsupportedProtocolVersion }
```

`UserId` — строка (`"user_id": "alice"`).

`IceServer { urls: Vec<String>, username: Option<String>, credential: Option<String> }` повторяет `RTCIceServer` (пустые `username`/`credential` не сериализуются). Сервер отдаёт `stun:<host>:3478` и `turn:<host>:3478?transport=udp|tcp` с временными кредами по схеме TURN REST API: `username = "<unix_expiry>:<user_id>"`, `credential = base64(HMAC-SHA1(TURN_SECRET, username))`, срок жизни 24h. Если сервер запущен без `TURN_HOST`/`TURN_SECRET`, список пустой. Добавлено в #23, `PROTOCOL_VERSION = 2`. После `Rejected` сервер закрывает соединение. Любое другое сообщение до успешного `Hello` → `Error { NotAuthenticated }`.

`auth_token` — см. `0006-auth-security.md` (пер-пользовательский долгоживущий токен, выданный вручную при деплое).

### Presence

```
Server -> Client: PeerStatus { online: bool }   // рассылается при коннекте/дисконнекте второго юзера
```

### WebRTC signaling relay

Сервер ничего не парсит внутри SDP/ICE — просто релеит от одного соединения к другому (единственному возможному собеседнику).

```
Client -> Server: Signal { payload: SignalPayload }
Server -> Client: Signal { payload: SignalPayload }   // как есть, от другого клиента

enum SignalPayload {
    Offer { sdp: String },
    Answer { sdp: String },
    IceCandidate { candidate: String, sdp_mid: Option<String>, sdp_mline_index: Option<u16> },
    CallEnd,
}
```

### Чат

```
Client -> Server: ChatMessage { id: Uuid, body: String, sent_at: i64 }
Server -> Client: ChatMessage { id, from: UserId, body, sent_at }
Server -> Client: ChatDelivered { id: Uuid }   // получатель онлайн, сообщение ему передано
Server -> Client: ChatQueued { id: Uuid }      // получатель офлайн, сообщение в in-memory очереди (см. 0004)
```

`id` генерируется клиентом (UUID v4) — используется для дедупликации/ack, не для сортировки (сортировка по `sent_at` + insertion order). `sent_at` — unix time в миллисекундах.

### Ошибки

```
Server -> Client: Error { code: ErrorCode, message: String }

enum ErrorCode {
    MalformedMessage,   // не распарсилось
    NotAuthenticated,   // сообщение до Hello
    PeerOffline,        // Signal, когда собеседник офлайн
    SessionReplaced,    // этот же user_id подключился заново; старое соединение закрывается
    QueueFull,          // офлайн-очередь получателя переполнена, сообщение отброшено
}
```

## Альтернативы

- **Отдельный WebSocket для чата и для сигналинга** — отклонено, не оправдано сложностью ради тривиального объёма трафика.
- **Protobuf/bincode** — отклонено для v1 ради простоты отладки (`wscat`/браузерные devtools читают JSON нативно); можно пересмотреть, если протокол станет узким местом (маловероятно при 2 пользователях).

## Затронутые компоненты

- `crates/protocol` (новый крейт)
- `crates/signaling-server`
- `crates/client`

## Открытые вопросы

- ~~Нужен ли heartbeat поверх WS?~~ Решено (#3): отдельных `Ping`/`Pong` в протоколе нет. Сервер шлёт WS ping-frame каждые 20s и закрывает соединение, если от клиента 60s не пришло ни одного фрейма. Handshake (`Hello`) должен прийти в течение 10s. Максимальный размер сообщения — 64 KiB.
