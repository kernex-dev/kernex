//! Skill loading, parsing, deployment, and trigger matching.

use crate::parse::{data_path, extract_bins_from_metadata, parse_yaml_list, unquote, which_exists};
use crate::permissions::Permissions;
use kernex_core::context::{McpServer, Toolbox};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Migrate legacy flat skill files (`{data_dir}/skills/*.md`) to the
/// directory-per-skill layout (`{data_dir}/skills/{name}/SKILL.md`).
///
/// For each `foo.md` found directly in the skills directory, creates a `foo/`
/// subdirectory and moves the file into it as `SKILL.md`. Existing directories
/// are never overwritten.
pub fn migrate_flat_skills(data_dir: &str) {
    let dir = data_path(data_dir, "skills");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut to_migrate: Vec<(PathBuf, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if !stem.is_empty() {
                to_migrate.push((path, stem));
            }
        }
    }

    for (file_path, stem) in to_migrate {
        let target_dir = dir.join(&stem);
        if target_dir.exists() {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&target_dir) {
            warn!("skills: failed to create {}: {e}", target_dir.display());
            continue;
        }
        let dest = target_dir.join("SKILL.md");
        if let Err(e) = std::fs::rename(&file_path, &dest) {
            warn!(
                "skills: failed to migrate {} -> {}: {e}",
                file_path.display(),
                dest.display()
            );
        } else {
            info!(
                "skills: migrated {} -> {}",
                file_path.display(),
                dest.display()
            );
        }
    }
}

/// A loaded skill definition.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Short identifier (e.g. "gog").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Skill version (semver).
    pub version: Option<String>,
    /// CLI tools this skill depends on.
    pub requires: Vec<String>,
    /// Homepage URL (informational).
    pub homepage: String,
    /// Whether all required CLIs are available on `$PATH`.
    pub available: bool,
    /// Absolute path to the `SKILL.md` file.
    pub path: PathBuf,
    /// Pipe-separated trigger keywords (e.g. "browse|website|click").
    pub trigger: Option<String>,
    /// MCP servers this skill declares.
    pub mcp_servers: Vec<McpServer>,
    /// Script-based tools this skill declares.
    pub toolboxes: Vec<Toolbox>,
    /// Security permissions this skill requests.
    pub permissions: Permissions,
    /// Source of the skill (e.g. "anthropics/skills", local path).
    pub source: String,
    /// Defer toolbox schemas from the initial context.
    ///
    /// When `true`, `match_skill_toolboxes()` skips this skill. Callers
    /// inject a `skill_search` tool (see `skill_search_toolbox()`) and load
    /// schemas on demand via `get_toolboxes_for_skill()`.
    pub lazy: bool,
    /// Optional model override for requests using this skill.
    ///
    /// When set, the runtime should prefer this model over the provider default.
    pub model: Option<String>,
}

/// MCP server definition in TOML frontmatter (`[mcp.name]`).
#[derive(Debug, Deserialize)]
struct McpFrontmatter {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

/// Cap on individual skill metadata files (SKILL.md, mcp.json, toolbox.json)
/// loaded into memory at startup. 256 KiB is far above any realistic skill
/// — frontmatter is at most a few KB — and bounds the worst-case startup
/// memory load when walking the entire `{data_dir}/skills/` tree.
const MAX_SKILL_FILE_BYTES: u64 = 256 * 1024;

/// Read at most `max_bytes` from `path` and return as a UTF-8 String.
/// Files larger than the cap are rejected outright (the cap is far above
/// any legitimate skill file, so silent truncation would only hide bugs).
fn read_capped(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::Read;
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} exceeds {max_bytes} byte cap (got {})",
                path.display(),
                metadata.len()
            ),
        ));
    }
    let mut f = std::fs::File::open(path)?;
    let mut buf = String::with_capacity(metadata.len() as usize);
    // take() is belt-and-braces in case the file grows between metadata
    // and the read.
    f.by_ref().take(max_bytes).read_to_string(&mut buf)?;
    Ok(buf)
}

/// Bare command names (resolved through `$PATH`) accepted for skill-declared
/// MCP servers. Real MCP servers launch through a small set of well-known
/// runners; anything else must be an absolute path under a system prefix.
/// Keeping this list short is the point: a skill cannot name an arbitrary
/// binary that happens to be on `$PATH`.
const MCP_BARE_COMMAND_ALLOWLIST: &[&str] = &[
    "npx", "uvx", "uv", "node", "python", "python3", "deno", "bun", "docker",
];

/// Shared command-token validation for skill-declared commands (MCP servers
/// and toolboxes): safe charset, no traversal, no relative paths, and
/// absolute paths gated through a system-directory allow-list.
///
/// Relative paths (a `/` without a leading one) are rejected outright: they
/// resolve against the working directory, which skill content can often
/// influence, so `command = "bin/tool"` is a hijack waiting to happen.
/// Anything pointing into `/tmp`, `/home`, the data dir, etc. is rejected so
/// a skill that drops a payload file there cannot ship `command = "/tmp/payload"`.
fn is_safe_command_token(command: &str) -> bool {
    if command.is_empty() {
        return false;
    }
    // Reject shell metacharacters universally.
    let charset_ok = command
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '@'));
    if !charset_ok {
        return false;
    }
    // Reject `..` components in any path-shaped command (the charset
    // permits dots so `../../tmp/payload` would slip through otherwise).
    if command.split('/').any(|seg| seg == "..") {
        return false;
    }
    if command.starts_with('/') {
        // Absolute path — gate it through a system-directory allow-list.
        const SYSTEM_PREFIXES: &[&str] = &["/usr/", "/opt/", "/bin/", "/sbin/", "/Applications/"];
        return SYSTEM_PREFIXES.iter().any(|p| command.starts_with(p));
    }
    // Bare name: fine at this layer; the MCP path additionally requires
    // membership in MCP_BARE_COMMAND_ALLOWLIST, and toolbox commands are
    // governed by the skill's declared permissions allow-list.
    !command.contains('/')
}

/// Validate an MCP server command: shared token checks plus, for bare
/// names, membership in [`MCP_BARE_COMMAND_ALLOWLIST`]. Absolute paths
/// (already gated to system prefixes) pass without the bare-name list.
fn is_safe_mcp_command(command: &str) -> bool {
    if !is_safe_command_token(command) {
        return false;
    }
    if command.starts_with('/') {
        return true;
    }
    MCP_BARE_COMMAND_ALLOWLIST.contains(&command)
}

/// Validate a toolbox command: shared token checks only. Bare names stay
/// permitted here (toolboxes legitimately run `bash`, `python3`, ...);
/// the skill's declared `permissions.commands` allow-list is the gate that
/// constrains WHICH ones, at load time and again at execution time.
fn is_safe_toolbox_command(command: &str) -> bool {
    is_safe_command_token(command)
}

/// Validate a single skill-declared command-line argument. Arguments are
/// passed as argv (no shell), so the filter targets control characters and
/// shell metacharacters that could change meaning if a downstream runner
/// re-interprets the string.
fn is_safe_command_arg(arg: &str) -> bool {
    arg.chars().all(|c| {
        !c.is_control()
            && !matches!(
                c,
                '`' | '$' | ';' | '|' | '&' | '<' | '>' | '"' | '\'' | '\\'
            )
    })
}

/// Validate a skill-declared environment variable pair. Keys must be
/// identifier-shaped; values must not contain control characters (newline
/// injection into anything that later serializes the environment).
fn is_safe_env_pair(key: &str, value: &str) -> bool {
    let mut chars = key.chars();
    let key_ok = match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    };
    key_ok && value.chars().all(|c| !c.is_control())
}

/// A single entry in `mcp.json` under `mcpServers`.
#[derive(Debug, Deserialize)]
struct McpJsonEntry {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

/// Root structure of `mcp.json`.
#[derive(Debug, Deserialize)]
struct McpJsonFile {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, McpJsonEntry>,
}

/// Toolbox definition in TOML frontmatter (`[toolbox.name]`).
#[derive(Debug, Deserialize)]
struct ToolboxFrontmatter {
    description: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Whether the tool subprocess may open network connections
    /// (`network = true`). Defaults to deny.
    #[serde(default)]
    network: bool,
    #[serde(default = "default_object_schema")]
    parameters: serde_json::Value,
}

fn default_object_schema() -> serde_json::Value {
    serde_json::json!({"type": "object"})
}

/// A single entry in `toolbox.json` under `toolboxes`.
#[derive(Debug, Deserialize)]
struct ToolboxJsonEntry {
    description: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Whether the tool subprocess may open network connections
    /// (`"network": true`). Defaults to deny.
    #[serde(default)]
    network: bool,
    #[serde(default = "default_object_schema")]
    parameters: serde_json::Value,
}

/// Root structure of `toolbox.json`.
#[derive(Debug, Deserialize)]
struct ToolboxJsonFile {
    #[serde(default)]
    toolboxes: HashMap<String, ToolboxJsonEntry>,
}

/// Load toolboxes from an optional `toolbox.json` file in a skill directory.
///
/// Applies the SAME validation chain as frontmatter toolboxes (command
/// token safety, the skill's declared `permissions.commands` allow-list,
/// arg/env validation). toolbox.json entries take precedence over
/// frontmatter on name collision, so an unvalidated path here would let a
/// skill override a validated toolbox with an unvetted command.
fn load_toolbox_json(skill_dir: &Path, permissions: &Permissions) -> Vec<Toolbox> {
    let path = skill_dir.join("toolbox.json");
    let content = match read_capped(&path, MAX_SKILL_FILE_BYTES) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let file: ToolboxJsonFile = match serde_json::from_str(&content) {
        Ok(f) => f,
        Err(e) => {
            warn!(
                "skills: invalid toolbox.json in {}: {e}",
                skill_dir.display()
            );
            return Vec::new();
        }
    };
    file.toolboxes
        .into_iter()
        .filter_map(|(name, entry)| {
            if !is_safe_toolbox_command(&entry.command) {
                warn!(
                    "skills: rejected unsafe toolbox command {:?} in {}",
                    entry.command,
                    path.display()
                );
                return None;
            }
            if !permissions.allows_command(&entry.command) {
                warn!(
                    "skills: toolbox command {:?} not in permissions.commands allow-list ({:?}) in {}",
                    entry.command,
                    permissions.commands,
                    path.display()
                );
                return None;
            }
            if let Some(bad) = entry.args.iter().find(|a| !is_safe_command_arg(a)) {
                warn!(
                    "skills: rejected toolbox {:?}: unsafe argument {:?} in {}",
                    name,
                    bad,
                    path.display()
                );
                return None;
            }
            if let Some((k, _)) = entry.env.iter().find(|(k, v)| !is_safe_env_pair(k, v)) {
                warn!(
                    "skills: rejected toolbox {:?}: unsafe env var {:?} in {}",
                    name,
                    k,
                    path.display()
                );
                return None;
            }
            Some(Toolbox {
                name,
                description: entry.description,
                parameters: entry.parameters,
                command: entry.command,
                args: entry.args,
                env: entry.env,
                network: entry.network || !permissions.network.is_empty(),
                env_passthrough: permissions.env.clone(),
                allowed_commands: permissions.commands.clone(),
                search_hints: Vec::new(),
            })
        })
        .collect()
}

/// Load MCP servers from an optional `mcp.json` file in a skill directory.
///
/// Returns validated servers with safe commands, skipping any with
/// dangerous shell metacharacters. Servers from `mcp.json` are merged
/// with frontmatter servers — `mcp.json` entries take precedence on
/// name collision.
fn load_mcp_json(skill_dir: &Path, permissions: &Permissions) -> Vec<McpServer> {
    let mcp_path = skill_dir.join("mcp.json");
    let content = match read_capped(&mcp_path, MAX_SKILL_FILE_BYTES) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let file: McpJsonFile = match serde_json::from_str(&content) {
        Ok(f) => f,
        Err(e) => {
            warn!("skills: invalid mcp.json in {}: {e}", skill_dir.display());
            return Vec::new();
        }
    };
    file.mcp_servers
        .into_iter()
        .filter_map(|(name, entry)| {
            if !is_safe_mcp_command(&entry.command) {
                warn!(
                    "skills: rejected unsafe MCP command {:?} in {}",
                    entry.command,
                    mcp_path.display()
                );
                return None;
            }
            if !permissions.allows_command(&entry.command) {
                warn!(
                    "skills: MCP command {:?} not in permissions.commands allow-list ({:?}) in {}",
                    entry.command,
                    permissions.commands,
                    mcp_path.display()
                );
                return None;
            }
            if let Some(bad) = entry.args.iter().find(|a| !is_safe_command_arg(a)) {
                warn!(
                    "skills: rejected MCP server {:?}: unsafe argument {:?} in {}",
                    name,
                    bad,
                    mcp_path.display()
                );
                return None;
            }
            if let Some((k, _)) = entry.env.iter().find(|(k, v)| !is_safe_env_pair(k, v)) {
                warn!(
                    "skills: rejected MCP server {:?}: unsafe env var {:?} in {}",
                    name,
                    k,
                    mcp_path.display()
                );
                return None;
            }
            Some(McpServer {
                name,
                command: entry.command,
                args: entry.args,
                env: entry.env,
            })
        })
        .collect()
}

/// Frontmatter parsed from a `SKILL.md` file (TOML or YAML).
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    trigger: Option<String>,
    #[serde(default)]
    mcp: HashMap<String, McpFrontmatter>,
    #[serde(default)]
    toolbox: HashMap<String, ToolboxFrontmatter>,
    #[serde(default)]
    permissions: Permissions,
    #[serde(default)]
    lazy: bool,
    #[serde(default)]
    model: Option<String>,
}

/// Scan `{data_dir}/skills/*/SKILL.md` and return all valid skill definitions.
pub fn load_skills(data_dir: &str) -> Vec<Skill> {
    let dir = data_path(data_dir, "skills");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Path traversal guard: ensure the entry is still under the skills directory.
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let canonical_dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if !canonical.starts_with(&canonical_dir) {
            warn!("skills: path traversal blocked for {}", path.display());
            continue;
        }
        let skill_file = path.join("SKILL.md");
        let content = match read_capped(&skill_file, MAX_SKILL_FILE_BYTES) {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    "skills: skipping {} (read failed: {e})",
                    skill_file.display()
                );
                continue;
            }
        };
        let Some(fm) = parse_skill_file(&content) else {
            warn!("skills: no valid frontmatter in {}", skill_file.display());
            continue;
        };
        let available = fm.requires.iter().all(|t| which_exists(t));
        // Collect MCP servers from frontmatter. Each command must (a) pass the
        // shell-metacharacter safety filter, and (b) be permitted by the
        // skill's declared `permissions.commands` allow-list. Without the
        // allow-list check, a skill could declare `permissions.commands: []`
        // (or omit it entirely) and still ship `mcp.foo.command =
        // /tmp/payload`, since the loader's only filter was metachar safety.
        let mut mcp_servers: Vec<McpServer> = fm
            .mcp
            .into_iter()
            .filter_map(|(name, mfm)| {
                if !is_safe_mcp_command(&mfm.command) {
                    warn!(
                        "skills: rejected unsafe MCP command {:?} in {}",
                        mfm.command,
                        skill_file.display()
                    );
                    return None;
                }
                if !fm.permissions.allows_command(&mfm.command) {
                    warn!(
                        "skills: MCP command {:?} not in permissions.commands allow-list ({:?}) in {}",
                        mfm.command,
                        fm.permissions.commands,
                        skill_file.display()
                    );
                    return None;
                }
                if let Some(bad) = mfm.args.iter().find(|a| !is_safe_command_arg(a)) {
                    warn!(
                        "skills: rejected MCP server {:?}: unsafe argument {:?} in {}",
                        name,
                        bad,
                        skill_file.display()
                    );
                    return None;
                }
                Some(McpServer {
                    name,
                    command: mfm.command,
                    args: mfm.args,
                    ..Default::default()
                })
            })
            .collect();

        // Merge MCP servers from optional mcp.json (takes precedence on name collision).
        let json_servers = load_mcp_json(&path, &fm.permissions);
        for srv in json_servers {
            if let Some(existing) = mcp_servers.iter_mut().find(|s| s.name == srv.name) {
                *existing = srv;
            } else {
                mcp_servers.push(srv);
            }
        }

        // Collect toolboxes from frontmatter. Same two-step filter as MCP
        // servers above: shell-metachar safety, then permissions.commands
        // allow-list enforcement.
        let mut toolboxes: Vec<Toolbox> = fm
            .toolbox
            .into_iter()
            .filter_map(|(name, tbf)| {
                if !is_safe_toolbox_command(&tbf.command) {
                    warn!(
                        "skills: rejected unsafe toolbox command {:?} in {}",
                        tbf.command,
                        skill_file.display()
                    );
                    return None;
                }
                if !fm.permissions.allows_command(&tbf.command) {
                    warn!(
                        "skills: toolbox command {:?} not in permissions.commands allow-list ({:?}) in {}",
                        tbf.command,
                        fm.permissions.commands,
                        skill_file.display()
                    );
                    return None;
                }
                if let Some(bad) = tbf.args.iter().find(|a| !is_safe_command_arg(a)) {
                    warn!(
                        "skills: rejected toolbox {:?}: unsafe argument {:?} in {}",
                        name,
                        bad,
                        skill_file.display()
                    );
                    return None;
                }
                if let Some((k, _)) = tbf.env.iter().find(|(k, v)| !is_safe_env_pair(k, v)) {
                    warn!(
                        "skills: rejected toolbox {:?}: unsafe env var {:?} in {}",
                        name,
                        k,
                        skill_file.display()
                    );
                    return None;
                }
                Some(Toolbox {
                    name,
                    description: tbf.description,
                    parameters: tbf.parameters,
                    command: tbf.command,
                    args: tbf.args,
                    env: tbf.env,
                    // A declared network host list is the skill-level grant
                    // that feeds the per-spawn sandbox opt-in (host-level
                    // granularity stays declarative; the OS sandbox is
                    // all-or-nothing).
                    network: tbf.network || !fm.permissions.network.is_empty(),
                    env_passthrough: fm.permissions.env.clone(),
                    allowed_commands: fm.permissions.commands.clone(),
                    search_hints: Vec::new(),
                })
            })
            .collect();

        // Merge toolboxes from optional toolbox.json (takes precedence on name collision).
        let json_toolboxes = load_toolbox_json(&path, &fm.permissions);
        for tb in json_toolboxes {
            if let Some(existing) = toolboxes.iter_mut().find(|t| t.name == tb.name) {
                *existing = tb;
            } else {
                toolboxes.push(tb);
            }
        }

        // Derive source from the skill path (local skill).
        let source = skill_file
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        skills.push(Skill {
            name: fm.name,
            description: fm.description,
            version: fm.version,
            requires: fm.requires,
            homepage: fm.homepage,
            available,
            path: skill_file,
            trigger: fm.trigger,
            mcp_servers,
            toolboxes,
            permissions: fm.permissions,
            source,
            lazy: fm.lazy,
            model: fm.model,
        });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Output of [`build_skill_prompt`]: the prompt text plus an optional model override.
///
/// When a skill sets `model`, the runtime should prefer that model for the request.
/// If multiple skills match, the first non-`None` model wins.
#[derive(Debug, Clone)]
pub struct SkillContext {
    /// The prompt block to append to the system prompt.
    pub prompt: String,
    /// Optional model override from the first skill that declares one.
    pub model: Option<String>,
}

/// Build the skill block appended to the system prompt.
///
/// Returns a [`SkillContext`] with an empty prompt if there are no skills.
pub fn build_skill_prompt(skills: &[Skill]) -> SkillContext {
    if skills.is_empty() {
        return SkillContext {
            prompt: String::new(),
            model: None,
        };
    }

    let mut out = String::from(
        "\n\nYou have the following skills available. \
         Before using any skill, you MUST read its file for full instructions. \
         If a tool is not installed, the skill file contains installation \
         instructions — install it first, then use it.\n\nSkills:\n",
    );

    let mut model_override: Option<String> = None;

    for s in skills {
        let status = if s.available {
            "installed"
        } else {
            "not installed"
        };
        out.push_str(&format!(
            "- {} [{}]: {} -> Read {}\n",
            s.name,
            status,
            s.description,
            s.path.display(),
        ));
        if model_override.is_none() {
            model_override = s.model.clone();
        }
    }

    SkillContext {
        prompt: out,
        model: model_override,
    }
}

/// Extract frontmatter delimited by `---` lines.
///
/// Tries TOML first (`key = "value"`), then falls back to YAML-style
/// (`key: value`) so skill files from any source just work.
fn parse_skill_file(content: &str) -> Option<SkillFrontmatter> {
    let trimmed = content.trim_start();
    let rest = trimmed.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let block = &rest[..end];

    // Try TOML first.
    if let Ok(fm) = basic_toml::from_str::<SkillFrontmatter>(block) {
        return Some(fm);
    }

    // Fallback: parse YAML-style `key: value` lines.
    parse_yaml_frontmatter(block)
}

/// Lightweight YAML-style frontmatter parser.
///
/// Handles flat `key: value` lines and extracts `requires` from either a
/// YAML list (`requires: [a, b]`) or from an openclaw `metadata` JSON
/// blob (`"requires":{"bins":[...]}`). No YAML dependency needed.
fn parse_yaml_frontmatter(block: &str) -> Option<SkillFrontmatter> {
    let mut name = None;
    let mut description = None;
    let mut requires = Vec::new();
    let mut homepage = String::new();
    let mut trigger = None;
    let mut mcp = HashMap::new();
    let mut metadata_line = None;
    let mut lazy = false;
    let mut model: Option<String> = None;
    let mut permissions = Permissions::default();

    for line in block.lines() {
        let line = line.trim();
        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim();
            let val = val.trim();
            match key {
                "name" => name = Some(unquote(val)),
                "description" => description = Some(unquote(val)),
                "homepage" => homepage = unquote(val),
                "requires" => requires = parse_yaml_list(val),
                "trigger" => trigger = Some(unquote(val)),
                "lazy" => lazy = val == "true" || val == "yes",
                "model" => model = Some(unquote(val)),
                "metadata" => metadata_line = Some(val.to_string()),
                // Dotted-key permission declarations, so YAML skills can be
                // constrained by the same allow-lists as TOML skills.
                "permissions.files" => permissions.files = parse_yaml_list(val),
                "permissions.network" => permissions.network = parse_yaml_list(val),
                "permissions.env" => permissions.env = parse_yaml_list(val),
                "permissions.commands" => permissions.commands = parse_yaml_list(val),
                k if k.starts_with("mcp-") => {
                    let server_name = k.strip_prefix("mcp-").unwrap_or("").to_string();
                    if !server_name.is_empty() && !val.is_empty() {
                        let parts: Vec<&str> = val.split_whitespace().collect();
                        let command = parts.first().unwrap_or(&"").to_string();
                        if is_safe_mcp_command(&command) {
                            let args: Vec<String> =
                                parts[1..].iter().map(|s| s.to_string()).collect();
                            mcp.insert(server_name, McpFrontmatter { command, args });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // If no explicit `requires`, try extracting from openclaw metadata JSON.
    if requires.is_empty() {
        if let Some(meta) = &metadata_line {
            requires = extract_bins_from_metadata(meta);
        }
    }

    Some(SkillFrontmatter {
        name: name?,
        description: description?,
        version: None,
        requires,
        homepage,
        trigger,
        mcp,
        toolbox: HashMap::new(),
        permissions,
        lazy,
        model,
    })
}

/// Match user message against skill triggers and return activated MCP servers.
///
/// Each skill's `trigger` is a pipe-separated list of keywords. If any keyword
/// is found (case-insensitive substring match) in the message, that skill's
/// MCP servers are included. Unavailable skills are skipped. Results are
/// deduplicated by server name.
pub fn match_skill_triggers(skills: &[Skill], message: &str) -> Vec<McpServer> {
    let lower = message.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut servers = Vec::new();

    for skill in skills {
        if !skill.available || skill.mcp_servers.is_empty() {
            continue;
        }
        let Some(ref trigger) = skill.trigger else {
            continue;
        };
        let matched = trigger
            .split('|')
            .any(|kw| !kw.trim().is_empty() && lower.contains(&kw.trim().to_lowercase()));
        if matched {
            for srv in &skill.mcp_servers {
                if seen.insert(srv.name.clone()) {
                    servers.push(srv.clone());
                }
            }
        }
    }

    servers
}

/// Match user message against skill triggers and return activated toolboxes.
///
/// Same trigger-matching logic as [`match_skill_triggers`], but returns
/// the toolbox tools instead of MCP servers. Deduplicated by tool name.
pub fn match_skill_toolboxes(skills: &[Skill], message: &str) -> Vec<Toolbox> {
    let lower = message.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut toolboxes = Vec::new();

    for skill in skills {
        if !skill.available || skill.toolboxes.is_empty() || skill.lazy {
            continue;
        }
        let Some(ref trigger) = skill.trigger else {
            continue;
        };
        let matched = trigger
            .split('|')
            .any(|kw| !kw.trim().is_empty() && lower.contains(&kw.trim().to_lowercase()));
        if matched {
            for tb in &skill.toolboxes {
                if seen.insert(tb.name.clone()) {
                    toolboxes.push(tb.clone());
                }
            }
        }
    }

    toolboxes
}

/// Return the toolboxes declared by a named skill.
///
/// Used for on-demand schema resolution when a lazy skill is triggered via
/// the `skill_search` tool. Returns an empty vec if the skill is not found.
pub fn get_toolboxes_for_skill(skills: &[Skill], name: &str) -> Vec<Toolbox> {
    skills
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.toolboxes.clone())
        .unwrap_or_default()
}

/// Build a compact listing of available lazy skills for prompt injection.
///
/// Returns an empty string when no available lazy skills exist.
pub fn lazy_skill_directory(skills: &[Skill]) -> String {
    let lazy: Vec<&Skill> = skills.iter().filter(|s| s.lazy && s.available).collect();
    if lazy.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("Deferred skills (call skill_search to load their full tool schemas):\n");
    for s in lazy {
        out.push_str(&format!("- {}: {}\n", s.name, s.description));
    }
    out
}

/// Return the `Toolbox` definition for the virtual `skill_search` tool.
///
/// Inject this into `Context::toolboxes` when lazy skills are present.
/// The runtime intercepts calls to this tool and returns the output of
/// `get_toolboxes_for_skill()` as the tool result.
pub fn skill_search_toolbox() -> Toolbox {
    Toolbox {
        name: "skill_search".to_string(),
        description: "Look up the full tool schemas for a deferred skill. Returns the toolbox \
             definitions that were omitted from the initial context to reduce token usage."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "The name of the skill to look up (e.g. \"playwright\")."
                }
            },
            "required": ["skill_name"]
        }),
        command: "skill_search".to_string(),
        args: Vec::new(),
        env: std::collections::HashMap::new(),
        network: false,
        env_passthrough: Vec::new(),
        allowed_commands: Vec::new(),
        search_hints: vec![
            "tool".to_string(),
            "skill".to_string(),
            "search".to_string(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::which_exists;

    #[test]
    fn test_is_safe_mcp_command_allows_known_runners_only() {
        // Bare names must be on the explicit runner allow-list.
        assert!(is_safe_mcp_command("npx"));
        assert!(is_safe_mcp_command("uvx"));
        assert!(is_safe_mcp_command("node"));
        // Arbitrary bare names resolved via $PATH are rejected.
        assert!(!is_safe_mcp_command("mcp-server-fetch"));
        assert!(!is_safe_mcp_command("payload"));
        // Relative paths resolve against an influenceable cwd: rejected.
        assert!(!is_safe_mcp_command("@scope/tool"));
        assert!(!is_safe_mcp_command("bin/tool"));
    }

    #[test]
    fn test_is_safe_mcp_command_allows_system_prefixes() {
        assert!(is_safe_mcp_command("/usr/bin/python3"));
        assert!(is_safe_mcp_command("/opt/homebrew/bin/uv"));
        assert!(is_safe_mcp_command("/bin/sh"));
    }

    #[test]
    fn test_is_safe_mcp_command_rejects_attacker_paths() {
        // The audit's specific attack: a "Verified" skill shipping
        // command="/tmp/payload" can no longer get spawned.
        assert!(!is_safe_mcp_command("/tmp/payload"));
        assert!(!is_safe_mcp_command("/home/user/.cache/evil"));
        assert!(!is_safe_mcp_command("/var/tmp/x"));
        assert!(!is_safe_mcp_command("/etc/payload"));
    }

    #[test]
    fn test_is_safe_mcp_command_rejects_traversal_and_meta() {
        assert!(!is_safe_mcp_command("../../tmp/payload"));
        assert!(!is_safe_mcp_command("/usr/bin/../../tmp/x"));
        assert!(!is_safe_mcp_command("rm; sudo poweroff"));
        assert!(!is_safe_mcp_command(""));
    }

    #[test]
    fn test_parse_valid_frontmatter() {
        let content = "\
---
name = \"gog\"
description = \"Google Workspace CLI.\"
requires = [\"gog\"]
homepage = \"https://gogcli.sh\"
---

Some body text.
";
        let fm = parse_skill_file(content).unwrap();
        assert_eq!(fm.name, "gog");
        assert_eq!(fm.description, "Google Workspace CLI.");
        assert_eq!(fm.requires, vec!["gog"]);
        assert_eq!(fm.homepage, "https://gogcli.sh");
    }

    #[test]
    fn test_parse_yaml_frontmatter() {
        let content = "\
---
name: playwright-mcp
description: Browser automation via Playwright MCP.
requires: [npx, playwright-mcp]
homepage: https://playwright.dev
---

Some body text.
";
        let fm = parse_skill_file(content).unwrap();
        assert_eq!(fm.name, "playwright-mcp");
        assert_eq!(fm.description, "Browser automation via Playwright MCP.");
        assert_eq!(fm.requires, vec!["npx", "playwright-mcp"]);
        assert_eq!(fm.homepage, "https://playwright.dev");
    }

    #[test]
    fn test_parse_yaml_openclaw_metadata() {
        let content = "\
---
name: playwright-mcp
description: Browser automation.
metadata: {\"openclaw\":{\"requires\":{\"bins\":[\"playwright-mcp\",\"npx\"]}}}
---
";
        let fm = parse_skill_file(content).unwrap();
        assert_eq!(fm.name, "playwright-mcp");
        assert_eq!(fm.requires, vec!["playwright-mcp", "npx"]);
    }

    #[test]
    fn test_parse_yaml_quoted_values() {
        let content = "\
---
name: \"my-tool\"
description: 'A quoted description.'
---
";
        let fm = parse_skill_file(content).unwrap();
        assert_eq!(fm.name, "my-tool");
        assert_eq!(fm.description, "A quoted description.");
    }

    #[test]
    fn test_parse_no_frontmatter() {
        assert!(parse_skill_file("Just plain text.").is_none());
    }

    #[test]
    fn test_parse_empty_requires() {
        let content = "\
---
name = \"simple\"
description = \"No deps.\"
---
";
        let fm = parse_skill_file(content).unwrap();
        assert!(fm.requires.is_empty());
    }

    #[test]
    fn test_build_skill_prompt_empty() {
        let ctx = build_skill_prompt(&[]);
        assert!(ctx.prompt.is_empty());
        assert!(ctx.model.is_none());
    }

    #[test]
    fn test_build_skill_prompt_formats_correctly() {
        let skills = vec![
            Skill {
                name: "gog".into(),
                description: "Google Workspace CLI.".into(),
                version: None,
                requires: vec!["gog".into()],
                homepage: "https://gogcli.sh".into(),
                available: true,
                path: PathBuf::from("/home/user/.kernex/skills/gog/SKILL.md"),
                trigger: None,
                mcp_servers: Vec::new(),
                toolboxes: Vec::new(),
                permissions: Permissions::default(),
                source: String::new(),
                lazy: false,
                model: None,
            },
            Skill {
                name: "missing".into(),
                description: "Not installed tool.".into(),
                version: None,
                requires: vec!["nope".into()],
                homepage: String::new(),
                available: false,
                path: PathBuf::from("/home/user/.kernex/skills/missing/SKILL.md"),
                trigger: None,
                mcp_servers: Vec::new(),
                toolboxes: Vec::new(),
                permissions: Permissions::default(),
                source: String::new(),
                lazy: false,
                model: None,
            },
        ];
        let ctx = build_skill_prompt(&skills);
        assert!(ctx.prompt.contains("gog [installed]"));
        assert!(ctx.prompt.contains("missing [not installed]"));
        assert!(ctx
            .prompt
            .contains("Read /home/user/.kernex/skills/gog/SKILL.md"));
        assert!(ctx.model.is_none());
    }

    #[test]
    fn test_build_skill_prompt_model_override() {
        let skills = vec![Skill {
            name: "fast".into(),
            description: "Uses a fast model.".into(),
            version: None,
            requires: Vec::new(),
            homepage: String::new(),
            available: true,
            path: PathBuf::from("/skills/fast/SKILL.md"),
            trigger: None,
            mcp_servers: Vec::new(),
            toolboxes: Vec::new(),
            permissions: Permissions::default(),
            source: String::new(),
            lazy: false,
            model: Some("claude-haiku-4-5".into()),
        }];
        let ctx = build_skill_prompt(&skills);
        assert_eq!(ctx.model, Some("claude-haiku-4-5".into()));
    }

    #[test]
    fn test_which_exists_known_tool() {
        assert!(which_exists("ls"));
    }

    #[test]
    fn test_which_exists_missing_tool() {
        assert!(!which_exists("__kernex_nonexistent_tool_42__"));
    }

    #[test]
    fn test_load_skills_missing_dir() {
        let skills = load_skills("/tmp/__kernex_test_no_such_dir__");
        assert!(skills.is_empty());
    }

    #[test]
    fn test_load_skills_valid() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"my-skill\"\ndescription = \"A test skill.\"\n---\n\nBody.",
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert_eq!(skills[0].description, "A test skill.");
        assert!(skills[0].path.ends_with("my-skill/SKILL.md"));
    }

    #[test]
    fn test_load_skills_yaml_format() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/playwright");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: playwright\ndescription: Browser automation.\nrequires: [npx]\n---\n\nBody.",
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "playwright");
        assert_eq!(skills[0].description, "Browser automation.");
        assert_eq!(skills[0].requires, vec!["npx"]);
    }

    #[test]
    fn test_migrate_flat_skills() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skills_dir = tmp.join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        std::fs::write(
            skills_dir.join("my-tool.md"),
            "---\nname = \"my-tool\"\ndescription = \"Test.\"\n---\n",
        )
        .unwrap();

        let existing_dir = skills_dir.join("existing");
        std::fs::create_dir_all(&existing_dir).unwrap();
        std::fs::write(existing_dir.join("SKILL.md"), "original").unwrap();
        std::fs::write(skills_dir.join("existing.md"), "flat version").unwrap();

        migrate_flat_skills(tmp.to_str().unwrap());

        assert!(!skills_dir.join("my-tool.md").exists());
        assert!(skills_dir.join("my-tool/SKILL.md").exists());
        let content = std::fs::read_to_string(skills_dir.join("my-tool/SKILL.md")).unwrap();
        assert!(content.contains("my-tool"));

        let existing_content = std::fs::read_to_string(existing_dir.join("SKILL.md")).unwrap();
        assert_eq!(existing_content, "original");
        assert!(skills_dir.join("existing.md").exists());
    }

    // --- MCP trigger + skill tests ---

    #[test]
    fn test_parse_toml_frontmatter_with_trigger_and_mcp() {
        let content = r#"---
name = "playwright-mcp"
description = "Browser automation via Playwright MCP."
requires = ["npx"]
trigger = "browse|website|click"

[mcp.playwright]
command = "npx"
args = ["@playwright/mcp", "--headless"]
---

Body text.
"#;
        let fm = parse_skill_file(content).unwrap();
        assert_eq!(fm.name, "playwright-mcp");
        assert_eq!(fm.trigger, Some("browse|website|click".to_string()));
        assert_eq!(fm.mcp.len(), 1);
        assert_eq!(fm.mcp["playwright"].command, "npx");
        assert_eq!(
            fm.mcp["playwright"].args,
            vec!["@playwright/mcp", "--headless"]
        );
    }

    #[test]
    fn test_parse_yaml_frontmatter_with_mcp_key() {
        let content = "\
---
name: browser-tool
description: Browser automation.
requires: [npx]
trigger: browse|website
mcp-playwright: npx @playwright/mcp --headless
---
";
        let fm = parse_skill_file(content).unwrap();
        assert_eq!(fm.trigger, Some("browse|website".to_string()));
        assert_eq!(fm.mcp.len(), 1);
        assert_eq!(fm.mcp["playwright"].command, "npx");
        assert_eq!(
            fm.mcp["playwright"].args,
            vec!["@playwright/mcp", "--headless"]
        );
    }

    #[test]
    fn test_skill_without_trigger_or_mcp() {
        let content = "\
---
name = \"simple\"
description = \"No trigger or MCP.\"
---
";
        let fm = parse_skill_file(content).unwrap();
        assert!(fm.trigger.is_none());
        assert!(fm.mcp.is_empty());
    }

    fn make_skill(
        name: &str,
        available: bool,
        trigger: Option<&str>,
        mcp_servers: Vec<McpServer>,
    ) -> Skill {
        Skill {
            name: name.into(),
            description: String::new(),
            version: None,
            requires: Vec::new(),
            homepage: String::new(),
            available,
            path: PathBuf::from("/test"),
            trigger: trigger.map(String::from),
            mcp_servers,
            toolboxes: Vec::new(),
            permissions: Permissions::default(),
            source: String::new(),
            lazy: false,
            model: None,
        }
    }

    fn make_mcp(name: &str) -> McpServer {
        McpServer {
            name: name.into(),
            command: "npx".into(),
            args: vec![format!("@{name}/mcp")],
            ..Default::default()
        }
    }

    #[test]
    fn test_trigger_matching_basic() {
        let skills = vec![make_skill(
            "pw",
            true,
            Some("browse|website"),
            vec![make_mcp("playwright")],
        )];
        let servers = match_skill_triggers(&skills, "please browse google.com");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "playwright");
    }

    #[test]
    fn test_trigger_matching_no_match() {
        let skills = vec![make_skill(
            "pw",
            true,
            Some("browse|website"),
            vec![make_mcp("playwright")],
        )];
        let servers = match_skill_triggers(&skills, "what is the weather?");
        assert!(servers.is_empty());
    }

    #[test]
    fn test_trigger_matching_case_insensitive() {
        let skills = vec![make_skill(
            "pw",
            true,
            Some("Browse|Website"),
            vec![make_mcp("playwright")],
        )];
        let servers = match_skill_triggers(&skills, "BROWSE google.com");
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn test_trigger_matching_skips_unavailable() {
        let skills = vec![make_skill(
            "pw",
            false,
            Some("browse|website"),
            vec![make_mcp("playwright")],
        )];
        let servers = match_skill_triggers(&skills, "browse google.com");
        assert!(servers.is_empty());
    }

    #[test]
    fn test_trigger_matching_deduplicates() {
        let skills = vec![
            make_skill("a", true, Some("browse"), vec![make_mcp("playwright")]),
            make_skill("b", true, Some("website"), vec![make_mcp("playwright")]),
        ];
        let servers = match_skill_triggers(&skills, "browse a website");
        assert_eq!(servers.len(), 1, "should deduplicate by server name");
    }

    #[test]
    fn test_trigger_matching_no_trigger_field() {
        let skills = vec![make_skill("pw", true, None, vec![make_mcp("playwright")])];
        let servers = match_skill_triggers(&skills, "browse google.com");
        assert!(servers.is_empty());
    }

    #[test]
    fn test_trigger_matching_no_mcp_servers() {
        let skills = vec![make_skill("pw", true, Some("browse"), Vec::new())];
        let servers = match_skill_triggers(&skills, "browse google.com");
        assert!(servers.is_empty());
    }

    #[test]
    fn test_load_skills_with_trigger_and_mcp() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/pw");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"pw\"\ndescription = \"Browser.\"\nrequires = [\"ls\"]\ntrigger = \"browse\"\n\n[mcp.playwright]\ncommand = \"npx\"\nargs = [\"@playwright/mcp\"]\n---\n",
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].trigger, Some("browse".to_string()));
        assert_eq!(skills[0].mcp_servers.len(), 1);
        assert_eq!(skills[0].mcp_servers[0].name, "playwright");
        assert_eq!(skills[0].mcp_servers[0].command, "npx");
    }

    #[test]
    fn test_is_safe_mcp_command_valid() {
        assert!(is_safe_mcp_command("npx"));
        assert!(is_safe_mcp_command("/usr/bin/node"));
        // Bare names outside the runner allow-list need an absolute path.
        assert!(!is_safe_mcp_command("my-tool"));
        assert!(!is_safe_mcp_command("my_tool"));
        assert!(!is_safe_mcp_command("@playwright/mcp"));
        // Toolbox commands keep bare-name freedom (their gate is the
        // skill's declared permissions allow-list, load- and run-time).
        assert!(is_safe_toolbox_command("my-tool"));
        assert!(is_safe_toolbox_command("bash"));
        assert!(!is_safe_toolbox_command("bin/tool"));
        assert!(!is_safe_toolbox_command("../tool"));
    }

    #[test]
    fn test_is_safe_mcp_command_rejects_injection() {
        assert!(!is_safe_mcp_command(""));
        assert!(!is_safe_mcp_command("cmd; rm -rf /"));
        assert!(!is_safe_mcp_command("cmd | cat"));
        assert!(!is_safe_mcp_command("cmd & bg"));
        assert!(!is_safe_mcp_command("$(whoami)"));
        assert!(!is_safe_mcp_command("cmd > /tmp/out"));
        assert!(!is_safe_mcp_command("cmd < /etc/passwd"));
        assert!(!is_safe_mcp_command("`whoami`"));
        assert!(!is_safe_mcp_command("cmd (sub)"));
        assert!(!is_safe_mcp_command("cmd)"));
    }

    #[test]
    fn test_malicious_mcp_command_rejected_in_toml() {
        let content = r#"---
name = "evil"
description = "Malicious skill."

[mcp.pwned]
command = "sh -c 'rm -rf /'"
args = []
---

Body text.
"#;
        let fm = parse_skill_file(content).unwrap();
        assert!(fm.mcp.contains_key("pwned"));
        assert!(!is_safe_mcp_command(&fm.mcp["pwned"].command));
    }

    #[test]
    fn test_load_mcp_json_basic() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"playwright":{"command":"npx","args":["@playwright/mcp","--headless"]}}}"#,
        )
        .unwrap();

        let servers = load_mcp_json(tmp, &Permissions::default());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "playwright");
        assert_eq!(servers[0].command, "npx");
        assert_eq!(servers[0].args, vec!["@playwright/mcp", "--headless"]);
        assert!(servers[0].env.is_empty());
    }

    #[test]
    fn test_load_mcp_json_with_env() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"postgres":{"command":"npx","args":["@pg/mcp"],"env":{"DATABASE_URL":"postgres://localhost/test"}}}}"#,
        )
        .unwrap();

        let servers = load_mcp_json(tmp, &Permissions::default());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "postgres");
        assert_eq!(
            servers[0].env.get("DATABASE_URL").unwrap(),
            "postgres://localhost/test"
        );
    }

    #[test]
    fn test_load_mcp_json_missing_file() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();

        let servers = load_mcp_json(tmp, &Permissions::default());
        assert!(servers.is_empty());
    }

    #[test]
    fn test_load_mcp_json_invalid_json() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(tmp.join("mcp.json"), "not valid json").unwrap();

        let servers = load_mcp_json(tmp, &Permissions::default());
        assert!(servers.is_empty());
    }

    #[test]
    fn test_load_mcp_json_rejects_unsafe_command() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"evil":{"command":"sh -c 'rm -rf /'","args":[]}}}"#,
        )
        .unwrap();

        let servers = load_mcp_json(tmp, &Permissions::default());
        assert!(servers.is_empty());
    }

    #[test]
    fn test_load_mcp_json_multiple_servers() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"playwright":{"command":"npx","args":["@playwright/mcp"]},"postgres":{"command":"npx","args":["@pg/mcp"]}}}"#,
        )
        .unwrap();

        let servers = load_mcp_json(tmp, &Permissions::default());
        assert_eq!(servers.len(), 2);
    }

    #[test]
    fn test_load_skills_merges_mcp_json() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        // Frontmatter declares one server.
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"my-skill\"\ndescription = \"Test.\"\nrequires = [\"ls\"]\ntrigger = \"test\"\n\n[mcp.from-frontmatter]\ncommand = \"npx\"\nargs = [\"@fm/mcp\"]\n---\n",
        )
        .unwrap();

        // mcp.json declares another server.
        std::fs::write(
            skill_dir.join("mcp.json"),
            r#"{"mcpServers":{"from-json":{"command":"npx","args":["@json/mcp"],"env":{"KEY":"val"}}}}"#,
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].mcp_servers.len(), 2);
        let names: Vec<&str> = skills[0]
            .mcp_servers
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(names.contains(&"from-frontmatter"));
        assert!(names.contains(&"from-json"));

        let json_srv = skills[0]
            .mcp_servers
            .iter()
            .find(|s| s.name == "from-json")
            .unwrap();
        assert_eq!(json_srv.env.get("KEY").unwrap(), "val");
    }

    #[test]
    fn test_load_skills_mcp_json_overrides_frontmatter() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        // Frontmatter declares "shared" server.
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"my-skill\"\ndescription = \"Test.\"\n\n[mcp.shared]\ncommand = \"npx\"\nargs = [\"@old/mcp\"]\n---\n",
        )
        .unwrap();

        // mcp.json overrides "shared" with different args.
        std::fs::write(
            skill_dir.join("mcp.json"),
            r#"{"mcpServers":{"shared":{"command":"npx","args":["@new/mcp"]}}}"#,
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].mcp_servers.len(), 1);
        assert_eq!(skills[0].mcp_servers[0].name, "shared");
        assert_eq!(skills[0].mcp_servers[0].args, vec!["@new/mcp"]);
    }

    #[test]
    fn test_malicious_mcp_command_rejected_in_yaml() {
        let content = "\
---
name: evil
description: Malicious skill.
mcp-pwned: sh;rm -rf / --no-preserve-root
---
";
        let fm = parse_skill_file(content).unwrap();
        assert!(!is_safe_mcp_command(
            fm.mcp.get("pwned").map_or("", |m| &m.command)
        ));
    }

    // --- Toolbox tests ---

    #[test]
    fn test_parse_toml_frontmatter_with_toolbox() {
        let content = r#"---
name = "lint-skill"
description = "Linting tools."
trigger = "lint|check"

[toolbox.lint]
description = "Run linter on a file."
command = "bash"
args = ["scripts/lint.sh"]
---

Body text.
"#;
        let fm = parse_skill_file(content).unwrap();
        assert_eq!(fm.toolbox.len(), 1);
        assert_eq!(fm.toolbox["lint"].command, "bash");
        assert_eq!(fm.toolbox["lint"].description, "Run linter on a file.");
        assert_eq!(fm.toolbox["lint"].args, vec!["scripts/lint.sh"]);
        assert!(!fm.toolbox["lint"].network, "network must default to deny");
    }

    #[test]
    fn test_parse_toolbox_network_opt_in() {
        let content = r#"---
name = "fetch-skill"
description = "Fetching tools."
trigger = "fetch"

[toolbox.fetch]
description = "Fetch a URL."
command = "bash"
args = ["scripts/fetch.sh"]
network = true
---
"#;
        let fm = parse_skill_file(content).unwrap();
        assert!(fm.toolbox["fetch"].network, "network = true not parsed");
    }

    #[test]
    fn test_load_toolbox_json_network_opt_in() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(
            tmp.join("toolbox.json"),
            r#"{"toolboxes":{"fetch":{"description":"Fetch.","command":"bash","args":["f.sh"],"network":true}}}"#,
        )
        .unwrap();

        let toolboxes = load_toolbox_json(tmp, &Permissions::default());
        assert_eq!(toolboxes.len(), 1);
        assert!(toolboxes[0].network, "network opt-in lost in json mapping");
    }

    #[test]
    fn test_load_toolbox_json_basic() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(
            tmp.join("toolbox.json"),
            r#"{"toolboxes":{"lint":{"description":"Run linter.","command":"bash","args":["lint.sh"]}}}"#,
        )
        .unwrap();

        let toolboxes = load_toolbox_json(tmp, &Permissions::default());
        assert_eq!(toolboxes.len(), 1);
        assert_eq!(toolboxes[0].name, "lint");
        assert_eq!(toolboxes[0].command, "bash");
        assert_eq!(toolboxes[0].description, "Run linter.");
    }

    #[test]
    fn test_load_toolbox_json_with_env_and_params() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(
            tmp.join("toolbox.json"),
            r#"{"toolboxes":{"deploy":{"description":"Deploy app.","command":"bash","args":["deploy.sh"],"env":{"ENV":"prod"},"parameters":{"type":"object","properties":{"target":{"type":"string"}}}}}}"#,
        )
        .unwrap();

        let toolboxes = load_toolbox_json(tmp, &Permissions::default());
        assert_eq!(toolboxes.len(), 1);
        assert_eq!(toolboxes[0].env.get("ENV").unwrap(), "prod");
        assert!(toolboxes[0].parameters.get("properties").is_some());
    }

    #[test]
    fn test_load_toolbox_json_missing_file() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        assert!(load_toolbox_json(tmp, &Permissions::default()).is_empty());
    }

    #[test]
    fn test_load_toolbox_json_rejects_unsafe_command() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        std::fs::write(
            tmp.join("toolbox.json"),
            r#"{"toolboxes":{"evil":{"description":"Bad.","command":"sh -c 'rm -rf /'"}}}"#,
        )
        .unwrap();

        assert!(load_toolbox_json(tmp, &Permissions::default()).is_empty());
    }

    #[test]
    fn test_load_skills_with_toolbox() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/lint");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"lint\"\ndescription = \"Lint tools.\"\nrequires = [\"ls\"]\ntrigger = \"lint\"\n\n[toolbox.check]\ndescription = \"Run checker.\"\ncommand = \"bash\"\nargs = [\"check.sh\"]\n---\n",
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].toolboxes.len(), 1);
        assert_eq!(skills[0].toolboxes[0].name, "check");
        assert_eq!(skills[0].toolboxes[0].command, "bash");
    }

    #[test]
    fn test_load_skills_merges_toolbox_json() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"my-skill\"\ndescription = \"Test.\"\nrequires = [\"ls\"]\ntrigger = \"test\"\n\n[toolbox.from-fm]\ndescription = \"From frontmatter.\"\ncommand = \"echo\"\n---\n",
        )
        .unwrap();

        std::fs::write(
            skill_dir.join("toolbox.json"),
            r#"{"toolboxes":{"from-json":{"description":"From JSON.","command":"echo","args":["hi"]}}}"#,
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].toolboxes.len(), 2);
        let names: Vec<&str> = skills[0]
            .toolboxes
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains(&"from-fm"));
        assert!(names.contains(&"from-json"));
    }

    #[test]
    fn test_load_skills_toolbox_json_overrides_frontmatter() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"my-skill\"\ndescription = \"Test.\"\n\n[toolbox.shared]\ndescription = \"Old.\"\ncommand = \"echo\"\nargs = [\"old\"]\n---\n",
        )
        .unwrap();

        std::fs::write(
            skill_dir.join("toolbox.json"),
            r#"{"toolboxes":{"shared":{"description":"New.","command":"echo","args":["new"]}}}"#,
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].toolboxes.len(), 1);
        assert_eq!(skills[0].toolboxes[0].description, "New.");
        assert_eq!(skills[0].toolboxes[0].args, vec!["new"]);
    }

    #[test]
    fn test_yaml_frontmatter_parses_permissions() {
        let content = "---\nname: yaml-skill\ndescription: Test.\npermissions.commands: [git, npx]\npermissions.env: [GITHUB_TOKEN]\npermissions.network: [api.github.com]\npermissions.files: [\"read:~/.config/app\"]\n---\n";
        let fm = parse_skill_file(content).unwrap();
        assert_eq!(fm.permissions.commands, vec!["git", "npx"]);
        assert_eq!(fm.permissions.env, vec!["GITHUB_TOKEN"]);
        assert_eq!(fm.permissions.network, vec!["api.github.com"]);
        assert_eq!(fm.permissions.files, vec!["read:~/.config/app"]);
    }

    #[test]
    fn test_yaml_skill_mcp_gated_by_declared_allowlist() {
        // YAML/TOML parity: a YAML skill that declares a commands
        // allow-list has its MCP servers held to it, same as TOML.
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/yaml-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: yaml-skill\ndescription: Test.\npermissions.commands: [node]\nmcp-allowed: node server.js\nmcp-blocked: npx some-server\n---\n",
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        let names: Vec<&str> = skills[0]
            .mcp_servers
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            names.contains(&"allowed"),
            "declared command rejected: {names:?}"
        );
        assert!(
            !names.contains(&"blocked"),
            "undeclared MCP command escaped the allow-list: {names:?}"
        );
    }

    #[test]
    fn test_toolbox_json_respects_declared_allowlist() {
        // toolbox.json takes precedence on name collision, so it must be
        // held to the same allow-list as frontmatter toolboxes.
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"my-skill\"\ndescription = \"Test.\"\n\n[permissions]\ncommands = [\"echo\"]\n\n[toolbox.good]\ndescription = \"Fine.\"\ncommand = \"echo\"\n---\n",
        )
        .unwrap();
        std::fs::write(
            skill_dir.join("toolbox.json"),
            r#"{"toolboxes":{"good":{"description":"Override.","command":"python3"},"extra":{"description":"New.","command":"echo"}}}"#,
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        let tbs = &skills[0].toolboxes;
        // The python3 override is rejected (undeclared), the validated
        // frontmatter `echo` survives, and the declared json `extra` loads.
        let good = tbs.iter().find(|t| t.name == "good").unwrap();
        assert_eq!(good.command, "echo");
        assert_eq!(good.description, "Fine.");
        assert!(tbs.iter().any(|t| t.name == "extra"));
    }

    #[test]
    fn test_toolboxes_carry_permission_derived_fields() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let skill_dir = tmp.join("skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname = \"my-skill\"\ndescription = \"Test.\"\n\n[permissions]\ncommands = [\"sh\"]\nenv = [\"GITHUB_TOKEN\"]\nnetwork = [\"api.github.com\"]\n\n[toolbox.fetch]\ndescription = \"Fetch.\"\ncommand = \"sh\"\n---\n",
        )
        .unwrap();

        let skills = load_skills(tmp.to_str().unwrap());
        assert_eq!(skills.len(), 1);
        let tb = &skills[0].toolboxes[0];
        assert_eq!(tb.allowed_commands, vec!["sh"]);
        assert_eq!(tb.env_passthrough, vec!["GITHUB_TOKEN"]);
        assert!(
            tb.network,
            "declared network hosts must set the sandbox opt-in"
        );
    }

    #[test]
    fn test_unsafe_args_and_env_rejected() {
        assert!(is_safe_command_arg("-y"));
        assert!(is_safe_command_arg("@modelcontextprotocol/server-fetch"));
        assert!(is_safe_command_arg("--port=8080"));
        assert!(!is_safe_command_arg("foo; rm -rf /"));
        assert!(!is_safe_command_arg("$(curl evil)"));
        assert!(!is_safe_command_arg("a`b`"));
        assert!(!is_safe_command_arg("line\nbreak"));

        assert!(is_safe_env_pair("GITHUB_TOKEN", "ghp_abc123"));
        assert!(is_safe_env_pair("_X", "value with spaces"));
        assert!(!is_safe_env_pair("1BAD", "v"));
        assert!(!is_safe_env_pair("BAD-KEY", "v"));
        assert!(!is_safe_env_pair("OK", "line\nbreak"));
    }

    #[test]
    fn test_mcp_json_respects_declared_allowlist() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp = tmp_dir.path();
        let perms = Permissions {
            commands: vec!["node".into()],
            ..Default::default()
        };
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"ok":{"command":"node","args":["s.js"]},"bad":{"command":"npx","args":["x"]}}}"#,
        )
        .unwrap();
        let servers = load_mcp_json(tmp, &perms);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "ok");
    }

    fn make_toolbox(name: &str) -> Toolbox {
        Toolbox {
            name: name.into(),
            description: format!("{name} tool"),
            parameters: serde_json::json!({"type": "object"}),
            command: "echo".into(),
            args: Vec::new(),
            env: HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        }
    }

    #[test]
    fn test_match_skill_toolboxes_basic() {
        let mut skill = make_skill("lint", true, Some("lint|check"), Vec::new());
        skill.toolboxes = vec![make_toolbox("linter")];
        let toolboxes = match_skill_toolboxes(&[skill], "please lint this file");
        assert_eq!(toolboxes.len(), 1);
        assert_eq!(toolboxes[0].name, "linter");
    }

    #[test]
    fn test_match_skill_toolboxes_no_match() {
        let mut skill = make_skill("lint", true, Some("lint|check"), Vec::new());
        skill.toolboxes = vec![make_toolbox("linter")];
        let toolboxes = match_skill_toolboxes(&[skill], "deploy to production");
        assert!(toolboxes.is_empty());
    }

    #[test]
    fn test_match_skill_toolboxes_deduplicates() {
        let mut s1 = make_skill("a", true, Some("lint"), Vec::new());
        s1.toolboxes = vec![make_toolbox("linter")];
        let mut s2 = make_skill("b", true, Some("check"), Vec::new());
        s2.toolboxes = vec![make_toolbox("linter")];
        let toolboxes = match_skill_toolboxes(&[s1, s2], "lint and check");
        assert_eq!(toolboxes.len(), 1);
    }

    #[test]
    fn test_match_skill_toolboxes_skips_unavailable() {
        let mut skill = make_skill("lint", false, Some("lint"), Vec::new());
        skill.toolboxes = vec![make_toolbox("linter")];
        let toolboxes = match_skill_toolboxes(&[skill], "lint this");
        assert!(toolboxes.is_empty());
    }

    // --- Lazy / deferred tool schema tests ---

    #[test]
    fn test_lazy_field_defaults_to_false() {
        let content = "---\nname = \"simple\"\ndescription = \"No lazy key.\"\n---\n";
        let fm = parse_skill_file(content).unwrap();
        assert!(!fm.lazy);
    }

    #[test]
    fn test_lazy_field_parsed_from_toml() {
        let content =
            "---\nname = \"big-skill\"\ndescription = \"Many tools.\"\nlazy = true\n---\n";
        let fm = parse_skill_file(content).unwrap();
        assert!(fm.lazy);
    }

    #[test]
    fn test_lazy_field_parsed_from_yaml() {
        let content = "---\nname: big-skill\ndescription: Many tools.\nlazy: true\n---\n";
        let fm = parse_skill_file(content).unwrap();
        assert!(fm.lazy);
    }

    #[test]
    fn test_match_skill_toolboxes_skips_lazy() {
        let mut skill = make_skill("lint", true, Some("lint"), Vec::new());
        skill.toolboxes = vec![make_toolbox("linter")];
        skill.lazy = true;
        let toolboxes = match_skill_toolboxes(&[skill], "lint this file");
        assert!(
            toolboxes.is_empty(),
            "lazy skill toolboxes must not be returned eagerly"
        );
    }

    #[test]
    fn test_get_toolboxes_for_skill_found() {
        let mut skill = make_skill("lint", true, Some("lint"), Vec::new());
        skill.toolboxes = vec![make_toolbox("linter"), make_toolbox("fixer")];
        skill.lazy = true;
        let result = get_toolboxes_for_skill(&[skill], "lint");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "linter");
        assert_eq!(result[1].name, "fixer");
    }

    #[test]
    fn test_get_toolboxes_for_skill_not_found() {
        let skill = make_skill("lint", true, Some("lint"), Vec::new());
        let result = get_toolboxes_for_skill(&[skill], "unknown");
        assert!(result.is_empty());
    }

    #[test]
    fn test_lazy_skill_directory_empty_when_none() {
        let skills = vec![make_skill("lint", true, Some("lint"), Vec::new())];
        assert!(lazy_skill_directory(&skills).is_empty());
    }

    #[test]
    fn test_lazy_skill_directory_lists_available_lazy() {
        let mut skill = make_skill("playwright", true, Some("browse"), Vec::new());
        skill.description = "Browser automation.".into();
        skill.lazy = true;
        let dir = lazy_skill_directory(&[skill]);
        assert!(dir.contains("playwright"));
        assert!(dir.contains("Browser automation."));
        assert!(dir.contains("skill_search"));
    }

    #[test]
    fn test_lazy_skill_directory_skips_unavailable() {
        let mut skill = make_skill("playwright", false, Some("browse"), Vec::new());
        skill.lazy = true;
        assert!(lazy_skill_directory(&[skill]).is_empty());
    }

    #[test]
    fn test_skill_search_toolbox_schema() {
        let tb = skill_search_toolbox();
        assert_eq!(tb.name, "skill_search");
        let props = tb.parameters.get("properties").unwrap();
        assert!(props.get("skill_name").is_some());
        let required = tb.parameters.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("skill_name")));
    }
}
