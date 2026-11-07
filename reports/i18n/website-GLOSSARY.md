# Orbiters localization: glossary and style guide

Single source of truth for translating the Orbiters web UI (about 10k strings) from English (`en`) into `ja`, `fr`, `es` (Latin-American) and `pt-BR`. Mandatory for every translator and reviewer. Items marked `!` need a native speaker (section 7).

**Precedence.** (1) Placeholder and markup rules (section 2) never bend. (2) Do-not-translate list (section 3). (3) Glossary (section 4). (4) Style rules (section 1). (5) Your judgment. If a glossary term does not fit a sentence, rewrite the sentence, not the term. If a term you need is missing, use the most natural term the VRChat creator community uses, and list it as `TERM?` in your hand-off notes; never invent variants of an existing term.

**Scope.** Public site only. Admin screens and the `/n4n0/workspace` workspace stay English. Shared wording already lives in `locales/<tag>/common.json` (actions, states, errors, count): reuse it with `t('common:actions.save')` instead of translating the same word again, and keep your own wording identical to it.

---

## 1. Product decisions

### 1.1 Locales

| Tag (folder) | URL prefix | Language | Address form |
|---|---|---|---|
| `en` | none | English (source) | you |
| `ja` | `/ja` | Japanese | polite です/ます, friendly-neutral, like a modern web service |
| `fr` | `/fr` | French (France) | informal **tu**, always. Never "vous/votre/vos" |
| `es` | `/es` | Latin-American neutral Spanish (es-419) | **tú**. Never vosotros, voseo or usted |
| `pt-BR` | `/pt-br` | Brazilian Portuguese | **você / seu / sua**. Never "tu/teu" |

### 1.2 Voice (all languages)

- Direct, calm, task-specific. Do not make copy friendlier, funnier or more marketing-like than the English (no slogans).
- Translate meaning, not words. One sentence stays one sentence; keep every warning ("This cannot be undone").
- **Mirror terminal punctuation.** English sentence with a period: end with the locale's full stop. Label, button, heading, tab, tooltip fragment or toast title without a period: no period.
- Same register everywhere. One "we" is allowed only where the English has "we"; do not add new ones (ja and fr: say "Orbiters" or use the passive).
- Sample and fictional names (Moon Jacket, Nova, Mika, Ultirex, Starlight...) and user-generated content are never translated.

**Tone check (same message, right and wrong register).**

| Lang | Do | Do not |
|---|---|---|
| ja | 変更を保存できませんでした。もう一度お試しください。 | 変更を保存することができませんでした。再度お試し頂けますようお願い申し上げます。 |
| fr | Tu n’as pas accès à ceci. Réessaie. | Vous n’avez pas accès à ce contenu. Veuillez réessayer. |
| es | No tienes acceso a esto. Inténtalo de nuevo. | Usted no tiene acceso. / No tenéis acceso. / No tenés acceso. |
| pt-BR | Você não tem acesso a isto. Tente de novo. | Tu não tens acesso a isto. / O utilizador não tem acesso. |

**ja specifics.** です/ます in sentences. No heavy keigo (ございます, いたします, お客様, 〜になります). Allowed: 〜できます, 〜してください, 〜しましょう (rare). Error pattern: 〜できませんでした。もう一度お試しください。 Do not use あなた (see 1.7). No exclamation marks except short celebratory toasts. Katakana: keep the trailing long-vowel ー on -er/-or/-ar words (サーバー, ユーザー, シェーダー, パラメーター, クリエイター, モデレーター); exceptions without ー: テクスチャ, プロパティ.

**fr specifics.** Warm but plain tu, no slang, no emojis added. Typographic apostrophe `’` always.

**es specifics.** Use only vocabulary shared by Mexico, Colombia, Argentina and Chile: say *computadora, celular, hacer clic, archivo, agregar*; never *ordenador, móvil, coger, vale, fichero, pinchar, añadir*. Use `¿ ¡` pairs. tú-imperatives: *Elige, Escribe, Selecciona, Inténtalo de nuevo*.

**pt-BR specifics.** Brazilian vocabulary: *tela, usuário, arquivo, mouse, celular, compartilhar, baixar, gerenciar, salvar*; never *ecrã, utilizador, ficheiro, telemóvel, partilhar, transferir, gerir, guardar* (for save). Post-2009 orthography. você-form imperatives (*Escolha, Digite, Tente de novo*), never tu-forms (*escolhe*).

### 1.3 Action labels: one rule per language

| Language | Rule | Why |
|---|---|---|
| ja | **Buttons, menu items, tabs: bare verbal noun (サ変名詞)**: 保存, 削除, 追加, 共有; with object: アセットを追加. Use the verb form only when the bare noun would read as a status: 公開する (公開 = "Public"), 連携する (連携 = "Connected"), and for state changes: 有効にする, 無効にする, 非公開にする; native verbs stay dictionary form: 閉じる, 開く, 戻る, 続ける. Never ます-form on a button. Confirmation questions and toasts: 削除しますか？ / 保存しました / 保存できませんでした。 | Standard in Japanese UI, shortest, same as the existing `common.json`. |
| fr | **Buttons, menu items, tabs, dialog actions: infinitive** (Enregistrer, Ajouter un asset). **Sentences addressed to the user: tu-imperative** (Choisis un serveur, Réessaie). Dialog titles: infinitive question (Supprimer cet asset ?). | Infinitive is the French UI convention for labels and avoids tu/vous in two-word buttons. |
| es | **Buttons, menu items, tabs: infinitive** (Guardar, Agregar rol). **Instructions: tú-imperative** (Elige un servidor). Dialog titles: ¿Eliminar este asset? | The infinitive is the same in every region, unlike the tú/vos/usted imperatives. |
| pt-BR | **Buttons, menu items, tabs: infinitive** (Salvar, Adicionar cargo). **Instructions: você-imperative** (Escolha um servidor, Tente de novo). Dialog titles: Excluir este asset? | Brazilian UI convention (Google, Microsoft, Discord). |

### 1.4 Typography

Always type the real character. Interpolation escaping is off, so an HTML entity such as `&amp;`, `&nbsp;` or `&quot;` would show literally: write `&`, U+00A0 and `“”` yourself (copy entities only if they already exist in the English source).

| Topic | ja | fr | es | pt-BR |
|---|---|---|---|---|
| Ellipsis | `…` (one character, U+2026), no space before. Also replaces `...` in the source | same | same | same |
| Sentence end | 。 | . | . | . |
| `?` `!` | full-width ？！ , no space | NBSP (U+00A0) before ? ! | `¿…?` `¡…!`, no space inside | no space before |
| `:` `;` | avoid trailing colons on labels; in text use `：` | NBSP before `:` and `;` | no space before | no space before |
| Quotes | 「…」, nested 『…』 | « NBSP…NBSP », nested “…” | “…”, nested ‘…’ | “…”, nested ‘…’ |
| Parentheses | （…） if the content contains Japanese; (…) if only Latin/digits; no spaces around | ( ) with a space before, none inside | same as fr | same as fr |
| Between scripts | **No space** between Japanese and Latin/digits/placeholders: `ReFitコミッション`, `{{count}}件`, `VRChatで開く` | normal | normal | normal |
| Ranges, lists | `〜` (U+301C) for ranges, `、` between items, `/` half-width for alternatives, `・` only to pair two short nouns | en dash `–` for ranges | same | same |
| `&` in labels | `・` or `と` ("Terms & operator" is 利用規約・運営者情報) | `et` | `y` | `e` |
| Em dash `—` | not used | not used: use colon, comma, parentheses or ` – ` | same | same |
| Literal numbers | half-width digits, `,` grouping | NBSP grouping, `,` decimal | `,` grouping, `.` decimal (es-419) | `.` grouping, `,` decimal |
| Percent | `50%` | `50 %` (NBSP) | `50%` | `50%` |
| File sizes | `10MB` | `10 Mo` (Ko, Mo, Go; NBSP) | `10 MB` | `10 MB` |
| Literal money in docs (`€2`) | `€2` | `2 €` (NBSP) | `€2` | `€ 2` (NBSP) |

Never type dates, times, relative times, prices or formatted counts yourself: code formats them in the page language (`useFormat()`); you only write the sentence around the value. Keyboard keys and shortcuts stay as in the source (`Ctrl+K`, `Esc`). In the fr examples of this guide, `NBSP` characters are real U+00A0; copy them exactly.

### 1.5 Capitalization

- **fr, es, pt-BR: sentence case everywhere** (buttons, tabs, headings, nav, table headers). Capitalize only the first word, proper nouns and brands. English Title Case ("Save Settings", "Add New Redirection") becomes "Enregistrer les paramètres", "Agregar redirección", "Adicionar redirecionamento".
- Day names, months and language names are lowercase (*lundi, mayo, japonés, português*).
- Keep accents on capitals (*À, É, Á, Ç*). Strings written ALL CAPS in the source are translated in normal case (CSS decides the display); `ja` has no case.
- Acronyms keep case (API, URL, ID, VPM, MCB).

### 1.6 Gender-neutral phrasing

- **fr**: no point médian, no parenthesized endings, no slashes. Prefer epicene or collective words: *membre, personne, artiste, équipe, communauté, compte*. Rewrite gendered participles that agree with "tu": "Connecté" becomes "Session ouverte", "Tu es invité" becomes "Tu as reçu une invitation". "Creator" is *créateur* as a modifier in compounds (*espace créateur, profil créateur, statut créateur*); for people in a sentence write *créateurs et créatrices* when space allows; in compact lists a bare *Créateurs* is accepted `!`.
- **es**: no `@`, `x`, `e` endings. Use *personas, quienes, comunidad, equipo, miembros*. Avoid gendered participles with "tú" ("Estás conectado"): use "Sesión iniciada como {{name}}", "¿Quieres continuar?" instead of "¿Estás seguro?". Compact role nouns use the generic masculine (*Creadores, Moderadores*).
- **pt-BR**: no `@`, `x`, `e` endings, no "(a)". Use *pessoas, comunidade, equipe, membros*. "Bem-vindo" becomes "Boas-vindas". "Tem certeza?" is fine (epicene). Compact role nouns use the generic masculine (*Criadores, Moderadores*).
- **ja**: no gender issue; never use 彼/彼女.

### 1.7 "You / your / my"

| English | ja | fr | es | pt-BR |
|---|---|---|---|---|
| Your account (text addressed to the user) | omit the pronoun: アカウント | ton compte | tu cuenta | sua conta |
| My Account (nav, button) | アカウント | Mon compte | Mi cuenta | Minha conta |
| My commissions | 自分のコミッション | Mes commissions | Mis comisiones | Minhas comissões |
| you can... | 〜できます | tu peux... | puedes... | você pode... |

ja: omit possessives; use 自分の only to tell the user's items from others'. Never あなた.

### 1.8 Third-party software the user must find on screen

- Unity (its editor UI exists in English, Japanese, Korean and Chinese, so fr/es/pt-BR users run an English editor), the VRChat SDK panel, Creator Companion (VCC), the VPM and Blender are looked up **by their English label**. Menu paths, buttons and tabs you tell the user to click stay in English exactly as written in the source: `Tools > Orbiters > Mixer console`, `Add Component`, `Manage Project`, `Build & Publish`, `Authentication`.
- Generic Unity nouns in prose follow the glossary (section 4.3): ja uses Unity's Japanese editor names (インスペクター, ヒエラルキー, コンソール); fr/es/pt-BR keep the English noun (Inspector, Hierarchy, Console).
- Discord is localized in all four languages: when a sentence points to a Discord setting, use Discord's own words (fr *salon*, pt-BR *cargo*...), which the glossary already follows.

---

## 2. Placeholders and markup

Parity with the English string is checked by `node scripts/i18n-check.cjs` (placeholders, numeric Trans tags, plural completeness, empty values, extra keys). The checker does **not** check named Trans tags, `$t()` or Markdown, so you must.

### 2.1 Never translate, alter, add or remove

| Item | Rule |
|---|---|
| `{{name}}`, `{{count}}`, `{{ name }}`, `{{value, number}}`, `{{- raw}}` | Copy character for character, case-sensitive. Do not add or remove spaces inside the braces. Each placeholder appears as many times as in English. Never replace one by its likely value. Word order around it may change. |
| `<0>…</0>`, `<1/>`, `<link>…</link>`, `<strong>…</strong>`, `<br/>` | Keep tag names, indexes, attributes and nesting. Translate only the text between tags. Do not merge, split, drop or add tags. Tags may move if the grammar demands it. In ja leave no space between a tag and adjoining Japanese. |
| `$t(key)`, `$t(ns:key, {"x":1})` | Untouched, including namespace colon and options. |
| HTML entities, URLs, emails, domains (`orbiters.cc`), paths, ids (`wrld_…`, `usr_…`), file names and extensions (`.unitypackage`, `.fbx`, `PNG`) | Untouched. |
| Emoji and symbols (including ZWJ sequences), `→`, `↗`, `⌘` | Untouched. |
| Keyboard keys: `Ctrl`, `Alt`, `Shift`, `Enter`, `Esc`, `Tab`, `F`, `Home` | Untouched. |
| Text in backticks, code, CSS classes, enum values, JSON keys | Untouched. Never translate JSON keys or `_context` suffixes. |
| Markdown syntax: `**bold**`, `_italic_`, `[text](url)`, lists, `#` headings | Keep the syntax. Translate link text and the words only; never the URL or anchor. |

JSON hygiene: valid JSON, UTF-8, LF, 2-space indent; escape `"` as `\"` (curly quotes need no escape); keep `\n`; same key order and nesting as English.

### 2.2 Sentences with placeholders

- Never depend on grammar that the placeholder breaks. If `{{item}}`, `{{role}}` or `{{type}}` can be any noun, avoid articles, adjectives and participles that must agree with it: write `Supprimer « {{name}} » ?`, `¿Eliminar “{{name}}”?`, `Excluir “{{name}}”?`, `「{{name}}」を削除しますか？`.
- `{{count}}` is already formatted by the code. Do not add thousands separators or units to it yourself.
- If a string cannot be translated safely (a fragment built in code, a placeholder that is itself a sentence), keep the English text unchanged and list the key in your hand-off notes.

### 2.3 Plural keys (i18next JSON v4, ICU is NOT used)

Verified with `Intl.PluralRules` on Node 22 (i18next 26.4.4):

| Tag | Keys to provide | Behaviour |
|---|---|---|
| `en` | `key_one`, `key_other` | 1 is *one*; 0 and everything else is *other*. |
| `ja` | `key_other` only | Every count is *other*. Never add `_one`. |
| `fr` | `key_one`, `key_many`, `key_other` | **0 and 1.x are *one*** ("0 élément", "1,5 jour"). 2+ is *other*. |
| `es` | `key_one`, `key_many`, `key_other` | 1 is *one*; **0 is *other*** ("0 elementos"). |
| `pt-BR` | `key_one`, `key_many`, `key_other` | **0 and 1.x are *one*** ("0 item"). 2+ is *other*. |

- **`_many` is the "millions" form.** It is selected only for exact multiples of 1,000,000 (1000000, 2000000…). The noun then needs the preposition: fr `d’`/`de`, es `de`, pt-BR `de`. It is never identical to `_other` (see examples).
- `_zero` (if the English source defines it for an empty state) is looked up first when count is 0 in every language: translate it in all four locales; never add it when English has none. `_two` and `_few` are never used in these locales.
- Every plural key must keep `{{count}}` in all of its forms, unless the English form deliberately omits it ("one item" style).
- Plural keys are only for `t(key, { count })`. A phrase like "3 days ago" is formatted by code.

### 2.4 Plural examples

| Locale | `_one` | `_many` | `_other` |
|---|---|---|---|
| en | `{{count}} asset` | n/a | `{{count}} assets` |
| ja | n/a | n/a | `{{count}}件のアセット` |
| fr | `{{count}} asset` | `{{count}} d’assets` | `{{count}} assets` |
| es | `{{count}} asset` | `{{count}} de assets` | `{{count}} assets` |
| pt-BR | `{{count}} asset` | `{{count}} de assets` | `{{count}} assets` |

Japanese counters: items/records **件**, people **人**, times **回**, files **個**, days **日**, weeks **週間**, months **か月**, years **年**, hours **時間**, minutes **分**, seconds **秒**. Attach to the number without a space.

### 2.5 Markdown documentation (if translated)

Translate front matter `title`, `section` and body prose. Never `id`, `domain`, `type`, `owner`, `audience`, `stage`, `order`, `lastVerified`, `relations`. Keep custom tags and attributes (`<audience include="creator, admin, dev">`, `<alpha>`), Mermaid blocks (translate only visible node labels), tables, and anchors. Link text is translated, link targets are not.

---

## 3. Do-not-translate list

Keep spelling and casing exactly as shown, in every language, in Latin script (ja: never katakana: not ディスコード, not ブイアールチャット). Never pluralize them (*des ReFit*, *los Discord*). In ja attach Japanese particles/nouns with no space (`Discordサーバー`). Verified against the code base and documentation.

| Group | Names |
|---|---|
| Orbiters products | **Orbiters**, Orbiters Toolkit, **ReFit**, **My Avatar** (code spelling `MyAvatar`; the UI spelling "My Avatar" wins), **My Custom Base** (abbreviation **MCB**), **Mixer** (its windows **Mixer console** and **Mixer Git**), Orbiters VPM, Orbiters MCP, Orbiters Knowledge MCP, N4N0, Hermes, Product Steward |
| Release stages | **Alpha**, **Beta** (identifier-like labels). "Public" is translated |
| VRChat and Unity | **VRChat**, VRChat+, VRChat SDK, Community Labs, **Creator Companion** (**VCC**), **VPM**, Unity, Unity Hub, Udon, UdonSharp, **PhysBones**, VRCFury, Modular Avatar, Poiyomi, lilToon, Blender, FBX, unitypackage |
| Trust rank names | Visitor, New User, User, Known User, Trusted User (the rank names are proper nouns; the phrase "trust rank" is translated, section 4.3) `!` |
| Platforms | **PC**, **Android**, **iOS**, **Quest**, Meta Quest, Windows, macOS, Linux. Orbiters writes "Android (Quest)" |
| Social and chat | **Discord**, **Telegram**, Twitch, Bluesky, Instagram, YouTube, TikTok, X, Mastodon, Reddit, Slack, Steam, Fur Affinity |
| Stores and payments | **Booth**, **Gumroad**, **Jinxxy**, **Ko-fi**, **Payhip**, **Patreon**, Lemon Squeezy, Fourthwall, VGen, **Stripe**, **PayPal** |
| Dev and work tools | **GitHub**, Trello, Notion, Cloudflare (R2), Raspberry Pi, Mermaid, Markdown, WebGL |
| AI products | ChatGPT, OpenAI, Claude (Claude Code, Haiku, Opus, Sonnet), Codex, Gemini, Gemma, Grok, Mistral AI, Groq, DeepSeek |
| Acronyms and formats | API, SDK, OAuth, MCP, URL, ID, JSON, CSV, ZIP, PNG, JPEG, WebP, SVG, TGA, HTML, 2FA when written as such |
| Community jargon | **YCH** ("Your Character Here", used worldwide, also in ja), **DM** (all four languages) |
| Sample data | Moon Jacket, Nova, Mika, Ultirex, Luna, Ari... and every user-generated name or title |

**Prepositions before platform names.** fr *sur* (sur Discord, sur Orbiters, dans VRChat); es *en* with no article (en Discord, en VRChat); pt-BR *no* (no Discord, no VRChat, no Orbiters, no Unity). When an article's gender is unclear, put a generic noun first: *a loja Booth*, *l’outil ReFit*, *la herramienta ReFit*, *a ferramenta ReFit*, *ReFitツール* (never a bare gendered article before ReFit, My Avatar, Mixer, Mixer console or N4N0).

**"Quest" is only the headset/platform.** Do not translate it. Never use it as a word for "quest/mission".

---

## 4. Glossary table

Columns: English | ja | fr | es | pt-BR | note. Note starts with the ja expectation: **K** katakana loanword expected (do not replace by a native word), **N** native Japanese word expected (do not katakana-ize), **KN** mixed compound, **L** Latin script. Source tags: **VRC** verified against the VRChat community wiki translations (wiki.vrchat.com); **DSC** Discord's own localization (from memory); **STR** Stripe dashboard wording (from memory); **UNI** Unity's Japanese editor (from memory); **CJ** already in `locales/*/common.json`. `!` = needs native confirmation. Where two English words share one translation on purpose, the note says so.

### 4.1 Actions and shared UI words (adopted from `common.json`)

Not repeated here because `common.json` is authoritative and unambiguous: Apply, Back, Close, Confirm, Done, Edit, Next, Previous, Open, View, Yes/No, On/Off, None, Unknown, Optional/Required, Loading, No results, Search, Send, Share, Copy, Create, Reset, Sort, Learn more.

| English | ja | fr | es | pt-BR | note |
|---|---|---|---|---|---|
| Add | 追加 | Ajouter | Agregar | Adicionar | N. CJ, but es.json has *Añadir*: correct to *Agregar* (4.9) |
| Cancel | キャンセル | Annuler | Cancelar | Cancelar | K. CJ |
| Clear (field, filters) | クリア | Effacer | Borrar | Limpar | K. CJ |
| Connect (a service) | 連携する | Connecter | Conectar | Conectar | N. CJ. See 4.8 rule 2 |
| Continue | 続ける | Continuer | Continuar | Continuar | N. CJ |
| Delete (destroys it) | 削除 | Supprimer | Eliminar | Excluir | N. CJ |
| Remove (takes out of a list or board) | 削除 (外す if the item survives elsewhere) | Retirer | Quitar | Remover | N. CJ |
| Disable | 無効にする | Désactiver | Desactivar | Desativar | N. CJ |
| Enable | 有効にする | Activer | Activar | Ativar | N. CJ |
| Disconnect (a service) | 連携を解除 | Déconnecter | Desconectar | Desconectar | N. CJ |
| Download | ダウンロード | Télécharger | Descargar | Baixar | K. CJ. Nouns: téléchargement, descarga, download |
| Filter / Filters | 絞り込み | Filtrer / Filtres | Filtrar / Filtros | Filtrar / Filtros | N. CJ. "Clear filters": 絞り込みをクリア, Effacer les filtres, Borrar filtros, Limpar filtros |
| More / Show more | もっと見る | Plus / Voir plus | Más / Ver más | Mais / Ver mais | N. CJ |
| Show less | 折りたたむ | Voir moins | Ver menos | Ver menos | N. CJ, but ja.json has 閉じる (identical to Close): correct (4.9) |
| Publish | 公開する | Publier | Publicar | Publicar | N. CJ |
| Unpublish | 非公開にする | Dépublier | Despublicar | Despublicar | N. CJ |
| Refresh | 更新 | Actualiser | Actualizar | Atualizar | N. CJ. Same word as Update in ja/es/pt-BR on purpose; fr keeps *mettre à jour* for Update |
| Update (save new data) | 更新 | Mettre à jour | Actualizar | Atualizar | N |
| Report (abuse) | 報告 | Signaler | Reportar | Denunciar | N. CJ. ja alternative 通報 `!` |
| Try again / Retry | もう一度試す | Réessayer | Reintentar | Tentar de novo | N. CJ. Sentences: もう一度お試しください / Réessaie / Inténtalo de nuevo / Tente de novo |
| Save | 保存 | Enregistrer | Guardar | Salvar | N. CJ |
| Sign in | ログイン | Se connecter | Iniciar sesión | Entrar | K. CJ |
| Log out | ログアウト | Se déconnecter | Cerrar sesión | Sair | K. CJ |
| Sign up / Create account | アカウントを作成 | Créer un compte | Crear cuenta | Criar conta | N |
| Upload | アップロード | Téléverser | Subir | Enviar arquivo | K. CJ. fr never "uploader". pt-BR: *fazer upload* for avatars and worlds sent to VRChat; noun *upload* |
| All | すべて | Tout | Todo | Tudo | N. CJ. Bare "All". "All assets" is *Tous les assets / Todos los assets / Todos os assets* |
| Select / Select all | 選択 / すべて選択 | Sélectionner / Tout sélectionner | Seleccionar / Seleccionar todo | Selecionar / Selecionar tudo | N |
| Discard (changes) | 破棄 | Abandonner | Descartar | Descartar | N |
| Keep editing | 編集を続ける | Continuer à modifier | Seguir editando | Continuar editando | N |
| Unsaved changes | 未保存の変更 | Modifications non enregistrées | Cambios sin guardar | Alterações não salvas | N |
| Dismiss (a notice) | 閉じる | Ignorer | Descartar | Dispensar | N |
| Revoke (token, key) / (access, invite) | 無効化 / 取り消す | Révoquer | Revocar | Revogar | N |
| Pause / Resume | 一時停止 / 再開 | Mettre en pause / Reprendre | Pausar / Reanudar | Pausar / Retomar | N |
| Archive / Restore | アーカイブ / 復元 | Archiver / Restaurer | Archivar / Restaurar | Arquivar / Restaurar | K / N |
| Rename | 名前を変更 | Renommer | Cambiar nombre | Renomear | N |
| Replace | 置き換える | Remplacer | Reemplazar | Substituir | N |
| Move | 移動 | Déplacer | Mover | Mover | N |
| Join | 参加 | Rejoindre | Unirse | Participar | N. pt-BR *Participar* (not *Juntar-se*) |
| Leave (community, group) | 退出 | Quitter | Salir | Sair | N |
| Invite (verb) / Invitation | 招待 | Inviter / Invitation | Invitar / Invitación | Convidar / Convite | N. DSC |
| Drag and drop | ドラッグ&ドロップ | Glisser-déposer | Arrastrar y soltar | Arrastar e soltar | K |
| Copy link | リンクをコピー | Copier le lien | Copiar enlace | Copiar link | K |
| Mark all read | すべて既読にする | Tout marquer comme lu | Marcar todo como leído | Marcar tudo como lido | N |

### 4.2 Statuses and states

| English | ja | fr | es | pt-BR | note |
|---|---|---|---|---|---|
| Draft | 下書き | Brouillon | Borrador | Rascunho | N |
| Public | 公開 | Public | Público | Público | N |
| Private | 非公開 | Privé | Privado | Privado | N |
| Unlisted | 限定公開 | Non répertorié | No listado | Não listado | N. Matches YouTube's localization |
| Published | 公開中 | Publié | Publicado | Publicado | N |
| Not published | 未公開 | Non publié | No publicado | Não publicado | N. Distinct from Private (非公開) |
| Visibility | 公開範囲 | Visibilité | Visibilidad | Visibilidade | N |
| Pending | 保留中 | En attente | Pendiente | Pendente | N |
| In progress | 進行中 | En cours | En curso | Em andamento | N |
| Completed | 完了 | Terminé | Completado | Concluído | N |
| Cancelled | キャンセル済み | Annulé | Cancelado | Cancelado | KN. Spell "Cancelled" and "Canceled" the same |
| Failed | 失敗 | Échec | Fallido | Falhou | N |
| Expired | 期限切れ | Expiré | Expirado | Expirado | N |
| Active / Inactive | アクティブ / 非アクティブ | Actif / Inactif | Activo / Inactivo | Ativo / Inativo | K |
| Enabled / Disabled | 有効 / 無効 | Activé / Désactivé | Activado / Desactivado | Ativado / Desativado | N |
| Verified | 認証済み | Vérifié | Verificado | Verificado | N |
| Error / Warning / Success | エラー / 警告 / 成功 | Erreur / Avertissement / Succès | Error / Advertencia / Éxito | Erro / Aviso / Sucesso | K / N / N |
| Status | ステータス | Statut | Estado | Status | K. pt-BR *status* (not *estado*) |

### 4.3 VRChat, avatars and Unity

| English | ja | fr | es | pt-BR | note |
|---|---|---|---|---|---|
| avatar | アバター | avatar | avatar | avatar | K. VRC |
| avatar base | ベースアバター | avatar de base | avatar base | avatar base | K. The original avatar creators customize and fit clothing to. Not 素体 |
| base body (unclothed) | 素体 | corps de base | cuerpo base | corpo base | N. ja community word, rare in UI |
| custom base | カスタムベース | base personnalisée | base personalizada | base personalizada | K. Generic noun; the product stays "My Custom Base" |
| world | ワールド | monde | mundo | mundo | K. VRC. Never 世界 |
| instance | インスタンス | instance | instancia | instância | K. VRC. Not *estancia* (7) |
| instance types: Public / Friends+ / Friends / Invite+ / Invite / Group / Group+ / Group Public | パブリック / フレンド+ / フレンド / 招待+ / 招待 / グループ / グループ+ / グループパブリック | Publique / Amis+ / Amis / Invitation+ / Invitation / Groupe / Groupe+ / Groupe public | Pública / Amigos+ / Amigos / Invitación+ / Invitación / Grupo / Grupo+ / Grupo público | Público / Amigos+ / Amigos / Convite+ / Convite / Grupo / Grupo+ / Grupo público | K. VRC for fr, es, pt-BR; ja `!` (no ja wiki page found) |
| friend | フレンド | ami | amigo | amigo | K. VRC. Friends list: フレンドリスト |
| friend request | フレンドリクエスト | demande d’ami | solicitud de amistad | pedido de amizade | K |
| group (VRChat or Discord) | グループ | groupe | grupo | grupo | K. VRC |
| platform | プラットフォーム | plateforme | plataforma | plataforma | K |
| trust rank | トラストランク | rang de confiance | rango de confianza | nível de confiança | K. VRC. pt-BR `!` |
| performance rank | パフォーマンスランク | rang de performance | rango de rendimiento | classificação de desempenho | K `!`. VRC es says *rendimiento* |
| Excellent / Good / Medium / Poor / Very Poor | エクセレント / グッド / ミディアム / プア / ベリープア | Excellent / Bon / Moyen / Mauvais / Très mauvais | Excelente / Bueno / Medio / Pobre / Muy pobre | Excelente / Bom / Médio / Ruim / Muito ruim | K `!`. Align with the VRChat client when verified |
| upload (to VRChat) | アップロード | téléverser | subir | fazer upload | K |
| thumbnail | サムネイル | miniature | miniatura | miniatura | K. "VRChat thumbnail": VRChatのサムネイル |
| facial expression | 表情 | expression faciale | expresión facial | expressão facial | N. Not the VRChat Expressions Menu (keep English outside ja) |
| parameter | パラメーター | paramètre | parámetro | parâmetro | K |
| shader | シェーダー | shader | shader | shader | K. fr/es/pt-BR lowercase, plural +s |
| material | マテリアル | material | material | material | K `!`. Unity word kept in fr (not *matériau*) |
| texture | テクスチャ | texture | textura | textura | K. No trailing ー |
| texture set | テクスチャセット | jeu de textures | conjunto de texturas | conjunto de texturas | K |
| blendshape | ブレンドシェイプ | blendshape | blendshape | blendshape | K. One word, lowercase |
| shape key (Blender) | シェイプキー | shape key | shape key | shape key | K. Blender's own word; only when Blender is meant |
| bone | ボーン | bone | bone | bone | K. Unity/VRChat word kept in fr, es, pt-BR |
| prefab | プレハブ | prefab | prefab | prefab | K. UNI |
| physics (hair, tail, skirt) | 揺れもの | physique | física | física | N `!`. Community word for PhysBones-driven parts. PhysBones itself stays Latin |
| gimmick (avatar feature) | ギミック | fonction spéciale | función especial | recurso especial | K. ja community word; paraphrase elsewhere. Rare |
| preset | プリセット | préréglage | preajuste | predefinição | K |
| clothing / outfit | 衣装 | vêtements (une tenue) | ropa | roupas | N |
| accessory | アクセサリー | accessoire | accesorio | acessório | K |
| supported avatar | 対応アバター | avatar compatible | avatar compatible | avatar compatível | KN. "Avatar compatibility": アバター対応状況 |
| dress-up (swap outfits) | 着せ替え | changement de tenue | cambio de ropa | troca de roupa | N |
| modification (of an existing asset) | 改変 | modification | modificación | modificação | N. 改変可 / 改変不可 = modification allowed / not allowed |
| distribute / distribution | 配布 | distribuer / distribution | distribuir / distribución | distribuir / distribuição | N. Use 配布 for free or general sharing; 頒布 is not used (7) |
| redistribution | 再配布 | redistribution | redistribución | redistribuição | N |
| free (no cost) / paid | 無料 / 有料 | gratuit / payant | gratis / de pago | grátis / pago | N |
| commercial use | 商用利用 | usage commercial | uso comercial | uso comercial | N |
| terms of use | 利用規約 | conditions d’utilisation | términos de uso | termos de uso | N |
| sona (own character) | ソナ | sona | sona | sona | K `!`. Furry/creator jargon; helper text may say "character" |
| Inspector | インスペクター | Inspector | Inspector | Inspector | K. UNI. English outside ja (1.8) |
| Hierarchy | ヒエラルキー | Hierarchy | Hierarchy | Hierarchy | K. UNI |
| Console | コンソール | Console | Console | Console | K. UNI |
| Scene view | シーンビュー | Scene view | Scene view | Scene view | K. UNI |
| Play Mode | プレイモード | Play Mode | Play Mode | Play Mode | K `!`. UNI |
| package | パッケージ | package | paquete | pacote | K. fr *package* (not *paquet*) |
| repository (VPM, Git) | リポジトリ | dépôt | repositorio | repositório | K. "VPM listing" uses the same words: VPMリポジトリ, dépôt VPM, repositorio VPM, repositório VPM |
| commit | コミット | commit | commit | commit | K. Mixer Git. Git words stay English in fr/es/pt-BR |
| branch | ブランチ | branche | rama | branch | K. Mixer Git |
| shelf (Mixer Git) | シェルフ | shelf | shelf | shelf | K. Mixer Git feature word |

### 4.4 Orbiters assets, commissions and money

| English | ja | fr | es | pt-BR | note |
|---|---|---|---|---|---|
| asset (Orbiters catalog entry) | アセット | asset | asset | asset | K. Not *ressource/recurso/ativo* |
| product (store item) | 商品 | produit | producto | produto | N. Provider product, distinct from asset |
| creator | クリエイター | créateur | creador | criador | K |
| creator status | クリエイターステータス | statut de créateur | estado de creador | status de criador | K |
| artist | アーティスト | artiste | artista | artista | K. Not 絵師 |
| client / commissioner | 依頼者 | client | cliente | cliente | N |
| buyer | 購入者 | acheteur | comprador | comprador | N |
| seller | 販売者 | vendeur | vendedor | vendedor | N |
| supporter | サポーター | soutien | mecenas | apoiador | K `!`. es *mecenas* is epicene (Patreon wording); fr *un soutien* |
| supporter tier | サポーターティア | palier de soutien | nivel de apoyo | nível de apoio | K |
| commission | コミッション | commission | comisión | comissão | K `!`. Art/ReFit work. Never use for fees |
| commission request | コミッション依頼 | demande de commission | solicitud de comisión | solicitação de comissão | KN |
| request (noun) | 依頼 (リクエスト for friend/API/invite requests) | demande | solicitud | solicitação | N |
| accept / decline (a request) | 承諾 / 辞退 | accepter / refuser | aceptar / declinar | aceitar / recusar | N |
| approve / reject (staff decision) | 承認 / 却下 | approuver / rejeter | aprobar / rechazar | aprovar / rejeitar | N |
| order (store purchase) | 注文 | commande | pedido | pedido | N |
| purchase | 購入 | achat | compra | compra | N |
| sticker | ステッカー | autocollant | sticker | adesivo | K. es keeps *sticker* (widely used, neutral) |
| sticker printing | ステッカー印刷 | impression d’autocollants | impresión de stickers | impressão de adesivos | KN |
| shipping | 配送 | expédition | envío | frete | N. pt-BR *frete* = shipping cost; "shipping estimate": 送料の目安, estimation des frais de port, estimación de envío, estimativa de frete |
| delivery (messages, events) | 配信 | diffusion | entrega | entrega | N. Physical goods: 配送, livraison, entrega, entrega |
| listing (catalog entry) | 掲載 | fiche | ficha | anúncio | N `!`. es *publicación* is taken by post |
| announcement | 告知 | annonce | anuncio | comunicado | N. In-app notice: お知らせ |
| price | 価格 | prix | precio | preço | N |
| currency | 通貨 | devise | moneda | moeda | N |
| payment | 支払い | paiement | pago | pagamento | N |
| payment method | 支払い方法 | moyen de paiement | método de pago | forma de pagamento | N |
| authorization hold (card) | 与信枠の確保 | blocage de l’autorisation | retención en la tarjeta | pré-autorização | N `!`. For the €2 request fee flow |
| fee | 手数料 | frais | tarifa | taxa | N. es never *comisión* for a fee. "Request fee": 依頼手数料, frais de demande, tarifa de solicitud, taxa de solicitação |
| revenue | 収益 | revenus | ingresos | receita | N |
| sales | 売上 | ventes | ventas | vendas | N |
| payout | 入金 | virement | transferencia | repasse | N `!`. STR |
| refund | 返金 | remboursement | reembolso | reembolso | N |
| discount | 割引 | réduction | descuento | desconto | N |
| tier | ティア | palier | nivel | nível | K |
| membership (Patreon, Ko-fi) | メンバーシップ | abonnement | membresía | assinatura | K |
| store / shop | ストア | boutique | tienda | loja | K. "Store integration": ストア連携, intégration de boutique, integración de tienda, integração de loja |
| license (licence) | ライセンス | licence | licencia | licença | K. Source mixes *license/licence*; same translation |
| license key | ライセンスキー | clé de licence | clave de licencia | chave de licença | K |
| redeem (a key) | 引き換える | activer | canjear | resgatar | N `!` |
| version | バージョン | version | versión | versão | K |
| release | リリース | version | versión | versão | K. A version shipped to an audience |
| release stage | リリース段階 | phase de publication | etapa de lanzamiento | estágio de lançamento | KN. Alpha, Beta stay Latin |
| changelog | 更新履歴 | journal des modifications | registro de cambios | registro de alterações | N |

### 4.5 Community, Discord, accounts and moderation

| English | ja | fr | es | pt-BR | note |
|---|---|---|---|---|---|
| account (Orbiters account, page, menu) | アカウント | compte | cuenta | conta | K |
| user (account) | ユーザー | utilisateur | usuario | usuário | K. Prefer *compte/membre* in fr when it reads naturally |
| member | メンバー | membre | miembro | membro | K. DSC |
| community | コミュニティ | communauté | comunidad | comunidade | K |
| role | ロール | rôle | rol | cargo | K. DSC (pt-BR Discord says *cargo*). VRChat group roles use the same words |
| server (Discord) | サーバー | serveur | servidor | servidor | K. DSC |
| channel (Discord) | チャンネル | salon | canal | canal | K. DSC. The English UI also says "room": translate "room" with the channel word |
| bot | ボット | bot | bot | bot | K |
| moderator | モデレーター | modérateur | moderador | moderador | K |
| administrator / admin | 管理者 | administrateur | administrador | administrador | N. Short UI: *admin* in fr/es/pt-BR |
| owner | オーナー | propriétaire | propietario | proprietário | K |
| staff | スタッフ | équipe | staff | equipe | K. "All staff": スタッフ全員, Toute l’équipe, Todo el staff, Toda a equipe |
| event | イベント | événement | evento | evento | K |
| event planning | 企画 | planification | planificación | planejamento | N |
| availability | 参加可能時間 | disponibilités | disponibilidad | disponibilidade | N |
| poll | 投票 | sondage | encuesta | enquete | N. Same word as vote in ja |
| vote | 投票 | vote | voto | voto | N. Verb: 投票する, voter, votar, votar |
| host (of an event) | ホスト | hôte | anfitrión | anfitrião | K |
| linked account | 連携アカウント | compte lié | cuenta vinculada | conta vinculada | KN |
| connection (Connections tab) | 連携 | connexion | conexión | conexão | N |
| integration | 連携 | intégration | integración | integração | N. Same ja word as connection |
| verification | 認証 | vérification | verificación | verificação | N |
| age verification | 年齢確認 | vérification de l’âge | verificación de edad | verificação de idade | N |
| two-factor authentication | 二要素認証 | double authentification | autenticación en dos pasos | autenticação em dois fatores | N |
| appeal | 異議申し立て | contestation | apelación | recurso | N |
| moderation | モデレーション | modération | moderación | moderação | K |
| report (document) | レポート | rapport | informe | relatório | K. Not the abuse report (4.1) |
| review (customer opinion) | レビュー | avis | reseña | avaliação | K |
| review (staff check, verb) | 審査 | examiner | revisar | analisar | N |
| permission | 権限 | permission | permiso | permissão | N |
| authorize / allow / deny (OAuth) | 許可 / 許可 / 拒否 | autoriser / autoriser / refuser | autorizar / permitir / denegar | autorizar / permitir / negar | N |
| privacy & data | プライバシーとデータ | Confidentialité et données | Privacidad y datos | Privacidade e dados | K |
| storage choices (cookies page) | ストレージの設定 | choix de stockage | opciones de almacenamiento | opções de armazenamento | N. CJ (footer) |

### 4.6 Content, boards, AI and developer

| English | ja | fr | es | pt-BR | note |
|---|---|---|---|---|---|
| gallery | ギャラリー | galerie | galería | galeria | K |
| showcase | ショーケース | vitrine | vitrina | vitrine | K |
| wall (profile wall) | ウォール | mur | muro | mural | K |
| banner | バナー | bannière | banner | banner | K |
| post | 投稿 | publication | publicación | postagem | N |
| blog | ブログ | blog | blog | blog | K |
| magazine | マガジン | magazine | revista | revista | K |
| live stream | ライブ配信 | direct | transmisión en vivo | transmissão ao vivo | N |
| message | メッセージ | message | mensaje | mensagem | K. DM stays Latin |
| file / image | ファイル / 画像 | fichier / image | archivo / imagen | arquivo / imagem | K / N |
| tag | タグ | tag | etiqueta | tag | K |
| favorite | お気に入り | favori | favorito | favorito | N |
| follow / follower | フォロー / フォロワー | suivre / abonné | seguir / seguidor | seguir / seguidor | K |
| idea box (nav) | アイデアボックス | boîte à idées | buzón de ideas | caixa de ideias | K |
| idea | アイデア | idée | idea | ideia | K |
| board | ボード | tableau | tablero | quadro | K. Trello wording |
| column | 列 | colonne | columna | coluna | N |
| card | カード | carte | tarjeta | cartão | K. Trello wording |
| proposal | 提案 | proposition | propuesta | proposta | N |
| issue (GitHub) | Issue | issue | issue | issue | L. GitHub's word, kept in all four |
| research | リサーチ | recherche | investigación | pesquisa | K |
| knowledge base | ナレッジベース | base de connaissances | base de conocimientos | base de conhecimento | K |
| documentation | ドキュメント | documentation | documentación | documentação | K |
| AI | AI | IA | IA | IA | L. ja writes `AI` |
| prompt | プロンプト | prompt | prompt | prompt | K |
| model (AI) | モデル | modèle | modelo | modelo | K |
| agent | エージェント | agent | agente | agente | K |
| API key | APIキー | clé d’API | clave de API | chave de API | KN |
| token | トークン | jeton | token | token | K |
| credentials | 認証情報 | identifiants | credenciales | credenciais | N |
| webhook | Webhook | webhook | webhook | webhook | L |
| callback URL | コールバックURL | URL de callback | URL de callback | URL de callback | KN |
| scope (OAuth) | スコープ | portée | alcance | escopo | K |
| sync | 同期 | synchroniser | sincronizar | sincronizar | N |
| import / export | インポート / エクスポート | importer / exporter | importar / exportar | importar / exportar | K |

### 4.7 Navigation and system

| English | ja | fr | es | pt-BR | note |
|---|---|---|---|---|---|
| homepage | ホーム | accueil | inicio | início | K |
| Tools (nav) | ツール | Outils | Herramientas | Ferramentas | K |
| Assets (nav) | アセット | Assets | Assets | Assets | K |
| Creator (nav) | クリエイター | Créateur | Creador | Criador | K |
| Documentation (nav) | ドキュメント | Documentation | Documentación | Documentação | K |
| dashboard | ダッシュボード | tableau de bord | panel | painel | K |
| workspace | ワークスペース | espace de travail | espacio de trabajo | espaço de trabalho | K |
| overview | 概要 | aperçu | resumen | visão geral | N |
| profile / public profile | プロフィール / 公開プロフィール | profil / profil public | perfil / perfil público | perfil / perfil público | K |
| settings | 設定 | paramètres | configuración | configurações | N. es never *ajustes* |
| preferences | 設定 | préférences | preferencias | preferências | N |
| notifications | 通知 | notifications | notificaciones | notificações | N |
| language | 言語 | langue | idioma | idioma | N. CJ |
| theme (dark / light) | テーマ (ダーク / ライト) | thème (sombre / clair) | tema (oscuro / claro) | tema (escuro / claro) | K |
| timezone | タイムゾーン | fuseau horaire | zona horaria | fuso horário | K |
| name / title / description | 名前 / タイトル / 説明 | nom / titre / description | nombre / título / descripción | nome / título / descrição | N / K / N |
| history | 履歴 | historique | historial | histórico | N |
| activity | アクティビティ | activité | actividad | atividade | K |
| newest first / oldest first | 新しい順 / 古い順 | Plus récents d’abord / Plus anciens d’abord | Más recientes primero / Más antiguos primero | Mais recentes primeiro / Mais antigos primeiro | N |
| today / earlier | 今日 / それ以前 | Aujourd’hui / Plus tôt | Hoy / Anteriores | Hoje / Anteriores | N |
| support / contact | サポート / お問い合わせ | assistance / contact | soporte / contacto | suporte / contato | K / N |
| feedback | フィードバック | retours | comentarios | feedback | K |

### 4.8 Disambiguation rules

1. **Remove vs Delete.** Delete destroys; Remove takes something out of a list while it survives elsewhere. Keep them distinct in fr/es/pt-BR.
2. **Connect vs Link vs Sign in.** *Connect* = external service (連携する, connecter, conectar). *Link* between two Orbiters records or an account = 関連付ける / 連携, lier or associer, vincular. *Sign in* = authenticate (ログイン, se connecter, iniciar sesión, entrar). fr keeps the reflexive: *Se connecter / Se déconnecter* (user) versus *Connecter / Déconnecter* (service); *Associer / Dissocier* for accounts.
3. **Report.** Abuse report (verb or button) = 報告, signaler, reportar, denunciar; noun in es is *reporte*. Report document = レポート, rapport, informe, relatório. "Report a problem" (bug) = 問題を報告, signaler un problème, reportar un problema, relatar um problema.
4. **Review.** Customer opinion = レビュー, avis, reseña, avaliação. Staff check = 審査, examiner, revisar, analisar.
5. **Request vs Order vs Commission.** Request = a question asked to a creator or system. Order = a store purchase. Commission = the paid art or ReFit work itself. pt-BR *pedido* means only order, es *comisión* means only commission (use *tarifa* for fees), fr *commission* never means a fee (use *frais*).
6. **Release vs Version.** Version is a numbered build; release is the same build as shipped to an audience (Alpha, Beta, Public).
7. **Private vs Not published.** Private (非公開, privé) is a visibility choice; Not published (未公開, non publié) means it was never published.
8. **Delivery vs Shipping.** Delivery = sending a message or event to Discord/VRChat or giving a file; Shipping = physical goods.
9. **Announcement.** ja 告知 for a promotional post to Discord/VRChat; お知らせ for an in-app notice.
10. **Quest.** Headset/platform name, untranslated (section 3).
11. **Shared words by design (do not "fix").** ja 更新 (Refresh/Update), 投票 (poll/vote), 連携 (connection/integration); es Actualizar and pt-BR Atualizar (Refresh/Update); es Descartar (Discard/Dismiss); fr, es, pt-BR *version/versión/versão* (version/release); pt-BR *Sair* (Log out/Leave).

### 4.9 Corrections requested to the existing `locales/*/common.json`

| File | Key | Now | Should be |
|---|---|---|---|
| `es/common.json` | `actions.add` | Añadir | Agregar (Latin-American) |
| `ja/common.json` | `actions.showLess` | 閉じる | 折りたたむ (not identical to Close) |

Everything else in `common.json` and `footer.json` is adopted as is. (One open item: ja `actions.report` 報告 versus 通報, section 7.)

---

## 5. Phrasebook (approved renderings)

Rules: reuse these shapes for similar strings. `{{x}}` values are placeholders; verbs are the glossary verbs. fr rows contain real NBSP before `? ! : ;` and inside « ».

### 5.1 Patterns

| English | ja | fr | es | pt-BR |
|---|---|---|---|---|
| Save / Save changes | 保存 / 変更を保存 | Enregistrer / Enregistrer les modifications | Guardar / Guardar cambios | Salvar / Salvar alterações |
| Cancel | キャンセル | Annuler | Cancelar | Cancelar |
| Delete | 削除 | Supprimer | Eliminar | Excluir |
| Are you sure? | よろしいですか？ | Confirmer cette action ? | ¿Quieres continuar? | Tem certeza? |
| This cannot be undone. | この操作は元に戻せません。 | Cette action est irréversible. | Esta acción no se puede deshacer. | Essa ação não pode ser desfeita. |
| Delete "{{name}}"? | 「{{name}}」を削除しますか？ | Supprimer « {{name}} » ? | ¿Eliminar “{{name}}”? | Excluir “{{name}}”? |
| Loading… | 読み込み中… | Chargement… | Cargando… | Carregando… |
| Loading assets… | アセットを読み込み中… | Chargement des assets… | Cargando assets… | Carregando assets… |
| No results | 該当なし | Aucun résultat | Sin resultados | Nenhum resultado |
| No results for "{{query}}" | 「{{query}}」に一致する結果はありません | Aucun résultat pour « {{query}} » | Sin resultados para “{{query}}” | Nenhum resultado para “{{query}}” |
| Try again | もう一度試す | Réessayer | Reintentar | Tentar de novo |
| Something went wrong. Please try again. | 問題が発生しました。もう一度お試しください。 | Une erreur s’est produite. Réessaie. | Algo salió mal. Inténtalo de nuevo. | Algo deu errado. Tente de novo. |
| Could not load assets. Please try again. | アセットを読み込めませんでした。もう一度お試しください。 | Impossible de charger les assets. Réessaie. | No se pudieron cargar los assets. Inténtalo de nuevo. | Não foi possível carregar os assets. Tente de novo. |
| Could not save your changes. | 変更を保存できませんでした。 | Impossible d’enregistrer tes modifications. | No se pudieron guardar tus cambios. | Não foi possível salvar suas alterações. |
| Could not copy the link (toast title) | コピーできませんでした | Impossible de copier le lien | No se pudo copiar el enlace | Não foi possível copiar o link |
| Sync failed (noun phrase) | 同期に失敗しました | Échec de la synchronisation | Error al sincronizar | Falha ao sincronizar |
| Signed in as {{name}} | {{name}}としてログイン中 | Session ouverte en tant que {{name}} | Sesión iniciada como {{name}} | Sessão iniciada como {{name}} |
| Sign in to continue. | 続けるにはログインしてください。 | Connecte-toi pour continuer. | Inicia sesión para continuar. | Entre para continuar. |
| You don't have permission to do this. | この操作を行う権限がありません。 | Tu n’as pas la permission de faire ça. | No tienes permiso para hacer esto. | Você não tem permissão para fazer isso. |
| {{count}} items (plural) | {{count}}件 | {{count}} élément / d’éléments / éléments | {{count}} elemento / de elementos / elementos | {{count}} item / de itens / itens |
| Updated {{time}} (time from code, e.g. "3 days ago") | {{time}}に更新 | Mis à jour {{time}} | Actualizado {{time}} | Atualizado {{time}} |
| Created by {{name}} | 作成者：{{name}} | Créé par {{name}} | Creado por {{name}} | Criado por {{name}} |
| Page {{page}} of {{total}} | {{total}}ページ中{{page}}ページ | Page {{page}} sur {{total}} | Página {{page}} de {{total}} | Página {{page}} de {{total}} |
| No notifications yet | まだ通知はありません | Aucune notification pour l’instant | Aún no hay notificaciones | Ainda não há notificações |
| No members yet | まだメンバーはいません | Aucun membre pour l’instant | Aún no hay miembros | Ainda não há membros |
| Nothing here yet | まだ何もありません | Rien ici pour l’instant | Aún no hay nada aquí | Ainda não há nada aqui |
| Search assets… (placeholder) | アセットを検索… | Rechercher des assets… | Buscar assets… | Buscar assets… |
| This field is required | この項目は必須です | Ce champ est obligatoire | Este campo es obligatorio | Este campo é obrigatório |
| Enter a valid URL | 有効なURLを入力してください | Saisis une URL valide | Escribe una URL válida | Digite uma URL válida |
| At most {{max}} characters | {{max}}文字以内で入力してください | {{max}} caractères maximum | Máximo {{max}} caracteres | Máximo de {{max}} caracteres |
| Already exists | すでに存在します | Existe déjà | Ya existe | Já existe |
| Link copied | リンクをコピーしました | Lien copié | Enlace copiado | Link copiado |
| Changes saved | 変更を保存しました | Modifications enregistrées | Cambios guardados | Alterações salvas |
| You have unsaved changes. Leave without saving? | 保存されていない変更があります。保存せずに移動しますか？ | Tu as des modifications non enregistrées. Quitter sans enregistrer ? | Tienes cambios sin guardar. ¿Salir sin guardar? | Você tem alterações não salvas. Sair sem salvar? |
| Connect Discord | Discordと連携する | Connecter Discord | Conectar Discord | Conectar o Discord |
| Disconnect this account? | このアカウントの連携を解除しますか？ | Déconnecter ce compte ? | ¿Desconectar esta cuenta? | Desconectar esta conta? |
| Open in VRChat / Join in VRChat | VRChatで開く / VRChatで参加 | Ouvrir dans VRChat / Rejoindre dans VRChat | Abrir en VRChat / Unirse en VRChat | Abrir no VRChat / Participar no VRChat |
| Invite sent. See you in VRChat! | 招待を送信しました。VRChatで会いましょう！ | Invitation envoyée. À bientôt dans VRChat ! | Invitación enviada. ¡Nos vemos en VRChat! | Convite enviado. Nos vemos no VRChat! |
| Notifications, {{count}} unread (aria-label) | 通知、未読{{count}}件 | Notifications, {{count}} non lues | Notificaciones, {{count}} sin leer | Notificações, {{count}} não lidas |

### 5.2 Fixed backend vocabularies

Notification categories (settings screen) and audiences (broadcast form). Keep these exact words everywhere they appear.

| English | ja | fr | es | pt-BR |
|---|---|---|---|---|
| Promotional updates | プロモーション情報 | Offres promotionnelles | Promociones | Promoções |
| Comments and mentions | コメントとメンション | Commentaires et mentions | Comentarios y menciones | Comentários e menções |
| Payments and commissions | 支払いとコミッション | Paiements et commissions | Pagos y comisiones | Pagamentos e comissões |
| Boards | ボード | Tableaux | Tableros | Quadros |
| Moderation | モデレーション | Modération | Moderación | Moderação |
| Creators and assets | クリエイターとアセット | Créateurs et assets | Creadores y assets | Criadores e assets |
| People you follow | フォロー中の人 | Personnes que tu suis | Personas que sigues | Pessoas que você segue |
| Orbiters updates | Orbitersからのお知らせ | Actualités d’Orbiters | Novedades de Orbiters | Novidades do Orbiters |
| Everyone / Creators | 全員 / クリエイター | Tout le monde / Créateurs | Todos / Creadores | Todos / Criadores |
| All staff / Moderators | スタッフ全員 / モデレーター | Toute l’équipe / Modérateurs | Todo el staff / Moderadores | Toda a equipe / Moderadores |
| Developers / Administrators / Owners | 開発者 / 管理者 / オーナー | Développeurs / Administrateurs / Propriétaires | Desarrolladores / Administradores / Propietarios | Desenvolvedores / Administradores / Proprietários |
| Legal footer: Privacy / Terms & operator / Content & reports / How results are ordered / Storage choices / AI use | プライバシー / 利用規約・運営者情報 / コンテンツと報告 / 表示順のしくみ / ストレージの設定 / AIの利用 | Confidentialité / Conditions et exploitant / Contenus et signalements / Ordre des résultats / Choix de stockage / Usage de l’IA | Privacidad / Términos y operador / Contenido y reportes / Cómo se ordenan los resultados / Opciones de almacenamiento / Uso de IA | Privacidade / Termos e operador / Conteúdo e denúncias / Como os resultados são ordenados / Opções de armazenamento / Uso de IA |

Emails and notifications: subject lines keep the source's " - Orbiters" suffix. Notification titles are capped at 180 characters and bodies at 4000: keep titles short (no more than English +30%). Same tone rules as the UI (tu, tú, você, です/ます).

---

## 6. QA checklist (translators and reviewers)

Run before handing off. Items 1-4 are mechanical (`node scripts/i18n-check.cjs --namespace <ns>` covers 1, 3 and 4 partly).

1. **Placeholder parity.** Every `{{…}}` in the English appears unchanged and the same number of times; `{{count}}` kept in every plural form. No new placeholders.
2. **Tag and nesting parity.** `<0>…</0>`, `<1/>`, named tags (`<link>`), `$t(…)`, Markdown links: same set, same nesting, URLs untouched. The checker ignores named tags and `$t()`: check by eye.
3. **Plural completeness.** ja: `_other` only. fr, es, pt-BR: `_one`, `_many`, `_other` for every plural key. `_many` contains the *de/d’* form; fr and pt-BR zero reads correctly as singular; es zero reads correctly as plural. Add `_zero` only if English has it.
4. **No empties, no extras.** Same keys as English, valid JSON, UTF-8, LF, 2 spaces.
5. **Glossary and DNT.** Every term from sections 3 and 4 used exactly as listed; brand names untouched and not inflected; no katakana for Latin brands; prepositions before platform names as in section 3.
6. **Register.** fr only tu; es only tú (no vos/usted/vosotros); pt-BR only você (no tu); ja です/ます without heavy keigo. Sentence case in fr/es/pt-BR (no Title Case). Gendered participles and `@/x/e/(a)` forms avoided.
7. **Action rule.** Buttons and tabs follow 1.3 (ja noun, fr/es/pt-BR infinitive); instructions follow the imperative rule.
8. **Punctuation.** `…` not `...`; terminal punctuation mirrors the English; ja full-width 。、？！「」 and no spaces between Japanese and Latin/digits; fr NBSP before `? ! : ;` and inside « »; es `¿ ¡` pairs; curly quotes; no `—`; no HTML entities.
9. **Length budget.** fr, es and pt-BR run 20-35% longer than English; ja is usually shorter in width but taller in lines. For buttons, tabs, segmented switches, chips, badges, table headers and icon-button tooltips (English up to about 16 characters) keep the translation within **English +30%** (hard cap +40%) by choosing the shortest accepted glossary term and dropping articles where the grammar allows (*Voir plus*, not *Afficher davantage*). Do not invent shorter synonyms outside the glossary (*Paramètres*, not *Réglages*) and never abbreviate with a period. Ellipsis-truncated labels must still be understandable at 60% length. Longer prose may expand freely but not by adding content. Check in the pseudo-locale (`/xx/…`, 35% longer) for clipping.
10. **Context fit.** Read the English key path and neighbors: `aria-label`, `placeholder`, `title`, `alt` and toast strings follow the same rules and take no ending period unless they are sentences. Do not translate IDs, enum values or API strings.
11. **Proper nouns inside strings.** No translated brand, product, avatar or example names; no transliteration into katakana.
12. **Meaning check.** Back-translate any string with a placeholder, negation or legal/financial wording (fees, refunds, age verification, privacy); never soften or drop a warning.
13. **Consistency sweep.** Search your file for each of the ten collision rules in 4.8 and confirm each English word kept its own translation.
14. **Hand-off notes.** List `TERM?` entries, untranslatable fragments, and every `!` item you had to decide yourself.

---

## 7. Open questions for native review

**ja**
- 配布 vs 頒布 (this guide: 配布 only); コミッション vs 依頼 for commissions; ベースアバター vs 素体 for "avatar base"; 揺れもの for PhysBones parts; ソナ for "sona"; 引き換える for redeem; 与信枠の確保 for card holds; 報告 vs 通報 for abuse reports; instance-type names (フレンド+ etc.) and performance-rank names (エクセレント…) in the Japanese VRChat client; プレイモード vs 再生モード; whether to show Unity's Japanese editor names next to English menu labels in tutorials.
**fr**
- *commission* (anglicism used by creators) vs *commande* (clashes with store orders); *asset*; *téléverser* vs *importer*; *issue*; *soutien* for supporters; Unity/3D loanwords (*material*, *bone*, *shader*) vs *matériau, os*; whether to adopt point médian in headings (this guide: no); bare *Créateurs* in compact lists.
**es**
- VRChat's community wiki writes *estancia* for instance; this guide uses *instancia* (the word creators say). *Mecenas* for supporters; *Configuración* vs *Ajustes*; *sticker* vs *calcomanía*; *Reportar* vs *Denunciar* (this guide: Reportar); *asset* vs *recurso*.
**pt-BR**
- *nível de confiança* for trust rank; *cargo* for roles (Discord) also for VRChat group roles; *Enviar* vs *Fazer upload*; *postagem* vs *publicação*; *anúncio* for listing; *adesivo* vs *sticker*; *Tentar de novo* vs *novamente*.
**All**
- Rank names (Visitor, New User, User, Known User, Trusted User) kept English; check whether the VRChat client localizes them.
- VRChat Help Center and the in-game strings could not be read (HTTP 403), so alignment with VRChat's official client localization is only verified through the VRChat community wiki translations; Discord, Stripe and Unity alignment is from memory.
- The mixed UK/US spelling in the English source (*licence/license*, *catalogue/catalog*, *Cancelled/Canceled*) should be normalized before extraction; translations treat them as identical.

---

## 8. Sources and verification

- Code base: `frontend/src` (navbar, pages, creator, community, commissions, boards, notifications), `frontend/src/metadata/routeDescriptions.json`, `Documentation/docs` (start-here, how-to, reference, tools), `backend/src/services/notificationService.js` (categories and audiences), `AGENTS.md`, and the existing `frontend/src/i18n` files (`README.md`, `locales/*/common.json`, `footer.json`, `scripts/i18n-check.cjs`).
- Plural rules verified with `Intl.PluralRules` (Node 22; i18next 26.4.4, react-i18next 17.0.16 in `frontend/node_modules`): ja `other`; fr/es/pt-BR `one, many, other`; fr 0 and 1.5 are *one*; es 0 is *other*; pt-BR 0 and 1.5 are *one*; `many` only at 1,000,000 multiples.
- VRChat terms checked on the community wiki translations (wiki.vrchat.com): `Instances/fr|es|pt-br` and `Trust_Rank/ja|fr|es|pt-br` (read 2026-10-13). help.vrchat.com and support.discord.com returned HTTP 403; those alignments are from knowledge and marked `!` where doubtful.
