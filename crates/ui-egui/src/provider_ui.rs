//! UI integration for the optional external file capability.
use crate::EffectcraftApp;
use serde_json::{Value, json};

pub fn document_id(app: &EffectcraftApp) -> u64 {
    app.session.script_ui.project_epoch
}
pub fn choose(app: &mut EffectcraftApp, id: &str) -> Option<Result<Value, String>> {
    let host = app.hooks.file_providers.clone();
    if !host.enabled() {
        return None;
    }
    let doc = document_id(app);
    if matches!(
        id,
        "file.newProject" | "file.open" | "file.openRecent" | "file.closeProject" | "file.revert" | "file.openDemoProject" | "templates.create" | "app.quit"
    ) && host.busy()
    {
        return Some(Err("Finish or cancel the pending provider operation before replacing the project".into()));
    }
    if !matches!(id, "file.save" | "file.saveAs") {
        return None;
    }
    let name = host.filename(doc).unwrap_or_else(|| format!("{}.ecbundle", "Untitled Project"));
    Some(host.choose(doc, app.session.revision, name, id == "file.saveAs").map(|_| json!({"pending":"fileProvider"})))
}

pub fn publish(app: &EffectcraftApp) {
    let host = &app.hooks.file_providers;
    if !host.connected() {
        return;
    }
    let doc = document_id(app);
    host.publish_documents(json!([{"id":doc.to_string(),"revision":app.session.revision,"name":host.filename(doc).unwrap_or_else(|| "Untitled Project.ecbundle".into()),"dirty":app.session.is_dirty(),"provider":host.bound(doc)}]),&[doc]);
}

/// Provider recent files are separate from device paths and remain account-scoped.
pub fn recent(app: &EffectcraftApp, ui: &mut egui::Ui) {
    app.hooks.file_providers.recent_ui(ui);
}

/// Safe suggested filename; project names may contain path separators or be long UTF-8.
pub fn suggested_name(stem: &str, extension: &str) -> String {
    let mut name = String::new();
    for c in stem.chars() {
        if name.len() + c.len_utf8() > 230 {
            break;
        }
        name.push(if c.is_control() || matches!(c, '/' | '\\') { '-' } else { c });
    }
    if name.trim().is_empty() {
        name = "Untitled".into();
    }
    format!("{name}.{extension}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn suggestion_is_bounded_utf8_and_not_a_device_path() {
        let name = suggested_name(&"á/\\nube".repeat(100), "ecbundle");
        assert!(name.len() <= 255);
        assert!(!name.contains(['/', '\\']));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod provider_routing_tests {
    use super::*;
    #[test]
    fn default_off_keeps_original_save_and_registration_only_emits_a_choice() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        assert!(choose(&mut app, "file.save").is_none());
        let host = app.hooks.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        let before = app.session.revision;
        assert!(choose(&mut app, "file.save").unwrap().is_ok());
        let events = host.take_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].metadata["type"], "saveDialog");
        assert_eq!(app.session.revision, before, "a choice must not mutate document state");
    }
}
