# Antigravity con audio en las tarjetas

[← Todas las guías](README.md)

Antigravity es la aplicación de IA gratuita de Google para tu ordenador. Su IA puede hacer toda la
configuración de EngrAmo por ti, audio incluido. Tú solo pegas un mensaje y respondes a un par de ventanas
emergentes. Tardas unos 10 minutos.

## Qué necesitas

- Antigravity: descárgalo en <https://antigravity.google> e inicia sesión con tu cuenta de Google
- Tu **token de API de EngrAmo** y tu **clave de API de Gemini**: [consigue tus claves](keys.md)

> **Nunca pegues tus claves en el chat.** La configuración pide cada clave en una pequeña ventana privada
> con un campo oculto. Pégala ahí.

## Pasos

1. Abre Antigravity y abre (o crea) cualquier carpeta. La configuración no cambia archivos dentro de ella.
2. Abre el panel **Agent** (Agente) e inicia una conversación nueva.
3. Si puedes, elige el modo del agente que ejecuta los pasos por sí solo (a veces llamado modo automático).
   Si no, Antigravity te pedirá aprobar cada paso; haz clic en **Accept** (Aceptar) o **Run** (Ejecutar)
   cada vez.
4. Pega este mensaje y envíalo. Está en inglés a propósito (es la versión probada); la última línea pide
   que la IA te responda en español:

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

   Talk to me in Spanish.
   ```

5. Cuando una ventana emergente pida el **EngrAmo API token**, pega tu token y haz clic en **OK**. Después
   haz lo mismo con la **Gemini API key**.
6. Antigravity te dirá cómo recargar sus servidores MCP (normalmente: menú **⋯** del panel Agent →
   **MCP Servers** → **Refresh**). Hazlo y escribe `listo`.
7. Cuando diga que todo está listo y pregunte qué tarjetas crear, ya está.

## Pruébalo

```text
Crea un catálogo de EngrAmo "Inglés para entrevistas de trabajo" con 15 tarjetas. Pon una frase natural en
inglés en el anverso y la traducción al español con un ejemplo corto en el reverso. Resalta las palabras
clave y añade audio a cada tarjeta con una voz inglesa natural.
```

Más ideas (en inglés): [ejemplos de mensajes](../../prompt-examples.md).

## Comprobar la configuración más tarde

Después de una actualización, o si algo deja de funcionar, pega:

```text
Comprueba mi conexión con EngrAmo: llama a get_server_version, list_tts_voices y get_subscription_usage.
Dime con palabras sencillas la versión del servidor, si el audio está listo (y la voz por defecto), mi
plan y cuántas tarjetas puedo crear todavía. Si alguna llamada falla, explica por qué y cómo arreglarlo.
```

## Solución de problemas

| Problema | Qué hacer |
|---|---|
| No apareció ninguna ventana | Mira detrás de la ventana de Antigravity. Si sigue sin aparecer, dile al agente «no apareció ninguna ventana» y usará un campo oculto en su terminal. |
| El audio no está listo | Dile al agente «vuelve a configurar mi clave de Gemini» y recarga los servidores MCP. |
| «Invalid token» | Genera un nuevo token de EngrAmo ([claves](keys.md)) y dile al agente «vuelve a configurar mi token de EngrAmo». |
| El audio se detiene con un mensaje de cuota o «rate limit» | Se agotó tu cuota diaria gratuita de Gemini. Inténtalo mañana o añade otra clave ([claves](keys.md)). |
