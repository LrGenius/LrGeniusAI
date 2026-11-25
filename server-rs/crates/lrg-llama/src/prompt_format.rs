//! How a conversation is put into the turn format a model was trained on.
//!
//! A GGUF carries its chat template as Jinja, but `llama_chat_apply_template`
//! is the legacy C path: it applies only templates it recognises, by built-in
//! name or by sniffing familiar markers, and answers anything else with a bare
//! `-1`. So for most models the choice is between the model's own template
//! (when llama.cpp recognises it) and a built-in one picked by architecture.
//!
//! Gemma 4 is the exception, and the reason this module exists (issue #490).
//! Its turns are `<|turn>user … <turn|>`, not the `<start_of_turn>user …
//! <end_of_turn>` of Gemma 1-3, and its vocabulary has no `<start_of_turn>`
//! token at all. llama.cpp's built-in `gemma` template — the one an
//! architecture match used to pick — therefore wrote turn markers that reached
//! the model as plain text: no system turn, no turn boundaries, no cue that
//! the answer starts. Decoding under the JSON grammar still produced JSON, but
//! out of distribution enough that Gemma 4 12B looped on one photo in five or
//! ten. Gemma 4 is rendered here instead, exactly as its canonical template
//! does (see [`render_gemma4`]).
//!
//! The same kind of mismatch can hit any fallback, so [`resolve`] also checks
//! that the template it settles on writes markers the vocabulary knows as
//! control tokens, and warns when it does not.

use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::token_type::LlamaTokenAttr;

/// Gemma 4's turn delimiters. Both are single control tokens in its
/// vocabulary (ids 105 and 106), and `<turn|>` is one of the tokens llama.cpp
/// stops generation on.
const GEMMA4_TURN_OPEN: &str = "<|turn>";
const GEMMA4_TURN_CLOSE: &str = "<turn|>";
/// What Gemma 4's canonical template appends to the generation prompt when
/// thinking is off: an empty thought channel, so the model goes straight to
/// the answer instead of opening one itself (which the JSON grammar would
/// forbid, forcing it off its trained path).
const GEMMA4_NO_THINKING: &str = "<|channel>thought\n<channel|>";
const GEMMA4_CHANNEL_OPEN: &str = "<|channel>";
const GEMMA4_CHANNEL_CLOSE: &str = "<channel|>";

pub(crate) enum PromptFormat {
    /// Gemma 4, rendered by [`render_gemma4`].
    Gemma4,
    /// A template llama.cpp applies itself: the model's own, or a built-in.
    Builtin(LlamaChatTemplate),
    /// No template llama.cpp can apply: plain concatenation.
    Plain,
}

/// Pick the format to render prompts for `model` in, and log the choice.
pub(crate) fn resolve(model: &LlamaModel) -> PromptFormat {
    let arch = model
        .meta_val_str("general.architecture")
        .unwrap_or_default()
        .to_lowercase();

    // The vocabulary is the reliable signal: it is what the model reads. The
    // architecture name would do today ("gemma4"), but a fine-tune or a
    // sibling architecture keeps the vocabulary and not necessarily the name.
    if is_control_token(model, GEMMA4_TURN_OPEN) && is_control_token(model, GEMMA4_TURN_CLOSE) {
        log::info!(
            "llama: Gemma 4 turn format (<|turn>…<turn|>) for architecture '{arch}', rendered \
             by LrGeniusAI: llama.cpp cannot apply this model's Jinja template, and its built-in \
             'gemma' template writes <start_of_turn> markers that are not tokens of this model"
        );
        return PromptFormat::Gemma4;
    }

    // Applying it is the only way to learn whether llama.cpp accepts it.
    let applies = |template: &LlamaChatTemplate| probe(model, template).is_some();

    if let Ok(own) = model.chat_template(None) {
        if applies(&own) {
            return PromptFormat::Builtin(own);
        }
    }

    // Sniffing the template text does not work for current models: they build
    // their turns through macros and contain few markers verbatim.
    let family: &[&str] = if arch.starts_with("gemma") {
        // Gemma 1-3 delimit turns the same way; the built-ins differ only in
        // extras. Newest first, and each is verified below.
        &["gemma3", "gemma2", "gemma"]
    } else if arch.starts_with("llama") {
        &["llama3", "llama2"]
    } else if arch.starts_with("mistral") || arch.starts_with("mixtral") {
        &["mistral-v7", "mistral-v1"]
    } else if arch.starts_with("qwen") || arch.starts_with("phi") || arch.starts_with("yi") {
        &["chatml"]
    } else {
        &[]
    };

    // Architecture match first; chatml last, because most instruct models
    // tolerate it and it still beats plain concatenation.
    for name in family.iter().copied().chain(std::iter::once("chatml")) {
        let Ok(template) = LlamaChatTemplate::new(name) else {
            continue;
        };
        let Some(rendered) = probe(model, &template) else {
            continue;
        };
        log::info!(
            "this model's own chat template is Jinja, which llama.cpp cannot apply; using the \
             built-in '{name}' template for architecture '{arch}' instead"
        );
        if !writes_control_tokens(model, &rendered) {
            log::warn!(
                "llama: the built-in '{name}' template writes turn markers that are not control \
                 tokens of this model, so it reads them as plain text; expect poor instruction \
                 following and runaway answers"
            );
        }
        return PromptFormat::Builtin(template);
    }
    PromptFormat::Plain
}

/// A one-turn prompt rendered through `template`, if llama.cpp applies it.
fn probe(model: &LlamaModel, template: &LlamaChatTemplate) -> Option<String> {
    let message = LlamaChatMessage::new("user".to_string(), "probe".to_string()).ok()?;
    model.apply_chat_template(template, &[message], true).ok()
}

/// Whether `text` is a single control token of this model's vocabulary.
fn is_control_token(model: &LlamaModel, text: &str) -> bool {
    match model.str_to_token(text, AddBos::Never) {
        Ok(tokens) => {
            tokens.len() == 1
                && model
                    .token_attr(tokens[0])
                    .contains(LlamaTokenAttr::Control)
        }
        Err(_) => false,
    }
}

/// Whether a rendered prompt contains any control token besides BOS: a
/// template whose markers all tokenize as text gives the model no turns.
fn writes_control_tokens(model: &LlamaModel, rendered: &str) -> bool {
    let Ok(tokens) = model.str_to_token(rendered, AddBos::Never) else {
        return true;
    };
    let bos = model.token_bos();
    tokens
        .iter()
        .any(|&t| t != bos && model.token_attr(t).contains(LlamaTokenAttr::Control))
}

/// One turn of a conversation to render: a chat-template role (`system`,
/// `user`, `assistant`) and its text.
pub(crate) struct Turn<'a> {
    pub role: &'a str,
    pub text: &'a str,
}

/// Render `turns` the way Gemma 4's canonical chat template does (Google,
/// 2026-07-09, as shipped in `gemma-4-12B-it-QAT` and the other Gemma 4
/// GGUFs), for the parts this crate uses — no tools, thinking off:
///
/// ```text
/// <|turn>system\n{system}<turn|>\n
/// <|turn>user\n{user}<turn|>\n
/// <|turn>model\n<|channel>thought\n<channel|>
/// ```
///
/// Like the template, a leading system turn gets a turn of its own, every
/// text is trimmed, an earlier answer has any thought channel stripped, and
/// consecutive assistant turns are one model turn. The template's `<bos>` is
/// left out: the tokenizer adds BOS, as it does for every built-in template.
/// Images need nothing here: the media marker sits in the text, and mtmd
/// wraps the image in Gemma 4's `<|image>…<image|>` itself.
pub(crate) fn render_gemma4(turns: &[Turn<'_>], add_generation_prompt: bool) -> String {
    let mut out = String::new();
    let mut rest = turns;
    if let Some(first) = turns.first() {
        if first.role == "system" || first.role == "developer" {
            out.push_str(GEMMA4_TURN_OPEN);
            out.push_str("system\n");
            out.push_str(first.text.trim());
            out.push_str(GEMMA4_TURN_CLOSE);
            out.push('\n');
            rest = &turns[1..];
        }
    }
    for (index, turn) in rest.iter().enumerate() {
        let model_turn = turn.role == "assistant";
        let continues_previous = model_turn && index > 0 && rest[index - 1].role == "assistant";
        if !continues_previous {
            out.push_str(GEMMA4_TURN_OPEN);
            out.push_str(if model_turn { "model" } else { turn.role });
            out.push('\n');
        }
        if model_turn {
            out.push_str(&strip_thinking(turn.text));
        } else {
            out.push_str(turn.text.trim());
        }
        let continues_into_next =
            model_turn && rest.get(index + 1).is_some_and(|n| n.role == "assistant");
        if !continues_into_next {
            out.push_str(GEMMA4_TURN_CLOSE);
            out.push('\n');
        }
    }
    if add_generation_prompt {
        out.push_str(GEMMA4_TURN_OPEN);
        out.push_str("model\n");
        out.push_str(GEMMA4_NO_THINKING);
    }
    out
}

/// An earlier answer without its thought channels, as the template's
/// `strip_thinking` macro leaves it: whatever sits between `<|channel>` and
/// `<channel|>` goes, and the rest is trimmed.
fn strip_thinking(text: &str) -> String {
    let mut out = String::new();
    for part in text.split(GEMMA4_CHANNEL_CLOSE) {
        match part.split_once(GEMMA4_CHANNEL_OPEN) {
            Some((before, _thought)) => out.push_str(before),
            None => out.push_str(part),
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn<'a>(role: &'a str, text: &'a str) -> Turn<'a> {
        Turn { role, text }
    }

    /// The indexing prompt: what Gemma 4's own template renders for a system
    /// and a user message with `add_generation_prompt` and thinking off.
    #[test]
    fn an_indexing_prompt_renders_as_gemma_4s_own_template_does() {
        let rendered = render_gemma4(
            &[
                turn("system", "  You describe photos. "),
                turn("user", "Keywords please.\n<__media__>\n"),
            ],
            true,
        );
        assert_eq!(
            rendered,
            "<|turn>system\nYou describe photos.<turn|>\n\
             <|turn>user\nKeywords please.\n<__media__><turn|>\n\
             <|turn>model\n<|channel>thought\n<channel|>"
        );
        // None of Gemma 1-3's markers, which Gemma 4 reads as text.
        assert!(!rendered.contains("start_of_turn"));
    }

    #[test]
    fn without_a_system_turn_the_user_turn_comes_first() {
        assert_eq!(
            render_gemma4(&[turn("user", "Hi")], true),
            "<|turn>user\nHi<turn|>\n<|turn>model\n<|channel>thought\n<channel|>"
        );
    }

    #[test]
    fn a_conversation_closes_every_turn_and_strips_old_thoughts() {
        let rendered = render_gemma4(
            &[
                turn("system", "Be brief."),
                turn("user", "One?"),
                turn("assistant", "<|channel>thought\nhmm<channel|>Yes."),
                turn("user", "Two?"),
            ],
            true,
        );
        assert_eq!(
            rendered,
            "<|turn>system\nBe brief.<turn|>\n\
             <|turn>user\nOne?<turn|>\n\
             <|turn>model\nYes.<turn|>\n\
             <|turn>user\nTwo?<turn|>\n\
             <|turn>model\n<|channel>thought\n<channel|>"
        );
    }

    /// The template joins consecutive assistant messages into one model turn
    /// rather than opening a second one.
    #[test]
    fn consecutive_answers_are_one_model_turn() {
        let rendered = render_gemma4(
            &[
                turn("user", "Go"),
                turn("assistant", "First."),
                turn("assistant", "Second."),
            ],
            false,
        );
        assert_eq!(
            rendered,
            "<|turn>user\nGo<turn|>\n<|turn>model\nFirst.Second.<turn|>\n"
        );
    }

    /// The prefix cache cuts the rendered prompt right after the run-constant
    /// text; that only works when the text appears verbatim.
    #[test]
    fn the_stable_prompt_survives_verbatim_for_the_prefix_cache() {
        let stable = "Vocabulary: beach, dune, harbour";
        let user = format!("{stable}\n\nTaken 2026-11-22\n<__media__>");
        let rendered = render_gemma4(&[turn("system", "S"), turn("user", &user)], true);
        let start = rendered.find(stable).expect("stable text must be present");
        assert!(rendered[..start].ends_with("<|turn>user\n"));
    }

    #[test]
    fn thinking_is_stripped_like_the_template_does() {
        assert_eq!(strip_thinking("  plain  "), "plain");
        assert_eq!(
            strip_thinking("a<|channel>thought\nx<channel|>b<|channel>y<channel|> c "),
            "ab c"
        );
    }
}
