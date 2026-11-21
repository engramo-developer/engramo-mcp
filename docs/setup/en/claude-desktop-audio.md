# Claude Desktop with card audio

[← All guides](README.md)

Install the EngrAmo extension in Claude Desktop so Claude can create cards **and add spoken audio** to
them. No other programs to install, no files to edit. Works on Mac, Windows and Linux, including on the
free Claude plan. Takes about 5 minutes.

## What you need

- The Claude Desktop app: download it from <https://claude.ai/download>
- Your **EngrAmo API token** and your **Gemini API key**: [get your keys](keys.md)

## Steps

1. Download the extension file: [engramo.mcpb](https://github.com/engramo-developer/engramo-mcp/releases/latest/download/engramo.mcpb).
2. Open Claude Desktop.
3. Double-click the downloaded `engramo.mcpb` file (or drag it onto the Claude Desktop window).
4. Click **Install**.

   Claude shows a red warning that the extension gets access to your computer and that the developer isn't
   verified by Anthropic. Claude shows this for every extension installed from a file. Click **Install**
   to continue.
5. Fill in the two fields:
   - **EngrAmo API token:** paste your EngrAmo token.
   - **Gemini API keys for card audio:** paste your Gemini key. (Several keys? Separate them with commas.)
6. Click **Save**, and make sure the extension is turned **on**.

Claude keeps both keys in your computer's secure storage (Keychain on Mac).

## Check that it works

Start a new **Chat** (the extension doesn't work in Claude's Code mode) and paste:

```text
Check my EngrAmo connection: call get_server_version, list_tts_voices and get_subscription_usage.
Tell me in plain words the server version, whether card audio is ready (and the default voice), my
plan, and how many cards I can still create. If any call fails, explain why and how to fix it.
```

When Claude asks for permission to use EngrAmo, click **Allow**. It should say card audio is ready.

Then try:

```text
Create an EngrAmo catalog "Job interview English" with 10 cards of useful interview phrases. Put the
phrase on the front and the meaning plus an example on the back, and add audio to every card.
```

## Good to know

- **Already connected EngrAmo in Claude on the web?** That connection has no audio. When you want audio,
  start your message with "Using the EngrAmo extension…", or turn the web connector off in Claude Desktop
  under **Settings → Connectors**.
- **Change your keys later:** **Settings → Extensions → EngrAmo → Configure**.
- **Updates:** the extension doesn't update itself. To update, download `engramo.mcpb` again and install
  it the same way.
- **Audio** is made for the front of each card, up to 20 cards at a time. Claude handles bigger decks in
  batches by itself.

## Troubleshooting

| Problem | What to do |
|---|---|
| Claude says audio isn't available | Check the Gemini key under **Settings → Extensions → EngrAmo → Configure**, then fully quit Claude Desktop (⌘Q on Mac) and open it again. |
| "Invalid token" or EngrAmo doesn't respond | Generate a new EngrAmo token ([keys](keys.md)) and paste it into **Configure**. |
| Audio stops with a quota or "rate limit" message | Your free daily Gemini quota ran out. Try again tomorrow, or add another key ([keys](keys.md)). |
