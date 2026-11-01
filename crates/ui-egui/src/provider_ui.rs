//! UI integration for the optional external file capability.
use crate::EffectcraftApp;
use serde_json::{Value, json};

pub fn document_id(app: &EffectcraftApp) -> u64 {
    app.session.script_ui.project_epoch
}
pub fn choose(app: &mut EffectcraftApp, id: &str) -> Option<Result<Value, String>> {
    let host = app.hooks.file_providers.clone();
    let doc = document_id(app);
    if !host.enabled() {
        if matches!(id, "file.save" | "file.saveAs") && (host.bound(doc) || host.pending_for(doc)) {
            return Some(Err("Re-enable the provider before saving this linked or pending project; use Save a Copy for a device download".into()));
        }
        return None;
    }
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

/// Provider reads may finish after a dialog or gesture starts. Refuse before decoding/staging,
/// preserving the old project's dialog checkpoints and account/resource identity.
pub fn check_open(app: &EffectcraftApp, ctx: Option<&egui::Context>) -> Result<(), String> {
    // Inline text and numeric editors can retain a draft on an event-empty delivery frame.
    let draft = ctx.is_some_and(|ctx| ctx.text_edit_focused())
        || app.dialog.is_some()
        || app.file_dialogs.count > 0
        || crate::panels::live_preview::active(app)
        || app.session.state.text_edit.is_some()
        || app.ui.fx_pick.is_some()
        || app.ui.text_color_pick.is_some();
    // input_active is post-UI activity telemetry and includes already-consumed pointer motion.
    // Admission depends on live editor ownership and potentially editing input still in this pass.
    let interaction = app.last_pointer_down
        || app.ui.viewer.interacting
        || app.ui.viewer.property_interacting
        || app.session.key_move.is_some()
        || app.synthetic_in_flight
        || !app.synthetic.is_empty()
        || !app.forwarded.events.is_empty()
        || !app.forwarded.files.is_empty()
        || ctx.is_some_and(|ctx| {
            ctx.input(|input| {
                input.events.iter().any(|event| {
                    !matches!(event, egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_) | egui::Event::PointerGone)
                }) || !input.raw.dropped_files.is_empty()
                    || input.pointer.any_down()
            })
        });
    let jobs = app.session.render_job.is_some()
        || app.session.track_job.is_some()
        || app.session.mask_job.is_some()
        || app.session.warp_job.is_some()
        || !app.session.warp_pending.is_empty()
        || app.session.camera_job.is_some()
        || !app.session.camera_pending.is_empty()
        || app.session.roto_job.is_some()
        || !app.session.roto_pending.is_empty()
        || !app.session.offloaded.is_empty()
        || !app.session.tasks.is_empty();
    // Host::busy also includes this open request; only pending saves block the document here.
    if app.session.is_dirty() || app.hooks.file_providers.pending_for(document_id(app)) || draft || interaction || jobs {
        return Err("Save changes and finish pending editor work before opening a provider project".into());
    }
    Ok(())
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

    #[test]
    fn disabled_linked_or_pending_save_cannot_fall_through_to_device_saving() {
        for linked in [false, true] {
            let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
            app.session.execute("comp.new", json!({"name":"Edited","width":16,"height":16,"duration":1})).unwrap();
            let host = app.hooks.file_providers.clone();
            host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
            host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
            let doc = document_id(&app);
            if linked {
                host.bind(&crate::file_providers::Capture { doc, revision: 1, name: "Linked.ecbundle".into(), save_as: false }, "linked".into());
            } else {
                host.choose(doc, app.session.revision, "Pending.ecbundle".into(), false).unwrap();
                host.take_events();
            }
            host.set_enabled(false).unwrap();
            let project = app.session.project.clone();
            let saved = app.session.saved_revision;
            let ctx = egui::Context::default();
            for command in ["file.save", "file.saveAs"] {
                let error = crate::menus::invoke(&mut app, &ctx, command, json!({"path":""})).unwrap_err();
                assert!(error.contains("Re-enable the provider"));
                assert!(app.session.is_dirty());
                assert_eq!(app.session.saved_revision, saved);
                assert!(std::sync::Arc::ptr_eq(&project, &app.session.project));
                assert!(app.session.path.is_none());
                assert_eq!(host.bound(doc), linked);
                assert_eq!(host.pending_for(doc), !linked);
                assert!(host.take_events().is_empty(), "rejected Save must deliver neither a chooser nor bytes");
            }
            host.set_enabled(true).unwrap();
            publish(&app);
            assert_eq!(host.snapshot()["documents"][0]["dirty"], true);
            assert_eq!(host.snapshot()["documents"][0]["provider"], linked);
        }
    }

    #[test]
    fn disabled_unbound_document_keeps_legacy_save_routing() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let host = app.hooks.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        host.set_enabled(false).unwrap();
        assert!(choose(&mut app, "file.save").is_none());
        assert!(choose(&mut app, "file.saveCopy").is_none());
        assert!(host.take_events().is_empty());
    }

    #[test]
    fn provider_read_rechecks_live_dialog_started_after_request_without_rejecting_its_own_read() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.session.execute("comp.new", json!({"name":"A","width":16,"height":16,"duration":1})).unwrap();
        app.session.mark_saved();
        let host = app.hooks.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        let connection = host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        let doc = document_id(&app);
        host.bind(&crate::file_providers::Capture { doc, revision: app.session.revision, name: "A.ecbundle".into(), save_as: false }, "a".into());
        let resources = vec![crate::file_providers::Resource { id: "b".into(), name: "B.ecbundle".into(), updated_at: String::new() }];
        host.set_resources(&connection, resources).unwrap();
        let request = host.request_open("b").unwrap();
        host.take_events();
        let ctx = egui::Context::default();
        assert!(host.busy());
        assert!(check_open(&app, Some(&ctx)).is_ok(), "an idle editor must admit its pending read");
        crate::panels::dialogs::open_comp_settings(&mut app).unwrap();
        assert!(crate::panels::live_preview::active(&app));
        host.open(request.clone(), "B.ecbundle".into(), vec![]).unwrap();
        assert!(matches!(host.take_commands().as_slice(), [crate::file_providers::Command::Open { .. }]));
        let project = app.session.project.clone();
        let revision = app.session.revision;
        let resources = host.snapshot()["resources"].clone();
        let error = check_open(&app, Some(&ctx)).unwrap_err();
        host.opened(&request, None, &error);
        assert!(std::sync::Arc::ptr_eq(&project, &app.session.project));
        assert_eq!(document_id(&app), doc);
        assert_eq!(app.session.revision, revision);
        assert!(app.dialog.is_some());
        assert_eq!(host.filename(doc).as_deref(), Some("A.ecbundle"));
        assert_eq!(host.snapshot()["resources"], resources);
        let event = host.take_events().remove(0);
        assert_eq!(event.metadata["type"], "openFailed");
        assert_eq!(event.metadata["resourceId"], "b");
        assert!(event.bytes.is_none());
        crate::panels::live_preview::cancel(&mut app);
        app.dialog = None;
        assert!(std::sync::Arc::ptr_eq(&project, &app.session.project), "Cancel must still belong to A");
        assert!(check_open(&app, Some(&ctx)).is_ok());
    }

    #[test]
    fn provider_open_refuses_unconsumed_input_and_editor_gestures() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        assert!(check_open(&app, Some(&ctx)).is_ok());
        // Match the activity flag that the real UI phase records after a pointer-only frame.
        app.input_active = true;
        let pointer_only = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(egui::pos2(10.0, 20.0)),
                egui::Event::MouseMoved(egui::vec2(1.0, 2.0)),
                egui::Event::PointerGone,
            ],
            ..Default::default()
        };
        let _ = ctx.run_ui(pointer_only, |ui| assert!(check_open(&app, Some(ui.ctx())).is_ok()));
        app.last_pointer_down = true;
        assert!(check_open(&app, Some(&ctx)).is_err());
        app.last_pointer_down = false;
        app.ui.viewer.interacting = true;
        assert!(check_open(&app, Some(&ctx)).is_err());
        app.ui.viewer.interacting = false;
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Text("pending edit".into()));
        let _ = ctx.run_ui(raw, |ui| assert!(check_open(&app, Some(ui.ctx())).is_err()));
    }

    #[test]
    fn focused_inline_draft_survives_an_event_empty_provider_delivery_frame() {
        let app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        assert!(check_open(&app, Some(&ctx)).is_ok());
        let project = app.session.project.clone();
        let revision = app.session.revision;
        let doc = document_id(&app);
        let id = egui::Id::unique("provider-inline-draft");
        let expected = "Uncommitted inline value";
        let mut draft = String::new();
        let editing = egui::RawInput { events: vec![egui::Event::Text(expected.into())], ..Default::default() };
        let mut first = ctx.run_ui(editing, |ui| {
            ui.memory_mut(|memory| memory.request_focus(id));
            let field = ui.add(egui::TextEdit::singleline(&mut draft).id(id));
            assert!(field.has_focus(), "a real TextEdit must own the draft");
        });
        first.textures_delta.clear();
        assert_eq!(draft, expected);
        let mut delivery = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(ui.input(|input| input.events.is_empty()), "delivery must have no fresh editing input");
            assert!(app.dialog.is_none() && app.session.state.text_edit.is_none() && !app.session.is_dirty());
            assert!(ui.ctx().text_edit_focused(), "focus must survive from the preceding widget frame");
            assert!(check_open(&app, Some(ui.ctx())).is_err(), "refuse before replacing or staging anything");
            assert_eq!(draft, expected);
            assert!(std::sync::Arc::ptr_eq(&project, &app.session.project));
            assert_eq!(app.session.revision, revision);
            assert_eq!(document_id(&app), doc);
            let field = ui.add(egui::TextEdit::singleline(&mut draft).id(id));
            assert!(field.has_focus());
        });
        delivery.textures_delta.clear();
        assert_eq!(draft, expected);
    }
}
