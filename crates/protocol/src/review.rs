//! Code-review data types shared by the core, the CLI and the desktop UI:
//! what is being reviewed (`ReviewTarget`), the structured result the reviewer
//! model returns (`ReviewOutput` with `ReviewFinding`s), the payload persisted
//! next to the assistant message (`ReviewPayload`) and the lenient parser for
//! the model's answer.

use serde::{Deserialize, Serialize};

/// What a review looks at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewTarget {
    /// Staged, unstaged and untracked changes of the working tree.
    Uncommitted,
    /// Changes since the merge base with `branch`.
    BaseBranch { branch: String },
    /// The changes introduced by one commit.
    Commit { sha: String, title: Option<String> },
    /// Free-form instructions from the user.
    Custom { instructions: String },
}

impl ReviewTarget {
    /// Short human description ("current changes", "changes against 'main'").
    pub fn hint(&self) -> String {
        match self {
            Self::Uncommitted => "current changes".to_string(),
            Self::BaseBranch { branch } => format!("changes against '{branch}'"),
            Self::Commit { sha, title } => {
                let short: String = sha.chars().take(7).collect();
                match title.as_deref().filter(|t| !t.is_empty()) {
                    Some(title) => format!("commit {short}: {title}"),
                    None => format!("commit {short}"),
                }
            }
            Self::Custom { instructions } => {
                let first = instructions.lines().next().unwrap_or("").trim();
                let cut: String = first.chars().take(60).collect();
                format!("custom review: {cut}")
            }
        }
    }
}

/// Inclusive line range of a finding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewLineRange {
    pub start: u32,
    pub end: u32,
}

/// Where a finding points.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewCodeLocation {
    /// Workspace-relative or absolute path as the reviewer wrote it.
    #[serde(alias = "path", alias = "file")]
    pub absolute_file_path: String,
    pub line_range: ReviewLineRange,
}

/// One issue found by the reviewer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewFinding {
    pub title: String,
    pub body: String,
    pub confidence_score: f32,
    /// 0 (P0, drop everything) .. 3 (P3, nice to have); `None` when unknown.
    pub priority: Option<i32>,
    pub code_location: ReviewCodeLocation,
}

impl ReviewFinding {
    /// `P0`..`P3`, or `P?` when the priority is unknown.
    pub fn priority_label(&self) -> String {
        match self.priority {
            Some(p) if (0..=3).contains(&p) => format!("P{p}"),
            _ => "P?".to_string(),
        }
    }

    /// The title without a leading `[P1]` style tag (the tag is shown as a
    /// separate chip).
    pub fn plain_title(&self) -> String {
        let title = self.title.trim();
        if let Some(rest) = title.strip_prefix("[P") {
            if let Some(close) = rest.find(']') {
                if close <= 2 {
                    return rest[close + 1..].trim().to_string();
                }
            }
        }
        title.to_string()
    }

    /// `path:start-end` (or `path:start` for one line, `path` without lines).
    pub fn location_label(&self) -> String {
        let loc = &self.code_location;
        let (start, end) = (loc.line_range.start, loc.line_range.end);
        match (start, end) {
            (0, _) => loc.absolute_file_path.clone(),
            (s, e) if e <= s => format!("{}:{s}", loc.absolute_file_path),
            (s, e) => format!("{}:{s}-{e}", loc.absolute_file_path),
        }
    }
}

/// The reviewer's structured answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewOutput {
    pub findings: Vec<ReviewFinding>,
    /// "patch is correct" / "patch is incorrect".
    pub overall_correctness: String,
    pub overall_explanation: String,
    pub overall_confidence_score: f32,
}

/// Parses the reviewer's reply: plain JSON, JSON inside a markdown fence, or
/// JSON surrounded by prose. Anything else becomes a findings-less output
/// whose explanation is the raw text, so nothing the model said is lost.
pub fn parse_review_output(text: &str) -> ReviewOutput {
    let trimmed = text.trim();
    if let Some(output) = try_parse(trimmed) {
        return output;
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            if let Some(output) = try_parse(&trimmed[start..=end]) {
                return output;
            }
        }
    }
    ReviewOutput { overall_explanation: text.to_string(), ..Default::default() }
}

/// Parses `json` when it is an object carrying review keys (`{}` or
/// unrelated JSON is not a review answer).
fn try_parse(json: &str) -> Option<ReviewOutput> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let object = value.as_object()?;
    if !(object.contains_key("findings") || object.contains_key("overall_correctness")) {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// Plain-text rendering used as the message `content` (and by text UIs).
pub fn render_review_text(output: &ReviewOutput) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !output.overall_correctness.trim().is_empty() {
        sections.push(format!("Verdict: {}", output.overall_correctness.trim()));
    }
    let explanation = output.overall_explanation.trim();
    if !explanation.is_empty() {
        sections.push(explanation.to_string());
    }
    if !output.findings.is_empty() {
        let mut lines = vec![if output.findings.len() > 1 {
            "Review comments:".to_string()
        } else {
            "Review comment:".to_string()
        }];
        for finding in &output.findings {
            lines.push(String::new());
            lines.push(format!(
                "- [{}] {} - {}",
                finding.priority_label(),
                finding.plain_title(),
                finding.location_label()
            ));
            for line in finding.body.lines() {
                lines.push(format!("  {line}"));
            }
        }
        sections.push(lines.join("\n"));
    }
    if sections.is_empty() {
        "The reviewer returned no output.".to_string()
    } else {
        sections.join("\n\n")
    }
}

/// Marker stored in `RunMessage.raw_response` of a review answer. Every
/// other `raw_response` is a JSON array (tool calls) or a call id, so this
/// object never collides with them.
pub const REVIEW_PAYLOAD_KIND: &str = "review_output";

/// The structured review persisted next to the assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewPayload {
    pub kind: String,
    pub target: ReviewTarget,
    pub hint: String,
    pub output: ReviewOutput,
}

impl ReviewPayload {
    pub fn new(target: ReviewTarget, output: ReviewOutput) -> Self {
        Self { kind: REVIEW_PAYLOAD_KIND.to_string(), hint: target.hint(), target, output }
    }

    /// Serialized form for `raw_response`.
    pub fn to_raw(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a payload back from a message's `raw_response`.
    pub fn from_raw(raw: &str) -> Option<Self> {
        let payload: Self = serde_json::from_str(raw).ok()?;
        (payload.kind == REVIEW_PAYLOAD_KIND).then_some(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{"findings":[{"title":"[P1] Off by one","body":"Loop skips the last item.","confidence_score":0.8,"priority":1,"code_location":{"absolute_file_path":"src/a.rs","line_range":{"start":10,"end":12}}}],"overall_correctness":"patch is incorrect","overall_explanation":"One bug.","overall_confidence_score":0.7}"#;

    #[test]
    fn parses_plain_json() {
        let out = parse_review_output(VALID);
        assert_eq!(out.findings.len(), 1);
        assert_eq!(out.findings[0].priority, Some(1));
        assert_eq!(out.findings[0].location_label(), "src/a.rs:10-12");
        assert_eq!(out.findings[0].plain_title(), "Off by one");
        assert_eq!(out.overall_correctness, "patch is incorrect");
    }

    #[test]
    fn parses_fenced_json_and_json_in_prose() {
        let fenced = format!("```json\n{VALID}\n```");
        assert_eq!(parse_review_output(&fenced).findings.len(), 1);
        let prose = format!("Here is my review:\n{VALID}\nHope it helps.");
        assert_eq!(parse_review_output(&prose).findings.len(), 1);
    }

    #[test]
    fn malformed_output_falls_back_to_plain_text() {
        let out = parse_review_output("Looks fine to me, {not json}");
        assert!(out.findings.is_empty());
        assert_eq!(out.overall_explanation, "Looks fine to me, {not json}");
        let unrelated = parse_review_output(r#"{"foo": 1}"#);
        assert!(unrelated.findings.is_empty());
        assert_eq!(unrelated.overall_explanation, r#"{"foo": 1}"#);
    }

    #[test]
    fn tolerates_missing_priority_and_path_alias() {
        let out = parse_review_output(
            r#"{"findings":[{"title":"t","body":"b","code_location":{"path":"x.rs","line_range":{"start":3,"end":3}}}],"overall_correctness":"patch is correct"}"#,
        );
        assert_eq!(out.findings[0].priority, None);
        assert_eq!(out.findings[0].priority_label(), "P?");
        assert_eq!(out.findings[0].location_label(), "x.rs:3");
    }

    #[test]
    fn renders_text_and_round_trips_payload() {
        let out = parse_review_output(VALID);
        let text = render_review_text(&out);
        assert!(text.contains("Verdict: patch is incorrect"));
        assert!(text.contains("- [P1] Off by one - src/a.rs:10-12"));
        let payload = ReviewPayload::new(ReviewTarget::Uncommitted, out);
        let back = ReviewPayload::from_raw(&payload.to_raw()).unwrap();
        assert_eq!(back, payload);
        assert!(ReviewPayload::from_raw("[]").is_none());
        assert!(ReviewPayload::from_raw("call-1").is_none());
    }

    #[test]
    fn target_hints() {
        assert_eq!(ReviewTarget::Uncommitted.hint(), "current changes");
        assert_eq!(
            ReviewTarget::Commit { sha: "abcdef1234".into(), title: Some("Fix".into()) }.hint(),
            "commit abcdef1: Fix"
        );
    }
}
