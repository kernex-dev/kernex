//! Context building and user profile formatting.
//!
//! Helper functions for onboarding, system prompt composition, language
//! detection, and relative time formatting live in `context_helpers`.

use super::Store;
use kernex_core::{
    config::SYSTEM_FACT_KEYS,
    context::{Context, ContextEntry, ContextNeeds},
    error::KernexError,
    message::Request,
};

// Re-export helpers so existing `super::context::*` paths in tests keep working.
pub use super::context_helpers::detect_language;
#[cfg(test)]
pub(super) use super::context_helpers::onboarding_hint_text;
pub(super) use super::context_helpers::{
    build_system_prompt, compute_onboarding_stage, SystemPromptContext,
};

/// Identity fact keys — shown first in the user profile.
const IDENTITY_KEYS: &[&str] = &["name", "preferred_name", "pronouns"];

/// Context fact keys — shown second in the user profile.
const CONTEXT_KEYS: &[&str] = &["timezone", "location", "occupation"];

impl Store {
    /// Build a conversation context from memory for the provider.
    ///
    /// The `channel` parameter identifies the communication channel since
    /// `Request` is channel-agnostic.
    pub async fn build_context(
        &self,
        channel: &str,
        incoming: &Request,
        base_system_prompt: &str,
        needs: &ContextNeeds,
        active_project: Option<&str>,
    ) -> Result<Context, KernexError> {
        let project_key = active_project.unwrap_or("");
        let conv_id = self
            .get_or_create_conversation(channel, &incoming.sender_id, project_key)
            .await?;

        let history_fut = async {
            let rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT role, content FROM (\
                     SELECT role, content, timestamp FROM messages \
                     WHERE conversation_id = ? ORDER BY timestamp DESC LIMIT ?\
                 ) ORDER BY timestamp ASC",
            )
            .bind(&conv_id)
            .bind(self.max_context_messages as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| KernexError::Store(format!("query failed: {e}")))?;

            Ok::<Vec<ContextEntry>, KernexError>(
                rows.into_iter()
                    .map(|(role, content)| ContextEntry { role, content })
                    .collect(),
            )
        };

        let facts_fut = async {
            self.get_facts(&incoming.sender_id)
                .await
                .unwrap_or_default()
        };

        let summaries_fut = async {
            if needs.summaries {
                self.get_recent_summaries(channel, &incoming.sender_id, 3)
                    .await
                    .unwrap_or_default()
            } else {
                vec![]
            }
        };

        let recall_fut = async {
            if needs.recall {
                self.search_messages(&incoming.text, &conv_id, &incoming.sender_id, 5)
                    .await
                    .unwrap_or_default()
            } else {
                vec![]
            }
        };

        let tasks_fut = async {
            if needs.pending_tasks {
                self.get_tasks_for_sender(&incoming.sender_id)
                    .await
                    .unwrap_or_default()
            } else {
                vec![]
            }
        };

        let outcomes_fut = async {
            if needs.outcomes {
                self.get_recent_outcomes(&incoming.sender_id, 15, active_project)
                    .await
                    .unwrap_or_default()
            } else {
                vec![]
            }
        };

        let lessons_fut = async {
            self.get_lessons(&incoming.sender_id, active_project)
                .await
                .unwrap_or_default()
        };

        let (history_res, facts, summaries, recall, pending_tasks, outcomes, lessons) = tokio::join!(
            history_fut,
            facts_fut,
            summaries_fut,
            recall_fut,
            tasks_fut,
            outcomes_fut,
            lessons_fut,
        );

        let history = history_res?;

        // Resolve language: stored preference > auto-detect > English.
        let language =
            if let Some((_, lang)) = facts.iter().find(|(k, _)| k == "preferred_language") {
                lang.clone()
            } else {
                let detected = detect_language(&incoming.text).to_string();
                let _ = self
                    .store_fact(&incoming.sender_id, "preferred_language", &detected)
                    .await;
                detected
            };

        // Progressive onboarding: compute stage and inject hint on transitions.
        let real_fact_count = facts
            .iter()
            .filter(|(k, _)| !SYSTEM_FACT_KEYS.contains(&k.as_str()))
            .count();
        let has_tasks = !pending_tasks.is_empty();

        let current_stage: u8 = facts
            .iter()
            .find(|(k, _)| k == "onboarding_stage")
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);

        let new_stage = compute_onboarding_stage(current_stage, real_fact_count, has_tasks);

        let onboarding_hint = if new_stage != current_stage {
            let _ = self
                .store_fact(
                    &incoming.sender_id,
                    "onboarding_stage",
                    &new_stage.to_string(),
                )
                .await;
            Some(new_stage)
        } else if current_stage == 0 && real_fact_count == 0 {
            Some(0u8)
        } else {
            if facts.iter().all(|(k, _)| k != "onboarding_stage") && current_stage == 0 {
                let bootstrapped = compute_onboarding_stage(0, real_fact_count, has_tasks);
                let final_stage = (0..=4).fold(0u8, |s, _| {
                    compute_onboarding_stage(s, real_fact_count, has_tasks)
                });
                if final_stage > 0 {
                    let _ = self
                        .store_fact(
                            &incoming.sender_id,
                            "onboarding_stage",
                            &final_stage.to_string(),
                        )
                        .await;
                }
                let _ = bootstrapped;
                None
            } else {
                None
            }
        };

        let facts_for_prompt: &[(String, String)] = if needs.profile { &facts } else { &[] };
        let system_prompt = build_system_prompt(&SystemPromptContext {
            base_rules: base_system_prompt,
            facts: facts_for_prompt,
            summaries: &summaries,
            recall: &recall,
            pending_tasks: &pending_tasks,
            outcomes: &outcomes,
            lessons: &lessons,
            language: &language,
            onboarding_hint,
        });

        Ok(Context {
            system_prompt,
            history,
            current_message: incoming.text.clone(),
            mcp_servers: Vec::new(),
            max_turns: None,
            allowed_tools: None,
            model: None,
            session_id: None,
            agent_name: None,
        })
    }
}

/// Format user facts into a structured profile, filtering system keys
/// and grouping identity facts first, then context, then the rest.
pub fn format_user_profile(facts: &[(String, String)]) -> String {
    let user_facts: Vec<&(String, String)> = facts
        .iter()
        .filter(|(k, _)| !SYSTEM_FACT_KEYS.contains(&k.as_str()))
        .collect();

    if user_facts.is_empty() {
        return String::new();
    }

    let mut lines = vec!["User profile:".to_string()];

    for key in IDENTITY_KEYS {
        if let Some((_, v)) = user_facts.iter().find(|(k, _)| k == key) {
            lines.push(format!("- {key}: {v}"));
        }
    }

    for key in CONTEXT_KEYS {
        if let Some((_, v)) = user_facts.iter().find(|(k, _)| k == key) {
            lines.push(format!("- {key}: {v}"));
        }
    }

    let known_keys: Vec<&str> = IDENTITY_KEYS
        .iter()
        .chain(CONTEXT_KEYS.iter())
        .copied()
        .collect();
    for (k, v) in &user_facts {
        if !known_keys.contains(&k.as_str()) {
            lines.push(format!("- {k}: {v}"));
        }
    }

    lines.join("\n")
}
