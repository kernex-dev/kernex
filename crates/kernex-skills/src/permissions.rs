//! Permission model for skills security.
//!
//! Skills declare permissions in their frontmatter, users approve them at install time,
//! and the runtime enforces them via sandbox profiles.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Permission categories that a skill can request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Permissions {
    /// File system paths the skill can access.
    /// Format: "read:path" or "write:path" or "!path" (deny).
    #[serde(default)]
    pub files: Vec<String>,

    /// Network hosts the skill can contact.
    /// Supports wildcards: "*.example.com", "api.github.com".
    #[serde(default)]
    pub network: Vec<String>,

    /// Environment variables the skill can read.
    #[serde(default)]
    pub env: Vec<String>,

    /// Commands/binaries the skill can execute.
    #[serde(default)]
    pub commands: Vec<String>,
}

impl Permissions {
    /// Check if the skill has any permissions declared.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
            && self.network.is_empty()
            && self.env.is_empty()
            && self.commands.is_empty()
    }

    /// Get all read paths.
    pub fn read_paths(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter_map(|p| p.strip_prefix("read:"))
            .collect()
    }

    /// Get all write paths.
    pub fn write_paths(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter_map(|p| p.strip_prefix("write:"))
            .collect()
    }

    /// Get all denied paths.
    pub fn denied_paths(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter_map(|p| p.strip_prefix('!'))
            .collect()
    }
}

/// Trust level assigned to a skill based on its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TrustLevel {
    /// Official organizations (anthropics, kernex-dev, modelcontextprotocol).
    /// Permissions are shown but can be auto-approved.
    Verified,

    /// Listed on skills.sh but from community authors.
    /// Requires explicit user approval.
    Community,

    /// User-created skills in local directory.
    /// Full trust, no approval needed.
    Local,

    /// Unknown source, unverified author.
    /// Requires explicit approval with high-risk warnings.
    #[default]
    Untrusted,
}

impl TrustLevel {
    /// Whether this trust level requires explicit user approval.
    pub fn requires_approval(&self) -> bool {
        !matches!(self, Self::Local)
    }

    /// Whether high-risk permission warnings should be shown.
    pub fn show_high_risk_warnings(&self) -> bool {
        matches!(self, Self::Untrusted | Self::Community)
    }
}

/// High-risk permission patterns that trigger warnings.
pub struct RiskDetector {
    sensitive_paths: Vec<&'static str>,
    sensitive_env_patterns: Vec<&'static str>,
}

impl Default for RiskDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl RiskDetector {
    /// Create a new risk detector with default patterns.
    pub fn new() -> Self {
        Self {
            sensitive_paths: vec![
                "~/.ssh",
                "~/.gnupg",
                "~/.aws",
                "~/.azure",
                "~/.config/gcloud",
                "~/.kube",
                "~/.docker",
                "/etc/passwd",
                "/etc/shadow",
            ],
            sensitive_env_patterns: vec![
                "TOKEN",
                "SECRET",
                "KEY",
                "PASSWORD",
                "CREDENTIAL",
                "PRIVATE",
            ],
        }
    }

    /// Detect high-risk file permissions.
    pub fn detect_risky_paths(&self, permissions: &Permissions) -> Vec<RiskWarning> {
        let mut warnings = Vec::new();

        for path in &permissions.files {
            let clean_path = path
                .strip_prefix("read:")
                .or_else(|| path.strip_prefix("write:"))
                .unwrap_or(path);

            for sensitive in &self.sensitive_paths {
                if clean_path.contains(sensitive) || path_matches_pattern(clean_path, sensitive) {
                    warnings.push(RiskWarning {
                        category: RiskCategory::SensitiveFile,
                        description: format!("Access to {}", sensitive),
                        pattern: path.clone(),
                    });
                    break;
                }
            }

            // Check for hidden files access
            if clean_path.contains("/.*") || clean_path.ends_with("/.*") {
                warnings.push(RiskWarning {
                    category: RiskCategory::HiddenFiles,
                    description: "Access to hidden files/directories".into(),
                    pattern: path.clone(),
                });
            }
        }

        warnings
    }

    /// Detect high-risk environment variable permissions.
    pub fn detect_risky_env(&self, permissions: &Permissions) -> Vec<RiskWarning> {
        let mut warnings = Vec::new();

        for env_var in &permissions.env {
            let upper = env_var.to_uppercase();
            for pattern in &self.sensitive_env_patterns {
                if upper.contains(pattern) {
                    warnings.push(RiskWarning {
                        category: RiskCategory::SensitiveEnv,
                        description: format!("Access to {} environment variable", env_var),
                        pattern: env_var.clone(),
                    });
                    break;
                }
            }
        }

        warnings
    }

    /// Detect unrestricted network access.
    pub fn detect_risky_network(&self, permissions: &Permissions) -> Vec<RiskWarning> {
        let mut warnings = Vec::new();

        for host in &permissions.network {
            if host == "*" {
                warnings.push(RiskWarning {
                    category: RiskCategory::UnrestrictedNetwork,
                    description: "Unrestricted network access to any host".into(),
                    pattern: host.clone(),
                });
            }
        }

        warnings
    }

    /// Run all risk detection checks.
    pub fn detect_all_risks(&self, permissions: &Permissions) -> Vec<RiskWarning> {
        let mut warnings = Vec::new();
        warnings.extend(self.detect_risky_paths(permissions));
        warnings.extend(self.detect_risky_env(permissions));
        warnings.extend(self.detect_risky_network(permissions));
        warnings
    }

    /// Check if any high-risk permissions are present.
    pub fn has_high_risk(&self, permissions: &Permissions) -> bool {
        !self.detect_all_risks(permissions).is_empty()
    }
}

/// A warning about a risky permission.
#[derive(Debug, Clone)]
pub struct RiskWarning {
    pub category: RiskCategory,
    pub description: String,
    pub pattern: String,
}

/// Categories of permission risks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskCategory {
    /// Access to SSH keys, GPG keys, cloud credentials.
    SensitiveFile,
    /// Access to hidden files/directories.
    HiddenFiles,
    /// Access to environment variables containing secrets.
    SensitiveEnv,
    /// Unrestricted network access.
    UnrestrictedNetwork,
}

impl RiskCategory {
    /// Get a human-readable label for this risk category.
    pub fn label(&self) -> &'static str {
        match self {
            Self::SensitiveFile => "Sensitive Files",
            Self::HiddenFiles => "Hidden Files",
            Self::SensitiveEnv => "Sensitive Environment",
            Self::UnrestrictedNetwork => "Unrestricted Network",
        }
    }
}

/// Check if a path matches a pattern (simple glob support).
fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    if pattern.contains('*') {
        // Simple wildcard matching
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            let (prefix, suffix) = (parts[0], parts[1]);
            return path.starts_with(prefix) && path.ends_with(suffix);
        }
    }
    path.starts_with(pattern)
}

/// Trusted organizations whose skills can be auto-approved.
pub const DEFAULT_TRUSTED_ORGS: &[&str] = &["anthropics", "kernex-dev", "modelcontextprotocol"];

/// Determine trust level based on skill source.
pub fn determine_trust_level(source: &str, trusted_orgs: &HashSet<String>) -> TrustLevel {
    // Local skills (no source URL)
    if source.is_empty() || source.starts_with('/') || source.starts_with('~') {
        return TrustLevel::Local;
    }

    // GitHub-style org/repo format
    if let Some(org) = source.split('/').next() {
        if trusted_orgs.contains(org) || DEFAULT_TRUSTED_ORGS.contains(&org) {
            return TrustLevel::Verified;
        }
    }

    // skills.sh source
    if source.contains("skills.sh") {
        return TrustLevel::Community;
    }

    TrustLevel::Untrusted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permissions_is_empty() {
        assert!(Permissions::default().is_empty());
        assert!(!Permissions {
            files: vec!["read:~/.config".into()],
            ..Default::default()
        }
        .is_empty());
    }

    #[test]
    fn test_permissions_path_helpers() {
        let perms = Permissions {
            files: vec![
                "read:~/.config/app".into(),
                "write:~/.data/app".into(),
                "!~/.ssh".into(),
            ],
            ..Default::default()
        };
        assert_eq!(perms.read_paths(), vec!["~/.config/app"]);
        assert_eq!(perms.write_paths(), vec!["~/.data/app"]);
        assert_eq!(perms.denied_paths(), vec!["~/.ssh"]);
    }

    #[test]
    fn test_trust_level_requires_approval() {
        assert!(!TrustLevel::Local.requires_approval());
        assert!(TrustLevel::Verified.requires_approval());
        assert!(TrustLevel::Community.requires_approval());
        assert!(TrustLevel::Untrusted.requires_approval());
    }

    #[test]
    fn test_determine_trust_level_local() {
        let trusted = HashSet::new();
        assert_eq!(determine_trust_level("", &trusted), TrustLevel::Local);
        assert_eq!(
            determine_trust_level("/home/user/.kx/skills/my-skill", &trusted),
            TrustLevel::Local
        );
        assert_eq!(
            determine_trust_level("~/.kx/skills/my-skill", &trusted),
            TrustLevel::Local
        );
    }

    #[test]
    fn test_determine_trust_level_verified() {
        let trusted = HashSet::new();
        assert_eq!(
            determine_trust_level("anthropics/skills", &trusted),
            TrustLevel::Verified
        );
        assert_eq!(
            determine_trust_level("kernex-dev/my-skill", &trusted),
            TrustLevel::Verified
        );
    }

    #[test]
    fn test_determine_trust_level_custom_trusted() {
        let mut trusted = HashSet::new();
        trusted.insert("my-org".into());
        assert_eq!(
            determine_trust_level("my-org/skill", &trusted),
            TrustLevel::Verified
        );
    }

    #[test]
    fn test_determine_trust_level_untrusted() {
        let trusted = HashSet::new();
        assert_eq!(
            determine_trust_level("random-user/skill", &trusted),
            TrustLevel::Untrusted
        );
    }

    #[test]
    fn test_risk_detector_sensitive_paths() {
        let detector = RiskDetector::new();
        let perms = Permissions {
            files: vec!["read:~/.ssh/id_rsa".into()],
            ..Default::default()
        };
        let risks = detector.detect_risky_paths(&perms);
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0].category, RiskCategory::SensitiveFile);
    }

    #[test]
    fn test_risk_detector_sensitive_env() {
        let detector = RiskDetector::new();
        let perms = Permissions {
            env: vec!["GITHUB_TOKEN".into(), "HOME".into()],
            ..Default::default()
        };
        let risks = detector.detect_risky_env(&perms);
        assert_eq!(risks.len(), 1);
        assert!(risks[0].description.contains("GITHUB_TOKEN"));
    }

    #[test]
    fn test_risk_detector_unrestricted_network() {
        let detector = RiskDetector::new();
        let perms = Permissions {
            network: vec!["*".into()],
            ..Default::default()
        };
        let risks = detector.detect_risky_network(&perms);
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0].category, RiskCategory::UnrestrictedNetwork);
    }

    #[test]
    fn test_risk_detector_no_risks() {
        let detector = RiskDetector::new();
        let perms = Permissions {
            files: vec!["read:~/.config/myapp".into()],
            network: vec!["api.github.com".into()],
            env: vec!["HOME".into()],
            commands: vec!["npx".into()],
        };
        assert!(!detector.has_high_risk(&perms));
    }

    #[test]
    fn test_path_matches_pattern() {
        // Exact prefix matching
        assert!(path_matches_pattern("~/.ssh/id_rsa", "~/.ssh"));
        assert!(path_matches_pattern("/etc/passwd", "/etc/passwd"));
        assert!(!path_matches_pattern("/home/user/.config", "~/.ssh"));
        // Wildcard matching
        assert!(path_matches_pattern("~/.config/app/data", "~/.config/*"));
        assert!(path_matches_pattern("logs.txt", "*.txt"));
    }
}
