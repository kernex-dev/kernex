//! Project loading and parsing.

use crate::parse::{data_path, parse_yaml_list};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::warn;

/// A loaded project definition.
#[derive(Debug, Clone)]
pub struct Project {
    /// Directory name (e.g. "real-estate").
    pub name: String,
    /// Contents of `ROLE.md` (body after frontmatter).
    pub instructions: String,
    /// Absolute path to the project directory.
    pub path: PathBuf,
    /// Skills declared in ROLE.md frontmatter.
    pub skills: Vec<String>,
}

/// Frontmatter parsed from a `ROLE.md` file.
#[derive(Debug, Deserialize, Default)]
struct ProjectFrontmatter {
    #[serde(default)]
    skills: Vec<String>,
}

/// Parse optional frontmatter from a ROLE.md file.
///
/// Looks for `---` delimited block at the start. Tries TOML first, YAML fallback.
/// Returns (frontmatter, body_after_frontmatter). Files without `---` return
/// default frontmatter and full content as body.
fn parse_project_frontmatter(content: &str) -> (ProjectFrontmatter, &str) {
    let trimmed = content.trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (ProjectFrontmatter::default(), content);
    };
    let Some(end) = rest.find("\n---") else {
        return (ProjectFrontmatter::default(), content);
    };
    let block = &rest[..end];
    let body = &rest[end + 4..]; // skip "\n---"
    let body = body.strip_prefix('\n').unwrap_or(body);

    // Try TOML first.
    if let Ok(fm) = toml::from_str::<ProjectFrontmatter>(block) {
        return (fm, body);
    }

    // Fallback: parse YAML-style skills list.
    let mut skills = Vec::new();
    for line in block.lines() {
        let line = line.trim();
        if let Some((key, val)) = line.split_once(':') {
            if key.trim() == "skills" {
                skills = parse_yaml_list(val);
            }
        }
    }

    (ProjectFrontmatter { skills }, body)
}

/// Create `{data_dir}/projects/` if it doesn't exist.
pub fn ensure_projects_dir(data_dir: &str) {
    let dir = data_path(data_dir, "projects");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("projects: failed to create {}: {e}", dir.display());
    }
}

/// Scan `{data_dir}/projects/*/ROLE.md` and return all valid projects.
pub fn load_projects(data_dir: &str) -> Vec<Project> {
    let dir = Path::new(&kernex_core::shellexpand(data_dir)).join("projects");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut projects = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Path traversal guard: ensure the entry is still under the projects directory.
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let canonical_dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if !canonical.starts_with(&canonical_dir) {
            warn!("projects: path traversal blocked for {}", path.display());
            continue;
        }
        // Try AGENTS.md first (modern), fallback to ROLE.md (legacy).
        let instructions_path = {
            let agents = path.join("AGENTS.md");
            if agents.exists() {
                agents
            } else {
                path.join("ROLE.md")
            }
        };
        let content = match std::fs::read_to_string(&instructions_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let trimmed = content.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let (fm, body) = parse_project_frontmatter(&trimmed);
        let instructions = body.trim().to_string();
        // If body is empty after stripping frontmatter, use full content (backward compat).
        let instructions = if instructions.is_empty() {
            trimmed
        } else {
            instructions
        };
        projects.push(Project {
            name,
            instructions,
            path,
            skills: fm.skills,
        });
    }

    projects.sort_by(|a, b| a.name.cmp(&b.name));
    projects
}

/// Find a project by name and return its instructions.
pub fn get_project_instructions<'a>(projects: &'a [Project], name: &str) -> Option<&'a str> {
    projects
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.instructions.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_projects_missing_dir() {
        let projects = load_projects("/tmp/__kernex_test_no_such_projects_dir__");
        assert!(projects.is_empty());
    }

    #[test]
    fn test_load_projects_valid() {
        let tmp = std::env::temp_dir().join("__kernex_test_projects_valid__");
        let _ = std::fs::remove_dir_all(&tmp);
        let proj_dir = tmp.join("projects/my-project");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("ROLE.md"), "You are a helpful assistant.").unwrap();

        let projects = load_projects(tmp.to_str().unwrap());
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "my-project");
        assert_eq!(projects[0].instructions, "You are a helpful assistant.");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_projects_empty_instructions() {
        let tmp = std::env::temp_dir().join("__kernex_test_projects_empty__");
        let _ = std::fs::remove_dir_all(&tmp);
        let proj_dir = tmp.join("projects/empty-proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("ROLE.md"), "   \n  ").unwrap();

        let projects = load_projects(tmp.to_str().unwrap());
        assert!(projects.is_empty(), "empty instructions should be skipped");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_projects_no_instructions_file() {
        let tmp = std::env::temp_dir().join("__kernex_test_projects_no_file__");
        let _ = std::fs::remove_dir_all(&tmp);
        let proj_dir = tmp.join("projects/no-file");
        std::fs::create_dir_all(&proj_dir).unwrap();

        let projects = load_projects(tmp.to_str().unwrap());
        assert!(projects.is_empty(), "dir without ROLE.md should be skipped");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_get_project_instructions() {
        let projects = vec![Project {
            name: "stocks".into(),
            instructions: "Track my portfolio.".into(),
            path: PathBuf::from("/home/user/.kernex/projects/stocks"),
            skills: Vec::new(),
        }];
        assert_eq!(
            get_project_instructions(&projects, "stocks"),
            Some("Track my portfolio.")
        );
        assert!(get_project_instructions(&projects, "unknown").is_none());
    }

    #[test]
    fn test_parse_project_frontmatter_toml() {
        let content = "\
---
skills = [\"ibkr-trader\", \"playwright-mcp\"]
---

You are a trading assistant.
";
        let (fm, body) = parse_project_frontmatter(content);
        assert_eq!(fm.skills, vec!["ibkr-trader", "playwright-mcp"]);
        assert!(body.contains("trading assistant"));
    }

    #[test]
    fn test_parse_project_frontmatter_yaml() {
        let content = "\
---
skills: [ibkr-trader, playwright-mcp]
---

You are a trading assistant.
";
        let (fm, body) = parse_project_frontmatter(content);
        assert_eq!(fm.skills, vec!["ibkr-trader", "playwright-mcp"]);
        assert!(body.contains("trading assistant"));
    }

    #[test]
    fn test_parse_project_frontmatter_none() {
        let content = "You are a trading assistant.";
        let (fm, body) = parse_project_frontmatter(content);
        assert!(fm.skills.is_empty());
        assert_eq!(body, content);
    }

    #[test]
    fn test_parse_project_frontmatter_empty_skills() {
        let content = "\
---
skills = []
---

Body text.
";
        let (fm, body) = parse_project_frontmatter(content);
        assert!(fm.skills.is_empty());
        assert!(body.contains("Body text"));
    }

    #[test]
    fn test_load_projects_with_frontmatter() {
        let tmp = std::env::temp_dir().join("__kernex_test_projects_fm__");
        let _ = std::fs::remove_dir_all(&tmp);
        let proj_dir = tmp.join("projects/trader");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(
            proj_dir.join("ROLE.md"),
            "---\nskills = [\"ibkr-trader\"]\n---\n\nYou are a trading assistant.",
        )
        .unwrap();

        let projects = load_projects(tmp.to_str().unwrap());
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "trader");
        assert_eq!(projects[0].skills, vec!["ibkr-trader"]);
        assert!(projects[0].instructions.contains("trading assistant"));
        assert!(
            !projects[0].instructions.contains("---"),
            "frontmatter should be stripped"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_projects_agents_md_preferred_over_role_md() {
        let tmp = std::env::temp_dir().join("__kernex_test_projects_agents_md__");
        let _ = std::fs::remove_dir_all(&tmp);
        let proj_dir = tmp.join("projects/modern");
        std::fs::create_dir_all(&proj_dir).unwrap();
        // Both files exist — AGENTS.md should win.
        std::fs::write(proj_dir.join("ROLE.md"), "legacy instructions").unwrap();
        std::fs::write(proj_dir.join("AGENTS.md"), "modern instructions").unwrap();

        let projects = load_projects(tmp.to_str().unwrap());
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].instructions, "modern instructions");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_projects_agents_md_only() {
        let tmp = std::env::temp_dir().join("__kernex_test_projects_agents_only__");
        let _ = std::fs::remove_dir_all(&tmp);
        let proj_dir = tmp.join("projects/agent-only");
        std::fs::create_dir_all(&proj_dir).unwrap();
        // Only AGENTS.md, no ROLE.md.
        std::fs::write(proj_dir.join("AGENTS.md"), "agent instructions").unwrap();

        let projects = load_projects(tmp.to_str().unwrap());
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].instructions, "agent instructions");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_projects_without_frontmatter_backward_compat() {
        let tmp = std::env::temp_dir().join("__kernex_test_projects_no_fm__");
        let _ = std::fs::remove_dir_all(&tmp);
        let proj_dir = tmp.join("projects/simple");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("ROLE.md"), "You are a helper.").unwrap();

        let projects = load_projects(tmp.to_str().unwrap());
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].instructions, "You are a helper.");
        assert!(projects[0].skills.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
