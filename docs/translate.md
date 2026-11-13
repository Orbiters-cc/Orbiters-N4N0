# Live translation in VR

Enzo, 2026-11-13: "live ingame translation using the overlay. It would translate the text the user sees in VR in live and
placed roughly in the 3D space in the same way google translate makes". Turned on from the orbit menu (All tools,
Translation) or Settings (Translation in VR); off by default, and nothing of it runs while it is off.

## How it works

| Step | Where | What |
|---|---|---|
| Look | `orbiters-desktop/src/overlay/translate.rs`, thread "translate" | One persistent pair of SteamVR mirror streams (`EyeReader::read_pair_into`, buffers and staging textures kept). The middle 70 % of the left eye (the right eye's part wider), at most every 330 ms, paced to about a third of a thread on average; none while a menu of ours or SteamVR's dashboard is up, or while the head turns faster than 70°/s. Below-normal thread priority, ONNX Runtime threads that never spin. |
| Head | same | The head pose recorded each overlay turn, the one nearest the look's moment (15 ms before the read) is the head at capture. |
| Read | `orbiters-translate/src/ocr.rs`, `pipeline.rs` | PP-OCRv6 small detector on the area scaled to 960 px (rotated boxes, any angle, vertical Japanese too), our own translations masked grey; PP-OCRv6 small recogniser (Chinese, Japanese, English, 46 Latin languages); lines it is unsure of read again by the Korean and East Slavic PP-OCRv5 recognisers, whose reads count only in their own script. |
| Which | `language.rs` | Script first (kana: ja, Hangul: ko, Han: zh, Cyrillic: ru/uk), short-word lists for Latin languages; lines in the target language (or English, when "Leave English" is on), numbers and lone letters are left alone. |
| Place | `place.rs` | The line's two halves swept along the left eye's rays from 0.3 to 40 m, projected into the right eye with each eye's own field, compared by normalised cross-correlation: the distance is the best distinct match. Lines of one sign fit a plane (the text's tilt); a line alone faces the eye; a line the eyes cannot match goes on its ray at its neighbours' distance or 2 m, its size from its angular size. Each line is a world-anchored rectangle in SteamVR's standing space. |
| Follow | `track.rs` | Same text (or a close misread) near the same place is one line: its place takes 35 % of each new measurement. Missed twice where it should have been seen, it goes; out of view it stays 20 s; under its own translation it stays while a ring of what surrounds it looks the same. |
| Translate | `provider.rs`, `core.rs` | On a thread of its own, one batch at a time (up to 24 lines, strict JSON in and out), each text translated once (cache by text). |
| Draw | `paint.rs`, overlay turn | Word Lens' way: the original's place filled with the colour around it (edges fading into the surface, no frame), the translation in the original's text colour, as large as the original letters, shrunk then lengthened then wrapped to fit. At most 20 pooled overlays (`cc.orbiters.desktop.translate.N`, sort order 6, under the menus), the most central and nearest lines first, fading in and out over 180 ms. |

## Translators

- **This computer** (default): an OpenAI-compatible server found on `127.0.0.1` at LM Studio's (1234), Ollama's (11434) or
  llama.cpp's (8080) port, looked for again every 15 s while none answers. Its model: the one chosen in Settings, else one
  made for translating (TranslateGemma, Hy-MT, MADLAD) or a Qwen/Gemma instruct model. Nothing leaves the computer.
- **AI server**: the same at an address typed in Settings (another computer of the network).
- **Orbiters**: `POST n4n0/assistant/translate` on Orbiters' server with the app's credential (below). Only after the
  person accepted "With Orbiters, the text you see in VR, other people's chat included, is sent to Orbiters to be
  translated."
- A model built into the app: not shipped, see `done3-translate.md` (the measured reason).

## Orbiters' route (contract)

`POST /n4n0/assistant/translate`, `Authorization: Bearer <website JWT or orbit- credential>`.

```json
{ "target": "en", "lines": [ { "text": "出口", "source": "ja" }, { "text": "Добро пожаловать" } ] }
```

At most 32 lines, 400 characters each, 4,000 in all. Answer `200 { "translations": ["Exit", "Welcome"] }`, one per line, in
order. Errors are the assistant's `{ code, message }`: `400 bad_request`, `401 unauthorized`, `403 unavailable` (AI features
turned off for the account), `429 rate_limited` (90 requests a minute), `429 quota_exhausted` (the daily allowance of
lines, Admin → AI `n4n0.dailyTranslations`, default 3,000, counted in the API process for the UTC day), `503 unavailable`.
The model is the one Admin → AI routes the `N4N0_TRANSLATE` feature to, else the cheapest platform model with a key
(Gemini 3.1 Flash Lite, then GPT-OSS 20B on Groq, then DeepSeek Flash). Nothing but usage is recorded.

## Testing

- `cargo test -p orbiters-translate` (stand-in eyes rendered with the interface's painter: placement, stability, masking,
  colours, the local-server client against a loopback stand-in, batching); with the models in `ORBITERS_TRANSLATE_MODELS`,
  `-- --ignored` also reads the five scripts at angles and runs whole looks; `--test bench -- --ignored --nocapture` times
  the models on the sandbox's real VRChat eye pictures.
- Server: `node --test test/n4n0Translate.test.js`.
