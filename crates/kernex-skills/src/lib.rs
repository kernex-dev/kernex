//! Skill and project loader for Kernex.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! Scans `{data_dir}/skills/*/SKILL.md` and `{data_dir}/projects/*/ROLE.md`
//! for definitions and exposes them to the system prompt so the AI knows
//! what tools and contexts are available.

mod parse;
mod projects;
mod skills;

pub use projects::{ensure_projects_dir, get_project_instructions, load_projects, Project};
pub use skills::{
    build_skill_prompt, load_skills, match_skill_toolboxes, match_skill_triggers,
    migrate_flat_skills, Skill,
};
