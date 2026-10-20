# MCP для YadaCall / YadaCall MCP

## Русский

YadaCall запускает MCP stdio-сервер прямо из desktop-приложения или CLI. Для Claude Desktop вставьте JSON из Настройки → Подключения → «Подключить к Claude (MCP)» в конфигурацию MCP. Для Claude Code скопируйте предложенную там команду. Вручную:

```json
{"mcpServers":{"yadacall":{"command":"/Applications/YadaCall.app/Contents/MacOS/yadacall-desktop","args":["--mcp"]}}}
```

```sh
claude mcp add yadacall -- "/Applications/YadaCall.app/Contents/MacOS/yadacall-desktop" --mcp
```

CLI для разработки: `yadacall mcp`. Доступ к изменению задач включается только аргументом `--allow-write`.

Инструменты: `list_meetings`, `get_meeting`, `search_meetings` (FTS + эмбеддинги, если активна установленная модель E5; иначе только FTS), `list_tasks`, `upcoming_meetings`, `complete_task` (только с записью). Календарные события доступны, если существует локальный кэш; standalone-сервер возвращает пустой результат с пояснением, когда кэш недоступен.

Агент читает тексты встреч, итоги и задачи; звук не передаётся. Агент может отправлять текст своей облачной модели в соответствии с настройками самого агента. Индекс открывается только для чтения по умолчанию.

Пример routine prompt: «Каждое утро проверь задачи на сегодня и на эту неделю, а также решения из вчерашних встреч. Кратко перечисли просроченные задачи отдельно и укажи название встречи для каждого пункта».

## English

YadaCall runs an MCP stdio server from the desktop executable or CLI. For Claude Desktop, paste the JSON from Settings → Connections → “Connect to Claude (MCP)” into its MCP configuration. For Claude Code, copy the command shown there. Example:

```json
{"mcpServers":{"yadacall":{"command":"/Applications/YadaCall.app/Contents/MacOS/yadacall-desktop","args":["--mcp"]}}}
```

```sh
claude mcp add yadacall -- "/Applications/YadaCall.app/Contents/MacOS/yadacall-desktop" --mcp
```

For development, run `yadacall mcp`. Task updates are enabled only with `--allow-write`.

Tools: `list_meetings`, `get_meeting`, `search_meetings` (FTS plus embeddings when the installed E5 model is active, FTS-only otherwise), `list_tasks`, `upcoming_meetings`, and `complete_task` (write mode only). Calendar events are returned when a local cache is available; otherwise the standalone server returns an empty result with a note.

The agent can read meeting text, summaries, and tasks. Audio is never passed to MCP. The agent may send text to its own cloud model, subject to that agent’s settings. The index is opened read-only by default.

Example routine prompt: “Every morning, check tasks due today and this week, then summarize decisions from yesterday’s meetings. List overdue tasks separately and include the meeting title for each item.”
