//! MCP Prompts — canned instructions that guide the LLM through Engram workflows.
//!
//! Available prompts:
//!   review_session      — guided spaced-repetition review (show card → wait → reveal → grade)
//!   create_flashcard    — generate a high-quality flashcard for a topic
//!   create_language_deck — generate a styled, translated, dictionary-annotated language deck
//!   explain_card        — explain a specific card in depth
//!   study_plan          — build a structured study plan from available catalogs

use rmcp::model::{
    ErrorData, GetPromptRequestParams, GetPromptResult, ListPromptsResult, Prompt, PromptArgument,
    PromptMessage, PromptMessageRole,
};

// ── Prompt names ──────────────────────────────────────────────────────────────

pub const PROMPT_REVIEW_SESSION: &str = "review_session";
pub const PROMPT_CREATE_FLASHCARD: &str = "create_flashcard";
pub const PROMPT_CREATE_LANGUAGE_DECK: &str = "create_language_deck";
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
            PROMPT_CREATE_LANGUAGE_DECK,
            Some("Create a deck of language-learning flashcards: source-language sentences with \
                  key words highlighted and translated (dictionary), a translated back side, and \
                  one consistent style reused across the whole deck. No paid AI — you (the calling \
                  model) do the translation yourself; audio is bring-your-own via upload_media."),
            Some(vec![
                PromptArgument::new("topic")
                    .with_description("The theme for the deck, e.g. 'ordering coffee', 'past tense verbs'")
                    .with_required(true),
                PromptArgument::new("source_lang")
                    .with_description("Language the card face is written in, e.g. 'Spanish'")
                    .with_required(true),
                PromptArgument::new("target_lang")
                    .with_description("Language to translate into for the back side and dictionary, e.g. 'English'")
                    .with_required(true),
                PromptArgument::new("count")
                    .with_description("Number of cards to create (default: 10)")
                    .with_required(false),
                PromptArgument::new("catalog_id")
                    .with_description("UUID of an existing catalog to add the cards to (optional — creates a new catalog if omitted)")
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
        PROMPT_CREATE_LANGUAGE_DECK => {
            let topic = get_arg("topic").unwrap_or_else(|| "<topic>".to_string());
            let source_lang = get_arg("source_lang").unwrap_or_else(|| "<source_lang>".to_string());
            let target_lang = get_arg("target_lang").unwrap_or_else(|| "<target_lang>".to_string());
            let count = get_arg("count").unwrap_or_else(|| "10".to_string());
            let catalog_id = get_arg("catalog_id");
            Ok(create_language_deck(
                &topic,
                &source_lang,
                &target_lang,
                &count,
                catalog_id.as_deref(),
            ))
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

fn create_language_deck(
    topic: &str,
    source_lang: &str,
    target_lang: &str,
    count: &str,
    catalog_id: Option<&str>,
) -> GetPromptResult {
    let catalog_line = match catalog_id {
        Some(id) => {
            format!("Add all cards to the existing catalog `{id}` (call `generate_cards`).")
        }
        None => "Create a new catalog for this deck (call `generate_catalog_with_cards`); pick a \
                 short, descriptive name from the topic."
            .to_string(),
    };

    GetPromptResult::new(vec![user_msg(format!(
        "Create a {count}-card language-learning deck:\n\
             \n\
             **Topic:** {topic}\n\
             **Source language (face):** {source_lang}\n\
             **Target language (back + dictionary):** {target_lang}\n\
             \n\
             Before drafting, read the `engram://card-schema` resource — Example 4 shows the \
             user-supplied-audio pattern referenced below.\n\
             \n\
             For EACH card:\n\
             1. Write a natural face sentence/phrase in {source_lang}.\n\
             2. Highlight 1-4 key words with `rich_text` — bold + fontColor \"#27AE60\" (green) — \
                and add each highlighted word to `face.dictionary`, translated into {target_lang}.\n\
             3. Translate the full sentence into {target_lang} for `back.text`. If a grammar \
                reference is useful (e.g. infinitive/root form), highlight it in `back.rich_text` \
                the same green.\n\
             \n\
             Pick ONE `CardStyle` (font/color/background) before drafting the first card, then \
             reuse that exact same style on every card's face and every card's back — this is \
             what makes the whole deck look consistent; there is no catalog-level style field, so \
             consistency comes from you repeating the same style object.\n\
             \n\
             {catalog_line}\n\
             \n\
             No paid AI is used anywhere in this flow — you do the translation and dictionary \
             yourself. Audio and a catalog cover image are optional and fully bring-your-own: if \
             the user already has a recording or an image, call `upload_media` first and set \
             `audio_id`/`visual_id` (cards) or `image_id` (catalog) to the returned `media_id` \
             before your final generate call — never fabricate a UUID. If they don't have media \
             yet, create the deck now and offer to add it afterward once they do (via `update_card` \
             for audio/images on existing cards). More worked examples, including a full deck \
             walkthrough, are in `docs/prompt-examples.md` in the engramo-mcp repo."
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
    fn test_list_all_returns_five_prompts() {
        let result = list_all();
        assert_eq!(result.prompts.len(), 5);
        let names: Vec<&str> = result.prompts.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&PROMPT_REVIEW_SESSION));
        assert!(names.contains(&PROMPT_CREATE_FLASHCARD));
        assert!(names.contains(&PROMPT_CREATE_LANGUAGE_DECK));
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
    fn test_create_language_deck_includes_topic_and_langs() {
        let params =
            GetPromptRequestParams::new(PROMPT_CREATE_LANGUAGE_DECK).with_arguments(args(&[
                ("topic", "ordering coffee"),
                ("source_lang", "Spanish"),
                ("target_lang", "English"),
            ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("ordering coffee"), "{text}");
            assert!(text.contains("Spanish"), "{text}");
            assert!(text.contains("English"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_create_language_deck_default_count() {
        let params =
            GetPromptRequestParams::new(PROMPT_CREATE_LANGUAGE_DECK).with_arguments(args(&[
                ("topic", "greetings"),
                ("source_lang", "French"),
                ("target_lang", "English"),
            ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("10-card"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_create_language_deck_custom_count() {
        let params =
            GetPromptRequestParams::new(PROMPT_CREATE_LANGUAGE_DECK).with_arguments(args(&[
                ("topic", "greetings"),
                ("source_lang", "French"),
                ("target_lang", "English"),
                ("count", "3"),
            ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("3-card"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_create_language_deck_with_catalog_hint() {
        let params =
            GetPromptRequestParams::new(PROMPT_CREATE_LANGUAGE_DECK).with_arguments(args(&[
                ("topic", "verbs"),
                ("source_lang", "Spanish"),
                ("target_lang", "English"),
                ("catalog_id", "00000000-0000-0000-0000-000000000002"),
            ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(
                text.contains("00000000-0000-0000-0000-000000000002"),
                "{text}"
            );
            assert!(text.contains("generate_cards"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_create_language_deck_no_catalog_hint_uses_catalog_with_cards() {
        let params =
            GetPromptRequestParams::new(PROMPT_CREATE_LANGUAGE_DECK).with_arguments(args(&[
                ("topic", "verbs"),
                ("source_lang", "Spanish"),
                ("target_lang", "English"),
            ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("generate_catalog_with_cards"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_create_language_deck_references_card_schema_and_dictionary() {
        let params =
            GetPromptRequestParams::new(PROMPT_CREATE_LANGUAGE_DECK).with_arguments(args(&[
                ("topic", "verbs"),
                ("source_lang", "Spanish"),
                ("target_lang", "English"),
            ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("engram://card-schema"), "{text}");
            assert!(text.contains("dictionary"), "{text}");
            assert!(text.contains("CardStyle"), "{text}");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_create_language_deck_mentions_upload_media_for_audio_no_paid_ai() {
        let params =
            GetPromptRequestParams::new(PROMPT_CREATE_LANGUAGE_DECK).with_arguments(args(&[
                ("topic", "verbs"),
                ("source_lang", "Spanish"),
                ("target_lang", "English"),
            ]));
        let result = get(params).unwrap();
        if let PromptMessageContent::Text { text } = &result.messages[0].content {
            assert!(text.contains("upload_media"), "{text}");
            assert!(text.contains("No paid AI"), "{text}");
            assert!(
                !text.to_lowercase().contains("generate_tts_for_cards"),
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
