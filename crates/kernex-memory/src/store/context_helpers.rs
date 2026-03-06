//! Helper functions for context building: onboarding stages, system prompt
//! composition, language detection, and relative time formatting.

use super::context::format_user_profile;

/// Compute the next onboarding stage based on current state.
///
/// Stages are sequential — can't skip. Each fires exactly once then advances.
/// - Stage 0: First contact (intro)
/// - Stage 1: 1+ real facts → teach /help
/// - Stage 2: 3+ real facts → teach personality
/// - Stage 3: First task created → teach task management
/// - Stage 4: 5+ real facts → teach projects
/// - Stage 5: Done (no more hints)
pub(super) fn compute_onboarding_stage(
    current_stage: u8,
    real_fact_count: usize,
    has_tasks: bool,
) -> u8 {
    match current_stage {
        0 if real_fact_count >= 1 => 1,
        1 if real_fact_count >= 3 => 2,
        2 if has_tasks => 3,
        3 if real_fact_count >= 5 => 4,
        4 => 5,
        _ => current_stage,
    }
}

/// Return the onboarding hint text for a given stage, or `None` if no hint.
pub(super) fn onboarding_hint_text(stage: u8, language: &str) -> Option<String> {
    match stage {
        0 => Some(format!(
            "\n\nThis is your first conversation with this person. Respond ONLY with this \
             introduction in {language} (adapt naturally, do NOT translate literally):\n\n\
             Start with '\u{1f44b}' followed by an appropriate greeting in {language} on the same line.\n\n\
             Glad to have them here. You are their personal agent — \
             but before jumping into action, you'd like to get to know them a bit.\n\n\
             Ask their name and what they do, so you can be more useful from the start.\n\n\
             Do NOT mention infrastructure, Rust, Claude, or any technical details. \
             Do NOT answer their message yet. Just this introduction, nothing else.",
        )),
        1 => Some(format!(
            "\n\nOnboarding hint: This person is new. At the end of your response, \
             casually mention that they can ask you anything or type /help to see what you can do. \
             Keep it brief and natural — one sentence max. Respond in {language}."
        )),
        2 => Some(format!(
            "\n\nOnboarding hint: This person hasn't customized your personality yet. \
             At the end of your response, casually mention they can tell you how to behave \
             (e.g. 'be more casual') or use /personality. One sentence max, only if it fits naturally. \
             Respond in {language}."
        )),
        3 => Some(format!(
            "\n\nOnboarding hint: This person just created their first task! \
             At the end of your response, briefly mention they can say 'show my tasks' \
             or type /tasks to see scheduled items. One sentence max. Respond in {language}."
        )),
        4 => Some(format!(
            "\n\nOnboarding hint: This person is getting comfortable. \
             At the end of your response, briefly mention they can organize work into projects — \
             just say 'create a project' or type /projects to see how. One sentence max. \
             Respond in {language}."
        )),
        _ => None,
    }
}

/// Parameters for building a dynamic system prompt.
#[allow(clippy::type_complexity)]
pub(super) struct SystemPromptContext<'a> {
    pub base_rules: &'a str,
    pub facts: &'a [(String, String)],
    pub summaries: &'a [(String, String)],
    pub recall: &'a [(String, String, String)],
    pub pending_tasks: &'a [(String, String, String, Option<String>, String, String)],
    pub outcomes: &'a [(i32, String, String, String)],
    pub lessons: &'a [(String, String, String)],
    pub language: &'a str,
    pub onboarding_hint: Option<u8>,
}

/// Build a dynamic system prompt enriched with facts, conversation history, and recalled messages.
pub(super) fn build_system_prompt(ctx: &SystemPromptContext<'_>) -> String {
    let mut prompt = String::from(ctx.base_rules);

    let profile = format_user_profile(ctx.facts);
    if !profile.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&profile);
    }

    if !ctx.summaries.is_empty() {
        prompt.push_str("\n\nRecent conversation history:");
        for (summary, timestamp) in ctx.summaries {
            prompt.push_str(&format!("\n- [{timestamp}] {summary}"));
        }
    }

    if !ctx.recall.is_empty() {
        prompt.push_str("\n\nRelated past context:");
        for (_role, content, timestamp) in ctx.recall {
            let truncated = if content.len() > 200 {
                let boundary = content.floor_char_boundary(200);
                format!("{}...", &content[..boundary])
            } else {
                content.clone()
            };
            prompt.push_str(&format!("\n- [{timestamp}] User: {truncated}"));
        }
    }

    if !ctx.pending_tasks.is_empty() {
        prompt.push_str("\n\nUser's scheduled tasks:");
        for (id, desc, due_at, repeat, task_type, project) in ctx.pending_tasks {
            let r = repeat.as_deref().unwrap_or("once");
            let type_badge = if task_type == "action" {
                " [action]"
            } else {
                ""
            };
            let project_badge = if project.is_empty() {
                String::new()
            } else {
                format!(" ({project})")
            };
            prompt.push_str(&format!(
                "\n- [{id_short}] {desc}{type_badge}{project_badge} (due: {due_at}, {r})",
                id_short = &id[..8.min(id.len())]
            ));
        }
    }

    if !ctx.lessons.is_empty() {
        prompt.push_str("\n\nLearned behavioral rules:");
        for (domain, rule, project) in ctx.lessons {
            if project.is_empty() {
                prompt.push_str(&format!("\n- [{domain}] {rule}"));
            } else {
                prompt.push_str(&format!("\n- [{domain}] ({project}) {rule}"));
            }
        }
    }

    if !ctx.outcomes.is_empty() {
        prompt.push_str("\n\nRecent outcomes:");
        let now = chrono::Utc::now();
        for (score, domain, lesson, timestamp) in ctx.outcomes {
            let ago = format_relative_time(timestamp, &now);
            let sign = if *score > 0 {
                "+"
            } else if *score < 0 {
                "-"
            } else {
                "~"
            };
            prompt.push_str(&format!("\n- [{sign}] {domain}: {lesson} ({ago})"));
        }
    }

    prompt.push_str(&format!(
        "\n\nIMPORTANT: Always respond in {}.",
        ctx.language
    ));

    if let Some(stage) = ctx.onboarding_hint {
        if let Some(hint) = onboarding_hint_text(stage, ctx.language) {
            prompt.push_str(&hint);
        }
    }

    prompt.push_str(
        "\n\nIf the user explicitly asks you to change language (e.g. 'speak in French'), \
         respond in the requested language. Include LANG_SWITCH: <language> on its own line \
         at the END of your response.",
    );

    prompt
}

/// Detect the most likely language of a text using stop-word heuristics.
pub fn detect_language(text: &str) -> &'static str {
    let lower = text.to_lowercase();

    let languages: &[(&str, &[&str])] = &[
        (
            "Spanish",
            &[
                " que ", " por ", " para ", " como ", " con ", " una ", " los ", " las ", " del ",
                " pero ", "hola", "gracias", "necesito", "quiero", "puedes",
            ],
        ),
        (
            "Portuguese",
            &[
                " que ", " com ", " para ", " uma ", " dos ", " das ", " não ", " mais ", " tem ",
                " isso ", "olá", "obrigado", "preciso", "você",
            ],
        ),
        (
            "French",
            &[
                " que ", " les ", " des ", " une ", " est ", " pas ", " pour ", " dans ", " avec ",
                " sur ", "bonjour", "merci", " je ", " nous ",
            ],
        ),
        (
            "German",
            &[
                " und ", " der ", " die ", " das ", " ist ", " nicht ", " ein ", " eine ", " ich ",
                " auf ", " mit ", " für ", " den ", "hallo",
            ],
        ),
        (
            "Italian",
            &[
                " che ", " per ", " con ", " una ", " gli ", " non ", " sono ", " della ", " nel ",
                " questo ", "ciao", "grazie", " io ", " anche ",
            ],
        ),
        (
            "Dutch",
            &[
                " de ", " het ", " een ", " van ", " en ", " niet ", " dat ", " met ", " voor ",
                " zijn ", " ook ", " maar ", "hallo", " ik ",
            ],
        ),
        (
            "Russian",
            &[
                " и ",
                " в ",
                " не ",
                " на ",
                " что ",
                " это ",
                " как ",
                " но ",
                " от ",
                " по ",
                "привет",
                "спасибо",
                " мне ",
                " для ",
            ],
        ),
    ];

    let mut best = "English";
    let mut best_score = 0usize;

    for (lang, words) in languages {
        let score = words.iter().filter(|w| lower.contains(**w)).count();
        if score > best_score {
            best_score = score;
            best = lang;
        }
    }

    let word_count = lower.split_whitespace().count();
    let threshold = if word_count <= 3 { 1 } else { 2 };
    if best_score >= threshold {
        best
    } else {
        "English"
    }
}

/// Format a UTC timestamp as a relative time string (e.g., "3h ago", "1d ago").
fn format_relative_time(timestamp: &str, now: &chrono::DateTime<chrono::Utc>) -> String {
    let parsed = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|dt| dt.and_utc());
    match parsed {
        Some(ts) => {
            let diff = *now - ts;
            let minutes = diff.num_minutes();
            if minutes < 60 {
                format!("{minutes}m ago")
            } else if minutes < 1440 {
                format!("{}h ago", minutes / 60)
            } else {
                format!("{}d ago", minutes / 1440)
            }
        }
        None => timestamp.to_string(),
    }
}
