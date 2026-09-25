//! Server-owned baseline instructions, independent of client configuration.

/// Baseline guidance complements server authorization; it does not enforce it.
pub const SECURITY_PROMPT: &str = include_str!("security_prompt.txt");

/// Applies the baseline exactly once, ahead of application personality/context.
pub fn instructions(prompts: &[String]) -> String {
    std::iter::once(SECURITY_PROMPT)
        .chain(
            prompts
                .iter()
                .map(String::as_str)
                .filter(|text| *text != SECURITY_PROMPT),
        )
        .collect::<Vec<_>>()
        .join("\n\n")
}
