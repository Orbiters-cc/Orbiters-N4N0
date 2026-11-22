//! The sentences the offline path knows, in the five languages, as [`super::quick`] normalizes them (lower case, no
//! accents, no punctuation, the fillers it drops left out; Japanese without spaces). Matched whole, never in part: a
//! sentence that is not exactly one of these goes to the model. Nobody reads this: it is what is heard.

/// A switchable feature the offline path knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Feature {
    FastWalk,
    Translation,
    Light,
    N4n0,
    Smoothing,
    Assistant,
    CameraQr,
}

/// Each feature's names, per language (en, fr, es, pt, ja).
pub(super) fn names(feature: Feature) -> [&'static [&'static str]; 5] {
    match feature {
        Feature::FastWalk => [
            &["fast walk", "fastwalk", "walk boost", "speed walk", "fast walking"],
            &["pas rapide", "marche rapide", "boost marche", "boost de marche"],
            &["paso rapido", "caminata rapida", "andar rapido", "caminar rapido"],
            &["andar rapido", "passo rapido", "caminhada rapida"],
            &["早歩き", "高速移動", "ファストウォーク"],
        ],
        Feature::Translation => [
            &["translation", "translations", "live translation", "translator", "subtitles"],
            &["traduction", "traductions", "traduction en direct", "traducteur", "sous titres"],
            &["traduccion", "traducciones", "traductor", "subtitulos"],
            &["traducao", "traducoes", "tradutor", "legendas"],
            &["翻訳", "通訳", "字幕"],
        ],
        Feature::Light => [
            &["light", "lamp", "flashlight", "n4n0 light", "nano light"],
            &["lumiere", "lampe", "lampe torche"],
            &["luz", "lampara", "linterna"],
            &["luz", "lanterna", "lampada"],
            &["ライト", "明かり", "電気"],
        ],
        Feature::N4n0 => [&["n4n0", "nano", "bot", "robot"], &["n4n0", "nano", "robot"], &["n4n0", "nano", "robot"], &["n4n0", "nano", "robo"], &["n4n0", "ナノ", "ロボット"]],
        Feature::Smoothing => [
            &["smooth tracking", "tracking smoothing", "smoothing"],
            &["suivi lisse", "lissage", "lissage suivi", "suivi lissé"],
            &["seguimiento suave", "suavizado", "suavizado seguimiento"],
            &["rastreamento suave", "suavizacao", "suavizacao rastreamento"],
            &["トラッキング補正", "スムージング"],
        ],
        Feature::Assistant => [&["assistant", "voice assistant"], &["assistant", "assistant vocal"], &["asistente", "asistente voz"], &["assistente", "assistente voz"], &["アシスタント"]],
        Feature::CameraQr => [&["qr code scanner", "qr scanner", "qr code", "qr"], &["scanner qr", "code qr", "qr"], &["escaner qr", "codigo qr", "qr"], &["scanner qr", "codigo qr", "qr"], &["qrコード", "qrスキャナー", "qr"]],
    }
}

/// Saying on (`{x}` the feature's name), per language.
pub(super) const ON: [&[&str]; 5] = [
    &["turn on {x}", "switch on {x}", "enable {x}", "activate {x}", "start {x}", "{x} on", "turn {x} on", "switch {x} on", "put {x} on"],
    &["active {x}", "activer {x}", "allume {x}", "allumer {x}", "mets {x}", "lance {x}", "demarre {x}", "{x} on", "{x} active"],
    &["activa {x}", "activar {x}", "enciende {x}", "encender {x}", "pon {x}", "prende {x}", "{x} on"],
    &["ativa {x}", "ativar {x}", "liga {x}", "ligar {x}", "acende {x}", "{x} on"],
    &["{x}をオンにして", "{x}オン", "{x}をオン", "{x}をつけて", "{x}つけて", "{x}を有効にして", "{x}をオンにする"],
];

/// Saying off.
pub(super) const OFF: [&[&str]; 5] = [
    &["turn off {x}", "switch off {x}", "disable {x}", "deactivate {x}", "stop {x}", "{x} off", "turn {x} off", "switch {x} off"],
    &["desactive {x}", "desactiver {x}", "eteins {x}", "eteindre {x}", "arrete {x}", "coupe {x}", "{x} off"],
    &["desactiva {x}", "desactivar {x}", "apaga {x}", "apagar {x}", "quita {x}", "{x} off"],
    &["desativa {x}", "desativar {x}", "desliga {x}", "desligar {x}", "apaga {x}", "{x} off"],
    &["{x}をオフにして", "{x}オフ", "{x}をオフ", "{x}を消して", "{x}消して", "{x}を無効にして", "{x}を止めて", "{x}をオフにする"],
];

/// Showing and hiding N4N0 itself.
pub(super) const SHOW: [&[&str]; 5] = [
    &["show {x}", "show yourself", "appear"],
    &["affiche {x}", "afficher {x}", "montre {x}", "montre toi", "apparais"],
    &["muestra {x}", "mostrar {x}", "muestrate", "aparece"],
    &["mostra {x}", "mostrar {x}", "aparece", "aparecer"],
    &["{x}を表示して", "{x}を出して", "出てきて", "表示して"],
];
pub(super) const HIDE: [&[&str]; 5] = [
    &["hide {x}", "hide yourself", "disappear"],
    &["cache {x}", "cacher {x}", "masque {x}", "masquer {x}", "cache toi", "disparais"],
    &["oculta {x}", "ocultar {x}", "esconde {x}", "escondete", "desaparece"],
    &["esconde {x}", "esconder {x}", "oculta {x}", "ocultar {x}", "desaparece"],
    &["{x}を隠して", "{x}を非表示にして", "隠れて", "消えて"],
];

/// The music and the volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Media {
    Pause,
    Play,
    Next,
    Previous,
    Louder,
    Quieter,
    Mute,
    Unmute,
}

pub(super) fn media(kind: Media) -> [&'static [&'static str]; 5] {
    match kind {
        Media::Pause => [
            &["pause", "pause music", "pause song", "stop music", "pause video", "pause it"],
            &["pause", "mets pause", "mets en pause", "pause musique", "arrete musique", "stop musique"],
            &["pausa", "pausa musica", "pon pausa", "para musica", "deten musica"],
            &["pausa", "pausar", "pausa musica", "para musica", "pausar musica"],
            &["一時停止", "一時停止して", "音楽を止めて", "音楽止めて", "ポーズ"],
        ],
        Media::Play => [
            &["play", "resume", "play music", "resume music", "unpause", "continue music"],
            &["lecture", "reprends", "reprendre", "joue musique", "relance musique", "remets musique", "reprends musique"],
            &["reproduce", "reanuda", "reproducir", "reanudar musica", "reanuda musica", "continua musica"],
            &["toca", "continua", "retoma", "tocar musica", "continuar musica", "continua musica"],
            &["再生", "再生して", "音楽を再生して", "音楽再生", "続けて"],
        ],
        Media::Next => [
            &["next", "next song", "next track", "skip", "skip song", "skip this", "skip track"],
            &["suivant", "chanson suivante", "morceau suivant", "passe", "passe chanson", "musique suivante"],
            &["siguiente", "siguiente cancion", "salta", "salta cancion", "pasa cancion"],
            &["proxima", "proxima musica", "pula", "pular musica", "passa musica", "proxima faixa"],
            &["次", "次の曲", "スキップ", "次の曲にして", "曲を飛ばして"],
        ],
        Media::Previous => [
            &["previous", "previous song", "previous track", "last song", "go back song"],
            &["precedent", "chanson precedente", "morceau precedent", "musique precedente"],
            &["anterior", "cancion anterior", "pista anterior"],
            &["anterior", "musica anterior", "faixa anterior"],
            &["前の曲", "前の曲にして", "前"],
        ],
        Media::Louder => [
            &["louder", "volume up", "turn it up", "turn up volume", "turn volume up", "raise volume", "more volume", "louder please"],
            &["plus fort", "monte son", "monte volume", "augmente volume", "augmente son", "monte"],
            &["mas alto", "sube volumen", "sube", "subir volumen", "mas volumen", "sube musica"],
            &["mais alto", "aumenta volume", "sobe volume", "aumentar volume", "mais volume", "aumenta som"],
            &["音量を上げて", "音量上げて", "ボリュームを上げて", "ボリューム上げて", "大きくして", "音を大きくして"],
        ],
        Media::Quieter => [
            &["quieter", "softer", "volume down", "turn it down", "turn down volume", "turn volume down", "lower volume", "less volume"],
            &["moins fort", "baisse son", "baisse volume", "diminue volume", "diminue son", "baisse"],
            &["mas bajo", "baja volumen", "baja", "bajar volumen", "menos volumen", "baja musica"],
            &["mais baixo", "abaixa volume", "diminui volume", "baixa volume", "menos volume", "abaixa som"],
            &["音量を下げて", "音量下げて", "ボリュームを下げて", "ボリューム下げて", "小さくして", "音を小さくして"],
        ],
        Media::Mute => [
            &["mute", "mute sound", "mute music", "mute volume", "mute computer"],
            &["coupe son", "mets en sourdine", "sourdine", "mute", "coupe volume"],
            &["silencia", "silencio", "silenciar", "mute", "silencia musica"],
            &["muta", "silencia", "mudo", "mute", "silenciar"],
            &["ミュート", "消音", "ミュートして", "消音して"],
        ],
        Media::Unmute => [
            &["unmute", "unmute sound", "unmute music", "sound back"],
            &["remets son", "reactive son", "enleve sourdine"],
            &["quita silencio", "activa sonido", "reactiva sonido"],
            &["desmuta", "ativa som", "tira mudo", "volta som"],
            &["ミュート解除", "消音解除", "ミュートを解除して"],
        ],
    }
}

/// Undoing the last action.
pub(super) const UNDO: [&[&str]; 5] = [
    &["undo", "undo that", "undo it", "cancel that", "revert that", "put it back", "undo last"],
    &["annule", "annule ca", "defais", "defais ca", "remets comme avant", "annule dernier"],
    &["deshaz", "deshacer", "deshazlo", "deshaz eso", "vuelve como antes", "deshaz ultimo"],
    &["desfaz", "desfazer", "desfaz isso", "volta como estava", "desfaz ultimo"],
    &["元に戻して", "戻して", "取り消して", "元に戻す", "取り消し"],
];

/// Stopping a recording.
pub(super) const STOP_RECORDING: [&[&str]; 5] = [
    &["stop recording", "stop record", "end recording"],
    &["arrete enregistrement", "stop enregistrement", "arrete enregistrer", "termine enregistrement"],
    &["deten grabacion", "para grabacion", "parar grabacion", "termina grabacion"],
    &["para gravacao", "parar gravacao", "pare gravacao", "termina gravacao"],
    &["録画を止めて", "録画停止", "録画止めて", "録画を終了して"],
];

/// A 3D picture of people (`{w}`: world for the world's, in each language).
pub(super) const PICTURE: [&[&str]; 5] = [
    &["take 3d picture", "3d picture", "take 3d photo", "3d photo", "take picture in 3d", "take 3d picture of people", "3d picture of people"],
    &["prends photo 3d", "photo 3d", "prendre photo 3d", "fais photo 3d", "prends photo en 3d", "photo 3d gens"],
    &["toma foto 3d", "foto 3d", "saca foto 3d", "haz foto 3d", "foto en 3d", "foto 3d gente"],
    &["tira foto 3d", "foto 3d", "tirar foto 3d", "faz foto 3d", "foto em 3d", "foto 3d pessoas"],
    &["3d写真を撮って", "3d写真", "3dで撮って", "3d写真撮って", "3d写真を撮る"],
];
pub(super) const PICTURE_WORLD: [&[&str]; 5] = [
    &["take 3d picture of world", "3d picture of world", "take 3d photo of world", "3d photo of world", "3d picture of this world"],
    &["prends photo 3d monde", "photo 3d monde", "photo 3d ce monde", "prends photo 3d ce monde"],
    &["toma foto 3d mundo", "foto 3d mundo", "foto 3d este mundo", "saca foto 3d mundo"],
    &["tira foto 3d mundo", "foto 3d mundo", "foto 3d deste mundo", "tirar foto 3d mundo"],
    &["ワールドの3d写真を撮って", "ワールドの3d写真", "ワールドを3dで撮って"],
];

/// Seconds: the words before a number and after it (`in 10 seconds`), per language; Japanese says `10秒後に`.
pub(super) const IN: [&[&str]; 5] = [&["in"], &["dans"], &["en"], &["em"], &[]];
pub(super) const SECONDS: [&[&str]; 5] = [
    &["seconds", "second", "secs", "sec", "s"],
    &["secondes", "seconde", "sec", "s"],
    &["segundos", "segundo", "seg", "s"],
    &["segundos", "segundo", "seg", "s"],
    &[],
];

/// Numbers said as words (one to ten, fifteen, twenty, thirty).
pub(super) const NUMBERS: &[(&str, u32)] = &[
    ("one", 1), ("two", 2), ("three", 3), ("four", 4), ("five", 5), ("six", 6), ("seven", 7), ("eight", 8), ("nine", 9), ("ten", 10), ("fifteen", 15), ("twenty", 20), ("thirty", 30),
    ("un", 1), ("une", 1), ("deux", 2), ("trois", 3), ("quatre", 4), ("cinq", 5), ("sept", 7), ("huit", 8), ("neuf", 9), ("dix", 10), ("quinze", 15), ("vingt", 20), ("trente", 30),
    ("uno", 1), ("dos", 2), ("tres", 3), ("cuatro", 4), ("cinco", 5), ("seis", 6), ("siete", 7), ("ocho", 8), ("nueve", 9), ("diez", 10), ("veinte", 20), ("treinta", 30),
    ("um", 1), ("uma", 1), ("dois", 2), ("duas", 2), ("quatro", 4), ("sete", 7), ("oito", 8), ("nove", 9), ("dez", 10), ("vinte", 20), ("trinta", 30),
];

/// Opening a page (`{p}` its name).
pub(super) const OPEN: [&[&str]; 5] = [
    &["open {p}", "show {p}", "go to {p}", "show me {p}", "open up {p}", "take me to {p}"],
    &["ouvre {p}", "ouvrir {p}", "affiche {p}", "va sur {p}", "va dans {p}", "montre {p}", "montre moi {p}"],
    &["abre {p}", "abrir {p}", "muestra {p}", "muestrame {p}", "ve a {p}", "llevame a {p}"],
    &["abre {p}", "abrir {p}", "mostra {p}", "mostrar {p}", "vai para {p}", "me mostra {p}"],
    &["{p}を開いて", "{p}開いて", "{p}を見せて", "{p}を表示して", "{p}を開く"],
];

/// The pages' names, per language.
pub(super) fn page_names(page: super::Page) -> [&'static [&'static str]; 5] {
    use super::Page as P;
    match page {
        P::AllTools => [&["all tools", "tools"], &["tous outils", "outils"], &["todas herramientas", "herramientas"], &["todas ferramentas", "ferramentas"], &["すべてのツール", "ツール"]],
        P::Pictures => [&["pictures", "photos", "gallery"], &["photos", "galerie"], &["fotos", "galeria"], &["fotos", "galeria"], &["写真", "ギャラリー"]],
        P::Worlds => [&["worlds", "worlds in 3d", "3d worlds", "world search"], &["mondes", "mondes en 3d"], &["mundos", "mundos en 3d"], &["mundos", "mundos em 3d"], &["3dワールド", "ワールド"]],
        P::Instances => [&["previous instances", "instances", "recent instances"], &["instances precedentes", "instances"], &["instancias anteriores", "instancias"], &["instancias anteriores", "instancias"], &["過去のインスタンス", "インスタンス"]],
        P::Youtube => [&["youtube"], &["youtube"], &["youtube"], &["youtube"], &["youtube", "ユーチューブ"]],
        P::VideoHistory => [&["video history", "history"], &["historique", "historique videos"], &["historial", "historial videos"], &["historico", "historico videos"], &["履歴", "動画の履歴"]],
        P::Ask => [&["ask n4n0", "conversation", "assistant page"], &["demander n4n0", "conversation"], &["preguntar n4n0", "conversacion"], &["perguntar n4n0", "conversa"], &["n4n0に聞く", "会話"]],
        P::MenuSettings => [&["settings", "menu settings"], &["parametres", "reglages"], &["configuracion", "ajustes"], &["configuracoes", "ajustes"], &["設定"]],
        P::Tracking => [&["tracking", "smoothing page"], &["suivi", "page lissage"], &["seguimiento"], &["rastreamento"], &["トラッキング"]],
        P::Microphones => [&["microphones", "mics", "microphone"], &["micros", "microphones", "micro"], &["microfonos", "microfono"], &["microfones", "microfone"], &["マイク"]],
        P::FpsBooster => [&["fps booster", "booster", "boost fps"], &["booster fps", "booster"], &["booster fps", "booster"], &["booster fps", "booster"], &["fpsブースター", "ブースター"]],
        P::Translation => [&["translation page", "translation settings"], &["page traduction", "reglages traduction"], &["pagina traduccion"], &["pagina traducao"], &["翻訳ページ", "翻訳設定"]],
        P::Music => [&["music"], &["musique"], &["musica"], &["musica"], &["音楽"]],
        P::Light => [&["light page", "light settings"], &["page lumiere", "reglages lumiere"], &["pagina luz"], &["pagina luz"], &["ライト設定"]],
        P::States => [&["states", "avatar states", "saved states"], &["etats", "etats avatar"], &["estados", "estados avatar"], &["estados", "estados avatar"], &["状態", "アバターの状態"]],
        P::Picture3d => [&["3d picture page", "camera page"], &["page photo 3d"], &["pagina foto 3d"], &["pagina foto 3d"], &["3d写真ページ"]],
        P::Calls => [&["calls", "call page"], &["appels"], &["llamadas"], &["chamadas"], &["通話"]],
        P::QrScanner => [&["qr page"], &["page qr"], &["pagina qr"], &["pagina qr"], &["qrページ"]],
        P::Suggest => [&["suggestions", "suggest features"], &["suggestions"], &["sugerencias"], &["sugestoes"], &["提案"]],
        P::AppHome => [&["home", "orbiters window", "app", "orbiters app"], &["accueil", "fenetre orbiters", "appli"], &["inicio", "ventana orbiters", "app"], &["inicio", "janela orbiters", "app"], &["ホーム", "アプリ"]],
        P::AppCapture => [&["capture page"], &["page capture"], &["pagina captura"], &["pagina captura"], &["キャプチャー"]],
        P::AppProjects => [&["projects", "my avatar"], &["projets"], &["proyectos"], &["projetos"], &["プロジェクト"]],
        P::AppN4n0 => [&["n4n0 page"], &["page n4n0"], &["pagina n4n0"], &["pagina n4n0"], &["n4n0ページ"]],
        P::AppCaptures => [&["photos and captures", "captures"], &["photos et captures", "captures"], &["fotos y capturas", "capturas"], &["fotos e capturas", "capturas"], &["写真とキャプチャー", "キャプチャー一覧"]],
        P::AppWorlds => [&["worlds page", "map explorer"], &["page mondes"], &["pagina mundos"], &["pagina mundos"], &["ワールドページ"]],
        P::AppEvents => [&["events"], &["evenements"], &["eventos"], &["eventos"], &["イベント"]],
        P::AppVideos => [&["videos", "saved videos"], &["videos", "videos enregistrees"], &["videos", "videos guardados"], &["videos", "videos salvos"], &["動画"]],
        P::AppSettings => [&["app settings", "orbiters settings"], &["parametres appli", "parametres orbiters"], &["configuracion app", "configuracion orbiters"], &["configuracoes app", "configuracoes orbiters"], &["アプリの設定"]],
    }
}

/// Words dropped before matching (articles, possessives, politeness, the companion's name), per language, all together:
/// a sentence's meaning for these commands never rests on them.
pub(super) const FILLERS: &[&str] = &[
    "the", "my", "your", "a", "an", "please", "pls", "now", "can", "could", "you", "would", "for", "me", "us", "it's", "hey", "ok", "okay", "n4n0", "nano", "of", "this", "to",
    "le", "la", "les", "l", "un", "une", "des", "du", "de", "d", "mon", "ma", "mes", "ton", "ta", "tes", "stp", "svp", "s", "il", "te", "plait", "tu", "peux", "maintenant", "ce", "cette",
    "el", "los", "las", "una", "unos", "unas", "mi", "mis", "tu", "tus", "por", "favor", "puedes", "ahora", "del", "este", "esta",
    "o", "os", "as", "um", "uma", "umas", "meu", "minha", "seu", "sua", "pode", "voce", "agora", "do", "da", "dos", "das", "deste", "desta",
];

/// Words between two commands of one sentence ("turn on fast walk and translation").
pub(super) const AND: &[&str] = &["and", "then", "et", "puis", "y", "luego", "e", "depois"];

/// Japanese endings that only make a request polite.
pub(super) const JA_POLITE: &[&str] = &["ください", "下さい", "お願いします", "おねがいします", "お願い", "ちょうだい"];
