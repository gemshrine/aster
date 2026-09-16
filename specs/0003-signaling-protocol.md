# 0003 — Протокол сигналинга

- **Status:** Accepted
- **Issue:** —

## Проблема

Клиентам нужен согласованный протокол поверх WebSocket для: аутентификации, обмена WebRTC SDP/ICE, передачи чат-сообщений и presence. Протокол должен жить в отдельном крейте `protocol`, чтобы клиент и сервер не расходились.

## Предлагаемое решение

### Транспорт

Один WebSocket (WSS) на клиента к `signaling-server`. Сообщения — JSON (читаемость и простота отладки важнее компактности при таком мизерном объёме трафика — только сигналинг и текст, не медиа).

### Версионирование

Каждое сообщение — enum `ClientMessage` / `ServerMessage` в `crates/protocol`, сериализуется как `{"type": "...", ...}` через `serde(tag = "type")`. Несовместимые изменения протокола — bump `PROTOCOL_VERSION: u32` константы, сервер отклоняет клиентов с несовпадающей версией на этапе `Hello`.

### Handshake / auth

```
Client -> Server: Hello { protocol_version, auth_token }
Server -> Client: Welcome { user_id, peer_online: bool }
                  | Rejected { reason }
```

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
Server -> Client: ChatDelivered { id: Uuid }   // ack отправителю, что сервер доставил получателю (не персистентность — см. 0004)
```

`id` генерируется клиентом (UUID v4) — используется для дедупликации/ack, не для сортировки (сортировка по `sent_at` + insertion order).

### Ошибки

```
Server -> Client: Error { code: ErrorCode, message: String }
```

## Альтернативы

- **Отдельный WebSocket для чата и для сигналинга** — отклонено, не оправдано сложностью ради тривиального объёма трафика.
- **Protobuf/bincode** — отклонено для v1 ради простоты отладки (`wscat`/браузерные devtools читают JSON нативно); можно пересмотреть, если протокол станет узким местом (маловероятно при 2 пользователях).

## Затронутые компоненты

- `crates/protocol` (новый крейт)
- `crates/signaling-server`
- `crates/client`

## Открытые вопросы

- Нужен ли heartbeat/ping поверх WS ping-frame для быстрого детекта дисконнекта? (Скорее да — добавить `Ping`/`Pong` с интервалом ~15s, либо положиться на встроенные WS ping frames в `tokio-tungstenite`.)
