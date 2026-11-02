//! Bounded, lossless transport for a project and the media it references.
//! This is a provider envelope, not a replacement for the native project format.
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_BYTES: usize = 80 * 1024 * 1024;
const MAX_PROJECT: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAGIC: &[u8; 8] = b"ACFPB001";

#[derive(Debug, PartialEq)]
pub struct Bundle {
    pub project: Vec<u8>,
    pub assets: BTreeMap<String, Vec<u8>>,
}

fn crc(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for b in bytes {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = if c & 1 == 0 { c >> 1 } else { 0xedb8_8320 ^ (c >> 1) };
        }
    }
    !c
}

fn valid_path(path: &str) -> bool {
    !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control)
}

impl Bundle {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if self.project.is_empty() || self.project.len() > MAX_PROJECT || self.assets.len() > MAX_FILES {
            return Err("provider bundle exceeds its project or asset limits".into());
        }
        let mut size = 20usize.saturating_add(self.project.len());
        for (path, bytes) in &self.assets {
            if !valid_path(path) {
                return Err("invalid provider asset path".into());
            }
            size = size.saturating_add(10).saturating_add(path.len()).saturating_add(bytes.len());
        }
        if size > MAX_BYTES {
            return Err("project and media exceed the 80 MiB provider limit".into());
        }
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.project.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.assets.len() as u32).to_le_bytes());
        out.extend_from_slice(&crc(&self.project).to_le_bytes());
        out.extend_from_slice(&self.project);
        for (path, bytes) in &self.assets {
            out.extend_from_slice(&(path.len() as u16).to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&crc(bytes).to_le_bytes());
            out.extend_from_slice(path.as_bytes());
            out.extend_from_slice(bytes);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("provider bundle exceeds 80 MiB".into());
        }
        fn take<'a>(b: &mut &'a [u8], len: usize) -> Result<&'a [u8], String> {
            if b.len() < len {
                return Err("truncated provider bundle".into());
            }
            let (head, rest) = b.split_at(len);
            *b = rest;
            Ok(head)
        }
        fn u32le(b: &mut &[u8]) -> Result<usize, String> {
            let a: [u8; 4] = take(b, 4)?.try_into().map_err(|_| "invalid provider length")?;
            Ok(u32::from_le_bytes(a) as usize)
        }
        let mut input = bytes;
        if take(&mut input, 8)? != MAGIC {
            return Err("not a provider project bundle".into());
        }
        let len = u32le(&mut input)?;
        let count = u32le(&mut input)?;
        let checksum = u32le(&mut input)? as u32;
        if len == 0 || len > MAX_PROJECT || count > MAX_FILES {
            return Err("invalid provider bundle limits".into());
        }
        let project = take(&mut input, len)?.to_vec();
        if crc(&project) != checksum {
            return Err("provider project checksum mismatch".into());
        }
        let mut assets = BTreeMap::new();
        for _ in 0..count {
            let path_len: [u8; 2] = take(&mut input, 2)?.try_into().map_err(|_| "invalid provider path length")?;
            let path_len = u16::from_le_bytes(path_len) as usize;
            let len = u32le(&mut input)?;
            let checksum = u32le(&mut input)? as u32;
            if path_len > 4096 {
                return Err("provider asset path exceeds its limit".into());
            }
            let path = std::str::from_utf8(take(&mut input, path_len)?).map_err(|_| "provider asset path is not UTF-8")?.to_string();
            if !valid_path(&path) || assets.contains_key(&path) {
                return Err("invalid or duplicate provider asset path".into());
            }
            let data = take(&mut input, len)?.to_vec();
            if crc(&data) != checksum {
                return Err("provider media checksum mismatch".into());
            }
            assets.insert(path, data);
        }
        if !input.is_empty() {
            return Err("trailing provider bundle data".into());
        }
        Ok(Self { project, assets })
    }

    /// Verify the native project's referenced files and their transitive model dependencies
    /// before a frontend changes its project, virtual files or decoder state.
    pub fn validate_resources(&self, paths: impl IntoIterator<Item = String>) -> Result<(), String> {
        let mut todo: Vec<String> = paths.into_iter().collect();
        let mut checked = BTreeSet::new();
        while let Some(path) = todo.pop() {
            if !checked.insert(path.clone()) {
                continue;
            }
            let bytes = self.assets.get(&path).ok_or_else(|| format!("Provider bundle is missing {path}"))?;
            todo.extend(sidecars(&path, bytes)?);
        }
        Ok(())
    }
}

/// Rebase only native footage references and properties explicitly presented as files.
pub fn rewrite(value: &mut serde_json::Value, replacements: &BTreeMap<String, String>) {
    if let Some(items) = value.get_mut("items").and_then(serde_json::Value::as_object_mut) {
        for item in items.values_mut() {
            if let Some(kind) = item.get_mut("kind")
                && kind.get("type").and_then(serde_json::Value::as_str) == Some("Footage")
            {
                rewrite_footage(kind, replacements);
            }
            if let Some(footage) = item.get_mut("proxy").and_then(|proxy| proxy.get_mut("footage")) {
                rewrite_footage(footage, replacements);
            }
        }
    }
    // Walk containers, not their string contents. A Str is also a valid text control, and a
    // popup's options or saved-view strings are never media references merely by equality.
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::Object(object) => {
                rewrite_property(object, replacements);
                pending.extend(object.values_mut());
            }
            serde_json::Value::Array(array) => pending.extend(array.iter_mut()),
            _ => {}
        }
    }
}

fn rewrite_property(object: &mut serde_json::Map<String, serde_json::Value>, replacements: &BTreeMap<String, String>) {
    if object.get("ui").and_then(|ui| ui.get("kind")).and_then(serde_json::Value::as_str) != Some("File") {
        return;
    }
    if let Some(value) = object.get_mut("value") {
        rewrite_file_value(value, replacements);
    }
    if let Some(keys) = object.get_mut("keys").and_then(serde_json::Value::as_array_mut) {
        for key in keys {
            if let Some(value) = key.get_mut("value") {
                rewrite_file_value(value, replacements);
            }
        }
    }
}

fn rewrite_footage(value: &mut serde_json::Value, replacements: &BTreeMap<String, String>) {
    // Embedded data footage does not refer to the browser media table.
    if value.get("data").is_some_and(|data| !data.is_null()) {
        return;
    }
    if let Some(path) = value.get_mut("path") {
        rewrite_path(path, replacements);
    }
    if let Some(sequence) = value.get_mut("sequence").and_then(serde_json::Value::as_array_mut) {
        for path in sequence {
            rewrite_path(path, replacements);
        }
    }
}

fn rewrite_file_value(value: &mut serde_json::Value, replacements: &BTreeMap<String, String>) {
    if value.get("t").and_then(serde_json::Value::as_str) == Some("Str")
        && let Some(path) = value.get_mut("v")
    {
        rewrite_path(path, replacements);
    }
}

fn rewrite_path(value: &mut serde_json::Value, replacements: &BTreeMap<String, String>) {
    if let serde_json::Value::String(path) = value
        && let Some(staged) = replacements.get(path)
    {
        *path = staged.clone();
    }
}

/// A private browser-table location. No provider identity is interpreted as an OS path.
/// Relative sidecar relationships are retained below this per-open root.
pub fn stage_path(root: &str, path: &str) -> Result<String, String> {
    if !root.starts_with("/__file_provider/") || !valid_path(path) {
        return Err("invalid provider staging path".into());
    }
    let normalized = path.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("provider asset escapes its root".into());
                }
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err("empty provider asset path".into());
    }
    Ok(format!("{root}/{}", parts.join("/")))
}

/// The JSON chunk of a bounded GLB v2 container, as read by the native model loader. Other
/// chunk types are skipped, never decoded or allocated. Invalid lengths fail before staging.
/// Container rules: Khronos glTF 2.0.1, section 4.4.3 (JSON/BIN order and four-byte alignment).
fn glb_json(bytes: &[u8]) -> Result<&[u8], String> {
    fn word(bytes: &[u8], at: usize) -> Result<u32, String> {
        let end = at.checked_add(4).ok_or("invalid GLB chunk offset")?;
        let value: [u8; 4] = bytes.get(at..end).ok_or("truncated GLB header")?.try_into().map_err(|_| "invalid GLB header")?;
        Ok(u32::from_le_bytes(value))
    }
    if bytes.len() > MAX_BYTES || !bytes.starts_with(b"glTF") || word(bytes, 4)? != 2 {
        return Err("invalid or unsupported GLB container".into());
    }
    let total = word(bytes, 8)? as usize;
    if total != bytes.len() || total < 12 {
        return Err("invalid GLB container length".into());
    }
    let mut at = 12usize;
    let mut chunks = 0usize;
    let mut json = None;
    while at < total {
        chunks += 1;
        if chunks > MAX_FILES {
            return Err("too many GLB chunks".into());
        }
        let len = word(bytes, at)? as usize;
        let kind = word(bytes, at.checked_add(4).ok_or("invalid GLB chunk offset")?)?;
        let start = at.checked_add(8).ok_or("invalid GLB chunk offset")?;
        let end = start.checked_add(len).ok_or("invalid GLB chunk length")?;
        let chunk = bytes.get(start..end).ok_or("GLB chunk runs past the end of the file")?;
        if !len.is_multiple_of(4) {
            return Err("GLB chunks must have four-byte alignment".into());
        }
        if kind == 0x4e4f_534a {
            if chunks != 1 || json.is_some() {
                return Err("GLB must start with exactly one JSON chunk".into());
            }
            json = Some(chunk);
        } else if kind == 0x004e_4942 && chunks != 2 {
            return Err("GLB BIN chunk must appear once, immediately after JSON".into());
        }
        at = end;
    }
    json.ok_or_else(|| "GLB without a JSON chunk".into())
}

/// Match the native model loader's percent-decoded relative filenames, then apply the same
/// local-only path policy to the decoded name (encoded traversal is not an escape hatch).
fn model_uri(uri: &str) -> Result<String, String> {
    if uri.len() > 4096 {
        return Err("external model resource URI exceeds its limit".into());
    }
    let mut input = uri.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    while let Some((&byte, rest)) = input.split_first() {
        if byte == b'%'
            && let Some(hex) = rest.get(..2)
            && hex.iter().all(u8::is_ascii_hexdigit)
            && let Ok(text) = std::str::from_utf8(hex)
            && let Ok(value) = u8::from_str_radix(text, 16)
        {
            decoded.push(value);
            input = rest.get(2..).ok_or("invalid model resource URI")?;
            continue;
        }
        decoded.push(byte);
        input = rest;
    }
    let path = String::from_utf8_lossy(&decoded).into_owned();
    if !valid_path(&path) {
        return Err("invalid external model resource URI".into());
    }
    Ok(path)
}

fn collect_gltf_sidecars(json: &[u8], join: &impl Fn(&str) -> Result<String, String>) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("invalid glTF sidecars: {e}"))?;
    for key in ["buffers", "images"] {
        for v in value.get(key).and_then(serde_json::Value::as_array).into_iter().flatten() {
            if let Some(uri) = v.get("uri").and_then(serde_json::Value::as_str) {
                if uri.starts_with("data:") {
                    continue;
                }
                let decoded = model_uri(uri)?;
                if decoded.starts_with("data:") {
                    return Err("encoded model resource scheme is unsupported".into());
                }
                let p = join(&decoded)?;
                if !p.is_empty() {
                    out.insert(p);
                    if out.len() > MAX_FILES {
                        return Err("too many model sidecars".into());
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Expand supported 3D/LUT sidecars without asking the provider to interpret project data.
/// Unknown sidecar formats fail closed instead of silently producing broken saved projects.
pub fn sidecars(path: &str, bytes: &[u8]) -> Result<Vec<String>, String> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
    let join = |p: &str| -> Result<String, String> {
        if p.starts_with("data:") {
            return Ok(String::new());
        }
        if p.contains("://") || p.starts_with('/') || p.contains('\\') || p.contains('?') || p.contains('#') || p.split('/').any(|segment| segment == "..") {
            return Err("external model resource URI is unsupported".into());
        }
        let relative = p.split('/').filter(|part| !matches!(*part, "" | ".")).collect::<Vec<_>>().join("/");
        if relative.is_empty() {
            return Err("empty external model resource path".into());
        }
        Ok(if parent.is_empty() { relative } else { format!("{parent}/{relative}") })
    };
    let mut out = BTreeSet::new();
    if matches!(ext.as_str(), "gltf" | "glb") {
        if bytes.len() > MAX_BYTES {
            return Err("model exceeds the provider media limit".into());
        }
        let json = if ext == "glb" { glb_json(bytes)? } else { bytes };
        out = collect_gltf_sidecars(json, &join)?;
    } else if matches!(ext.as_str(), "obj" | "mtl") {
        let text = std::str::from_utf8(bytes).map_err(|_| "3D sidecar is not UTF-8")?;
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let kind = words.next().unwrap_or("");
            if kind == "mtllib" || kind.starts_with("map_") || matches!(kind, "bump" | "disp" | "decal" | "norm") {
                let p = words.collect::<Vec<_>>().join(" ");
                if p.starts_with('-') {
                    return Err("OBJ material map options are unsupported in provider bundles".into());
                }
                if !p.is_empty() {
                    out.insert(join(&p)?);
                }
            }
        }
    } else if matches!(ext.as_str(), "ocio" | "c4d" | "fbx") {
        return Err("this external-resource format cannot yet be packaged losslessly".into());
    }
    Ok(out.into_iter().collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn glb(json: &[u8]) -> Vec<u8> {
        let mut json = json.to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut bytes = b"glTF".to_vec();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&((20 + json.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0x4e4f_534au32.to_le_bytes());
        bytes.extend_from_slice(&json);
        bytes
    }
    #[test]
    fn project_and_media_round_trip_and_corruption_is_rejected() {
        let b = Bundle { project: br#"{"native":true}"#.to_vec(), assets: BTreeMap::from([("/picked/movie.bin".into(), vec![1, 0, 255, 7])]) };
        let bytes = b.encode().unwrap();
        assert_eq!(Bundle::decode(&bytes).unwrap(), b);
        let mut corrupt = bytes.clone();
        let n = corrupt.len();
        corrupt[n - 1] ^= 1;
        assert!(Bundle::decode(&corrupt).is_err());
        for n in 0..bytes.len() {
            assert!(Bundle::decode(&bytes[..n]).is_err());
        }
    }
    #[test]
    fn provider_media_rebase_preserves_native_text_and_only_changes_file_controls() {
        use effectcraft_color::Label;
        use effectcraft_project::{Comp, Footage, FootageKind, ItemKind, Keyframe, LayerSource, Node, ParamUi, Project, Property, Proxy, SavedView, Value};
        use effectcraft_time::{FrameRate, Tick};
        use std::sync::Arc;

        let path = "/picked/image_0001.png";
        let next = "/picked/image_0002.png";
        let rate = FrameRate::FPS_25;
        let footage = Footage {
            path: path.into(),
            kind: FootageKind::Sequence,
            width: 1,
            height: 1,
            has_video: true,
            frame_rate: rate,
            duration: rate.tick_of(2),
            sequence: vec![path.into(), next.into()],
            ..Default::default()
        };
        let mut project = Project::default();
        let media_id = project.add_item("Image sequence", Label::Blue, None, ItemKind::Footage(footage.clone()));
        project.item_mut(media_id).unwrap().comment = path.into();
        project.item_mut(media_id).unwrap().proxy = Some(Box::new(Proxy { footage, enabled: true }));
        let mut comp = Comp::new(1, 1, rate, rate.tick_of(2));
        let mut layer = effectcraft_project::build::layer(&mut project, &comp, "File and text controls", LayerSource::Footage { item: media_id }, (1, 1), None);
        let mut text = Property::new(project.alloc(), "literal", "Literal text", Value::Str(path.into())).with_ui(ParamUi::Text);
        text.keys.push(Keyframe::new(Tick::ZERO, Value::Str(next.into())));
        let mut file = Property::new(project.alloc(), "file", "File", Value::Str(path.into())).with_ui(ParamUi::File { filters: vec!["png".into()] });
        file.keys.push(Keyframe::new(Tick::ZERO, Value::Str(next.into())));
        let options = Property::new(project.alloc(), "options", "Options", Value::Scalar(1.0)).with_ui(ParamUi::Popup { options: vec![path.into()] });
        layer.props.children.extend([Node::Prop(text), Node::Prop(file), Node::Prop(options)]);
        comp.layers.push(layer);
        let comp_id = project.add_item("Composition", Label::Blue, None, ItemKind::Comp(Arc::new(comp)));
        // The media is generated in this fixture, not a mislabeled or truncated placeholder file.
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();
        let bundle = Bundle {
            project: project.to_file_json_with_view(&SavedView::default(), false).unwrap().into_bytes(),
            assets: BTreeMap::from([(path.into(), bytes.clone()), (next.into(), bytes)]),
        };
        let decoded = Bundle::decode(&bundle.encode().unwrap()).unwrap();
        let staged = stage_path("/__file_provider/open-text", path).unwrap();
        let staged_next = stage_path("/__file_provider/open-text", next).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&decoded.project).unwrap();
        rewrite(&mut json, &BTreeMap::from([(path.into(), staged.clone()), (next.into(), staged_next.clone())]));
        let restored = Project::from_json(&serde_json::to_string(&json).unwrap()).unwrap();
        let item = restored.item(media_id).unwrap();
        let ItemKind::Footage(media) = &item.kind else { panic!("expected footage") };
        assert_eq!(media.path, staged);
        assert_eq!(media.sequence, vec![staged.clone(), staged_next.clone()]);
        assert_eq!(item.proxy.as_ref().unwrap().footage.path, staged);
        assert_eq!(item.comment, path);
        let layer = restored.comp(comp_id).unwrap().layers.first().unwrap();
        let property = |id: &str| layer.props.children.iter().filter_map(Node::as_prop).find(|p| p.match_id == id).unwrap();
        assert_eq!(property("literal").value, Value::Str(path.into()));
        assert_eq!(property("literal").keys.first().unwrap().value, Value::Str(next.into()));
        assert_eq!(property("file").value, Value::Str(staged));
        assert_eq!(property("file").keys.first().unwrap().value, Value::Str(staged_next));
        assert_eq!(property("options").ui, ParamUi::Popup { options: vec![path.into()] });
        assert_eq!(image::load_from_memory(decoded.assets.get(path).unwrap()).unwrap().width(), 1);
    }

    #[test]
    fn staging_is_private_and_rejects_escape() {
        assert_eq!(stage_path("/__file_provider/page-1", "/files/a/../b.png").unwrap(), "/__file_provider/page-1/files/b.png");
        assert!(stage_path("/__file_provider/page-1", "../../secret").is_err());
    }
    #[test]
    fn model_sidecars_are_kept_and_network_dependencies_are_refused() {
        assert_eq!(
            sidecars("/files/scene/model.gltf", br#"{"buffers":[{"uri":"mesh.bin"}],"images":[{"uri":"textures/albedo.png"}]}"#).unwrap(),
            vec!["/files/scene/mesh.bin", "/files/scene/textures/albedo.png"]
        );
        assert!(sidecars("/files/a.gltf", br#"{"images":[{"uri":"https://example.invalid/image"}]}"#).is_err());
    }

    #[test]
    fn glb_external_buffers_and_images_are_collected_with_native_uri_decoding() {
        let json = br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"./mesh.bin","byteLength":12}],"images":[{"uri":"textures/./albedo%20map.png"},{"uri":"data:image/png;base64,AA=="}]}"#;
        let expected = vec!["/files/scene/mesh.bin", "/files/scene/textures/albedo map.png"];
        assert_eq!(sidecars("/files/scene/model.glb", &glb(json)).unwrap(), expected);
        assert_eq!(sidecars("/files/scene/model.gltf", json).unwrap(), expected);
        assert!(sidecars("/files/scene/model.glb", &glb(br#"{"images":[{"uri":"%2e%2e/secret.png"}]}"#)).is_err());
        assert!(sidecars("/files/scene/model.glb", &glb(br#"{"images":[{"uri":"https://example.invalid/texture.png"}]}"#)).is_err());
        assert!(sidecars("/files/scene/model.glb", &glb(br#"{"images":[{"uri":"data%3Aexternal.png"}]}"#)).is_err());
        let mut embedded = glb(br#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4}]}"#);
        embedded.extend_from_slice(&4u32.to_le_bytes());
        embedded.extend_from_slice(&0x004e_4942u32.to_le_bytes());
        embedded.extend_from_slice(&[1, 2, 3, 4]);
        let length = embedded.len() as u32;
        embedded[8..12].copy_from_slice(&length.to_le_bytes());
        assert!(sidecars("/files/scene/embedded.glb", &embedded).unwrap().is_empty());
    }

    #[test]
    fn glb_missing_sidecars_fail_the_same_preflight_used_by_provider_open() {
        let path = "/files/scene/model.glb";
        let model = glb(br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"mesh.bin","byteLength":4}],"images":[{"uri":"albedo.png"}]}"#);
        let mut bundle = Bundle { project: b"{}".to_vec(), assets: BTreeMap::from([(path.into(), model)]) };
        let validate = |bundle: &Bundle| bundle.validate_resources([path.to_string()]);
        assert!(validate(&bundle).unwrap_err().contains("missing /files/scene/mesh.bin"));
        bundle.assets.insert("/files/scene/mesh.bin".into(), vec![1, 2, 3, 4]);
        assert!(validate(&bundle).unwrap_err().contains("missing /files/scene/albedo.png"));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1, 1, image::Rgba([17, 39, 81, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        bundle.assets.insert("/files/scene/albedo.png".into(), png.into_inner());
        let restored = Bundle::decode(&bundle.encode().unwrap()).unwrap();
        validate(&restored).unwrap();
        assert_eq!(restored, bundle, "all external resource bytes survive the envelope");
        let root = "/__file_provider/glb-roundtrip";
        assert_eq!(stage_path(root, path).unwrap(), format!("{root}/files/scene/model.glb"));
        for sidecar in sidecars(path, restored.assets.get(path).unwrap()).unwrap() {
            assert!(stage_path(root, &sidecar).unwrap().starts_with(&format!("{root}/files/scene/")));
        }
    }

    #[test]
    fn malformed_glb_chunks_fail_before_dependency_restoration() {
        let bytes = glb(br#"{"asset":{"version":"2.0"}}"#);
        for end in 0..bytes.len() {
            assert!(sidecars("/files/truncated.glb", &bytes[..end]).is_err(), "accepted prefix {end}");
        }
        let mut oversized_chunk = bytes.clone();
        oversized_chunk[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(sidecars("/files/oversized.glb", &oversized_chunk).is_err());
        let mut wrong_version = bytes.clone();
        wrong_version[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(sidecars("/files/version.glb", &wrong_version).is_err());
        let mut no_json = bytes.clone();
        no_json[16..20].copy_from_slice(&0x004e_4942u32.to_le_bytes());
        assert!(sidecars("/files/no-json.glb", &no_json).is_err());
        let mut duplicate = bytes.clone();
        duplicate.extend_from_slice(&bytes[12..]);
        let length = duplicate.len() as u32;
        duplicate[8..12].copy_from_slice(&length.to_le_bytes());
        let mut unaligned = bytes.clone();
        unaligned.push(b' ');
        let length = unaligned.len() as u32;
        unaligned[8..12].copy_from_slice(&length.to_le_bytes());
        unaligned[12..16].copy_from_slice(&(length - 20).to_le_bytes());
        let mut bin_first = bytes[..12].to_vec();
        bin_first.extend_from_slice(&4u32.to_le_bytes());
        bin_first.extend_from_slice(&0x004e_4942u32.to_le_bytes());
        bin_first.extend_from_slice(&[0; 4]);
        bin_first.extend_from_slice(&bytes[12..]);
        let length = bin_first.len() as u32;
        bin_first[8..12].copy_from_slice(&length.to_le_bytes());
        for malformed in [duplicate, unaligned, bin_first] {
            assert!(sidecars("/files/layout.glb", &malformed).is_err(), "invalid GLB chunk layout was accepted");
        }
    }
}
