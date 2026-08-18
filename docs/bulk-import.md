# Bulk import: many cards + many media files in one request

For adding a lot of cards with their own audio/images at once (e.g. a full language-learning deck
with a pronunciation clip per card) — more efficient and more reliable than calling `upload_media`
once per file through a chat client. This is a **direct API recipe**, not an MCP tool: it needs
real file access (a terminal, a script, or an agentic CLI like Claude Code that can read local
files and run `curl`), not a JSON-RPC tool-call argument. It's the same endpoint EngrAmo's own
YouTube pipeline uses in production — see `engram-youtube/modules/engram_client.py` for a complete
reference implementation.

## The endpoint

```
POST /catalogs/batch-import
Authorization: Bearer <your engram_ token>
Content-Type: multipart/form-data
```

Three form fields:

| Field | Required | Content |
|---|---|---|
| `catalog_metadata` | yes | JSON: `{"name": "...", "description": "...", "tags": [...], "visibility": "private"}` |
| `catalog_cover_image` | no | An image file for the catalog's cover |
| `content_file` | yes | Either a `.zip` (media files + `index.json`) or a raw `.json` file — see below |

**Whole-request limit: 10MB** (same as the single-file `/media` upload) — this is for batching many
*small* files (short audio clips), not for large media.

## `content_file` shape

A JSON array of cards, referencing media by filename (which must exist inside the zip alongside
`index.json` — or be omitted entirely if you POST a raw JSON file with no media):

```json
[
  {
    "orderNumber": 1,
    "face": {
      "text": "¿Me puede traer un café, por favor?",
      "audioFileName": "card1_face.mp3",
      "dictionary": { "café": "coffee", "traer": "to bring" },
      "style": { "backgroundColor": "#FFF8E1" }
    },
    "back": {
      "text": "Could you bring me a coffee, please?",
      "audioFileName": "card1_back.mp3",
      "style": { "backgroundColor": "#E1F5FE" }
    }
  },
  {
    "orderNumber": 2,
    "face": { "text": "...", "audioFileName": "card2_face.mp3" },
    "back": { "text": "...", "audioFileName": "card2_back.mp3" }
  }
]
```

Each `face`/`back` side supports: `text` (required), `audioFileName`, `imageFileName`,
`videoFileName` (all reference files by name inside the zip), `dictionary`, `style` — same fields
as a regular card, per `engram://card-schema` in the MCP server. No server-side translation or TTS
happens here; every piece of content must already be provided.

## Minimal curl example

```bash
# Build the zip: index.json + every referenced audio file, flat, no subfolders.
zip -j deck.zip index.json card1_face.mp3 card1_back.mp3 card2_face.mp3 card2_back.mp3

curl -X POST https://api.engramo.app/catalogs/batch-import \
  -H "Authorization: Bearer $ENGRAM_API_TOKEN" \
  -F 'catalog_metadata={"name":"Ordering Coffee","tags":["spanish"]};type=application/json' \
  -F "content_file=@deck.zip;type=application/zip"
```

Response is a full `CatalogDto` (id, name, cardCount, ...) on `201`, same shape as any other
catalog-creation call. Note: the response's `cardCount` may show `0` even though the cards did
land — confirmed by immediately fetching `GET /catalogs/{id}/cards` afterward, which shows every
card with the correct text/dictionary/style and a resolvable `audioUrl` per attached file. Don't
treat `cardCount: 0` in the POST response as a failure signal; verify with a follow-up `GET` if
you want confirmation.

## No media at all? Skip the zip

If you just have text content (no audio/images), `content_file` can be a plain `.json` file with
the same `[{orderNumber, face, back}]` array and no `audioFileName`/etc. fields — no zip needed.

## Reference implementation

`engram-youtube/modules/engram_client.py`'s `upload_to_engram_catalog()` builds exactly this shape
in Python (zipping generated TTS audio segments + a face/back index) and is a working example to
copy from if scripting this yourself.
