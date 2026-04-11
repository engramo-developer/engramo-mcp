//! MCP Prompts — canned instructions that guide the LLM through Engram workflows.
//!
//! Available prompts:
//!   review_session   — guided spaced-repetition review (show card → wait → reveal → grade)
//!   create_flashcard — generate a high-quality flashcard for a topic
//!   explain_card     — explain a specific card in depth
//!   study_plan       — build a structured study plan from available catalogs

use rmcp::model::{
    ErrorData, GetPromptRequestParams, GetPromptResult, ListPromptsResult, Prompt, PromptArgument,
    PromptMessage, PromptMessageRole,
};

// ── Prompt names ──────────────────────────────────────────────────────────────

pub const PROMPT_REVIEW_SESSION: &str = "review_session";
pub const PROMPT_CREATE_FLASHCARD: &str = "create_flashcard";
pub const PROMPT_EXPLAIN_CARD: &str = "explain_card";
pub const PROMPT_STUDY_PLAN: &str = "study_plan";

// ── Prompt list ───────────────────────────────────────────────────────────────

pub fn list_all() -> ListPromptsResult {
    ListPromptsResult::with_all_items(vec![
        Prompt::new(
            PROMPT_REVIEW_SESSION,
            Some("Start a spaced-repetition review session. Show one card at a time, wait for the user to recall the answer, then reveal and grade."),
            Some(vec![
                PromptArgument::new("limit")
                    .with_description("Maximum number of cards to review (default: 20)")
                    .with_required(false),
            ]),
        ),
        Prompt::new(
            PROMPT_CREATE_FLASHCARD,
            Some("Create a high-quality flashcard for a given topic. Produces a concise question (face) and a clear answer (back)."),
            Some(vec![
                PromptArgument::new("topic")
                    .with_description("The concept or topic to create a flashcard for")
                    .with_required(true),
                PromptArgument::new("catalog_id")
                    .with_description("UUID of the catalog to add the card to (optional)")
                    .with_required(false),
            ]),
        ),
        Prompt::new(
            PROMPT_EXPLAIN_CARD,
            Some("Explain a specific flashcard in depth — expand the back with examples, analogies, and related concepts."),
            Some(vec![
                PromptArgument::new("face")
                    .with_description("The front (question) of the card")
                    .with_required(true),
                PromptArgument::new("back")
                    .with_description("The back (answer) of the card")
                    .with_required(true),
            ]),
        ),
        Prompt::new(
            PROMPT_STUDY_PLAN,
            Some("Build a structured study plan. Lists available catalogs and due cards, then suggests a session order."),
            Some(vec![
                PromptArgument::new("goal")
                    .with_description("What the user wants to accomplish (e.g. 'review all due cards', 'learn Rust basics')")
                    .with_required(false),
            ]),
        ),
    ])
}

// ── Prompt dispatch ───────────────────────────────────────────────────────────

pub fn get(params: GetPromptRequestParams) -> Result<GetPromptResult, ErrorData> {
    let args = params.arguments.unwrap_or_default();
    // JsonObject = Map<String, serde_json::Value>; extract as string if possible.
    let get_arg =
        |key: &str| -> Option<String> { args.get(key).and_then(|v| v.as_str()).map(str::to_owned) };

    match params.name.as_str() {
        PROMPT_REVIEW_SESSION => Ok(review_session(get_arg("limit"))),
        PROMPT_CREATE_FLASHCARD => {
            let topic = get_arg("topic").unwrap_or_else(|| "<topic>".to_string());
            let catalog_id = get_arg("catalog_id");
            Ok(create_flashcard(&topic, catalog_id.as_deref()))
        }
        PROMPT_EXPLAIN_CARD => {
            let face = get_arg("face").unwrap_or_else(|| "<face>".to_string());
            let back = get_arg("back").unwrap_or_else(|| "<back>".to_string());
            Ok(explain_card(&face, &back))
        }
        PROMPT_STUDY_PLAN => {
            let goal = get_arg("goal");
            Ok(study_plan(goal.as_deref()))
        }
        other => Err(ErrorData::invalid_params(
            format!("Unknown prompt: {other}"),
            None,
        )),
    }
}

// ── Prompt builders ───────────────────────────────────────────────────────────

fn user_msg(text: impl Into<String>) -> PromptMessage {
    PromptMessage::new_text(PromptMessageRole::User, text)
}

fn assistant_msg(text: impl Into<String>) -> PromptMessage {
    PromptMessage::new_text(PromptMessageRole::Assistant, text)
}

fn review_session(limit: Option<String>) -> GetPromptResult {
    let limit_str = limit.unwrap_or_else(|| "20".to_string());
    GetPromptResult::new(vec![
        user_msg(format!(
            "Please start a spaced-repetition review session for me.\n\
             \n\
             Rules:\n\
             1. Call `get_due_cards` with limit={limit_str} to fetch today's due cards.\n\
             2. For each card:\n\
                a. Show ONLY the face (question). Do not show the answer yet.\n\
                b. Wait for me to say I'm ready (e.g. \"show answer\" or \"reveal\").\n\
                c. Reveal the back (answer).\n\
                d. Ask me to rate my recall: again / hard / good / easy, and note it down.\n\
             3. After all cards are reviewed, show a summary with your recall ratings.\n\
                Submit your grades via the Engram app after the session.\n\
             \n\
             Start now — show the first card."
        )),
        assistant_msg(
            "Understood! I'll fetch your due cards and guide you through the session one card at \
             a time. I will NOT reveal the answer until you ask.",
        ),
    ])
}

fn create_flashcard(topic: &str, catalog_id: Option<&str>) -> GetPromptResult {
    let catalog_hint = catalog_id
        .map(|id| format!(" Add it to catalog `{id}`."))
        .unwrap_or_default();

    GetPromptResult::new(vec![user_msg(format!(
        "Create a high-quality flashcard for the following topic:\n\
             \n\
             **Topic:** {topic}\n\
             \n\
             Before drafting the card, read the `engram://card-schema` resource for the complete \
             format guide (validation rules, concrete examples, color codes).\n\
             \n\
             Guidelines:\n\
             - Face: a single, focused question. Avoid yes/no questions.\n\
             - Back: a concise, precise answer (1-3 sentences). Include an example if helpful.\n\
             - Use `rich_text` spans for code (fontFamily=monospace) and key terms (bold=true).\n\
             - After drafting, call `generate_card` to save it.{catalog_hint}"
    ))])
}

fn explain_card(face: &str, back: &str) -> GetPromptResult {
    GetPromptResult::new(vec![user_msg(format!(
        "Please explain the following flashcard in depth.\n\
             \n\
             **Question (face):** {face}\n\
             **Answer (back):** {back}\n\
             \n\
             Explanation should include:\n\
             - Why the answer is correct\n\
             - A concrete example or analogy\n\
             - Common misconceptions to avoid\n\
             - Related concepts worth knowing"
    ))])
}

fn study_plan(goal: Option<&str>) -> GetPromptResult {
    let goal_line = goal
        .map(|g| format!("\n**My goal:** {g}"))
        .unwrap_or_default();

    GetPromptResult::new(vec![user_msg(format!(
        "Help me plan today's study session.{goal_line}\n\
             \n\
             Steps:\n\
             1. Call `get_due_cards` (limit=5) to see what's due.\n\
             2. Call `list_catalogs` to see available catalogs.\n\
             3. Based on what's due and my goal, suggest:\n\
                - Which catalogs to focus on\n\
                - How many cards to review\n\
                - Whether to add any new cards from a specific catalog\n\
             4. Offer to start the review session when I'm ready."
    ))])
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rmcp::model::{JsonObject, PromptMessageContent};
    use serde_json::Value;

    use super::*;

    fn args(pairs: &[(&str, &str)]) -> JsonObject {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn test_list_all_returns_four_prompts() {
        let result = list_all();
        assert_eq!(result.prompts.len(), 4);
        let names: Vec<&str> = result.prompts.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&PROMPT_REVIEW_SESSION));
        assert!(names.contains(&PROMPT_CREATE_FLASHCARD));
        assert!(names.contains(&PROMPT_EXPLAIN_CARD));
        assert!(names.contains(&PROMPT_STUDY_PLAN));
    }

    #[test]
    fn test_review_session_prompt_has_user_and_assistant_messages() {
        let params = GetPromptRequestParams::new(PROMPT_REVIEW_SESSION);
        let result = get(params).unwrap();
        assert_eq!(result.messages.len(), 2);
        assert!(matches!(result.messages[0].role, PromptMessageRole::User));
        assert!(matches!(
            result.messages[1].role,
            PromptMessageRole::Assistant
        ));
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("get_due_cards"), "{text}");
            assert!(text.contains("Engram app"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_review_session_custom_limit() {
        let params = GetPromptRequestParams::new(PROMPT_REVIEW_SESSION)
            .with_arguments(args(&[("limit", "10")]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("limit=10"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_create_flashcard_includes_topic() {
        let params = GetPromptRequestParams::new(PROMPT_CREATE_FLASHCARD)
            .with_arguments(args(&[("topic", "Rust ownership")]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("Rust ownership"), "{text}");
            assert!(text.contains("generate_card"), "{text}");
            assert!(text.contains("engram://card-schema"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_create_flashcard_with_catalog_hint() {
        let params = GetPromptRequestParams::new(PROMPT_CREATE_FLASHCARD).with_arguments(args(&[
            ("topic", "Lifetimes"),
            ("catalog_id", "00000000-0000-0000-0000-000000000001"),
        ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(
                text.contains("00000000-0000-0000-0000-000000000001"),
                "{text}"
            );
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_explain_card_includes_face_and_back() {
        let params = GetPromptRequestParams::new(PROMPT_EXPLAIN_CARD).with_arguments(args(&[
            ("face", "What is ownership?"),
            ("back", "Every value has one owner."),
        ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("What is ownership?"), "{text}");
            assert!(text.contains("Every value has one owner."), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_study_plan_includes_goal() {
        let params = GetPromptRequestParams::new(PROMPT_STUDY_PLAN)
            .with_arguments(args(&[("goal", "review all due Rust cards")]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("review all due Rust cards"), "{text}");
            assert!(text.contains("get_due_cards"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_unknown_prompt_returns_invalid_params_error() {
        let params = GetPromptRequestParams::new("nonexistent_prompt");
        let err = get(params).unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn test_all_prompts_have_descriptions() {
        let result = list_all();
        for p in &result.prompts {
            assert!(
                p.description.is_some(),
                "prompt '{}' is missing a description",
                p.name
            );
        }
    }
}
