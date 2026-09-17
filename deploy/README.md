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

## Логи

`docker compose logs -f signaling-server` — по умолчанию уровень `info` (подключения, отказы по токену). Подробнее — добавить `RUST_LOG: debug` в `environment` сервиса `signaling-server`.

## Обновление

Автоматически — после каждого мержа в `main` (см. «Автодеплой» ниже). Вручную, со сборкой на самом VPS:

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

## Автодеплой

`.github/workflows/deploy.yml` (`specs/0014-auto-deploy.md`) на каждый push в `main` собирает образ `ghcr.io/gemshrine/aster-signaling-server:<sha>`, заходит на VPS по SSH, переключает репозиторий на этот коммит, делает `docker compose pull` + `up -d` и проверяет `https://$ASTER_DOMAIN/health`. Пока секреты не заданы, публикуется только образ.

Разовая настройка:

1. **Ключ только для деплоя** (локально):
   ```bash
   ssh-keygen -t ed25519 -N '' -C aster-deploy -f aster-deploy
   ```
   `aster-deploy.pub` — дописать на VPS в `~/.ssh/authorized_keys` пользователя, который в группе `docker` и владеет `~/aster`.
2. **Отпечаток хоста:** `ssh-keyscan -t ed25519 <host>` — весь вывод.
3. **VPS видит GHCR:** репозиторий приватный, пакет тоже. Создать на GitHub fine-grained/classic токен только с `read:packages` и на VPS выполнить
   ```bash
   echo <token> | docker login ghcr.io -u gemshrine --password-stdin
   ```
   Токен остаётся в `~/.docker/config.json` на VPS, в GitHub он не нужен.
4. **VPS видит репозиторий:** `git -C ~/aster fetch` должен работать без пароля (deploy key на чтение или https-токен в remote).
5. **Секреты** в GitHub → Settings → Secrets and variables → Actions:

   | Секрет | Значение |
   |---|---|
   | `DEPLOY_HOST` | адрес VPS |
   | `DEPLOY_USER` | пользователь из шага 1 |
   | `DEPLOY_SSH_KEY` | содержимое приватного `aster-deploy` |
   | `DEPLOY_KNOWN_HOSTS` | вывод из шага 2 |
   | `ASTER_DOMAIN` | домен из `deploy/.env` |

Проверка: Actions → Deploy → Run workflow. **Откат:** открыть успешный запуск на нужном коммите → Re-run all jobs: VPS вернётся ровно на тот коммит и образ.

`caddy` и `coturn` автоматически не обновляются: `docker compose pull caddy coturn && docker compose up -d`.
