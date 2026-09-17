# 0014 — Автодеплой на VPS при мерже в main

- **Status:** Accepted
- **Issue:** #65

## Проблема

По `0007` деплой ручной: зайти на VPS, `git pull`, `docker compose up -d --build`. Сборка Rust на VPS 1 vCPU / 1 ГБ занимает долго и может упасть по памяти, а после мержа легко забыть обновить сервер.

## Предлагаемое решение

### Образ собирается в CI, VPS его только скачивает

Новый workflow `.github/workflows/deploy.yml`, запускается на `push` в `main` (то есть после мержа PR) и вручную (`workflow_dispatch`):

1. **`image`** — собирает `deploy/Dockerfile` и публикует в GitHub Container Registry:
   - `ghcr.io/gemshrine/aster-signaling-server:<sha коммита>`
   - `ghcr.io/gemshrine/aster-signaling-server:latest`
   Кэш слоёв — GitHub Actions cache (`type=gha`), чтобы зависимости не пересобирались каждый раз.
2. **`deploy`** — после `image`, по SSH на VPS:
   ```bash
   cd ~/aster
   git fetch origin main && git reset --hard origin/main
   cd deploy
   ASTER_IMAGE_TAG=<sha> docker compose pull signaling-server
   ASTER_IMAGE_TAG=<sha> docker compose up -d
   ```
   `git reset --hard` на VPS допустим: там не ведётся разработка, а секреты лежат в `deploy/.env`, который в `.gitignore`.
3. **Проверка:** `curl -fsS https://$ASTER_DOMAIN/health` с ретраями до 60 с. Не ответил — job красный.

Деплой и CI (`ci.yml`) — разные workflow: CI уже прошёл на PR, мерж не ждёт повторного прогона тестов.

Одновременно идёт не больше одного деплоя (`concurrency: deploy`, без отмены текущего).

### docker-compose

Сервис получает оба поля:

```yaml
signaling-server:
  image: ghcr.io/gemshrine/aster-signaling-server:${ASTER_IMAGE_TAG:-latest}
  build:
    context: ..
    dockerfile: deploy/Dockerfile
```

- `docker compose pull && up -d` — готовый образ из GHCR (автодеплой).
- `docker compose up -d --build` — локальная сборка, как раньше (ручной деплой из `0007` продолжает работать).

`ASTER_IMAGE_TAG` — конкретный sha, а не `latest`: сервер всегда ровно на том коммите, который задеплоен, и откат — повторный запуск workflow на старом коммите.

### Секреты и разовая настройка

Секреты репозитория (GitHub → Settings → Secrets and variables → Actions):

| Секрет | Что |
|---|---|
| `DEPLOY_HOST` | адрес VPS |
| `DEPLOY_USER` | пользователь SSH на VPS, в группе `docker` |
| `DEPLOY_SSH_KEY` | приватный ключ отдельной пары только для деплоя |
| `DEPLOY_KNOWN_HOSTS` | строка `ssh-keyscan <host>`, чтобы не отключать проверку ключа хоста |
| `ASTER_DOMAIN` | домен для проверки `/health` |

Если `DEPLOY_HOST` не задан, job `deploy` пропускается (образ всё равно публикуется) — workflow не краснеет, пока деплой не настроен.

На VPS один раз:

- публичный ключ деплоя в `~/.ssh/authorized_keys` пользователя `DEPLOY_USER`;
- репозиторий склонирован в `~/aster`, `deploy/.env` заполнен (`0007`);
- `docker login ghcr.io` с fine-grained токеном, у которого только `read:packages` — репозиторий приватный, значит, и пакет тоже. Токен живёт в `~/.docker/config.json` на VPS, в GitHub его нет.

Пошагово — в `deploy/README.md`.

### Что не деплоится автоматически

- `caddy` и `coturn` — официальные образы с закреплённой мажорной версией, их обновление остаётся ручным (`docker compose pull caddy coturn`).
- Клиент — это десктопное приложение, к VPS отношения не имеет.

## Альтернативы

- **Сборка на VPS по SSH** — отклонено: медленно и рискованно по памяти на целевом железе.
- **Watchtower / авто-pull на VPS** — отклонено: деплой становится невидимым из GitHub, нет проверки `/health` и понятной точки отката.
- **Docker Hub** — отклонено: GHCR уже в экосистеме репозитория, права через `GITHUB_TOKEN`.

## Затронутые компоненты

- `.github/workflows/deploy.yml` — новый.
- `deploy/docker-compose.yml` — `image` рядом с `build`.
- `deploy/README.md` — настройка автодеплоя.
- `specs/0007-deployment.md` — открытый вопрос про автодеплой закрыт.

## Открытые вопросы

- Уведомление о результате деплоя (например, в Telegram) — не нужно, пока статус виден в GitHub.
