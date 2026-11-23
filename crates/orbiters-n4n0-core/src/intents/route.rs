//! Which of the app's tools a turn carries (r23). Every tool definition counts against the provider's tokens-per-minute
//! limit each request (Groq's 8,000 a minute for gpt-oss-120b: one request with all 24 tools was 6,551 prompt tokens),
//! so a turn carries only the groups its words name, found here by a keyword pass in the five languages (no model call),
//! the groups of the turn before (for "the second one", "louder", "undo that"), and the always-sent `orbiters_more`,
//! with which the model opens any other group when the pass missed it (one more round, few tokens).
//! Nothing here decides what runs: a call still goes through `intents::gate` whatever tools were offered.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A group of the app's client tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolGroup {
    Avatar,
    Music,
    Pictures,
    Videos,
    Walk,
    Worlds,
    Instances,
    Friends,
    Profile,
    Calls,
    Translation,
    Companion,
    Pages,
    State,
}

impl ToolGroup {
    pub const ALL: [ToolGroup; 14] = [
        ToolGroup::Avatar,
        ToolGroup::Music,
        ToolGroup::Pictures,
        ToolGroup::Videos,
        ToolGroup::Walk,
        ToolGroup::Worlds,
        ToolGroup::Instances,
        ToolGroup::Friends,
        ToolGroup::Profile,
        ToolGroup::Calls,
        ToolGroup::Translation,
        ToolGroup::Companion,
        ToolGroup::Pages,
        ToolGroup::State,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ToolGroup::Avatar => "avatar",
            ToolGroup::Music => "music",
            ToolGroup::Pictures => "pictures",
            ToolGroup::Videos => "videos",
            ToolGroup::Walk => "walk",
            ToolGroup::Worlds => "worlds",
            ToolGroup::Instances => "instances",
            ToolGroup::Friends => "friends",
            ToolGroup::Profile => "profile",
            ToolGroup::Calls => "calls",
            ToolGroup::Translation => "translation",
            ToolGroup::Companion => "n4n0",
            ToolGroup::Pages => "pages",
            ToolGroup::State => "state",
        }
    }

    pub fn of_key(key: &str) -> Option<ToolGroup> {
        ToolGroup::ALL.into_iter().find(|g| g.key() == key)
    }

    /// What it is for, for `orbiters_more` (the model reads it; English).
    pub fn about(self) -> &'static str {
        match self {
            ToolGroup::Avatar => "the avatar's toggles, outfits, poses and parameters", // i18n: not shown
            ToolGroup::Music => "the computer's music and volume", // i18n: not shown
            ToolGroup::Pictures => "pictures, 3D pictures, recordings, QR codes, sharing captures", // i18n: not shown
            ToolGroup::Videos => "YouTube, the worlds' videos, the clipboard", // i18n: not shown
            ToolGroup::Walk => "Fast walk", // i18n: not shown
            ToolGroup::Worlds => "worlds: where the owner is, search, pages, likes, labels, comments, going there", // i18n: not shown
            ToolGroup::Instances => "previous instances, invites to them, Home's places and Play", // i18n: not shown
            ToolGroup::Friends => "VRChat friends: who is online, boops, invite requests, invites", // i18n: not shown
            ToolGroup::Profile => "the owner's VRChat status, status line and bio", // i18n: not shown
            ToolGroup::Calls => "voice calls with friends", // i18n: not shown
            ToolGroup::Translation => "live translation", // i18n: not shown
            ToolGroup::Companion => "N4N0 itself: shown or hidden, its light, menu, states, smoothing, AFK message, suggestions", // i18n: not shown
            ToolGroup::Pages => "opening the app's pages, a few settings, what is not done by voice", // i18n: not shown
            ToolGroup::State => "questions about how things are now, and undo", // i18n: not shown
        }
    }
}

/// The group of a client tool by its name; `None` for `orbiters_more`.
pub fn group_of(tool: &str) -> Option<ToolGroup> {
    Some(match tool {
        "set_avatar_control" | "find_avatar_controls" | "hold_avatar_control" | "set_avatar_parameter" => ToolGroup::Avatar,
        "control_media" => ToolGroup::Music,
        "take_photo" | "orbiters_pictures" => ToolGroup::Pictures,
        "save_video" | "list_saved_videos" | "copy_to_clipboard" | "orbiters_videos" => ToolGroup::Videos,
        "orbiters_walk" => ToolGroup::Walk,
        "orbiters_worlds" => ToolGroup::Worlds,
        "orbiters_instances" => ToolGroup::Instances,
        "orbiters_friends" => ToolGroup::Friends,
        "orbiters_profile" => ToolGroup::Profile,
        "orbiters_calls" => ToolGroup::Calls,
        "orbiters_translation" => ToolGroup::Translation,
        "set_afk_message" | "n4n0_light" | "n4n0_mood" | "orbiters_n4n0" => ToolGroup::Companion,
        "orbiters_pages" => ToolGroup::Pages,
        "orbiters_state" => ToolGroup::State,
        _ => return None,
    })
}

/// Words that name each group's topic, in English, French, Spanish, Portuguese and Japanese (matched whole after the
/// offline path's folding: lower case, no accents; Japanese by containment).
fn topics(group: ToolGroup) -> &'static [&'static str] {
    match group {
        ToolGroup::Avatar => &[
            "avatar", "outfit", "clothes", "hoodie", "jacket", "shirt", "glasses", "ears", "tail", "hat", "toggle", "wear", "put on", "put my", "take off", "pose", "emote", "tenue", "vetement", "vetements", "capuche",
            "chapeau", "enleve", "porte", "mets mon", "mets ma", "mets mes", "mets le", "mets la", "mets les", "enfile", "sweat", "lunettes", "oreilles", "queue", "ropa", "sudadera", "sombrero", "quitate", "ponte", "gafas", "orejas", "camiseta", "roupa", "oculos", "orelhas", "moletom", "chapeu", "tira", "veste", "アバター", "服", "帽子", "衣装", "パーカー", "着て", "脱いで",
        ],
        ToolGroup::Music => &[
            "music", "song", "track", "volume", "louder", "quieter", "pause", "play", "skip", "spotify", "mute", "musique", "chanson", "morceau", "plus fort",
            "musica", "cancion", "volumen", "alto", "bajo", "som", "faixa", "音楽", "曲", "音量", "ボリューム", "再生", "一時停止",
        ],
        ToolGroup::Pictures => &[
            "picture", "photo", "photos", "pictures", "camera", "3d", "capture", "captures", "record", "recording", "qr", "selfie", "screenshot", "share", "photographie",
            "enregistre", "enregistrement", "partage", "foto", "fotos", "camara", "grabacion", "graba", "comparte", "gravacao", "compartilha", "写真", "カメラ", "録画", "共有",
        ],
        ToolGroup::Videos => &[
            "youtube", "video", "videos", "clip", "link", "clipboard", "lien", "copie", "enlace", "copia", "copiar", "vinculo", "動画", "リンク", "ユーチューブ",
        ],
        ToolGroup::Walk => &["fast walk", "walk", "boost", "walking", "marche", "pas rapide", "caminata", "paso", "caminar", "andar", "passo", "早歩き", "歩き", "移動"],
        ToolGroup::Worlds => &[
            "world", "worlds", "map", "where am i", "label", "like", "comment", "monde", "mondes", "ou suis je", "aime", "commentaire", "mundo", "mundos", "donde estoy",
            "comentario", "onde estou", "curte", "ワールド", "どこ", "いいね", "コメント",
        ],
        ToolGroup::Instances => &[
            "instance", "instances", "invite me", "rejoin", "go back", "go to", "home", "play", "start vrchat", "launch", "join", "rejoindre", "retourne", "lance",
            "joue", "instancia", "instancias", "vuelve", "inicia", "juega", "volta", "jogar", "iniciar", "インスタンス", "起動", "戻",
        ],
        ToolGroup::Friends => &[
            "friend", "friends", "online", "boop", "who is", "invite", "profile", "ami", "amis", "en ligne", "invitation", "profil", "amigo", "amigos", "conectado",
            "invitacion", "perfil", "convite", "フレンド", "友達", "オンライン", "ブープ", "招待",
        ],
        ToolGroup::Profile => &[
            "status", "orange", "red", "green", "blue", "busy", "ask me", "join me", "bio", "statut", "rouge", "vert", "bleu", "occupe", "ocupado", "estado", "naranja",
            "rojo", "verde", "azul", "laranja", "vermelho", "ステータス", "オレンジ", "取り込み中", "自己紹介",
        ],
        ToolGroup::Calls => &["call", "hang up", "answer", "appel", "appelle", "raccroche", "llama", "llamada", "cuelga", "liga", "ligacao", "chamada", "desliga", "電話", "通話"],
        ToolGroup::Translation => &["translation", "translate", "subtitles", "traduction", "traduis", "traduccion", "traduce", "traducao", "traduz", "legendas", "翻訳", "字幕"],
        ToolGroup::Companion => &[
            "n4n0", "nano", "light", "lamp", "afk", "away", "smooth", "smoothing", "tracking", "suggest", "lumiere", "lampe", "lissage", "suivi", "luz", "suavizado",
            "seguimiento", "rastreamento", "ausente", "ライト", "ナノ", "トラッキング", "提案",
        ],
        ToolGroup::Pages => &[
            "open", "show me", "page", "settings", "menu", "theme", "language", "sign out", "delete", "ouvre", "affiche", "parametres", "langue", "abre", "muestra",
            "configuracion", "idioma", "mostra", "configuracoes", "開いて", "設定", "言語", "ページ",
        ],
        ToolGroup::State => &[
            "undo", "is it", "how many", "how much", "what", "points", "annule", "combien", "quel", "quelle", "deshaz", "cuantos", "cuanto", "desfaz", "quantos",
            "qual", "元に戻", "いくつ", "何", "ポイント",
        ],
    }
}

/// Folds `text` as the offline path matches it: lower case, accents off, words separated by single spaces.
fn folded(text: &str) -> String {
    let lowered: String = text
        .chars()
        .map(|c| match orbiters_contracts::n4n0::unaccented(c) {
            Some(base) => base.to_lowercase(),
            None => c.to_lowercase().collect(),
        })
        .collect();
    let words: Vec<&str> = lowered.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    format!(" {} ", words.join(" "))
}

fn japanese(word: &str) -> bool {
    word.chars().any(|c| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}'))
}

/// The groups `text` names. `controls`: the avatar's control labels (a sentence naming one is about the avatar).
pub fn route(text: &str, controls: &[String]) -> BTreeSet<ToolGroup> {
    let words = folded(text);
    let raw = text.to_lowercase();
    let has = |word: &str| if japanese(word) { raw.contains(word) } else { words.contains(&format!(" {word} ")) || words.contains(&format!(" {word}s ")) };
    let mut out: BTreeSet<ToolGroup> = ToolGroup::ALL.into_iter().filter(|g| topics(*g).iter().any(|w| has(w))).collect();
    // A question asks how things are.
    if text.contains('?') || text.contains('？') {
        out.insert(ToolGroup::State);
    }
    // A control of the avatar named by its label ("put my Hoodie on").
    if controls.iter().filter(|c| c.chars().count() >= 3).any(|c| {
        let label = folded(c);
        let label = label.trim();
        !label.is_empty() && words.contains(&format!(" {label} "))
    }) {
        out.insert(ToolGroup::Avatar);
    }
    out
}
