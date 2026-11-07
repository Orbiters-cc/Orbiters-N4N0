# N4N0 glossary

Keeps the batches consistent. **Read `website-GLOSSARY.md` first.** It is the website's own glossary (copied from `frontend/src/i18n/GLOSSARY.md`, server commit `6317750b` or later), and all of it applies: register, punctuation, capitalization, plural rules, gender-neutral phrasing. This file only adds what N4N0 needs. Where they disagree, this file wins, because N4N0 runs beside VRChat. `{{name}}` there is `{name}` here.

## The rules that matter most

| | ja | fr | es | pt-BR |
|---|---|---|---|---|
| Address | です/ます, no あなた | **tu**, never vous | **tú**, never usted or vosotros | **você** |
| Buttons, tabs | Verbal noun: 保存 | Infinitive: Enregistrer | Infinitive: Guardar | Infinitive: Salvar |
| Instructions | 〜してください | Tu-imperative: Choisis | Tú-imperative: Elige | Você-imperative: Escolha |
| Case | n/a | Sentence case | Sentence case | Sentence case |
| Ellipsis | `…` | `…` | `…` | `…` |
| Before `? ! : ;` | Full-width `？！：`, no space | NBSP (U+00A0) | No space; `¿…?` `¡…!` | No space |
| Apostrophe | n/a | `’` always | n/a | n/a |

Keep the English string's end: a period stays a period, a label stays without one. Never write ALL CAPS: Retro uppercases text itself.

## Never translated

Always in Latin script, as written here (ja: never katakana).

- **Orbiters:** Orbiters, Orbiters - N4N0, N4N0, Orbiters Toolkit, My Avatar, ReFit, Mixer
- **VRChat and tools:** VRChat, VRChat+, Unity, Unity Hub, Creator Companion, VCC, VPM, SDK, Udon, Community Labs
- **VR and systems:** SteamVR, Spout, OSC, OSCQuery, Quest, PC, Android, iOS, Windows, macOS, VR
- **Services:** YouTube, Discord, Blender, GitHub, Booth, Gumroad, Ko-fi, Patreon, Stripe, Steam
- **AI:** Claude, ChatGPT, OpenAI, Gemini, Codex
- **Keys:** Ctrl, Alt, Left Alt, Shift, Esc, Enter, Tab, and letters and digits used as keys
- **User content:** world names, avatar names, people, song and video titles, file names, and the sample names in scenes
- **Language names in the picker:** always in their own language (`Language::native_name`)

## VRChat's own menus: **translated, in VRChat's words**

Enzo decided (2026-11-07): Quick Menu is translated. VRChat's own translated pages do too, and people run VRChat in these languages, so we use VRChat's words.

| English | ja | fr | es | pt-BR |
|---|---|---|---|---|
| Quick Menu | クイックメニュー | menu rapide | menú rápido | Menu Rápido |
| Action Menu | アクションメニュー | menu d’action | menú de acción | Menu de Ação |
| Main Menu | メインメニュー | menu principal | menú principal | Menu Principal |

**How sure this is:** taken from the Quick Menu and Controls pages of [wiki.vrchat.com](https://wiki.vrchat.com/wiki/Quick_Menu/fr), which are community translations (97–100% complete, read 2026-11-07), not from the game. Checking the game was out of scope. The Spanish wiki says both *menú de acción* and *menú de acciones*; we use the first. VRChat's own UI words not listed here keep their English name, in Latin script, as the website's glossary section 1.8 does for Unity's panels. That includes **Stream Camera**, which people have to find on screen.

## Words of these menus

Where the column says "menu", the word comes from N4N0's existing menu translations (`crates/orbiters-n4n0-core/menu-strings`), which already shipped. Where it says "website", it comes from the website. "New" was chosen here, with no earlier use to match.

| English | ja | fr | es | pt-BR | From |
|---|---|---|---|---|---|
| 3D picture | 3D写真 | photo 3D | foto 3D | foto 3D | menu |
| picture, pictures | 写真 | photo, photos | foto, fotos | foto, fotos | website, menu |
| 3D capture, capture | 3Dキャプチャ, キャプチャ | capture 3D, capture | captura 3D, captura | captura 3D, captura | website |
| world | ワールド | monde | mundo | mundo | website |
| Worlds in 3D | 3Dワールド | Mondes en 3D | Mundos en 3D | Mundos em 3D | new |
| instance | インスタンス | instance | instancia | instância | website |
| avatar | アバター | avatar | avatar | avatar | website |
| friend | フレンド | ami | amigo | amigo | website |
| invite | 招待 | inviter, invitation | invitar, invitación | convidar, convite | website |
| gallery | ギャラリー | galerie | galería | galeria | website |
| settings | 設定 | paramètres | configuración | configurações | website |
| language | 言語 | langue | idioma | idioma | website |
| sign in | ログイン | se connecter | iniciar sesión | entrar | website |
| saved look (N4N0's states) | 状態 | état | estado | estado | menu |
| fast walk | 早歩き | pas rapide | paso rápido | andar rápido | menu |
| stay here | ここに固定 | rester ici | quedarse | ficar aqui | menu |
| light | ライト | lumière | luz | luz | menu |
| music | 音楽 | musique | música | música | menu, website |
| tile (on the wing) | タイル | tuile | mosaico | bloco | new |
| record (start recording, a verb) | 録画 | enregistrer | grabar | gravar | new; **not** the website's "Record", which is a file |
| recording (a noun) | 録画 | enregistrement | grabación | gravação | new |
| reconstruct, reconstruction | 再構築 | reconstruire, reconstruction | reconstruir, reconstrucción | reconstruir, reconstrução | new |
| splat | スプラット | splat | splat | splat | new |
| session | セッション | session | sesión | sessão | new |
| Drop in world | ワールドに置く | Déposer dans le monde | Dejar en el mundo | Deixar no mundo | new |
| headset | ヘッドセット | casque | visor | headset | new |
| laser | レーザー | laser | láser | laser | new |
| computer | コンピューター | ordinateur | computadora | computador | website |

## Words with two senses

These were kept out of `catalog/website`. Choose by what the string names, and use `tr_ctx` if one English word needs two translations in the same area:

Record, Yours, Shared, People, Look, Sharp, Top, Base, Light, Test, Comment, Where, Desktop, Manual, Automatic, Mine, All, None, Live, Show, Hide, Next, Previous, Default.

Participles and adjectives agree with a noun in French, Spanish and Portuguese (Sent, Approved, Cancelled, Hidden, Connected, Paused). Look at what they describe. If the same text describes several nouns, rephrase it neutrally (`En pause`) or use `tr_ctx`.

## Layout

- **French:** often 30% longer. Prefer the shorter good wording first (`La sortie de Windows au démarrage de SteamVR`, not `La sortie utilisée par Windows…`).
- **Retro:** monospace capitals are wider still. Render in Retro too.
- **Japanese:** no spaces, so it wraps anywhere and looks short but tall. Check that it reads, not just that it fits.
