# Antigravity з аудіо в картках

[← Усі інструкції](README.md)

Antigravity — безкоштовний ШІ-застосунок від Google для комп'ютера. Його ШІ може зробити все
налаштування EngrAmo за вас, разом з аудіо. Вам треба лише вставити одне повідомлення й відповісти на
кілька спливних вікон. Займає близько 10 хвилин.

## Що потрібно

- Antigravity: завантажте на <https://antigravity.google> і увійдіть з акаунтом Google
- Ваш **API-токен EngrAmo** і **API-ключ Gemini**: [отримайте ключі](keys.md)

> **Ніколи не вставляйте ключі в чат.** Налаштування запитає кожен ключ у маленькому приватному вікні з
> прихованим полем. Вставляйте туди.

## Кроки

1. Відкрийте Antigravity і відкрийте (або створіть) будь-яку папку. Налаштування не змінює в ній файлів.
2. Відкрийте панель **Agent** (Агент) і почніть нову розмову.
3. Якщо можете, виберіть режим агента, у якому він сам виконує кроки (іноді його називають
   автоматичним). Інакше Antigravity проситиме схвалити кожен крок; щоразу натискайте **Accept**
   (Прийняти) або **Run** (Виконати).
4. Вставте це повідомлення й надішліть. Воно англійською навмисно (це перевірена версія); останній рядок
   просить ШІ відповідати вам українською:

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

   Talk to me in Ukrainian.
   ```

5. Коли спливне вікно попросить **EngrAmo API token**, вставте свій токен і натисніть **OK**. Потім
   зробіть те саме для **Gemini API key**.
6. Antigravity підкаже, як перезавантажити MCP-сервери (зазвичай: меню **⋯** на панелі Agent →
   **MCP Servers** → **Refresh**). Зробіть це й напишіть `готово`.
7. Коли агент скаже, що все готово, і запитає, які картки створити, налаштування завершено.

## Спробуйте

```text
Створи каталог EngrAmo «Англійська для співбесіди» з 15 картками. На лицьовому боці — природна фраза
англійською, на звороті — переклад українською та короткий приклад. Виділи ключові слова й додай аудіо
до кожної картки природним англійським голосом.
```

Більше ідей (англійською): [приклади запитів](../../prompt-examples.md).

## Перевірити налаштування пізніше

Після оновлення або якщо щось перестало працювати, вставте:

```text
Перевір моє підключення до EngrAmo: виклич get_server_version, list_tts_voices і get_subscription_usage.
Простими словами скажи версію сервера, чи готове аудіо (і який голос за замовчуванням), мій тариф і
скільки карток я ще можу створити. Якщо якийсь виклик не вдався, поясни чому і як це виправити.
```

## Якщо щось не так

| Проблема | Що робити |
|---|---|
| Спливне вікно не з'явилося | Пошукайте за вікном Antigravity. Якщо його все одно немає, напишіть агенту «вікно не з'явилося», і він скористається прихованим полем у своєму терміналі. |
| Аудіо не готове | Напишіть агенту «налаштуй мій ключ Gemini ще раз», потім перезавантажте MCP-сервери. |
| «Invalid token» | Згенеруйте новий токен EngrAmo ([ключі](keys.md)) і напишіть агенту «налаштуй мій токен EngrAmo ще раз». |
| Аудіо зупиняється з повідомленням про квоту чи «rate limit» | Безкоштовну денну квоту Gemini вичерпано. Спробуйте завтра або додайте ще один ключ ([ключі](keys.md)). |
