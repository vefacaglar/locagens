//! Turns what the user typed into the run prompt. One implementation for the
//! TUI, the REPL and `lcgn -p` (QA-048).
//!
//! `@path` mentions of **existing project files** are attached as
//! `## Attached file:` blocks in front of the text; the text itself is kept
//! verbatim (newlines included). Anything else that merely starts with `@`
//! (`@alice`, `@../secret`, a missing file) stays plain text.

use std::path::{Component, Path};

/// Largest file `@path` will inline; bigger ones are an error instead of
/// silently flooding the context.
pub const MAX_ATTACHMENT_BYTES: u64 = 256 * 1024;

/// Builds the prompt for `text`: attachments for `@path` mentions of existing
/// files inside `project`, then the verbatim text.
pub fn build_prompt(text: &str, project: &Path) -> Result<String, String> {
    let mut attachments = String::new();
    let mut seen: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        let Some(relative) = word.strip_prefix('@') else {
            continue;
        };
        let relative = relative.trim_end_matches([',', '.', ';', ':', '!', '?', ')']);
        if relative.is_empty() || seen.contains(&relative) || !is_project_file(project, relative) {
            continue;
        }
        seen.push(relative);
        attachments.push_str(&attachment_block(project, relative)?);
    }
    let prompt = format!("{attachments}{text}").trim().to_string();
    if prompt.is_empty() {
        return Err("No task given. Pass a message, `@file` attachments or piped stdin.".to_string());
    }
    Ok(prompt)
}

/// Appends piped stdin (when not blank) to `prompt`.
pub fn append_stdin(prompt: String, stdin: Option<&str>) -> Result<String, String> {
    let piped = stdin.map(str::trim).unwrap_or("");
    let combined = match (prompt.trim().is_empty(), piped.is_empty()) {
        (_, true) => prompt,
        (true, false) => piped.to_string(),
        (false, false) => format!("{}\n\n{piped}", prompt.trim_end()),
    };
    let combined = combined.trim().to_string();
    if combined.is_empty() {
        return Err("No task given. Pass a message, `@file` attachments or piped stdin.".to_string());
    }
    Ok(combined)
}

/// A relative path made only of normal components that names an existing file.
fn is_project_file(project: &Path, relative: &str) -> bool {
    Path::new(relative).components().all(|part| matches!(part, Component::Normal(_)))
        && project.join(relative).is_file()
}

fn attachment_block(project: &Path, relative: &str) -> Result<String, String> {
    let path = project.join(relative);
    let size = std::fs::metadata(&path)
        .map_err(|err| format!("Cannot read @file \"{relative}\": {err}"))?
        .len();
    if size > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "@file \"{relative}\" is {size} bytes; the limit is {MAX_ATTACHMENT_BYTES}. Ask the model to read it instead."
        ));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|err| format!("Cannot read @file \"{relative}\": {err}"))?;
    Ok(format!("## Attached file: {relative}\n```\n{content}\n```\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lcgn-prompt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() {}").unwrap();
        dir
    }

    #[test]
    fn mentions_of_existing_files_are_attached_once() {
        let dir = project();
        let prompt = build_prompt("explain @src/a.rs, then @src/a.rs again\nplease", &dir).unwrap();
        assert!(prompt.starts_with("## Attached file: src/a.rs"));
        assert_eq!(prompt.matches("## Attached file:").count(), 1);
        assert!(prompt.contains("explain @src/a.rs, then @src/a.rs again\nplease"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_or_escaping_mentions_stay_plain_text() {
        let dir = project();
        let prompt = build_prompt("ping @alice and read @../secret.txt", &dir).unwrap();
        assert_eq!(prompt, "ping @alice and read @../secret.txt");
        // A path outside the project is never read, even when it exists.
        let outside = dir.parent().unwrap().join(format!("outside-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&outside, "top secret").unwrap();
        let name = outside.file_name().unwrap().to_string_lossy().to_string();
        let prompt = build_prompt(&format!("see @../{name}"), &dir).unwrap();
        assert!(!prompt.contains("top secret"));
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newlines_and_spacing_survive_and_a_leading_mention_is_attached() {
        let dir = project();
        let prompt = build_prompt("@src/a.rs explain\n\n  indented", &dir).unwrap();
        assert!(prompt.contains("fn a() {}"));
        assert!(prompt.ends_with("@src/a.rs explain\n\n  indented"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_attachments_are_rejected() {
        let dir = project();
        std::fs::write(dir.join("big.txt"), vec![b'x'; MAX_ATTACHMENT_BYTES as usize + 1]).unwrap();
        let err = build_prompt("read @big.txt", &dir).unwrap_err();
        assert!(err.contains("big.txt") && err.contains("limit"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_input_is_an_error_and_stdin_is_appended() {
        let dir = project();
        assert!(build_prompt("   ", &dir).is_err());
        assert_eq!(append_stdin("from stdin:".into(), Some("piped body\n")).unwrap(), "from stdin:\n\npiped body");
        assert_eq!(append_stdin(String::new(), Some("only stdin")).unwrap(), "only stdin");
        assert_eq!(append_stdin("just text".into(), None).unwrap(), "just text");
        assert!(append_stdin(String::new(), Some("  ")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
