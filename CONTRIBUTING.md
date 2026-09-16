# Contributing

Этот репозиторий приватный, для двух человек, но процесс всё равно строгий — так проще держать историю читаемой и не терять контекст решений.

## Workflow

1. **Issue** — перед тем как писать код, создаётся issue (`gh issue create`) с описанием задачи. Для нетривиальных фич/архитектуры — сначала спека в `specs/`, issue ссылается на неё.
2. **Assign** — `gh issue edit <n> --add-assignee @me` перед тем как начать.
3. **Branch** — от актуального `main`: `git checkout -b issue-<n>-<slug>`.
4. **PR** — `gh pr create --base main --head issue-<n>-<slug> --title "..." --body "Closes #<n>\n\n..."`. PR должен проходить CI.
5. **Merge** — squash merge после того как CI зелёный (и, если работали вдвоём над одним куском — после ревью).
6. **Close PR / Close issue** — происходит автоматически за счёт `Closes #<n>` в описании PR.

## Спеки

- Лежат в `specs/`, нумеруются `0001`, `0002`, ... по возрастанию.
- Формат — см. `specs/0000-template.md`.
- Спека мержится через свой issue/PR (`spec: ...`) до начала реализации соответствующей фичи.
- Устаревшая спека не удаляется, а помечается `Status: Superseded by 00NN`.

## Именование веток и коммитов

- Ветка: `issue-<n>-<короткий-slug>`, например `issue-7-opus-encoding`.
- Коммит: `<scope>: <summary>` в повелительном наклонении, например `client: add push-to-talk hotkey`.

## Перед PR

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
