//! What the model is told about its job. The tool list and the answer format
//! are added by the model's own `ChatModel`, since a local model and a cloud
//! one are told about tools differently.

/// `instructions` is the MCP server's own guidance for agents; `today` lets
/// the model place "last summer".
#[must_use]
pub fn system_prompt(instructions: &str, today: &str) -> String {
    let mut prompt = format!(
        "You are the assistant in LrGeniusAI, a plugin for Adobe Lightroom Classic, chatting with \
         the photographer whose catalog it is. You work on their catalog with the tools below. \
         Today is {today}.\n\n"
    );
    if !instructions.trim().is_empty() {
        prompt.push_str(instructions.trim());
        prompt.push_str("\n\n");
    }
    // Each line answers a mistake seen when local models were probed with
    // the real tools: rating the whole selection when asked for the best
    // five, copying photo IDs out of a preview (and inventing the ones it
    // did not show), and retrying a change the user had just turned down.
    prompt.push_str(
        "In this chat:\n\
         - When the user wants something done to some of the photos (the best five, the sharp \
         ones, those of Anna), first find exactly those photos, then pass that result set on — \
         never the whole selection.\n\
         - Pass result sets with `set`. Only use a photo ID that appeared in a tool result, \
         never one you made up.\n\
         - The user may have to approve a change. When a call comes back as not allowed, do what \
         the user said about it, if they said anything — with corrected arguments, not the same \
         call again. If they said nothing, do not try it another way; ask what they want.\n\
         - Keep replies short: what you found (how many photos) or what you changed. Do not \
         repeat tool output the user can see.\n\
         - Answer in the language the user writes in.",
    );
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_the_date_and_the_servers_guidance() {
        let prompt = system_prompt("Start with get_status.", "2026-10-22");
        assert!(prompt.contains("Today is 2026-10-22."));
        assert!(prompt.contains("Start with get_status.\n\nIn this chat:"));
    }

    #[test]
    fn works_without_guidance() {
        let prompt = system_prompt("  ", "2026-10-22");
        assert!(prompt.contains("Today is 2026-10-22.\n\nIn this chat:"));
    }
}
