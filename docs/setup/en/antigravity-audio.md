# Antigravity with card audio

[← All guides](README.md)

Antigravity is Google's free AI app for your computer. Its AI can do the whole EngrAmo setup for you,
including card audio. You just paste one message and answer a couple of pop-ups. Takes about 10 minutes.

## What you need

- Antigravity: download it from <https://antigravity.google> and sign in with your Google account
- Your **EngrAmo API token** and your **Gemini API key**: [get your keys](keys.md)

> **Never paste your keys into the chat.** The setup asks for each key in a small private window with a
> hidden field. Paste it there.

## Steps

1. Open Antigravity and open (or create) any folder. The setup doesn't change files in it.
2. Open the **Agent** panel and start a new conversation.
3. If you can, choose the agent mode that runs steps on its own (sometimes called auto mode). Otherwise
   Antigravity will ask you to approve each step; click **Accept** or **Run** each time.
4. Paste this message and send it:

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
   ```

5. When a pop-up asks for the **EngrAmo API token**, paste your token and click **OK**. Then do the same
   for the **Gemini API key**.
6. Antigravity tells you how to reload its MCP servers (usually: **⋯** menu in the Agent panel →
   **MCP Servers** → **Refresh**). Do it, then type `done`.
7. When it says **"Ready! What cards should we make?"**, you're set.

## Try it

```text
Create an EngrAmo catalog "Spanish for job interviews" with 15 cards. Put a natural Spanish phrase on
the front and the English meaning plus a short example on the back. Highlight the key words, and add
audio to every card with a natural Spanish voice.
```

More ideas: [prompt examples](../../prompt-examples.md).

## Check the setup later

After an update, or if something stops working, paste:

```text
Check my EngrAmo connection: call get_server_version, list_tts_voices and get_subscription_usage.
Tell me in plain words the server version, whether card audio is ready (and the default voice), my
plan, and how many cards I can still create. If any call fails, explain why and how to fix it.
```

## Troubleshooting

| Problem | What to do |
|---|---|
| No pop-up appeared | Look behind the Antigravity window. If there's still none, tell the agent "no pop-up appeared". It will use a hidden prompt in its terminal instead. |
| Audio isn't ready | Tell the agent "set my Gemini key again", then reload the MCP servers. |
| "Invalid token" | Generate a new EngrAmo token ([keys](keys.md)) and tell the agent "set my EngrAmo token again". |
| Audio stops with a quota or "rate limit" message | Your free daily Gemini quota ran out. Try again tomorrow, or add another key ([keys](keys.md)). |
