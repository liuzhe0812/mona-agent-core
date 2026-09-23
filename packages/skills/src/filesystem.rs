use crate::{
    cancelled, check_cancel, error, valid_name, validate_summary, Limits, SkillDefinition,
    SkillProvider, SkillSummary,
};
use api::{async_trait, CancellationToken, ErrorCode, Result};
use serde_yaml_ng::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Path, PathBuf},
};
use tokio::{
    fs::{self, DirEntry, File},
    io::AsyncReadExt,
};

/// A bounded local filesystem implementation of [`SkillProvider`].
///
/// The host supplies the complete set of roots. This provider never adds
/// conventional user/project directories on its own, so mounting it does not
/// accidentally widen the host's filesystem policy.
pub struct LocalSkills {
    id: String,
    roots: Vec<PathBuf>,
    limits: Limits,
}

#[derive(Clone, Debug)]
struct LocatedSkill {
    summary: SkillSummary,
    content: String,
    location: SkillLocation,
}

#[derive(Clone, Debug)]
enum SkillLocation {
    Bundle {
        directory: PathBuf,
        canonical_directory: PathBuf,
    },
    Flat,
}

#[derive(Clone, Debug)]
struct RootEntry {
    path: PathBuf,
    name: String,
    kind: EntryKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl LocalSkills {
    pub fn new(id: impl Into<String>, roots: Vec<PathBuf>, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let id = id.into();
        if !valid_name(&id) {
            return Err(error(ErrorCode::Configuration, "invalid skill provider ID"));
        }
        let cwd = std::env::current_dir().map_err(|_| {
            error(
                ErrorCode::Configuration,
                "cannot resolve the current directory",
            )
        })?;
        let roots = roots
            .into_iter()
            .map(|root| {
                if root.is_absolute() {
                    root
                } else {
                    cwd.join(root)
                }
            })
            .collect();
        Ok(Self { id, roots, limits })
    }

    async fn scan(&self, cancel: &CancellationToken) -> Result<Vec<LocatedSkill>> {
        check_cancel(cancel)?;
        let mut selected = BTreeMap::<String, LocatedSkill>::new();
        for root in &self.roots {
            check_cancel(cancel)?;
            let known_names: BTreeSet<String> = selected.keys().cloned().collect();
            for skill in self.scan_root(root, &known_names, cancel).await? {
                check_cancel(cancel)?;
                selected.entry(skill.summary.name.clone()).or_insert(skill);
            }
        }
        let mut skills: Vec<_> = selected.into_values().collect();
        skills.sort_by(|left, right| left.summary.name.cmp(&right.summary.name));
        let summaries: Vec<_> = skills.iter().map(|skill| skill.summary.clone()).collect();
        let catalog = serde_json::to_vec(&summaries)
            .map_err(|_| error(ErrorCode::Tool, "cannot encode skill catalog"))?;
        if catalog.len() > self.limits.max_catalog_bytes {
            return Err(error(
                ErrorCode::Limit,
                "skill catalog exceeds its byte limit",
            ));
        }
        check_cancel(cancel)?;
        Ok(skills)
    }

    async fn scan_root(
        &self,
        root: &Path,
        known_names: &BTreeSet<String>,
        cancel: &CancellationToken,
    ) -> Result<Vec<LocatedSkill>> {
        check_cancel(cancel)?;
        let Some(metadata) = optional_io(
            cancel,
            fs::symlink_metadata(root),
            "cannot inspect skill root",
        )
        .await?
        else {
            // Windows can report NotFound for a child of a regular file. Only a
            // missing directory chain is an empty root, not an invalid ancestor.
            for ancestor in root.ancestors().skip(1) {
                if let Some(parent) = optional_io(
                    cancel,
                    fs::symlink_metadata(ancestor),
                    "cannot inspect skill root ancestor",
                )
                .await?
                {
                    if !parent.is_dir() {
                        return Err(error(
                            ErrorCode::Unsupported,
                            "skill root ancestor is not a directory",
                        ));
                    }
                    break;
                }
            }
            return Ok(Vec::new());
        };
        if !metadata.is_dir() {
            return Err(error(
                ErrorCode::Unsupported,
                "skill root is not a directory",
            ));
        }
        let canonical_root =
            io_call(cancel, fs::canonicalize(root), "cannot resolve skill root").await?;
        let mut entries = Vec::new();
        let mut directory = io_call(cancel, fs::read_dir(root), "cannot read skill root").await?;
        loop {
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(cancelled()),
                result = directory.next_entry() => result,
            }
            .map_err(|_| error(ErrorCode::Tool, "cannot enumerate skill root"))?;
            let Some(entry) = next else { break };
            if entries.len() >= self.limits.max_entries_per_root {
                return Err(error(
                    ErrorCode::Limit,
                    "skill root contains too many entries",
                ));
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = self.entry_kind(&entry, cancel).await?;
            entries.push(RootEntry { path, name, kind });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));

        let mut result = Vec::new();
        let mut root_names = BTreeSet::new();
        for entry in entries {
            check_cancel(cancel)?;
            if entry.name.starts_with('.') {
                continue;
            }
            let skill = match entry.kind {
                EntryKind::Directory => {
                    let canonical_directory =
                        secure_directory(&entry.path, &canonical_root, cancel).await?;
                    let file = canonical_directory.join("SKILL.md");
                    let Some(file) = secure_file(&file, &canonical_directory, cancel).await? else {
                        continue;
                    };
                    self.parse_skill(
                        &file,
                        SkillLocation::Bundle {
                            directory: entry.path,
                            canonical_directory,
                        },
                        cancel,
                    )
                    .await?
                }
                EntryKind::File if entry.name.ends_with(".md") => {
                    let Some(file) = secure_file(&entry.path, &canonical_root, cancel).await?
                    else {
                        continue;
                    };
                    self.parse_skill(&file, SkillLocation::Flat, cancel).await?
                }
                EntryKind::Symlink => {
                    return Err(error(
                        ErrorCode::Unsupported,
                        "symbolic links are not allowed in skill roots",
                    ));
                }
                EntryKind::Other | EntryKind::File => continue,
            };
            if known_names.contains(&skill.summary.name)
                || !root_names.insert(skill.summary.name.clone())
            {
                continue;
            }
            if known_names.len() + root_names.len() > self.limits.max_skills {
                return Err(error(
                    ErrorCode::Limit,
                    "skill catalog exceeds its entry limit",
                ));
            }
            result.push(skill);
        }
        Ok(result)
    }

    async fn entry_kind(&self, entry: &DirEntry, cancel: &CancellationToken) -> Result<EntryKind> {
        let file_type = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            result = entry.file_type() => result,
        }
        .map_err(|_| error(ErrorCode::Tool, "cannot inspect skill entry"))?;
        if file_type.is_symlink() {
            return Ok(EntryKind::Symlink);
        }
        if file_type.is_dir() {
            return Ok(EntryKind::Directory);
        }
        if file_type.is_file() {
            return Ok(EntryKind::File);
        }
        Ok(EntryKind::Other)
    }

    async fn parse_skill(
        &self,
        file: &Path,
        location: SkillLocation,
        cancel: &CancellationToken,
    ) -> Result<LocatedSkill> {
        let raw = read_bounded_text(file, self.limits.max_file_bytes, cancel).await?;
        let (mut summary, content) = parse_document(&raw, &self.limits)?;
        summary.location = Some(file.to_string_lossy().into_owned());
        check_cancel(cancel)?;
        Ok(LocatedSkill {
            summary,
            content,
            location,
        })
    }
}

#[async_trait]
impl SkillProvider for LocalSkills {
    fn id(&self) -> &str {
        &self.id
    }

    async fn list(&self, cancel: CancellationToken) -> Result<Vec<SkillSummary>> {
        Ok(self
            .scan(&cancel)
            .await?
            .into_iter()
            .map(|skill| skill.summary)
            .collect())
    }

    async fn load(&self, name: &str, cancel: CancellationToken) -> Result<Option<SkillDefinition>> {
        if !valid_name(name) {
            return Ok(None);
        }
        let skill = self
            .scan(&cancel)
            .await?
            .into_iter()
            .find(|skill| skill.summary.name == name);
        Ok(skill.map(|skill| SkillDefinition {
            summary: skill.summary,
            content: skill.content,
        }))
    }

    async fn read_resource(
        &self,
        name: &str,
        path: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        if !valid_name(name) {
            return Err(error(ErrorCode::Tool, "invalid skill name"));
        }
        let skill = self
            .scan(&cancel)
            .await?
            .into_iter()
            .find(|skill| skill.summary.name == name)
            .ok_or_else(|| error(ErrorCode::Tool, "skill is unknown or no longer available"))?;
        if !skill.summary.model_invocable {
            return Err(error(
                ErrorCode::Policy,
                "skill is not available for model invocation",
            ));
        }
        let SkillLocation::Bundle {
            directory,
            canonical_directory,
        } = skill.location
        else {
            return Err(error(
                ErrorCode::Unsupported,
                "flat skills do not expose resources",
            ));
        };
        let relative = resource_path(path)?;
        let current_directory = io_call(
            &cancel,
            fs::canonicalize(&directory),
            "cannot resolve skill bundle",
        )
        .await?;
        if current_directory != canonical_directory {
            return Err(error(
                ErrorCode::Unsupported,
                "skill bundle changed while loading resource",
            ));
        }
        let resource = canonical_directory.join(relative);
        let Some(resource) = secure_file(&resource, &canonical_directory, &cancel).await? else {
            return Err(error(ErrorCode::Tool, "skill resource is unavailable"));
        };
        read_bounded_text(
            &resource,
            self.limits
                .max_file_bytes
                .min(self.limits.max_content_bytes),
            &cancel,
        )
        .await
    }
}

async fn secure_directory(path: &Path, root: &Path, cancel: &CancellationToken) -> Result<PathBuf> {
    let metadata = io_call(
        cancel,
        fs::symlink_metadata(path),
        "cannot inspect skill bundle",
    )
    .await?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error(
            ErrorCode::Unsupported,
            "skill bundle is not a regular directory",
        ));
    }
    let canonical = io_call(
        cancel,
        fs::canonicalize(path),
        "cannot resolve skill bundle",
    )
    .await?;
    if !is_contained(root, &canonical) {
        return Err(error(
            ErrorCode::Unsupported,
            "skill path escapes its configured root",
        ));
    }
    Ok(canonical)
}

async fn secure_file(
    path: &Path,
    containing: &Path,
    cancel: &CancellationToken,
) -> Result<Option<PathBuf>> {
    let Some(metadata) = optional_io(
        cancel,
        fs::symlink_metadata(path),
        "cannot inspect skill file",
    )
    .await?
    else {
        return Ok(None);
    };
    if metadata.file_type().is_symlink() {
        return Err(error(
            ErrorCode::Unsupported,
            "symbolic links are not allowed for skill files",
        ));
    }
    if !metadata.is_file() {
        return Err(error(
            ErrorCode::Unsupported,
            "skill file is not a regular file",
        ));
    }
    let canonical = io_call(cancel, fs::canonicalize(path), "cannot resolve skill file").await?;
    if !is_contained(containing, &canonical) {
        return Err(error(
            ErrorCode::Unsupported,
            "skill file escapes its configured root",
        ));
    }
    let final_metadata = io_call(
        cancel,
        fs::metadata(&canonical),
        "cannot inspect skill file",
    )
    .await?;
    if !final_metadata.is_file() {
        return Err(error(
            ErrorCode::Unsupported,
            "skill file is not a regular file",
        ));
    }
    Ok(Some(canonical))
}

fn is_contained(root: &Path, child: &Path) -> bool {
    child
        .strip_prefix(root)
        .map(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(false)
}

async fn read_bounded_text(
    path: &Path,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<String> {
    check_cancel(cancel)?;
    let metadata = io_call(cancel, fs::metadata(path), "cannot inspect skill file").await?;
    if !metadata.is_file() {
        return Err(error(
            ErrorCode::Unsupported,
            "skill file is not a regular file",
        ));
    }
    if metadata.len() > limit as u64 {
        return Err(error(ErrorCode::Limit, "skill file exceeds its byte limit"));
    }
    let mut file = io_call(cancel, File::open(path), "cannot open skill file").await?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let count = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            result = file.read(&mut chunk) => result,
        }
        .map_err(|_| error(ErrorCode::Tool, "cannot read skill file"))?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > limit {
            return Err(error(ErrorCode::Limit, "skill file exceeds its byte limit"));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    if bytes.contains(&0) {
        return Err(error(
            ErrorCode::Schema,
            "skill file must be text without NUL bytes",
        ));
    }
    String::from_utf8(bytes).map_err(|_| error(ErrorCode::Schema, "skill file must be valid UTF-8"))
}

fn parse_document(raw: &str, limits: &Limits) -> Result<(SkillSummary, String)> {
    let (frontmatter, body) = split_frontmatter(raw)?;
    let yaml: Value = serde_yaml_ng::from_str(frontmatter)
        .map_err(|_| error(ErrorCode::Schema, "skill frontmatter is invalid YAML"))?;
    let mapping = yaml
        .as_mapping()
        .ok_or_else(|| error(ErrorCode::Schema, "skill frontmatter must be a mapping"))?;
    let mut name = None;
    let mut description = None;
    let mut model_invocable = true;
    let mut user_invocable = true;
    for (key, value) in mapping {
        let Some(key) = key.as_str() else { continue };
        match key {
            "name" => name = Some(yaml_string(value, "name")?),
            "description" => description = Some(yaml_string(value, "description")?),
            "disable-model-invocation" => model_invocable = !yaml_bool(value, key)?,
            "user-invocable" => user_invocable = yaml_bool(value, key)?,
            "disableModelInvocation"
            | "disable_model_invocation"
            | "modelInvocable"
            | "model_invocable"
            | "userInvocable"
            | "user_invocable"
            | "invocation" => {
                return Err(error(
                    ErrorCode::Schema,
                    "skill invocation policy uses an unsupported field",
                ));
            }
            _ => {}
        }
    }
    let summary = SkillSummary {
        name: name.ok_or_else(|| error(ErrorCode::Schema, "skill frontmatter requires name"))?,
        description: description
            .ok_or_else(|| error(ErrorCode::Schema, "skill frontmatter requires description"))?,
        model_invocable,
        user_invocable,
        location: None,
    };
    validate_summary(&summary)?;
    let content = body.trim().to_owned();
    if content.len() > limits.max_content_bytes {
        return Err(error(
            ErrorCode::Limit,
            "skill instructions exceed the content limit",
        ));
    }
    Ok((summary, content))
}

fn split_frontmatter(raw: &str) -> Result<(&str, &str)> {
    let Some(first_end) = raw.find('\n') else {
        return Err(error(
            ErrorCode::Schema,
            "skill document requires YAML frontmatter",
        ));
    };
    let first = raw[..first_end]
        .strip_suffix('\r')
        .unwrap_or(&raw[..first_end]);
    if first != "---" {
        return Err(error(
            ErrorCode::Schema,
            "skill document requires YAML frontmatter",
        ));
    }
    let yaml_start = first_end + 1;
    let mut line_start = yaml_start;
    while line_start <= raw.len() {
        let line_end = raw[line_start..]
            .find('\n')
            .map(|offset| line_start + offset)
            .unwrap_or(raw.len());
        let line = raw[line_start..line_end]
            .strip_suffix('\r')
            .unwrap_or(&raw[line_start..line_end]);
        if line == "---" {
            let body_start = if line_end < raw.len() {
                line_end + 1
            } else {
                raw.len()
            };
            return Ok((&raw[yaml_start..line_start], &raw[body_start..]));
        }
        if line_end == raw.len() {
            break;
        }
        line_start = line_end + 1;
    }
    Err(error(ErrorCode::Schema, "skill frontmatter is not closed"))
}

fn yaml_string(value: &Value, field: &str) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        _ => Err(error(
            ErrorCode::Schema,
            &format!("skill frontmatter field {field} must be a string"),
        )),
    }
}

fn yaml_bool(value: &Value, field: &str) -> Result<bool> {
    match value {
        Value::Bool(value) => Ok(*value),
        Value::Number(value) => match value.to_string().as_str() {
            "1" => Ok(true),
            "0" => Ok(false),
            _ => Err(error(
                ErrorCode::Schema,
                &format!("skill frontmatter field {field} must be a boolean"),
            )),
        },
        Value::String(value) => match value.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(true),
            "false" | "no" | "off" | "0" => Ok(false),
            _ => Err(error(
                ErrorCode::Schema,
                &format!("skill frontmatter field {field} must be a boolean"),
            )),
        },
        _ => Err(error(
            ErrorCode::Schema,
            &format!("skill frontmatter field {field} must be a boolean"),
        )),
    }
}

fn resource_path(path: &str) -> Result<PathBuf> {
    if path.is_empty() || path.contains(':') || Path::new(path).is_absolute() {
        return Err(error(
            ErrorCode::Unsupported,
            "skill resource path must be a relative text path",
        ));
    }
    let mut relative = PathBuf::new();
    for component in path.split(['/', '\\']) {
        if component.is_empty() || component == "." || component == ".." {
            return Err(error(
                ErrorCode::Unsupported,
                "skill resource path escapes its bundle",
            ));
        }
        relative.push(component);
    }
    if relative.as_os_str().is_empty() {
        return Err(error(
            ErrorCode::Unsupported,
            "skill resource path must be a file",
        ));
    }
    Ok(relative)
}

async fn io_call<T, F>(cancel: &CancellationToken, future: F, message: &str) -> Result<T>
where
    F: Future<Output = std::io::Result<T>>,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(cancelled()),
        result = future => result.map_err(|_| error(ErrorCode::Tool, message)),
    }
}

async fn optional_io<T, F>(
    cancel: &CancellationToken,
    future: F,
    message: &str,
) -> Result<Option<T>>
where
    F: Future<Output = std::io::Result<T>>,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(cancelled()),
        result = future => match result {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(crate::error(ErrorCode::Tool, message)),
        },
    }
}
