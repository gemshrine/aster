# 0007 — Деплой на VPS

- **Status:** Accepted
- **Issue:** —

## Проблема

Нужен простой, воспроизводимый способ развернуть `signaling-server` + TURN на дешёвом VPS (ориентир: 1 vCPU / 1 ГБ RAM, Ubuntu/Debian), без Kubernetes и подобной тяжёлой инфраструктуры.

## Предлагаемое решение

### Состав

`deploy/docker-compose.yml` с тремя сервисами:

- `signaling-server` — собственный образ (multi-stage Dockerfile: `cargo build --release` → минимальный runtime-образ, например `debian:bookworm-slim` или `gcr.io/distroless/cc`).
- `caddy` — reverse proxy, TLS termination (Let's Encrypt), проксирует WSS на `signaling-server`.
- `coturn` — TURN/STUN сервер, официальный образ `coturn/coturn`, с static-auth-secret.

### Сеть/порты

- `caddy`: 80/443 наружу (HTTP-01 challenge + основной трафик).
- `coturn`: 3478 (STUN/TURN, UDP+TCP), 5349 (TURNS), + UDP relay range (например 49152–49252, минимальный диапазон для двух пользователей, чтобы не открывать тысячи портов на дешёвом VPS с ограниченным файрволом).
- `signaling-server` слушает только на internal docker-сети, наружу не торчит напрямую (только через Caddy).

### Конфигурация

- `deploy/.env` (не в git) — auth-токены пользователей, coturn static-auth-secret, домен для Caddy.
- `deploy/.env.example` — шаблон, коммитится.
- `deploy/Caddyfile` — конфиг реверс-прокси, домен параметризован через env.
- `deploy/coturn.conf` — базовый конфиг coturn с static-auth-secret и realm.

### Процесс деплоя (ручной, v1)

```bash
# на VPS
git clone <repo> && cd gavno-voice/deploy
cp .env.example .env   # заполнить токены/домен
docker compose up -d --build
```

Обновление — `git pull && docker compose up -d --build`. CI/CD (авто-деплой по мержу в `main`) — вне скоупа v1, отдельная спека/issue при необходимости.

### Ресурсы и стоимость

Расчёт на минимальный VPS: сигналинг-сервер держит 2 WS-соединения и релеит текст/сигналинг — тривиальная нагрузка на CPU/RAM. `coturn` активен только когда P2P не устанавливается напрямую (нечастый случай), трафик через него — только аудио двух человек, не постоянная нагрузка.

## Альтернативы

- **Serverless/managed WebSocket (например Cloudflare)** — отклонено: цель — полный контроль и минимальная стоимость через собственный VPS, не привязываться к платному managed-сервису.
- **Systemd-юниты вместо Docker** — рассматривалось (меньше overhead), но Docker Compose даёт более простую воспроизводимость и изоляцию зависимостей (особенно для `coturn`), решили в пользу Compose.

## Затронутые компоненты

- `deploy/` (весь каталог — новый)
- `crates/signaling-server` — Dockerfile

## Открытые вопросы

- Нужен ли автоматический деплой (CI job, который SSH'ится на VPS и обновляет) — отложено, решить отдельным issue после того как ручной процесс обкатан.
