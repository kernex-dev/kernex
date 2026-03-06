//! Skill loading, parsing, deployment, and trigger matching.

use crate::parse::{data_path, extract_bins_from_metadata, parse_yaml_list, unquote, which_exists};
use kernex_core::context::McpServer;
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
}

/// MCP server definition in TOML frontmatter (`[mcp.name]`).
#[derive(Debug, Deserialize)]
struct McpFrontmatter {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

/// Validate an MCP command name contains only safe characters.
///
/// Allows alphanumeric, hyphens, underscores, dots, and forward slashes
/// (for paths like `/usr/bin/foo`). Rejects shell metacharacters that
/// could enable injection: `; | & $ \` > < ( ) { } ! ~ #`.
fn is_safe_mcp_command(command: &str) -> bool {
    if command.is_empty() {
        return false;
    }
    command
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '@'))
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

/// Load MCP servers from an optional `mcp.json` file in a skill directory.
///
/// Returns validated servers with safe commands, skipping any with
/// dangerous shell metacharacters. Servers from `mcp.json` are merged
/// with frontmatter servers — `mcp.json` entries take precedence on
/// name collision.
fn load_mcp_json(skill_dir: &Path) -> Vec<McpServer> {
    let mcp_path = skill_dir.join("mcp.json");
    let content = match std::fs::read_to_string(&mcp_path) {
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
            if is_safe_mcp_command(&entry.command) {
                Some(McpServer {
                    name,
                    command: entry.command,
                    args: entry.args,
                    env: entry.env,
                })
            } else {
                warn!(
                    "skills: rejected unsafe MCP command {:?} in {}",
                    entry.command,
                    mcp_path.display()
                );
                None
            }
        })
        .collect()
}

/// Frontmatter parsed from a `SKILL.md` file (TOML or YAML).
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    trigger: Option<String>,
    #[serde(default)]
    mcp: HashMap<String, McpFrontmatter>,
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
        let content = match std::fs::read_to_string(&skill_file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let Some(fm) = parse_skill_file(&content) else {
            warn!("skills: no valid frontmatter in {}", skill_file.display());
            continue;
        };
        let available = fm.requires.iter().all(|t| which_exists(t));
        // Collect MCP servers from frontmatter.
        let mut mcp_servers: Vec<McpServer> = fm
            .mcp
            .into_iter()
            .filter_map(|(name, mfm)| {
                if is_safe_mcp_command(&mfm.command) {
                    Some(McpServer {
                        name,
                        command: mfm.command,
                        args: mfm.args,
                        ..Default::default()
                    })
                } else {
                    warn!(
                        "skills: rejected unsafe MCP command {:?} in {}",
                        mfm.command,
                        skill_file.display()
                    );
                    None
                }
            })
            .collect();

        // Merge MCP servers from optional mcp.json (takes precedence on name collision).
        let json_servers = load_mcp_json(&path);
        for srv in json_servers {
            if let Some(existing) = mcp_servers.iter_mut().find(|s| s.name == srv.name) {
                *existing = srv;
            } else {
                mcp_servers.push(srv);
            }
        }
        skills.push(Skill {
            name: fm.name,
            description: fm.description,
            requires: fm.requires,
            homepage: fm.homepage,
            available,
            path: skill_file,
            trigger: fm.trigger,
            mcp_servers,
        });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Build the skill block appended to the system prompt.
///
/// Returns an empty string if there are no skills.
pub fn build_skill_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n\nYou have the following skills available. \
         Before using any skill, you MUST read its file for full instructions. \
         If a tool is not installed, the skill file contains installation \
         instructions — install it first, then use it.\n\nSkills:\n",
    );

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
    }

    out
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
    if let Ok(fm) = toml::from_str::<SkillFrontmatter>(block) {
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
                "metadata" => metadata_line = Some(val.to_string()),
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
        requires,
        homepage,
        trigger,
        mcp,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::which_exists;

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
        assert!(build_skill_prompt(&[]).is_empty());
    }

    #[test]
    fn test_build_skill_prompt_formats_correctly() {
        let skills = vec![
            Skill {
                name: "gog".into(),
                description: "Google Workspace CLI.".into(),
                requires: vec!["gog".into()],
                homepage: "https://gogcli.sh".into(),
                available: true,
                path: PathBuf::from("/home/user/.kernex/skills/gog/SKILL.md"),
                trigger: None,
                mcp_servers: Vec::new(),
            },
            Skill {
                name: "missing".into(),
                description: "Not installed tool.".into(),
                requires: vec!["nope".into()],
                homepage: String::new(),
                available: false,
                path: PathBuf::from("/home/user/.kernex/skills/missing/SKILL.md"),
                trigger: None,
                mcp_servers: Vec::new(),
            },
        ];
        let prompt = build_skill_prompt(&skills);
        assert!(prompt.contains("gog [installed]"));
        assert!(prompt.contains("missing [not installed]"));
        assert!(prompt.contains("Read /home/user/.kernex/skills/gog/SKILL.md"));
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
        let tmp = std::env::temp_dir().join("__kernex_test_skills_valid__");
        let _ = std::fs::remove_dir_all(&tmp);
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
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_skills_yaml_format() {
        let tmp = std::env::temp_dir().join("__kernex_test_skills_yaml__");
        let _ = std::fs::remove_dir_all(&tmp);
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
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_migrate_flat_skills() {
        let tmp = std::env::temp_dir().join("__kernex_test_migrate__");
        let _ = std::fs::remove_dir_all(&tmp);
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

        let _ = std::fs::remove_dir_all(&tmp);
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
            requires: Vec::new(),
            homepage: String::new(),
            available,
            path: PathBuf::from("/test"),
            trigger: trigger.map(String::from),
            mcp_servers,
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
        let tmp = std::env::temp_dir().join("__kernex_test_skills_mcp__");
        let _ = std::fs::remove_dir_all(&tmp);
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
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_is_safe_mcp_command_valid() {
        assert!(is_safe_mcp_command("npx"));
        assert!(is_safe_mcp_command("my-tool"));
        assert!(is_safe_mcp_command("my_tool"));
        assert!(is_safe_mcp_command("/usr/bin/node"));
        assert!(is_safe_mcp_command("@playwright/mcp"));
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
        let tmp = std::env::temp_dir().join("__kernex_test_mcp_json_basic__");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"playwright":{"command":"npx","args":["@playwright/mcp","--headless"]}}}"#,
        )
        .unwrap();

        let servers = load_mcp_json(&tmp);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "playwright");
        assert_eq!(servers[0].command, "npx");
        assert_eq!(servers[0].args, vec!["@playwright/mcp", "--headless"]);
        assert!(servers[0].env.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_mcp_json_with_env() {
        let tmp = std::env::temp_dir().join("__kernex_test_mcp_json_env__");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"postgres":{"command":"npx","args":["@pg/mcp"],"env":{"DATABASE_URL":"postgres://localhost/test"}}}}"#,
        )
        .unwrap();

        let servers = load_mcp_json(&tmp);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "postgres");
        assert_eq!(
            servers[0].env.get("DATABASE_URL").unwrap(),
            "postgres://localhost/test"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_mcp_json_missing_file() {
        let tmp = std::env::temp_dir().join("__kernex_test_mcp_json_missing__");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let servers = load_mcp_json(&tmp);
        assert!(servers.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_mcp_json_invalid_json() {
        let tmp = std::env::temp_dir().join("__kernex_test_mcp_json_invalid__");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("mcp.json"), "not valid json").unwrap();

        let servers = load_mcp_json(&tmp);
        assert!(servers.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_mcp_json_rejects_unsafe_command() {
        let tmp = std::env::temp_dir().join("__kernex_test_mcp_json_unsafe__");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"evil":{"command":"sh -c 'rm -rf /'","args":[]}}}"#,
        )
        .unwrap();

        let servers = load_mcp_json(&tmp);
        assert!(servers.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_mcp_json_multiple_servers() {
        let tmp = std::env::temp_dir().join("__kernex_test_mcp_json_multi__");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("mcp.json"),
            r#"{"mcpServers":{"playwright":{"command":"npx","args":["@playwright/mcp"]},"postgres":{"command":"npx","args":["@pg/mcp"]}}}"#,
        )
        .unwrap();

        let servers = load_mcp_json(&tmp);
        assert_eq!(servers.len(), 2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_skills_merges_mcp_json() {
        let tmp = std::env::temp_dir().join("__kernex_test_skills_merge_mcp__");
        let _ = std::fs::remove_dir_all(&tmp);
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
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_skills_mcp_json_overrides_frontmatter() {
        let tmp = std::env::temp_dir().join("__kernex_test_skills_mcp_override__");
        let _ = std::fs::remove_dir_all(&tmp);
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
        let _ = std::fs::remove_dir_all(&tmp);
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
            &fm.mcp.get("pwned").map_or("", |m| &m.command)
        ));
    }
}
