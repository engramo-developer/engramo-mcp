# Connect EngrAmo to ChatGPT

[← All guides](README.md)

Works at chatgpt.com. Takes about 3 minutes.

> This connection has no card audio. For audio, see [Claude Desktop with audio](claude-desktop-audio.md)
> or [Antigravity with audio](antigravity-audio.md).

## What you need

- A ChatGPT account. Adding your own app needs **developer mode**, which isn't on every plan. If you don't
  see it in step 2, your plan doesn't include it.
- An EngrAmo account (create one at <https://study.engramo.app> if you don't have one)

> EngrAmo isn't listed in the ChatGPT app directory yet. Once it is, you'll be able to search for
> **EngrAmo** under **Apps** and click **Connect**, with no developer mode needed. Until then, use the steps below.

## Steps

1. On a computer, open <https://chatgpt.com> and sign in.
2. Click your name (bottom left) → **Settings** → **Apps & Connectors** (called **Connectors** in some
   versions) → **Advanced settings**, and turn on **Developer mode**.
3. Back in **Apps & Connectors**, click **Create**.
4. Fill in:
   - **Name:** `EngrAmo`
   - **MCP server URL:** `https://mcp.engramo.app/`
   - **Authentication:** `OAuth`
5. Tick the box confirming you trust this app, then click **Create**.
6. An EngrAmo sign-in page opens. Sign in with your **EngrAmo email and password** and click **Approve**.
7. You're back in ChatGPT, and EngrAmo shows as connected.

## Check that it works

1. Start a new chat.
2. Click **+** next to the message box, open **More** (or **Developer mode**), and choose **EngrAmo**.
3. Type:

   ```text
   Show my EngrAmo catalogs.
   ```

ChatGPT asks you to confirm before it changes anything in your account (for example, creating cards).
Click **Confirm**.

## Disconnect

In ChatGPT: **Settings → Apps & Connectors → EngrAmo → Delete**. Or in the EngrAmo app:
**Settings → Connected apps**.
