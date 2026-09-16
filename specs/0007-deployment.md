# 0007 — Деплой на VPS

- **Status:** Accepted
- **Issue:** #8

## Проблема

Нужен простой, воспроизводимый способ развернуть `signaling-server` + TURN на дешёвом VPS (ориентир: 1 vCPU / 1 ГБ RAM, Ubuntu/Debian), без Kubernetes и подобной тяжёлой инфраструктуры.

## Предлагаемое решение

### Состав

`deploy/docker-compose.yml` с тремя сервисами:

- `signaling-server` — собственный образ (multi-stage Dockerfile: `cargo build --release` → минимальный runtime-образ, например `debian:bookworm-slim` или `gcr.io/distroless/cc`).
- `caddy` — reverse proxy, TLS termination (Let's Encrypt), проксирует WSS на `signaling-server`.
- `coturn` — TURN/STUN сервер, официальный образ `coturn/coturn`, с `use-auth-secret` (временные HMAC-креды, выдаются клиентам сигналинг-сервером — отдельный issue). Запускается с `network_mode: host`: relay-диапазон UDP и реальные адреса клиентов не проходят через Docker NAT.

### Сеть/порты

- `caddy`: 80/443 наружу (HTTP-01 challenge + основной трафик).
- `coturn`: 3478 (STUN/TURN, UDP+TCP) + UDP relay range 49160–49200 (минимальный диапазон для двух пользователей). TURNS (5349) в v1 не поднимаем: медиа и так DTLS-SRTP end-to-end, TLS на TURN помогает только в сетях, где режут UDP и нестандартные TCP-порты, а требует прокидывать сертификаты Caddy в coturn и перезапускать его при их обновлении.
- coturn не релеит на приватные/loopback адреса (`denied-peer-ip`), чтобы TURN нельзя было использовать для доступа к внутренним сервисам VPS.
- `signaling-server` слушает только на internal docker-сети, наружу не торчит напрямую (только через Caddy).

### Конфигурация

- `deploy/.env` (не в git) — auth-токены пользователей, coturn static-auth-secret, домен для Caddy.
- `deploy/.env.example` — шаблон, коммитится.
- `deploy/Caddyfile` — конфиг реверс-прокси, домен параметризован через env.
- `deploy/turnserver.conf` — статичная часть конфига coturn; `realm` и `static-auth-secret` передаются аргументами из `.env`.
- `deploy/README.md` — пошаговая инструкция, порты для файрвола.

### Процесс деплоя (ручной, v1)

```bash
# на VPS
git clone <repo> && cd aster/deploy
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
- `deploy/Dockerfile` — образ `signaling-server` (контекст сборки — корень репозитория)

## Открытые вопросы

- Нужен ли автоматический деплой (CI job, который SSH'ится на VPS и обновляет) — отложено, решить отдельным issue после того как ручной процесс обкатан.
