# Деплой Aster на VPS

Стек (`docker-compose.yml`): `signaling-server` + `caddy` (TLS, Let's Encrypt) + `coturn` (STUN/TURN). Решения — `specs/0007-deployment.md`.

## Требования

- VPS с публичным IPv4 (Ubuntu/Debian), Docker Engine + compose plugin.
- Домен, A-запись которого указывает на VPS.
- Открытые порты:

| Порт | Протокол | Кто |
|---|---|---|
| 80, 443 | TCP | caddy (ACME + WSS) |
| 3478 | UDP + TCP | coturn (STUN/TURN) |
| 49160–49200 | UDP | coturn (relay) |

Например, для `ufw`:

```bash
ufw allow 80,443/tcp
ufw allow 3478
ufw allow 49160:49200/udp
```

## Первый запуск

```bash
git clone https://github.com/gemshrine/aster.git
cd aster/deploy
cp .env.example .env
# заполнить .env: домен, токены (`openssl rand -hex 32`), TURN_SECRET
chmod 600 .env
docker compose up -d --build
```

Проверка:

```bash
curl https://<ASTER_DOMAIN>/health   # -> ok
docker compose ps                     # все три сервиса running
docker compose logs -f signaling-server
```

Каждому пользователю передать его `id` и токен из `ASTER_USERS` по приватному каналу — они вводятся в клиенте.

## Обновление

```bash
cd aster && git pull
cd deploy && docker compose up -d --build
```

Перезапуск `signaling-server` теряет in-memory очередь недоставленных сообщений (`specs/0004-text-chat.md`).

## Смена токенов

Отредактировать `ASTER_USERS` в `.env`, затем `docker compose up -d signaling-server`.

## Если VPS за NAT

Если публичный IP не назначен на интерфейс (некоторые облака), добавить в `turnserver.conf`:

```
external-ip=<публичный IP>/<приватный IP>
```
