# Get your keys (for card audio)

[← All guides](README.md)

The setups with card audio ([Claude Desktop](claude-desktop-audio.md), [Antigravity](antigravity-audio.md))
need two keys. A key is a long password-like text that lets an app act for you. Takes about 5 minutes.

> **Keep your keys private.** Don't paste them into a chat, an email or a screenshot. Anyone who has a key
> can use your account. If a key leaks, delete it and create a new one.

## 1. EngrAmo API token

This lets the app on your computer use your EngrAmo account.

1. Open <https://study.engramo.app> and sign in.
2. Click the **person icon** (top right) → **API token**.
3. Click **Generate**.
4. Copy the token (it starts with `engramo_`) and keep it somewhere safe, for example a password manager.
   You may not be able to see it again later. If you lose it, just generate a new one.

## 2. Gemini API key (free)

This lets the app make card audio with Google's voices. It's free, and the audio uses your own Google
quota, not your EngrAmo plan.

1. Open <https://aistudio.google.com/apikey> and sign in with a Google account.
2. If asked, accept the terms.
3. Click **Create API key**. If asked to choose a project, pick any (or let it create one).
4. Copy the key (it usually starts with `AIza`) and keep it somewhere safe.

**Making lots of audio?** The free quota is limited per day. You can create keys in a few different Google
projects and enter them all, separated by commas (`key1,key2`). EngrAmo switches to the next key when one
runs out.

## Next

- [Claude Desktop with audio](claude-desktop-audio.md)
- [Antigravity with audio](antigravity-audio.md)
