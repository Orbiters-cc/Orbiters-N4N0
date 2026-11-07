# Translating one batch

You are translating one batch of Orbiters - N4N0 (its windows and overlays) into `ja`, `fr`, `es` and `pt-BR`. English stays the source text. Enzo asked for this: he uses the website's languages, and his language preference follows his account.

Your batch's name and files are in `BATCHES.md`. **Edit only those files and your own `crates/orbiters-i18n/catalog/<batch>/` folder.** That is what lets the batches merge cleanly.

## Set up

- Branch `i18n/<batch>` from `sota/i18n`, in your own worktree. Never `git stash`. Don't push, and don't merge into `main` or `sota/i18n`: the orchestrator does that.
- Read `AGENTS.md`, `GLOSSARY.md` and `website-GLOSSARY.md`.
- **The example to copy:** `crates/orbiters-desktop/src/overlay/orbit/panels/settings_sheet.rs` and `crates/orbiters-i18n/catalog/overlay-settings/`.
- Build the desktop crate only through `C:/Users/Enzo/.orbiters-build/sota/bench/deskcargo.sh "$PWD" <cargo args>`, and check its exit code.
- Don't run anything in VRChat. Renders and tests are enough. Leave Unity alone.
- **Don't edit:** `Cargo.toml`, `Cargo.lock`, `crates/orbiters-i18n/src`, `tests`, `not-shown.txt`, other catalog folders, or anyone else's source files. Every crate already depends on `orbiters-i18n`. If you need a change there, report it.
- Commits are authored by Enzo Dutra (`git -c user.name="Enzo Dutra" -c user.email=blackorbit.eclaire@gmail.com commit`) and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## The work

1. Make `catalog/<batch>/` with `ja.json`, `fr.json`, `es.json`, `pt-BR.json` (flat `{ "English": "translation" }`, UTF-8) and `files.txt`. List your files in `files.txt` **as you finish each one**: from then on the test holds it to the rules.
2. List a file's text, then wrap it:
   ```
   deskcargo.sh "$PWD" run -q -p orbiters-i18n --example i18n_scan -- file crates/.../page.rs
   ```
3. Translate each text you wrapped into all four languages. Texts already in `catalog/website` or `catalog/overlay-settings` need nothing.
4. Check: `deskcargo.sh "$PWD" test -p orbiters-i18n` and the tests of the code you touched.
5. Render and fix what no longer fits (below).

## How to wrap

| You have | Write |
|---|---|
| `"Save"` | `tr("Save")`, with `use orbiters_i18n::tr;` |
| `format!("Signed in as {username}")` | `tr!("Signed in as {username}", username = username)` |
| `format!("{:.1} MB left", x)` | `tr!("{size} MB left", size = format!("{x:.1}"))` |
| `"{n} pictures"`, by count | `tr_n("{count} picture", "{count} pictures", n as u64)` |
| A `const` table or a static label | `mark("Worlds")` where it is written, `tr(label)` where it is drawn |
| One English word needing two translations | `tr_ctx("the button", "Record")`, with key `"the button\u0004Record"` in the catalog |
| Text no person reads, but the scan flags it | Add `// i18n: not shown` at the end of that line |
| A text that stays the same in every language | A line in your area's `allow.txt` |

`tr` keeps the lifetime it was given, so `tr("x")` is still `&'static str`. Keys must be single string literals, or the test can't read them.

**Rewrite whole sentences, never pieces.** `format!("{}with {n} friend{}", a, b)` can't be translated: word order differs, and Japanese has no plural. Write the whole sentence for each case, even if that makes more strings. A value that is a fragment of a sentence is the same problem.

## Traps

- **Logic must not read translated text.** Find every comparison, `match`, `starts_with` or `contains` on a string you wrapped, and move it to an enum or to the English key. Don't call `tr` on something that is later compared.
- **egui ids:** `ui.id_salt(label)` and the like must stay on the English text, or an open popup loses its state when the language changes.
- **Text kept across frames won't change language.** Text computed once (`OnceLock`, a field set at start, a cached galley) keeps the old language. Compute it where it is drawn, or store the English and `tr` it at draw time.
- **Keep it English:** logs, and anything sent to a server or read by an AI, including prompts, tool descriptions, `Suggest` payloads and the assistant's instructions (`policy.rs`, `assistant.rs`). `not-shown.txt` covers some of this. Mark the rest `// i18n: not shown`.
- **Errors:** `anyhow!`, `bail!`, `.context()` and `Err(...)` strings are counted apart and **not** held to the test. Whether they get translated is an open decision: leave them unless your orchestrator says otherwise.
- **Numbers, dates and sizes:** pass them in as values. Don't type formatted numbers into a translation.
- **Tests:** they run in English by default, so tests that assert English text keep passing. To test another language use `orbiters_i18n::in_language(Language::Fr, || ...)`. Never `set_language` in a test: it changes every thread.

### The avatar's own screen (batch `n4n0`, and anything reaching `layout.rs` or `panel_text`)

The N4N0 panel and the live parts of the Screen, in VRChat, are drawn from a **256-glyph atlas** on the avatar (`contracts/Docs/n4n0.md`, Glyphs). It has the accents of French, Spanish and Portuguese but **no Japanese at all**: `fold` drops or replaces those letters with `?`, so Japanese text there would come out empty or as `???`.

- For text that reaches those places use **`tr_latin("...")`**. It gives the translation in French, Spanish and Portuguese, and English in Japanese. It takes `{name}` values with `orbiters_i18n::fill`.
- Those slots are also cut to a fixed number of cells (`slot_text`, 32 columns on the panel). A long French text is cut there, so check its length.
- **The Screen's fixed page pictures** are baked per language by `--n4n0-menu --strings`, and are not part of this work.
- The overlay and the desktop sheet use a real font, and draw N4N0's fixed texts (`Strings`) in Japanese without trouble. `slot_text` is shared with the screen, so its live parts follow the same rule.

## Fitting

Render your scenes in English and in the other languages, with the report on:

```
E=C:/Users/Enzo/.orbiters-build/sota/target-desktop/debug/orbiters-desktop.exe   # copy it out first: another agent's build replaces it
for l in en fr ja es pt-BR; do "$E" --screenshot out/$l --only <scenes> --language $l --text-report out/$l.tsv; done
python scripts/i18n/overflow-diff.py out/en.tsv out/fr.tsv
```

`--only orbit` renders all the overlay menus; other scene names are in `crates/orbiters-desktop/src/screenshot.rs`. A plain `--screenshot` with no `--only` panics in the projects scene, so always name your scenes.

The diff lists what a language has more of than English, and exits 1 if there is any. **English's own findings are the baseline**: it reports counts by scene and kind, so a new problem can hide behind a fixed one. Read the list.

| Kind | Means |
|---|---|
| `cut` | Ended in `…`, or ran out of rows |
| `box` | Runs past the box it is in |
| `clip` | Runs past its clip |
| `over` | Lies on other text |

Fix it in this order:
1. A shorter translation.
2. The code's own `fit` or wrapping, if it isn't already used.
3. A small layout change, only if the first two fail. Mention it in your report.

Don't change how anything looks in English. Don't redesign. The check can't see everything, so **look at a few renders yourself**, especially French in Retro and Japanese. Keep picture files few: they are hard on the harness and use lots of tokens, so at most 8 for a batch.

Say so if a scene doesn't exist for something you wrapped: it was tested, not seen.

## The same text in two batches

`one_english_text_has_one_translation` fails when two areas translate one English text differently. Batches can't see each other, so this appears at merge:
- **While you work:** use `catalog/website`, `catalog/overlay-settings` and `GLOSSARY.md`, and only add texts to your own folder. If the text is already in one of those, don't repeat it.
- **At merge:** the orchestrator keeps the first batch's translation and removes the other. If the two truly mean different things, it uses `tr_ctx`.

## When you finish

```
deskcargo.sh "$PWD" test -p orbiters-i18n
deskcargo.sh "$PWD" test -p orbiters-desktop --bin orbiters-desktop <what you touched> -- --test-threads=4
```

Report:
- Your branch and commit, and that it merges cleanly into `sota/i18n`.
- Files done, strings wrapped, and any file you could not finish.
- What the scan still lists, and what you marked `// i18n: not shown` or put in `allow.txt`, with a line on why.
- Overflow left after your fixes, by scene and language, and any layout you changed.
- Tests you ran with their results. What you looked at in renders. What was only tested.
- Anything you think is mistranslated, or that a native speaker should check.

Don't report as checked what you did not check.
