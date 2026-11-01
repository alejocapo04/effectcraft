//! Web-only project/media transport. Resource IDs never enter the browser file table.
use effectcraft_engine::{
    Session,
    project::{Footage, ItemKind, Project},
};
use effectcraft_ui_egui::{
    EffectcraftApp,
    file_providers::{Capture, Command},
    provider_bundle::{self, Bundle, MAX_BYTES},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

#[derive(Default)]
pub struct Runtime {
    checkpoints: HashMap<(u64, u64), Arc<Project>>,
}

fn footage_paths(f: &Footage, out: &mut BTreeSet<String>) {
    if f.data.is_some() {
        return;
    }
    if f.sequence.is_empty() {
        if !f.path.is_empty() {
            out.insert(f.path.clone());
        }
    } else {
        out.extend(f.sequence.iter().cloned());
    }
}
fn file_properties(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(o) => {
            if o.get("ui").and_then(|v| v.get("kind")).and_then(Value::as_str) == Some("File") {
                fn strings(v: &Value, out: &mut BTreeSet<String>) {
                    if v.get("t").and_then(Value::as_str) == Some("Str") {
                        if let Some(s) = v.get("v").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                            out.insert(s.into());
                        }
                    }
                }
                if let Some(v) = o.get("value") {
                    strings(v, out);
                }
                if let Some(keys) = o.get("keys").and_then(Value::as_array) {
                    for k in keys {
                        if let Some(v) = k.get("value") {
                            strings(v, out);
                        }
                    }
                }
            }
            for v in o.values() {
                file_properties(v, out);
            }
        }
        Value::Array(a) => {
            for v in a {
                file_properties(v, out)
            }
        }
        _ => {}
    }
}
fn referenced(project: &Project, text: &Value) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for item in project.items.values() {
        if let ItemKind::Footage(f) = &item.kind {
            footage_paths(f, &mut paths);
        }
        if let Some(proxy) = &item.proxy {
            footage_paths(&proxy.footage, &mut paths);
        }
    }
    file_properties(text, &mut paths);
    paths
}

fn capture(session: &Session) -> Result<Vec<u8>, String> {
    let project = session.project.to_file_json_with_view(&session.saved_view(), false).map_err(|e| e.to_string())?.into_bytes();
    if project.len() > 16 * 1024 * 1024 {
        return Err("Provider project metadata exceeds 16 MiB".into());
    }
    let value: Value = serde_json::from_slice(&project).map_err(|e| e.to_string())?;
    let mut todo: Vec<String> = referenced(&session.project, &value).into_iter().collect();
    let mut assets = BTreeMap::new();
    let mut total = project.len();
    while let Some(path) = todo.pop() {
        if assets.contains_key(&path) {
            continue;
        }
        if assets.len() >= 10_000 {
            return Err("too many provider media resources".into());
        }
        let data = crate::files::get(&path).ok_or_else(|| format!("Cannot include missing media {path}"))?;
        if data.len() > MAX_BYTES.saturating_sub(total) {
            return Err("Project and media exceed 80 MiB".into());
        }
        let bytes = data.to_vec();
        total = total.saturating_add(bytes.len());
        if total > MAX_BYTES {
            return Err("project and media exceed 80 MiB".into());
        }
        todo.extend(provider_bundle::sidecars(&path, &bytes)?);
        assets.insert(path, bytes);
    }
    Bundle { project, assets }.encode()
}

fn open(app: &mut EffectcraftApp, request: &str, bytes: &[u8], ctx: Option<&egui::Context>) -> Result<(), String> {
    effectcraft_ui_egui::provider_ui::check_open(app, ctx)?;
    let bundle = Bundle::decode(bytes)?;
    let text = std::str::from_utf8(&bundle.project).map_err(|e| e.to_string())?;
    let project = Project::from_json(text).map_err(|e| e.to_string())?;
    let mut value: Value = serde_json::from_slice(&bundle.project).map_err(|e| e.to_string())?;
    let mut todo: Vec<String> = referenced(&project, &value).into_iter().collect();
    let mut checked = BTreeSet::new();
    while let Some(path) = todo.pop() {
        if !checked.insert(path.clone()) {
            continue;
        }
        let bytes = bundle.assets.get(&path).ok_or_else(|| format!("Provider bundle is missing {path}"))?;
        todo.extend(provider_bundle::sidecars(&path, bytes)?);
    }
    let root = format!("/__file_provider/{request}");
    let mut replacements = BTreeMap::new();
    let mut staged = BTreeSet::new();
    for path in bundle.assets.keys() {
        let target = provider_bundle::stage_path(&root, path)?;
        if !staged.insert(target.clone()) {
            return Err("provider media paths alias after normalization".into());
        }
        replacements.insert(path.clone(), target);
    }
    provider_bundle::rewrite(&mut value, &replacements);
    let text = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    let project = Project::from_json(&text).map_err(|e| e.to_string())?;
    let view = effectcraft_engine::project::saved_view(&text);
    for (path, bytes) in bundle.assets {
        if let Some(target) = replacements.get(&path) {
            crate::files::put(target, bytes.into());
        }
    }
    app.session.replace_project(project, None);
    app.session.restore_view(&view);
    app.ui.start_screen = false;
    for (path, _) in crate::files::list() {
        if path.starts_with("/__file_provider/") && !path.starts_with(&format!("{root}/")) {
            crate::files::STORE.remove_file(&path);
        }
    }
    Ok(())
}

impl Runtime {
    pub fn tick(&mut self, app: &mut EffectcraftApp, ctx: &egui::Context) {
        let host = app.hooks.file_providers.clone();
        for command in host.take_commands() {
            match command {
                Command::Save { request, doc, destination, name, save_as } => {
                    if doc != effectcraft_ui_egui::provider_ui::document_id(app) {
                        host.failed(&request, "The document was replaced before capture");
                        continue;
                    }
                    let revision = app.session.revision;
                    let checkpoint = app.session.project.clone();
                    let result = if destination == "device" {
                        let native_name = std::path::Path::new(&name).with_extension("ecproj").to_string_lossy().into_owned();
                        app.session
                            .project
                            .to_file_json_with_view(&app.session.saved_view(), app.session.prefs.project.compact_project_files)
                            .map_err(|e| e.to_string())
                            .and_then(|text| crate::files::download(&native_name, text.as_bytes()).map_err(|e| format!("Download failed: {e:?}")))
                            .map(|_| host.finish_device(&request))
                    } else {
                        capture(&app.session).and_then(|bytes| {
                            host.capture(&request, Capture { doc, revision, name, save_as }, bytes)?;
                            self.checkpoints.insert((doc, revision), checkpoint);
                            Ok(())
                        })
                    };
                    if let Err(error) = result {
                        host.failed(&request, &error);
                        app.ui.status = error;
                    }
                }
                Command::Result { capture, resource, status } => {
                    let checkpoint = self.checkpoints.remove(&(capture.doc, capture.revision));
                    if status == "saved" && capture.doc == effectcraft_ui_egui::provider_ui::document_id(app) {
                        host.bind(&capture, resource);
                        if let Some(project) = checkpoint {
                            app.session.acknowledge_external_save(project, capture.revision);
                        }
                    }
                }
                Command::Open { request, resource: _, name, bytes } => match open(app, &request, &bytes, Some(ctx)) {
                    Ok(()) => host.opened(
                        &request,
                        Some(Capture { doc: effectcraft_ui_egui::provider_ui::document_id(app), revision: app.session.revision, name, save_as: false }),
                        "",
                    ),
                    Err(error) => {
                        host.opened(&request, None, &error);
                        app.ui.status = error;
                    }
                },
            }
        }
        effectcraft_ui_egui::provider_ui::publish(app);
        crate::file_providers::publish(&host);
    }
}

/// Reopen a downloaded complete bundle without binding it to a provider account.
pub fn open_device(app: &mut EffectcraftApp, bytes: &[u8]) -> Result<(), String> {
    let root = format!("device-{:016x}-{:016x}", js_sys::Date::now().to_bits(), js_sys::Math::random().to_bits());
    open(app, &root, bytes, None)
}
