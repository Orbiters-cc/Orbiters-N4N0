//! Looks: the layers of an edit kept under a name, to put on another picture or on the 3D view (where they stay while
//! the camera moves). Kept in `looks.json` in the app's settings folder, the last saved first; a look of the same name
//! is replaced. Only the layers over the whole picture are kept, not the crop (it belongs to its picture) nor a VRChat
//! multi-layer picture's own stacks.

use orbiters_i18n::tr;
use orbiters_ui::photo::{Edit, Layer};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub name: String,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    looks: Vec<Look>,
}

fn path() -> PathBuf {
    // Another folder for runs that must not write the user's (screenshots, tests).
    if let Some(d) = std::env::var_os("ORBITERS_EDITOR_LOOKS").map(PathBuf::from) {
        return d.join("looks.json");
    }
    orbiters_core::paths::config_dir().join("looks.json")
}

/// The looks saved, the last first.
pub fn list() -> Vec<Look> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok()).map(|f| f.looks).unwrap_or_default()
}

fn write(looks: Vec<Look>) -> anyhow::Result<()> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    orbiters_core::write_atomic(&p, &serde_json::to_vec_pretty(&File { looks })?)?;
    Ok(())
}

/// Keeps the layers of `edit` as the look `name` (first in the list; one of that name replaced).
pub fn save(name: &str, edit: &Edit) -> anyhow::Result<()> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "{}", tr("a look needs a name"));
    let mut looks = list();
    looks.retain(|l| !l.name.eq_ignore_ascii_case(name));
    looks.insert(0, Look { name: name.to_string(), layers: edit.layers.clone() });
    write(looks)
}

pub fn remove(name: &str) -> anyhow::Result<()> {
    let mut looks = list();
    looks.retain(|l| l.name != name);
    write(looks)
}

/// `look`'s layers put over `edit`'s, each with an id of `edit`'s.
pub fn apply(edit: &mut Edit, look: &Look) {
    for l in &look.layers {
        let mut fresh = edit.layer(l.effect.clone());
        fresh.visible = l.visible;
        fresh.opacity = l.opacity;
        fresh.mask = l.mask.clone();
        edit.layers.push(fresh);
    }
}

/// The Looks button in `rect` of the top bar: each look saved to put over the layers, the layers kept as a new look,
/// and each look to remove.
pub(super) fn menu(ed: &mut super::Editor, ui: &mut egui::Ui, rect: egui::Rect) {
    use orbiters_ui::popup::{self, Item};
    // Read again while the list is open (another editor may have saved one).
    let looks = list();
    let name = new_name(&looks);
    let applying: Vec<String> = looks.iter().map(|l| tr!("Apply “{name}”", name = l.name)).collect();
    let removing: Vec<String> = looks.iter().map(|l| tr!("Remove “{name}”", name = l.name)).collect();
    let saving = tr!("Save these layers as “{name}”", name = name);
    let mut items: Vec<Item<'_>> = applying.iter().map(|a| Item::new(a).icon(orbiters_ui::icons::SPARKLE)).collect();
    if !ed.doc.layers.is_empty() {
        items.push(Item::new(&saving).icon(orbiters_ui::icons::FLOPPY_DISK));
    }
    items.extend(removing.iter().map(|r| Item::new(r).icon(orbiters_ui::icons::TRASH).danger()));
    if items.is_empty() {
        items.push(Item::new(tr("Add layers, then keep them here as a look")).detail(tr("none yet")));
    }
    let Some(i) = popup::menu_button(ui, "editor-looks", rect, orbiters_ui::icons::SPARKLE, tr("Looks"), &items) else { return };
    let saves = !ed.doc.layers.is_empty();
    if i < looks.len() {
        apply(&mut ed.doc, &looks[i]);
        ed.selected = super::Selected::Nothing;
    } else if saves && i == looks.len() {
        if let Err(e) = save(&name, &ed.doc) {
            ed.error = Some(tr!("The look could not be saved: {error}", error = format!("{e:#}")));
        }
    } else if let Some(look) = looks.get(i - looks.len() - usize::from(saves)) {
        if let Err(e) = remove(&look.name) {
            ed.error = Some(tr!("The look could not be removed: {error}", error = format!("{e:#}")));
        }
    }
}

/// A name for a new look that no other has ("Look 3").
pub fn new_name(looks: &[Look]) -> String {
    (1..).map(|n| tr!("Look {n}", n = n)).find(|n| !looks.iter().any(|l| l.name.eq_ignore_ascii_case(n))).expect("a free name")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_ui::photo::{Effect, Light, Mask, Vignette};

    /// The refusal is said in the language of the person, when it happens (not when the program started).
    #[test]
    fn a_look_without_a_name_is_refused_in_each_language() {
        use orbiters_i18n::{in_language, Language};
        let edit = Edit::default();
        assert_eq!(save("  ", &edit).unwrap_err().to_string(), "a look needs a name");
        for (language, said) in [(Language::Fr, "un style a besoin d’un nom"), (Language::Es, "un estilo necesita un nombre"), (Language::PtBr, "um estilo precisa de um nome"), (Language::Ja, "ルックには名前が必要です")] {
            assert_eq!(in_language(language, || save("  ", &edit).unwrap_err().to_string()), said);
        }
    }

    #[test]
    fn a_look_is_kept_by_name_and_put_over_another_edit() {
        let dir = std::env::temp_dir().join(format!("orbiters-looks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("ORBITERS_EDITOR_LOOKS", &dir);
        let mut edit = Edit::default();
        let mut l = edit.layer(Effect::Light(Light { exposure: 0.5, ..Default::default() }));
        l.opacity = 0.7;
        l.mask = Mask::radial();
        edit.layers.push(l);
        let v = edit.layer(Effect::Vignette(Vignette::default()));
        edit.layers.push(v);
        assert_eq!(new_name(&list()), "Look 1");
        save("Dusk", &edit).unwrap();
        save("Night", &Edit::default()).unwrap();
        save("dusk", &edit).unwrap();
        let looks = list();
        assert_eq!(looks.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["dusk", "Night"], "the same name replaced, the last first");
        assert_eq!(new_name(&looks), "Look 1");
        let mut other = Edit::default();
        let first = other.layer(Effect::Light(Light::default()));
        other.layers.push(first);
        apply(&mut other, &looks[0]);
        assert_eq!(other.layers.len(), 3);
        assert_eq!(other.layers.iter().map(|l| l.id).collect::<Vec<_>>(), [1, 2, 3], "ids of its own");
        assert_eq!((other.layers[1].opacity, &other.layers[1].mask), (0.7, &Mask::radial()));
        remove("Night").unwrap();
        assert_eq!(list().len(), 1);
        std::env::remove_var("ORBITERS_EDITOR_LOOKS");
        let _ = std::fs::remove_dir_all(dir);
    }
}
