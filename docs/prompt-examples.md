# Prompt examples

Ready-to-paste prompts for generating rich EngrAmo flashcards through `engramo-mcp` — translation,
dictionary highlights, consistent styling, a catalog cover image, and your own audio. None of this
uses EngrAmo's paid AI: the calling model (Claude, ChatGPT, ...) does the translation itself, and
audio/images are entirely bring-your-own (record your own voice, use your own tools — `engramo-mcp`
only stores and attaches what you give it).

If your MCP client supports a prompt picker, `create_language_deck` is available there directly with
these same arguments as form fields — the examples below are for clients without one, or if you'd
rather just type it.

## 1. Simplest — plain request, no special prompt

```
Make me 5 Spanish flashcards about greetings.
```

Works today since the `generate_card`/`generate_catalog_with_cards` tools already explain
dictionary/translation/styling to the model, but the result is inconsistent — sometimes rich,
sometimes plain text, depending on how the request is phrased. Use recipe #2 below for a
consistent result every time.

## 2. Structured deck — translation + dictionary + consistent styling

```
Use the create_language_deck prompt:
topic="ordering coffee", source_lang="Spanish", target_lang="English", count=5
```

Produces 5 cards, each with 1-4 key words highlighted and added to the dictionary, a translated
back side, and one style reused across the whole deck. See `engram://card-schema`'s Example 3 for
exactly what the output looks like.

Add an existing catalog instead of creating a new one:

```
Use the create_language_deck prompt:
topic="past tense verbs", source_lang="Spanish", target_lang="English", count=8,
catalog_id="<your-catalog-uuid>"
```

## 3. Adding a catalog cover image

```
Give the catalog a cover image — here's one I like.
```
*(attach an image file in the chat)*

The model calls `upload_media` with the image, then passes the returned `media_id` as `image_id`
when creating (or updating) the catalog.

## 4. Adding your own audio — filename convention

Record or generate short audio clips yourself, name them `card{N}_face`/`card{N}_back` (e.g.
`card1_face.mp3`, `card1_back.mp3`, `card2_face.mp3`, ...), then:

```
I recorded myself for each card — here are the files.
```
*(attach all the audio files in the chat)*

The model recognizes the naming convention, calls `upload_media` once per file, and attaches each
`media_id` to the matching card's `audio_id`.

## 5. Single-card touch-up, no convention needed

```
Add audio to just card 3's back — here's the file.
```
*(attach one audio file, no naming convention required — say which card/side directly)*

## 6. Fully natural language — no tool or prompt names mentioned

```
I want to practice French cooking verbs. Make 6 cards, put a nice cooking-themed picture on the
catalog, and I'll record myself saying each one after — just do the text and translations for now.
```

The model follows the same recipe as `create_language_deck` from the tool descriptions alone, notes
no image was attached yet, creates the deck, and is ready for audio via recipe #4/#5 afterward.
