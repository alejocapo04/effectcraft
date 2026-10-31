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
}

/// Rebase media references and file properties without touching document text.
pub fn rewrite(value: &mut serde_json::Value, replacements: &BTreeMap<String, String>) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(p) = replacements.get(s) {
                *s = p.clone();
            }
        }
        serde_json::Value::Array(a) => {
            for v in a {
                rewrite(v, replacements);
            }
        }
        serde_json::Value::Object(o) => {
            if o.get("t").and_then(serde_json::Value::as_str) == Some("Str") {
                if let Some(v) = o.get_mut("v") {
                    rewrite(v, replacements);
                }
            }
            for (key, v) in o.iter_mut() {
                if matches!(key.as_str(), "path" | "sequence") {
                    rewrite(v, replacements);
                } else if v.is_object() || v.is_array() {
                    rewrite(v, replacements);
                }
            }
        }
        _ => {}
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

/// Expand textual 3D/LUT sidecars without asking the provider to interpret project data.
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
        Ok(if parent.is_empty() { p.into() } else { format!("{parent}/{p}") })
    };
    let mut out = BTreeSet::new();
    if ext == "gltf" {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("invalid glTF sidecars: {e}"))?;
        for key in ["buffers", "images"] {
            for v in value.get(key).and_then(serde_json::Value::as_array).into_iter().flatten() {
                if let Some(uri) = v.get("uri").and_then(serde_json::Value::as_str) {
                    let p = join(uri)?;
                    if !p.is_empty() {
                        out.insert(p);
                    }
                }
            }
        }
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
}
