//! The templates bundled with the app, put in its own cache (app data): what the copier of projects then copies from.
use orbiters_core::locks::*;
use crate::templates::{copy, Template};
use anyhow::Context;
use orbiters_i18n::tr;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// A fresh install has no VCC template directory. Bundle ALCOM's MIT-licensed modern template assets so these are
/// available offline; VPM still downloads and resolves the SDK through its normal official repository.
pub fn builtins(cache: &Path) -> anyhow::Result<Vec<Template>> {
    static MATERIALIZE: Mutex<()> = Mutex::new(());
    let _one = MATERIALIZE.locked();
    let mut out = vec![];
    for (id, files) in crate::builtin_templates::TEMPLATES {
        let path = cache.join(id);
        crate::disk::create_dir_all(&path)?;
        for (relative, content) in *files {
            let p = path.join(relative);
            if !p.is_file() {
                crate::disk::create_dir_all(p.parent().unwrap())?;
                crate::disk::write(&p, content)?;
            }
        }
        let avatars = id.starts_with("avatars");
        out.push(Template {
            name: if avatars { tr("VRChat Avatar Project") } else { tr("VRChat World Project") }.into(),
            path,
            unity: "2022.3.22f1".into(),
            builtin: true,
            id: if avatars { "com.vrchat.template.avatar" } else { "com.vrchat.template.world" }.into(),
            banner: None,
        });
    }
    // Found by its id: its name is translated, so it is the one thing that cannot be compared.
    let avatar = out.iter().find(|t| t.id == "com.vrchat.template.avatar").with_context(|| tr("Bundled avatar template is missing"))?;
    let path = cache.join("base-2022.3.22f1");
    if !path.join("ProjectSettings/ProjectVersion.txt").is_file() {
        copy(&avatar.path, &path, &AtomicBool::new(false), &Arc::new(Mutex::default()))?;
    }
    // Written when it is not this already (PROJECTS-24 a): every look at a project's packages lists the templates, and each
    // wrote, flushed to disk and replaced the same file. The avatar template's manifest copied in first is replaced.
    let manifest = path.join("Packages/vpm-manifest.json");
    let base = serde_json::json!({"dependencies":{"com.vrchat.base":{"version":"3.x"}}});
    if std::fs::read(&manifest).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()).as_ref() != Some(&base) {
        crate::write_json(&manifest, &base)?;
    }
    out.push(Template {
        name: String::from(tr("VRChat Base Project")),
        path,
        unity: "2022.3.22f1".into(),
        builtin: true,
        id: "com.vrchat.template.base".into(),
        banner: None,
    });
    Ok(out)
}
