# Antigravity с аудио в карточках

[← Все инструкции](README.md)

Antigravity — бесплатное ИИ-приложение от Google для компьютера. Его ИИ может сделать всю настройку
EngrAmo за вас, вместе с аудио. Вам нужно только вставить одно сообщение и ответить на пару всплывающих
окон. Занимает около 10 минут.

## Что нужно

- Antigravity: скачайте на <https://antigravity.google> и войдите с аккаунтом Google
- Ваш **API-токен EngrAmo** и **API-ключ Gemini**: [получите ключи](keys.md)

> **Никогда не вставляйте ключи в чат.** Настройка запросит каждый ключ в маленьком приватном окне со
> скрытым полем. Вставляйте туда.

## Шаги

1. Откройте Antigravity и откройте (или создайте) любую папку. Настройка не меняет в ней файлы.
2. Откройте панель **Agent** (Агент) и начните новый разговор.
3. Если можете, выберите режим агента, в котором он сам выполняет шаги (иногда его называют
   автоматическим). Иначе Antigravity будет просить одобрить каждый шаг; каждый раз нажимайте **Accept**
   (Принять) или **Run** (Выполнить).
4. Вставьте это сообщение и отправьте. Оно на английском намеренно (это проверенная версия); последняя
   строка просит ИИ отвечать вам по-русски:

   ```text
   Set up the EngrAmo MCP server for me, with card audio, following
   https://github.com/engramo-developer/engramo-mcp. I'm not a developer, so do every step yourself and
   tell me in plain words what you're doing.

   1. Download the prebuilt engramo-mcp binary for my computer from the latest GitHub release
      (https://github.com/engramo-developer/engramo-mcp/releases/latest/download/<file>, where <file> is
      engramo-mcp-darwin-arm64, engramo-mcp-darwin-x64, engramo-mcp-linux-x64, engramo-mcp-linux-arm64
      or engramo-mcp-win32-x64.exe). Save it to ~/.engramo/bin/ and make it executable. Don't install
      Node.js or npm.
   2. Add an "engramo" server to this app's MCP config file. Keep any servers already there. Use the
      binary's full path as "command" and set three env values: ENGRAMO_API_URL=https://api.engramo.app,
      ENGRAMO_API_TOKEN and ENGRAMO_TTS_GEMINI_API_KEYS.
   3. For the two keys, never ask me to paste them into this chat. Ask for each key in my operating
      system's own pop-up with a hidden input field (macOS: osascript "display dialog ... with hidden
      answer"; Windows: Get-Credential in powershell.exe; Linux: zenity --password or kdialog --password),
      and write the value straight into the config file from that same command. Never print, echo or
      read back the keys. Only if no pop-up is possible, use a hidden terminal prompt instead. Use one
      method at a time, and don't make me edit the config file.
   4. Tell me how to reload the MCP servers in this app, and wait until I say done.
   5. Check the connection: call get_server_version and list_tts_voices. If something fails, fix it.
   6. When both work, say "Ready! What cards should we make?" with the server version.

   Talk to me in Russian.
   ```

5. Когда всплывающее окно попросит **EngrAmo API token**, вставьте свой токен и нажмите **OK**. Затем
   сделайте то же самое для **Gemini API key**.
6. Antigravity подскажет, как перезагрузить MCP-серверы (обычно: меню **⋯** на панели Agent →
   **MCP Servers** → **Refresh**). Сделайте это и напишите `готово`.
7. Когда агент скажет, что всё готово, и спросит, какие карточки создать, настройка завершена.

## Попробуйте

```text
Создай каталог EngrAmo «Английский для собеседования» из 15 карточек. На лицевой стороне — естественная
фраза на английском, на обороте — перевод на русский и короткий пример. Выдели ключевые слова и добавь
аудио к каждой карточке естественным английским голосом.
```

Больше идей (на английском): [примеры запросов](../../prompt-examples.md).

## Проверить настройку позже

После обновления или если что-то перестало работать, вставьте:

```text
Проверь моё подключение к EngrAmo: вызови get_server_version, list_tts_voices и get_subscription_usage.
Простыми словами скажи версию сервера, готово ли аудио (и какой голос по умолчанию), мой тариф и
сколько карточек я ещё могу создать. Если какой-то вызов не удался, объясни почему и как это исправить.
```

## Если что-то не так

| Проблема | Что делать |
|---|---|
| Всплывающее окно не появилось | Поищите за окном Antigravity. Если его всё равно нет, напишите агенту «окно не появилось», и он воспользуется скрытым полем в своём терминале. |
| Аудио не готово | Напишите агенту «настрой мой ключ Gemini ещё раз», затем перезагрузите MCP-серверы. |
| «Invalid token» | Сгенерируйте новый токен EngrAmo ([ключи](keys.md)) и напишите агенту «настрой мой токен EngrAmo ещё раз». |
| Аудио останавливается с сообщением о квоте или «rate limit» | Бесплатная дневная квота Gemini исчерпана. Попробуйте завтра или добавьте ещё один ключ ([ключи](keys.md)). |
