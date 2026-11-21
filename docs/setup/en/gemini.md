# Connect EngrAmo to Gemini

[← All guides](README.md)

Works at gemini.google.com and, once connected, in the Gemini mobile app too. Takes about 3 minutes.

> This connection has no card audio. For audio, see [Antigravity with audio](antigravity-audio.md).

## What you need

Google currently limits custom apps in Gemini to people who:

- are **18 or older** and **in the US**
- use a **personal** Google account (not a work or school account)
- have Gemini **History** (also called **Keep Activity**) turned on. Check it at
  <https://myactivity.google.com/product/gemini>. When it's off, custom apps don't work.

Custom apps only work in English for now. You also need an EngrAmo account (create one at
<https://study.engramo.app> if you don't have one).

## Steps

1. On a computer, open <https://gemini.google.com> and sign in.
2. Click **Settings** (bottom left), then **Connected Apps**. (In some versions it's under
   **Personal Intelligence**. A shortcut: <https://gemini.google.com/apps>.)
3. Under **Custom apps**, click **Add a custom app**.
4. Enter the address: `https://mcp.engramo.app`
5. Click **Next**.
6. An EngrAmo sign-in page opens. Sign in with your **EngrAmo email and password** and click **Approve**.
7. You're back in Gemini, and EngrAmo shows under **Custom apps**.

If step 5 shows an error, try the address with a slash at the end: `https://mcp.engramo.app/`

## Check that it works

Start a new chat, type `@`, choose **EngrAmo**, then type:

```text
Show my EngrAmo catalogs.
```

Typing `@EngrAmo` makes sure Gemini uses EngrAmo for that message. Gemini asks you to confirm before it
changes anything in your account (for example, creating cards).

## Disconnect

In Gemini: **Settings → Connected Apps**, then under **Custom apps**:

- turn off the switch next to EngrAmo to pause it, or
- click **More details → Remove app** to remove it completely.

Or in the EngrAmo app: **Settings → Connected apps**.
