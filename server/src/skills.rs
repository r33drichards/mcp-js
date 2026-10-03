//! Startup snapshot of operator-supplied Agent Skills. Only registered files
//! are addressable; resource URIs are never translated into filesystem paths.
use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::{ErrorData, model::*};
use rmcp_skills::model::skills::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;
const PAGE_SIZE: usize = 100;

#[derive(Clone, Default)]
pub struct SkillCatalog {
    skills: BTreeMap<String, SkillEntry>,
    files: BTreeMap<String, Vec<u8>>,
}

impl SkillCatalog {
    /// Load a directory containing skill folders (including nested folders).
    /// Symlinks are rejected rather than publishing files outside those folders.
    pub fn load(root: &Path) -> Result<Self> {
        ensure!(
            root.is_dir(),
            "skills directory is not a directory: {}",
            root.display()
        );
        let mut catalog = Self::default();
        let mut files = BTreeMap::new();
        let mut total = 0;
        collect_files(root, root, &mut files, &mut total)?;
        for (path, bytes) in &files {
            if !path.ends_with("/SKILL.md") && path != "SKILL.md" {
                continue;
            }
            let (skill_path, _) = path
                .rsplit_once('/')
                .context("put SKILL.md inside a named skill folder, not at the skills root")?;
            let text =
                std::str::from_utf8(bytes).with_context(|| format!("{path} is not UTF-8"))?;
            let normalized = text.replace("\r\n", "\n");
            let yaml = normalized
                .strip_prefix("---\n")
                .with_context(|| format!("{path} must start with YAML frontmatter"))?;
            let end = yaml
                .lines()
                .scan(0, |offset, line| {
                    let start = *offset;
                    *offset += line.len() + 1;
                    Some((start, line))
                })
                .find(|(_, line)| *line == "---")
                .map(|(offset, _)| offset)
                .with_context(|| format!("{path} has no closing frontmatter delimiter"))?;
            let frontmatter: Value = serde_yaml::from_str(&yaml[..end])
                .with_context(|| format!("invalid YAML in {path}"))?;
            let name = frontmatter
                .get("name")
                .and_then(Value::as_str)
                .with_context(|| format!("{path} requires a string name"))?;
            let description = frontmatter
                .get("description")
                .and_then(Value::as_str)
                .with_context(|| format!("{path} requires a string description"))?;
            ensure!(
                valid_name(name) && skill_path.rsplit('/').next() == Some(name),
                "{path}: name must match its folder and use lowercase letters, digits and single hyphens (1–64 characters)"
            );
            ensure!(
                !description.trim().is_empty() && description.chars().count() <= 1024,
                "{path}: description must contain 1–1024 characters"
            );
            let prefix = format!("{skill_path}/");
            let resources = files
                .iter()
                .filter(|(p, _)| p.starts_with(&prefix))
                .map(|(p, data)| {
                    let uri = format!("skill://{p}");
                    catalog.files.insert(uri.clone(), data.clone());
                    SkillResource::new(
                        uri,
                        format!("sha256:{:x}", Sha256::digest(data)),
                        data.len() as u64,
                    )
                })
                .collect();
            let uri = format!("skill://{path}");
            let mut skill = SkillEntry::new(&uri, frontmatter);
            skill.resources = Some(SkillResources::FileList(resources));
            catalog.skills.insert(uri, skill);
        }
        Ok(catalog)
    }

    /// Bridge the branch's typed extension messages into the stable SDK's
    /// extension hook; this avoids changing the transport and tasks lifecycle.
    pub fn request(&self, request: CustomRequest) -> Result<CustomResult, ErrorData> {
        if self.is_empty() {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                "Skills extension is not configured",
                None,
            ));
        }
        let value = match request.method.as_str() {
            "skills/list" => {
                let params: Option<rmcp_skills::model::PaginatedRequestParams> = request
                    .params
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|_| ErrorData::invalid_params("Invalid skills/list params", None))?;
                serde_json::to_value(self.list(params.as_ref().and_then(|p| p.cursor.as_deref()))?)
            }
            "skills/get" => {
                let params: SkillsGetRequestParams = serde_json::from_value(
                    request.params.unwrap_or(Value::Null),
                )
                .map_err(|_| ErrorData::invalid_params("skills/get requires a string uri", None))?;
                serde_json::to_value(self.get(&params.uri)?)
            }
            _ => {
                return Err(ErrorData::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    "Unknown method",
                    None,
                ));
            }
        }
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        // resultType/cache fields belong to the newer base protocol. This
        // server negotiates 2025-11-25, so send the extension's legacy shape.
        let mut value = value;
        if let Some(object) = value.as_object_mut() {
            object.remove("resultType");
            object.remove("ttlMs");
            object.remove("cacheScope");
        }
        Ok(CustomResult(value))
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn list(&self, cursor: Option<&str>) -> Result<SkillsListResult, ErrorData> {
        let start = match cursor {
            None => 0,
            Some(cursor) => {
                let offset = cursor.parse::<usize>().map_err(|_| invalid_cursor())?;
                if offset == 0 || offset >= self.skills.len() || offset % PAGE_SIZE != 0 {
                    return Err(invalid_cursor());
                }
                offset
            }
        };
        let mut result = SkillsListResult::new(
            self.skills
                .values()
                .skip(start)
                .take(PAGE_SIZE)
                .cloned()
                .collect(),
        );
        if start + PAGE_SIZE < self.skills.len() {
            result.next_cursor = Some((start + PAGE_SIZE).to_string());
        }
        result.ttl_ms = Some(0);
        result.cache_scope = Some("private".into());
        Ok(result)
    }

    pub fn get(&self, uri: &str) -> Result<SkillsGetResult, ErrorData> {
        self.skills
            .get(uri)
            .cloned()
            .map(SkillsGetResult::new)
            .ok_or_else(|| ErrorData::invalid_params("Unknown skill URI", None))
    }

    pub fn resources(&self) -> Vec<Resource> {
        self.skills
            .values()
            .map(|skill| {
                let mut resource =
                    RawResource::new(&skill.uri, skill.frontmatter["name"].as_str().unwrap());
                resource.description = skill.frontmatter["description"].as_str().map(str::to_owned);
                resource.mime_type = Some("text/markdown".into());
                resource.no_annotation()
            })
            .collect()
    }

    pub fn read(&self, uri: &str) -> Option<ReadResourceResult> {
        let bytes = self.files.get(uri)?;
        let content = match std::str::from_utf8(bytes) {
            Ok(text) => ResourceContents::text(text, uri),
            Err(_) => ResourceContents::blob(STANDARD.encode(bytes), uri),
        };
        Some(ReadResourceResult::new(vec![content]))
    }

    pub fn advertise(&self, capabilities: &mut ServerCapabilities) {
        if !self.is_empty() {
            capabilities
                .extensions
                .get_or_insert_with(Default::default)
                .insert("io.modelcontextprotocol/skills".into(), Default::default());
        }
    }
}

fn invalid_cursor() -> ErrorData {
    ErrorData::invalid_params("Invalid skills cursor", None)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn collect_files(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    total: &mut usize,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        ensure!(
            !kind.is_symlink(),
            "symlinks are not supported in skills: {}",
            path.display()
        );
        if kind.is_dir() {
            collect_files(root, &path, files, total)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(root)?;
            let segments: Vec<_> = relative
                .iter()
                .map(|s| s.to_str().context("skill paths must be UTF-8"))
                .collect::<Result<_>>()?;
            ensure!(
                segments.iter().all(|s| s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))),
                "skill paths must use URI-safe letters, digits, hyphens, underscores and dots: {}",
                path.display()
            );
            ensure!(
                entry.metadata()?.len() <= (MAX_CATALOG_BYTES - *total) as u64,
                "skills catalog exceeds 64 MiB"
            );
            let bytes = fs::read(&path)?;
            *total += bytes.len();
            ensure!(*total <= MAX_CATALOG_BYTES, "skills catalog exceeds 64 MiB");
            files.insert(segments.join("/"), bytes);
        } else {
            bail!("unsupported file type in skills: {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("acme/refunds/references")).unwrap();
        fs::write(dir.path().join("acme/refunds/SKILL.md"), "---\nname: refunds\ndescription: |\n  Process refunds safely.\nmetadata:\n  version: '1'\n---\nRead references/rules.md.\n").unwrap();
        fs::write(
            dir.path().join("acme/refunds/references/rules.md"),
            "Ask for the order ID.",
        )
        .unwrap();
        fs::write(dir.path().join("acme/refunds/image.bin"), [0xff, 0x00]).unwrap();
        fs::write(dir.path().join("private.txt"), "not a skill file").unwrap();
        dir
    }

    #[test]
    fn manifests_include_entrypoint_nested_files_and_exact_bytes() {
        let dir = fixture();
        let catalog = SkillCatalog::load(dir.path()).unwrap();
        let listed = catalog.list(None).unwrap();
        assert_eq!(listed.skills.len(), 1);
        let skill = &listed.skills[0];
        assert_eq!(skill.frontmatter["metadata"]["version"], "1");
        let files = skill.resources.as_ref().unwrap().as_files().unwrap();
        assert_eq!(files.len(), 3);
        for file in files {
            let bytes = &catalog.files[&file.uri];
            assert_eq!(file.size, bytes.len() as u64);
            assert_eq!(file.digest, format!("sha256:{:x}", Sha256::digest(bytes)));
        }
        assert_eq!(catalog.get(&skill.uri).unwrap().skill, *skill);
        let binary =
            serde_json::to_value(catalog.read("skill://acme/refunds/image.bin").unwrap()).unwrap();
        assert_eq!(binary["contents"][0]["blob"], "/wA=");
        assert!(catalog.read("skill://private.txt").is_none());
        assert!(
            catalog
                .read("skill://acme/refunds/../../private.txt")
                .is_none()
        );
        fs::write(
            dir.path().join("acme/refunds/references/rules.md"),
            "changed",
        )
        .unwrap();
        assert_eq!(
            catalog.files["skill://acme/refunds/references/rules.md"],
            b"Ask for the order ID."
        );
    }

    #[test]
    fn extension_dispatch_rejects_invalid_params_and_unknown_methods() {
        let dir = fixture();
        let catalog = SkillCatalog::load(dir.path()).unwrap();
        let request = |method: &str, params: Value| CustomRequest {
            method: method.into(),
            params: Some(params),
            extensions: Default::default(),
        };
        let result = catalog
            .request(request("skills/list", json!({})))
            .unwrap()
            .0;
        assert_eq!(result["skills"][0]["uri"], "skill://acme/refunds/SKILL.md");
        assert!(result.get("resultType").is_none());
        assert!(
            catalog
                .request(request("skills/list", json!({"cursor": "bad"})))
                .is_err()
        );
        assert!(
            catalog
                .request(request("skills/get", json!({"uri": 4})))
                .is_err()
        );
        assert!(
            catalog
                .request(request(
                    "skills/get",
                    json!({"uri": "skill://unknown/SKILL.md"})
                ))
                .is_err()
        );
        assert_eq!(
            catalog
                .request(request("other/method", json!({})))
                .unwrap_err()
                .code,
            ErrorCode::METHOD_NOT_FOUND
        );
        let mut caps = ServerCapabilities::default();
        catalog.advertise(&mut caps);
        assert_eq!(
            serde_json::to_value(caps).unwrap()["extensions"]["io.modelcontextprotocol/skills"],
            json!({})
        );
        assert!(
            SkillCatalog::default()
                .request(request("skills/list", json!({})))
                .is_err()
        );
    }

    #[test]
    fn rejects_invalid_frontmatter() {
        let dir = fixture();
        let path = dir.path().join("acme/refunds/SKILL.md");
        for text in [
            "name: refunds",
            "---\nname: refunds\ndescription: test\n",
            "---\nname: wrong\ndescription: test\n---\n",
            "---\nname: refunds\ndescription: ''\n---\n",
        ] {
            fs::write(&path, text).unwrap();
            assert!(SkillCatalog::load(dir.path()).is_err(), "accepted {text:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_files_and_directories() {
        let dir = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("acme/refunds/escape")).unwrap();
        assert!(SkillCatalog::load(dir.path()).is_err());
        fs::remove_file(dir.path().join("acme/refunds/escape")).unwrap();
        std::os::unix::fs::symlink(
            "references/rules.md",
            dir.path().join("acme/refunds/link.md"),
        )
        .unwrap();
        assert!(SkillCatalog::load(dir.path()).is_err());
    }

    #[test]
    fn paginates_catalog_without_skipping_entries() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..101 {
            let name = format!("skill-{i}");
            fs::create_dir(dir.path().join(&name)).unwrap();
            fs::write(
                dir.path().join(name.clone()).join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Example\n---\n"),
            )
            .unwrap();
        }
        let catalog = SkillCatalog::load(dir.path()).unwrap();
        let first = catalog.list(None).unwrap();
        assert_eq!(first.skills.len(), 100);
        let second = catalog.list(first.next_cursor.as_deref()).unwrap();
        assert_eq!(second.skills.len(), 1);
        assert!(second.next_cursor.is_none());
        assert!(catalog.list(Some("101")).is_err());
    }
}
