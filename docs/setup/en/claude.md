# Connect EngrAmo to Claude

[← All guides](README.md)

Works on claude.ai, in Claude Desktop and in the Claude mobile app. You set it up once and it's available
everywhere you sign in to Claude. Takes about 2 minutes.

> This connection has no card audio. For audio, see [Claude Desktop with audio](claude-desktop-audio.md).

## What you need

- A Claude account
- An EngrAmo account (create one at <https://study.engramo.app> if you don't have one)

## Steps

1. Open the EngrAmo page in the Claude directory: <https://claude.ai/directory/engramo>.
   (Or, in Claude: **Settings → Connectors → Browse**, then search for **EngrAmo**.)
2. Click **Connect**.
3. Claude shows a short notice that EngrAmo is a community connector. Continue.
4. An EngrAmo sign-in page opens. Sign in with your **EngrAmo email and password**.
5. Click **Approve** to let Claude use your EngrAmo account.
6. You're back in Claude, and EngrAmo shows as connected.

## Check that it works

Start a new chat and type:

```text
Show my EngrAmo catalogs.
```

The first time Claude uses EngrAmo, it asks for permission. Click **Allow** (or **Always allow** so it
stops asking).

## If you can't find EngrAmo in the directory

Some work accounts hide the directory. Add EngrAmo by its address instead:

1. In Claude, open **Settings → Connectors**.
2. Click **Add custom connector**.
3. Name: `EngrAmo`. URL: `https://mcp.engramo.app/`
4. Click **Add**, then **Connect**, and continue from step 4 above.

On a Team or Enterprise plan, an admin may need to allow it first.

## Disconnect

In Claude: **Settings → Connectors → EngrAmo → Disconnect**. Or in the EngrAmo app:
**Settings → Connected apps**.
