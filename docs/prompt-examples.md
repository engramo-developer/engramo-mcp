# Prompt examples

Ready-to-paste prompts for `engramo-mcp`. Paste one into your AI app, change the topic, and send. You
don't need to know the tool names; they're listed under each prompt only so you know what to expect.

The model you're chatting with (Claude, Gemini, GPT, ...) writes, translates and styles the cards itself,
so it doesn't use your EngrAmo plan's AI quotas. Card audio uses your own Gemini key (local setups only, see
[Text-to-speech](../README.md#text-to-speech-bring-your-own-key)). Not set up yet? Start with
[Set up with your AI assistant](../README.md#set-up-with-your-ai-assistant).

**Contents**

- [Check your setup](#1-check-your-setup)
- [Create decks](#create-decks): [language deck with audio](#2-language-deck-with-audio-cover-and-styling) ·
  [from your notes or an article](#3-turn-notes-an-article-or-a-transcript-into-cards) ·
  [any subject](#4-any-subject-exam-terms-and-definitions) · [quick prompt picker](#5-quick-deck-with-the-create_language_deck-prompt)
- [Improve decks](#improve-decks): [add cards](#6-add-more-cards-to-a-deck) ·
  [add audio](#7-add-audio-to-a-deck-you-already-have) · [restyle and fix](#8-restyle-or-fix-cards) ·
  [your own recordings](#9-attach-your-own-recordings)
- [Study](#study): [start learning](#10-start-learning-a-deck) · [learning path](#11-build-a-learning-path) ·
  [search](#12-find-cards-across-all-your-decks) · [shadowing](#13-speaking-practice-shadowing)
- [Account](#account): [plan and usage](#14-check-your-plan-and-usage) · [share a deck](#15-share-a-deck) ·
  [find and classify decks](#16-find-and-classify-decks)

## 1. Check your setup

```text
Check my EngrAmo connection: call get_server_version, list_tts_voices and get_subscription_usage.
Tell me in plain words the server version, whether card audio is ready (and the default voice), my
plan, and how many cards I can still create. If any call fails, explain why and how to fix it.
```

If `list_tts_voices` is missing, the Gemini key isn't set, the app wasn't reloaded, or you're on a remote
(hosted) connection, which never has card audio.

## Create decks

### 2. Language deck with audio, cover and styling

The all-in-one prompt. Change the language, topic and number of cards.

```text
Create a new EngrAmo catalog "Spanish for job interviews" with 15 cards.
- Front: a natural Spanish phrase I'd really say in an interview.
- Back: the English meaning, plus one short example sentence.
- Highlight 1-3 key words on each card and add them to the card's dictionary with a translation.
- Use one consistent, readable style for the whole deck (same colors and font size on every card).
- Use the image at ~/Downloads/cover.jpg as the catalog cover.
- Then add audio to every card with a natural Spanish voice.
Show me the finished list of cards at the end.
```

Tools: `upload_media` (cover) → `generate_catalog_with_cards` (with `image_id`) → `list_tts_voices` →
`generate_card_audio`.

- The cover can also be added or replaced later (`update_catalog` with `image_id`); it can't be removed.
- In apps that can read files (Antigravity, Claude Code, Cursor), give the file's path. In chat apps,
  attach the image instead. Keep it small (under ~500 KB): the model copies every byte of it, so large
  images are slow or fail.
- Audio is made for the front of each card, up to 20 cards per call; the model splits bigger decks
  itself.
- Free Gemini keys allow only a few audio requests per minute and per day. If a batch hits the limit,
  the tool tells the model whether to wait about a minute and retry the failed cards or stop for the day.

### 3. Turn notes, an article or a transcript into cards

```text
Here is an article I'm reading: <paste the text, or give a file path or link>.
Make an EngrAmo catalog "<title>" with the 20 most useful words and phrases from it. Put the word or
phrase on the front, exactly as it appears in the text. On the back, give the meaning in English and
the original sentence it came from. Skip words a B1 learner already knows. Add audio to every card.
```

Works the same for lecture notes, a YouTube transcript, song lyrics or a chapter of a book.

### 4. Any subject: exam terms and definitions

```text
I'm preparing for a biology exam on cell structure. Create an EngrAmo catalog "Biology: the cell" with
25 cards. Front: a short question ("What does the mitochondrion do?"). Back: a clear 1-2 sentence
answer, with the key term highlighted. Order the cards from basic to advanced. No audio needed.
```

Good for medicine, law, history dates, programming concepts, certification exams: anything with a
question and an answer.

### 5. Quick deck with the `create_language_deck` prompt

If your app has a prompt picker (a `/` menu or a "prompts" button), choose `create_language_deck` and
fill in the form. Otherwise type:

```text
Use the create_language_deck prompt:
topic="ordering coffee", source_lang="Spanish", target_lang="English", count=8
```

You get cards with 1-4 highlighted dictionary words, a translated back and one style across the deck.
Add `catalog_id="<uuid>"` to put the cards into a catalog you already have.

## Improve decks

### 6. Add more cards to a deck

```text
Add 10 more cards to my "Spanish for job interviews" catalog, about talking about salary and
benefits. Keep exactly the same style and format as the existing cards, don't repeat any phrase
that's already there, and add audio to the new cards.
```

Tools: `list_catalogs` → `list_cards` (to copy the style and avoid duplicates) → `generate_cards` →
`generate_card_audio`.

### 7. Add audio to a deck you already have

```text
Add audio to every card in my "French travel phrases" catalog that doesn't have it yet. First show me
the available voices and suggest two that sound natural for French, then use the one I pick.
```

Cards that already have audio are skipped. Say "replace the existing audio" to redo them (the old files
stay in your media library).

### 8. Restyle or fix cards

```text
Go through my "Biology: the cell" catalog. Fix any mistakes in the answers, make every back side at
most two sentences, and make the highlighted key terms bold and dark blue. Tell me what you changed.
```

Tools: `list_cards` → `update_card`. To remove cards: "delete the cards about ribosomes" (`delete_card`;
cards you already started learning are archived, not lost).

### 9. Attach your own recordings

Record short clips, name them `card1_face.mp3`, `card1_back.mp3`, `card2_face.mp3`, and so on, then:

```text
I recorded myself for each card in my "Italian basics" catalog. Attach the files in ~/Recordings to
the matching cards.
```

For a single card no naming is needed: "Add this recording to the back of card 3."

## Study

Grading happens in the EngrAmo app (<https://study.engramo.app> or the mobile app). Your AI assistant
sets things up and tells you what's due.

### 10. Start learning a deck

```text
Add all cards from my "Spanish for job interviews" catalog to my learning queue, then tell me how many
cards are due today.
```

Tools: `add_catalog_to_learning` → `get_due_cards`. Later: "What's due for review today?"

### 11. Build a learning path

```text
Create a learning path "Spanish B1 in 3 months" from my catalogs "Spanish basics", "Spanish for job
interviews" and "Spanish travel phrases", in that order, and activate it.
```

Tools: `create_learning_path` (with `catalog_ids`) → `activate_learning_path`. An active path's catalogs
are included in your daily reviews; deactivate it to pause them. Later: "Add my 'Spanish food' catalog
to that path" (`add_catalog_to_learning_path`) or "Remove 'Spanish basics' from it"
(`remove_catalog_from_learning_path`).

### 12. Find cards across all your decks

```text
Do I already have cards about "negotiating salary"? Search all my catalogs and show me what you find.
```

Tools: `search_global` (cards, catalogs and learning paths). To find a catalog by name or by its short
code from the app, `search_catalogs`. Handy before creating a deck, to avoid duplicates.

### 13. Speaking practice (shadowing)

```text
Turn on shadowing for my "Spanish for job interviews" catalog.
```

The model calls `set_catalog_shadowing` with the language of the card fronts (e.g. `es-ES`). The
catalog then appears in the app's Shadowing tab, where you repeat each phrase out loud. Later: "Show
my shadowing catalogs."

## Account

### 14. Check your plan and usage

```text
How much of my EngrAmo plan have I used? When do my limits reset?
```

Tools: `get_subscription_usage`. After a "Quota exceeded" or "Not included in your plan" error, the model
calls it by itself to explain what ran out and when it comes back. Plan changes aren't self-service
yet; quote your user id when you contact the EngrAmo team.

### 15. Share a deck

Sharing needs a paid plan.

```text
Share my "Spanish for job interviews" catalog with user 3f2b8c1e-5d4a-4e7b-9a10-2c6d8e9f0a11 as a viewer.
```

Tools: `share_catalog`. Users are identified by user id (a UUID, not an email): each person gets theirs by
asking their own assistant "What's my EngrAmo user id?". Say "editor" to let them change cards too.
Later: "Who has access to this catalog?" (`list_catalog_shares`), "Stop sharing it with that user"
(`unshare_catalog`), "Show catalogs shared with me" (`list_catalogs` with `filter: "shared"`).

### 16. Find and classify decks

```text
Find public Spanish catalogs for beginners about medicine. Then take my "Pharmacy phrases" catalog and
set its languages, topic and level so it shows up in that search too.
```

The model calls `get_catalog_filter_options` for the valid topics and language codes, then `list_catalogs`
with `front_languages`, `levels`, `topics` and `filter: public`. To classify your own deck it calls
`update_catalog` with `front_languages`, `back_languages`, `topics` and `levels`.
