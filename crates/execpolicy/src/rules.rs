//! Codex-style Starlark `.rules` engine.
//!
//! Parses `prefix_rule(...)` / `network_rule(...)` declarations from
//! `*.rules` files (Starlark syntax: keyword arguments, string and list
//! literals, `#` line comments), validates them against `match` /
//! `not_match` example commands, and evaluates compound shell commands
//! (split with [`split_shell_commands`]) to an [`Evaluation`].

use serde::{Deserialize, Serialize};
use std::path::Path;

// ----------------------------------------------------------------------
// Decisions, patterns and rules
// ----------------------------------------------------------------------

/// Strictness-ordered decision: `Allow < Prompt < Forbidden`, so `.max()`
/// always yields the strictest decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Prompt,
    Forbidden,
}

impl Decision {
    pub fn from_str(value: &str) -> Result<Self, String> {
        match value.trim().to_lowercase().as_str() {
            "allow" => Ok(Self::Allow),
            "prompt" => Ok(Self::Prompt),
            "forbidden" => Ok(Self::Forbidden),
            other => Err(format!("unknown execpolicy decision: {other:?}")),
        }
    }
}

/// One token of a [`PrefixRule`] pattern: a literal, or a set of acceptable
/// alternatives (e.g. `["check", "test"]`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PatternToken {
    Single(String),
    Alts(Vec<String>),
}

impl PatternToken {
    fn matches(&self, token: &str) -> bool {
        match self {
            Self::Single(expected) => expected == token,
            Self::Alts(alternatives) => alternatives.iter().any(|alt| alt == token),
        }
    }
}

/// A rule matching a command prefix (the first `pattern.len()` tokens).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixRule {
    pub pattern: Vec<PatternToken>,
    pub decision: Decision,
    pub justification: Option<String>,
}

impl PrefixRule {
    /// Whether `tokens` begins with this rule's pattern.
    pub fn matches(&self, tokens: &[String]) -> bool {
        tokens.len() >= self.pattern.len()
            && self
                .pattern
                .iter()
                .zip(tokens.iter())
                .all(|(pattern, token)| pattern.matches(token))
    }
}

/// A rule for outbound network access (host/protocol scoped).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRule {
    pub host: String,
    pub protocol: Option<String>,
    pub decision: Decision,
    pub justification: Option<String>,
}

/// The outcome of evaluating a command against a [`Policy`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub decision: Decision,
    pub justification: Option<String>,
    pub matched_rule: bool,
}

// ----------------------------------------------------------------------
// Compound shell command splitting
// ----------------------------------------------------------------------

/// Splits a compound shell command into segments of argv tokens.
///
/// - Lexes single quotes, double quotes (handling `\"`, `\\`, `\$`, `` \` ``)
///   and unquoted backslash escapes.
/// - Splits on unquoted `;`, `&&`, `||`, `|` and newlines.
/// - Rejects redirections, subshell parens, command substitution and process
///   substitution outside single quotes.
/// - Unwraps nested shells (`sh -c <inner>`, `cmd /C <inner>`) by recursively
///   splitting the inner command and splicing its segments in place.
/// - Returns `Ok(vec![])` for empty commands (callers treat that as unsafe).
pub fn split_shell_commands(command: &str) -> Result<Vec<Vec<String>>, String> {
    let segments = split_shell_segments(command)?;
    // Unwrap nested shells: `sh -c <inner>` / `cmd /C <inner>`.
    let mut unwrapped: Vec<Vec<String>> = Vec::new();
    for segment in segments {
        if let Some(inner) = nested_shell_inner(&segment) {
            unwrapped.extend(split_shell_commands(&inner)?);
        } else {
            unwrapped.push(segment);
        }
    }
    Ok(unwrapped)
}

/// Splits a command into raw segments without unwrapping nested shells.
#[allow(unused_assignments)]
fn split_shell_segments(command: &str) -> Result<Vec<Vec<String>>, String> {
    const REJECT: &str = "Command contains redirection, subshell, or command substitution.";
    let chars: Vec<char> = command.chars().collect();
    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut token = String::new();
    let mut has_token = false;
    let mut i = 0usize;

    macro_rules! push_token {
        () => {{
            if has_token {
                current.push(std::mem::take(&mut token));
                has_token = false;
            }
        }};
    }
    macro_rules! push_segment {
        () => {{
            push_token!();
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
        }};
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                // Single quotes: everything is literal until the next `'`.
                let mut j = i + 1;
                let mut closed = false;
                while j < chars.len() {
                    if chars[j] == '\'' {
                        closed = true;
                        break;
                    }
                    token.push(chars[j]);
                    j += 1;
                }
                if !closed {
                    return Err(REJECT.to_string());
                }
                has_token = true;
                i = j + 1;
            }
            '"' => {
                // Double quotes: escape handling plus command-substitution
                // rejection.
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    match chars[i] {
                        '\\' => {
                            if i + 1 >= chars.len() {
                                return Err(REJECT.to_string());
                            }
                            match chars[i + 1] {
                                '"' | '\\' | '$' | '`' => {
                                    token.push(chars[i + 1]);
                                    i += 2;
                                }
                                _ => {
                                    token.push('\\');
                                    token.push(chars[i + 1]);
                                    i += 2;
                                }
                            }
                        }
                        '$' if i + 1 < chars.len() && chars[i + 1] == '(' => {
                            return Err(REJECT.to_string());
                        }
                        '`' => return Err(REJECT.to_string()),
                        '"' => {
                            i += 1;
                            closed = true;
                            break;
                        }
                        other => {
                            token.push(other);
                            i += 1;
                        }
                    }
                }
                if !closed {
                    return Err(REJECT.to_string());
                }
                has_token = true;
            }
            '\\' => {
                // Unquoted backslash escapes the next character.
                if i + 1 < chars.len() {
                    token.push(chars[i + 1]);
                    i += 2;
                } else {
                    token.push('\\');
                    i += 1;
                }
                has_token = true;
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '(' => {
                return Err(REJECT.to_string());
            }
            '`' => return Err(REJECT.to_string()),
            '(' | ')' => return Err(REJECT.to_string()),
            '<' | '>' => return Err(REJECT.to_string()),
            '&' => {
                if i + 1 < chars.len() && chars[i + 1] == '&' {
                    push_segment!();
                    i += 2;
                } else {
                    // A lone `&` (background / redirection variant) is not
                    // representable in the split model.
                    return Err(REJECT.to_string());
                }
            }
            '|' => {
                if i + 1 < chars.len() && chars[i + 1] == '|' {
                    push_segment!();
                    i += 2;
                } else {
                    push_segment!();
                    i += 1;
                }
            }
            ';' => {
                push_segment!();
                i += 1;
            }
            '\n' => {
                push_segment!();
                i += 1;
            }
            whitespace if whitespace.is_whitespace() => {
                push_token!();
                i += 1;
            }
            other => {
                token.push(other);
                has_token = true;
                i += 1;
            }
        }
    }
    push_segment!();

    Ok(segments)
}

/// Returns the inner command of `sh -c <inner>` / `cmd /C <inner>` shapes.
fn nested_shell_inner(segment: &[String]) -> Option<String> {
    if segment.len() != 3 {
        return None;
    }
    let program = segment[0]
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&segment[0])
        .to_lowercase();
    let program = program.strip_suffix(".exe").unwrap_or(&program);
    let flag = segment[1].to_lowercase();
    match program {
        "sh" | "bash" | "zsh" => {
            if flag == "-c" || flag == "-lc" {
                Some(segment[2].clone())
            } else {
                None
            }
        }
        "cmd" | "cmd.exe" => {
            if flag == "/c" {
                Some(segment[2].clone())
            } else {
                None
            }
        }
        _ => None,
    }
}

// ----------------------------------------------------------------------
// Starlark `.rules` parsing
// ----------------------------------------------------------------------

#[derive(Debug)]
enum StarlarkValue {
    Str(String),
    List(Vec<StarlarkValue>),
}

#[derive(Debug)]
enum Token {
    Ident(String),
    Str(String),
    Punct(char),
}

/// Strips `# ...` line comments (outside string literals).
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let mut in_string: Option<char> = None;
        let mut keep = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match (in_string, c) {
                (Some(quote), c) if c == quote => {
                    keep.push(c);
                    in_string = None;
                }
                (Some(_quote), '\\') => {
                    keep.push(c);
                    if let Some(next) = chars.next() {
                        keep.push(next);
                    }
                }
                (Some(_quote), c) => keep.push(c),
                (None, '\'') | (None, '"') => {
                    keep.push(c);
                    in_string = Some(c);
                }
                (None, '#') => break,
                (None, c) => keep.push(c),
            }
        }
        out.push_str(&keep);
        out.push('\n');
    }
    out
}

fn lex_starlark(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = strip_comments(src).chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '(' | ')' | '[' | ']' | ',' | '=' | '{' | '}' => {
                tokens.push(Token::Punct(c));
                i += 1;
            }
            '\'' | '"' => {
                let quote = c;
                let mut value = String::new();
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    match chars[i] {
                        '\\' if i + 1 < chars.len() => {
                            let next = chars[i + 1];
                            match next {
                                'n' => value.push('\n'),
                                't' => value.push('\t'),
                                'r' => value.push('\r'),
                                other => value.push(other),
                            }
                            i += 2;
                        }
                        c if c == quote => {
                            i += 1;
                            closed = true;
                            break;
                        }
                        c => {
                            value.push(c);
                            i += 1;
                        }
                    }
                }
                if !closed {
                    return Err("unterminated string literal in rules file".to_string());
                }
                tokens.push(Token::Str(value));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(Token::Ident(chars[start..i].iter().collect()));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                tokens.push(Token::Ident(chars[start..i].iter().collect()));
            }
            other => {
                return Err(format!("unexpected character {other:?} in rules file"));
            }
        }
    }
    Ok(tokens)
}

/// Parses a Starlark value: string literal or (nested) list of strings.
fn parse_value(tokens: &[Token], index: &mut usize) -> Result<StarlarkValue, String> {
    match tokens.get(*index) {
        Some(Token::Str(value)) => {
            *index += 1;
            Ok(StarlarkValue::Str(value.clone()))
        }
        Some(Token::Punct('[')) => {
            *index += 1;
            let mut items = Vec::new();
            loop {
                match tokens.get(*index) {
                    Some(Token::Punct(']')) => {
                        *index += 1;
                        return Ok(StarlarkValue::List(items));
                    }
                    None => return Err("unterminated list literal in rules file".to_string()),
                    _ => {
                        items.push(parse_value(tokens, index)?);
                        match tokens.get(*index) {
                            Some(Token::Punct(',')) => *index += 1,
                            Some(Token::Punct(']')) => {}
                            _ => {
                                return Err(
                                    "expected ',' or ']' in rules list literal".to_string()
                                )
                            }
                        }
                    }
                }
            }
        }
        other => Err(format!(
            "expected a string or list literal in rules file, found {other:?}"
        )),
    }
}

/// Parses one argument (positional or keyword) of a rule call.
fn parse_argument(tokens: &[Token], index: &mut usize) -> Result<(String, StarlarkValue), String> {
    // keyword form: ident '=' value
    if let (Some(Token::Ident(name)), Some(Token::Punct('='))) =
        (tokens.get(*index), tokens.get(*index + 1))
    {
        *index += 2;
        return Ok((name.clone(), parse_value(tokens, index)?));
    }
    Ok((String::new(), parse_value(tokens, index)?))
}

/// The parsed rule set loaded from `*.rules` files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy {
    prefix_rules: Vec<PrefixRule>,
    network_rules: Vec<NetworkRule>,
}

impl Policy {
    /// Parses rule declarations from a `.rules` source string.
    pub fn parse_str(src: &str) -> Result<Self, String> {
        let tokens = lex_starlark(src)?;
        let mut index = 0usize;
        let mut policy = Policy::default();
        while index < tokens.len() {
            let name = match tokens.get(index) {
                Some(Token::Ident(name)) => name.clone(),
                Some(other) => {
                    return Err(format!("unexpected {other:?} at top level of rules file"))
                }
                None => break,
            };
            index += 1;
            match tokens.get(index) {
                Some(Token::Punct('(')) => index += 1,
                other => {
                    return Err(format!(
                        "expected '(' after {name:?} in rules file, found {other:?}"
                    ))
                }
            }
            let mut args: Vec<(String, StarlarkValue)> = Vec::new();
            loop {
                match tokens.get(index) {
                    Some(Token::Punct(')')) => {
                        index += 1;
                        break;
                    }
                    None => return Err("unterminated rule call in rules file".to_string()),
                    _ => {
                        args.push(parse_argument(&tokens, &mut index)?);
                        match tokens.get(index) {
                            Some(Token::Punct(',')) => index += 1,
                            Some(Token::Punct(')')) => {
                                index += 1;
                                break;
                            }
                            _ => {
                                return Err(
                                    "expected ',' or ')' in rules file argument list".to_string()
                                )
                            }
                        }
                    }
                }
            }
            match name.as_str() {
                "prefix_rule" => policy.prefix_rules.push(parse_prefix_rule(args)?),
                "network_rule" => policy.network_rules.push(parse_network_rule(args)?),
                // Unknown declarations are skipped gracefully.
                _ => {}
            }
        }
        Ok(policy)
    }

    /// Loads and merges all `*.rules` files found under each directory
    /// (missing directories are skipped; invalid files are logged and
    /// ignored).
    pub fn load_from_dirs(dirs: &[std::path::PathBuf]) -> Self {
        let mut policy = Policy::default();
        for dir in dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            let mut files: Vec<std::path::PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_file() && path.extension().map(|ext| ext == "rules").unwrap_or(false)
                })
                .collect();
            files.sort();
            for file in files {
                match std::fs::read_to_string(&file).map_err(|err| err.to_string()).and_then(|src| {
                    Policy::parse_str(&src).map_err(|err| format!("{}: {err}", file.display()))
                }) {
                    Ok(parsed) => {
                        policy.prefix_rules.extend(parsed.prefix_rules);
                        policy.network_rules.extend(parsed.network_rules);
                    }
                    Err(err) => eprintln!("[execpolicy] ignoring invalid rules file: {err}"),
                }
            }
        }
        policy
    }

    /// Evaluates a (possibly compound) shell command against the rules.
    pub fn evaluate_command(&self, command: &str) -> Evaluation {
        let mut evaluation = self.evaluate_segments(command);
        // A dangerous marker without an explicit rule override is never
        // allowed silently.
        if crate::command_safety::has_dangerous_marker(command)
            && !evaluation.matched_rule
            && evaluation.decision == Decision::Allow
        {
            evaluation.decision = Decision::Prompt;
        }
        evaluation
    }

    fn evaluate_segments(&self, command: &str) -> Evaluation {
        let segments = match split_shell_commands(command) {
            Ok(segments) => segments,
            Err(reason) => {
                return Evaluation {
                    decision: Decision::Prompt,
                    justification: Some(reason),
                    matched_rule: false,
                };
            }
        };
        if segments.is_empty() {
            return Evaluation {
                decision: Decision::Prompt,
                justification: None,
                matched_rule: false,
            };
        }

        let mut decision: Option<Decision> = None;
        let mut justification: Option<String> = None;
        let mut matched_rule = false;
        for tokens in &segments {
            let mut segment_decision: Option<(Decision, Option<String>)> = None;
            for rule in &self.prefix_rules {
                if rule.matches(tokens) {
                    let candidate = (rule.decision, rule.justification.clone());
                    segment_decision = Some(match segment_decision.take() {
                        Some(existing) => {
                            if candidate.0 > existing.0 {
                                candidate
                            } else {
                                existing
                            }
                        }
                        None => candidate,
                    });
                }
            }
            let (seg_decision, seg_justification, seg_matched) = match segment_decision {
                Some((decision, justification)) => (decision, justification, true),
                None => {
                    let joined = tokens.join(" ");
                    match crate::command_safety::classify_single_segment(&joined) {
                        crate::command_safety::CommandSafety::KnownSafeReadOnly => {
                            (Decision::Allow, None, false)
                        }
                        crate::command_safety::CommandSafety::RequiresApproval
                        | crate::command_safety::CommandSafety::Dangerous => {
                            (Decision::Prompt, None, false)
                        }
                    }
                }
            };
            matched_rule |= seg_matched;
            let candidate = (seg_decision, seg_justification);
            match decision {
                Some(existing) => {
                    if candidate.0 > existing {
                        decision = Some(candidate.0);
                        justification = candidate.1;
                    }
                }
                None => {
                    decision = Some(candidate.0);
                    justification = candidate.1;
                }
            };
        }

        Evaluation {
            decision: decision.unwrap_or(Decision::Prompt),
            justification,
            matched_rule,
        }
    }

    /// All prefix rules (for tests and tooling).
    pub fn prefix_rules(&self) -> &[PrefixRule] {
        &self.prefix_rules
    }

    /// All network rules (for tests and tooling).
    pub fn network_rules(&self) -> &[NetworkRule] {
        &self.network_rules
    }
}

fn parse_decision(args: &[(String, StarlarkValue)]) -> Result<Decision, String> {
    for (name, value) in args {
        if name == "decision" {
            if let StarlarkValue::Str(raw) = value {
                return Decision::from_str(raw);
            }
        }
    }
    Ok(Decision::Prompt)
}

fn parse_justification(args: &[(String, StarlarkValue)]) -> Option<String> {
    for (name, value) in args {
        if name == "justification" {
            if let StarlarkValue::Str(raw) = value {
                return Some(raw.clone());
            }
        }
    }
    None
}

fn parse_pattern(args: &[(String, StarlarkValue)]) -> Result<Vec<PatternToken>, String> {
    let value = args
        .iter()
        .find(|(name, _)| name.is_empty() || name == "pattern")
        .map(|(_, value)| value)
        .ok_or_else(|| "prefix_rule requires a pattern".to_string())?;
    let items = match value {
        StarlarkValue::List(items) => items,
        _ => return Err("prefix_rule pattern must be a list".to_string()),
    };
    let mut pattern = Vec::new();
    for item in items {
        match item {
            StarlarkValue::Str(raw) => pattern.push(PatternToken::Single(raw.clone())),
            StarlarkValue::List(alts) => {
                let mut alternatives = Vec::new();
                for alt in alts {
                    match alt {
                        StarlarkValue::Str(raw) => alternatives.push(raw.clone()),
                        _ => {
                            return Err(
                                "prefix_rule pattern alternatives must be string literals"
                                    .to_string(),
                            )
                        }
                    }
                }
                if alternatives.is_empty() {
                    return Err("prefix_rule pattern alternative is empty".to_string());
                }
                pattern.push(PatternToken::Alts(alternatives));
            }
        }
    }
    if pattern.is_empty() {
        return Err("prefix_rule pattern is empty".to_string());
    }
    Ok(pattern)
}

/// Whitespace/shlex-ish tokenizing of example commands for validation.
fn tokenize_example(example: &str) -> Vec<String> {
    example
        .split_whitespace()
        .map(|token| token.trim_matches(|c| c == '\'' || c == '"').to_string())
        .filter(|token| !token.is_empty())
        .collect()
}

fn validate_examples(
    rule_name: &str,
    pattern: &[PatternToken],
    args: &[(String, StarlarkValue)],
) -> Result<(), String> {
    let probe = PrefixRule {
        pattern: pattern.to_vec(),
        decision: Decision::Allow,
        justification: None,
    };
    for (name, value) in args {
        if name != "match" && name != "not_match" {
            continue;
        }
        let examples = match value {
            StarlarkValue::List(items) => items,
            _ => return Err(format!("{rule_name} {name} must be a list of strings")),
        };
        for item in examples {
            let example = match item {
                StarlarkValue::Str(raw) => raw.clone(),
                _ => return Err(format!("{rule_name} {name} examples must be strings")),
            };
            let tokens = tokenize_example(&example);
            let matches = probe.matches(&tokens);
            if name == "match" && !matches {
                return Err(format!(
                    "{rule_name} match example {example:?} does not match the pattern"
                ));
            }
            if name == "not_match" && matches {
                return Err(format!(
                    "{rule_name} not_match example {example:?} unexpectedly matches the pattern"
                ));
            }
        }
    }
    Ok(())
}

fn parse_prefix_rule(args: Vec<(String, StarlarkValue)>) -> Result<PrefixRule, String> {
    let pattern = parse_pattern(&args)?;
    validate_examples("prefix_rule", &pattern, &args)?;
    Ok(PrefixRule {
        pattern,
        decision: parse_decision(&args)?,
        justification: parse_justification(&args),
    })
}

fn parse_network_rule(args: Vec<(String, StarlarkValue)>) -> Result<NetworkRule, String> {
    let host = args
        .iter()
        .find(|(name, _)| name == "host")
        .and_then(|(_, value)| match value {
            StarlarkValue::Str(raw) => Some(raw.clone()),
            _ => None,
        })
        .ok_or_else(|| "network_rule requires host = \"...\"".to_string())?;
    let protocol = args
        .iter()
        .find(|(name, _)| name == "protocol")
        .and_then(|(_, value)| match value {
            StarlarkValue::Str(raw) => Some(raw.clone()),
            _ => None,
        });
    Ok(NetworkRule {
        host,
        protocol,
        decision: parse_decision(&args)?,
        justification: parse_justification(&args),
    })
}

// ----------------------------------------------------------------------
// Rule amendment (allow prefix rules)
// ----------------------------------------------------------------------

/// Interpreter / privilege-escalation programs whose invocations are never
/// amended into allow rules automatically.
const BANNED_AMEND_PREFIXES: [&str; 26] = [
    "sh", "bash", "zsh", "fish", "cmd", "cmd.exe", "powershell", "pwsh", "python", "python3",
    "node", "deno", "bun", "ruby", "perl", "php", "sudo", "su", "doas", "rm", "del", "rmdir",
    "env", "xargs", "eval", "exec",
];

/// Escapes a string as a Starlark string literal.
fn escape_starlark_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Appends an allow `prefix_rule` for `command` to `rules_file`, unless the
/// command starts with a banned interpreter / dangerous program or an existing
/// rule already allows it. Returns whether the rule was appended.
pub fn append_allow_prefix_rule(rules_file: &Path, command: &str) -> Result<bool, String> {
    // Raw segments (no nested-shell unwrapping) so a `sh -c …` / `python …`
    // wrapper itself is caught by the banned-prefix check.
    let segments = match split_shell_segments(command) {
        Ok(segments) => segments,
        Err(_) => return Ok(false),
    };
    if segments.len() != 1 || segments[0].is_empty() {
        return Ok(false);
    }
    let program = segments[0][0]
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&segments[0][0])
        .to_lowercase();
    let program = program.strip_suffix(".exe").unwrap_or(&program);
    if BANNED_AMEND_PREFIXES.contains(&program) {
        return Ok(false);
    }

    // Already allowed by an existing matching rule?
    if let Ok(source) = std::fs::read_to_string(rules_file) {
        if let Ok(existing) = Policy::parse_str(&source) {
            let evaluation = existing.evaluate_command(command);
            if evaluation.matched_rule && evaluation.decision == Decision::Allow {
                return Ok(false);
            }
        }
    }

    if let Some(parent) = rules_file.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("append_allow_prefix_rule: {err}"))?;
    }
    let pattern = segments[0]
        .iter()
        .map(|token| escape_starlark_string(token))
        .collect::<Vec<_>>()
        .join(", ");
    let line = format!("prefix_rule(pattern = [{pattern}], decision = \"allow\")\n");
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(rules_file)
        .map_err(|err| format!("append_allow_prefix_rule: {err}"))?;
    file.write_all(line.as_bytes())
        .map_err(|err| format!("append_allow_prefix_rule: {err}"))?;
    Ok(true)
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_commands_split_and_reject_unsafe_constructs() {
        let segments =
            split_shell_commands("cargo check && echo done; git status | head -n 3").unwrap();
        assert_eq!(
            segments,
            vec![
                vec!["cargo".to_string(), "check".to_string()],
                vec!["echo".to_string(), "done".to_string()],
                vec!["git".to_string(), "status".to_string()],
                vec!["head".to_string(), "-n".to_string(), "3".to_string()],
            ]
        );

        // Quotes and escapes.
        let segments = split_shell_commands("echo 'a && b' \"c ; d\" plain\\ token").unwrap();
        assert_eq!(
            segments,
            vec![vec![
                "echo".to_string(),
                "a && b".to_string(),
                "c ; d".to_string(),
                "plain token".to_string(),
            ]]
        );

        // Nested shells are unwrapped and spliced.
        let segments = split_shell_commands("sh -c 'echo one && echo two'").unwrap();
        assert_eq!(
            segments,
            vec![
                vec!["echo".to_string(), "one".to_string()],
                vec!["echo".to_string(), "two".to_string()],
            ]
        );
        let segments = split_shell_commands("cmd.exe /C \"echo a && echo b\"").unwrap();
        assert_eq!(
            segments,
            vec![
                vec!["echo".to_string(), "a".to_string()],
                vec!["echo".to_string(), "b".to_string()],
            ]
        );

        // Unsafe constructs are rejected.
        for command in [
            "echo $(whoami)",
            "echo `whoami`",
            "echo hi > out.txt",
            "echo hi >> out.txt",
            "cat < input.txt",
            "diff <(ls) <(ls -a)",
            "ls >($$)",
            "(echo hi)",
            "echo hi &",
            "echo 'unclosed",
            "2>&1",
        ] {
            assert!(
                split_shell_commands(command).is_err(),
                "{command} should be rejected"
            );
        }

        // Command substitution is rejected inside double quotes too.
        assert!(split_shell_commands("echo \"x $(whoami)\"").is_err());
        assert!(split_shell_commands("echo \"x `whoami`\"").is_err());
        // …but single quotes make it safe text.
        assert!(split_shell_commands("echo '$(whoami)'").is_ok());

        // Empty input splits to nothing.
        assert!(split_shell_commands("").unwrap().is_empty());
        assert!(split_shell_commands("  ; ;  ").unwrap().is_empty());
    }

    #[test]
    fn starlark_prefix_and_network_rules_parse_and_evaluate() {
        let source = r#"
# Locagens exec rules
prefix_rule(pattern = ["cargo", ["check", "test"], "--workspace"], decision = "allow",
            justification = "cargo check/test is safe")
prefix_rule(pattern = ["npm", "publish"], decision = "forbidden",
            justification = "publishing must be manual")
prefix_rule(pattern = ["git", "status"], decision = "allow",
            match = ["git status", "git status --short"],
            not_match = ["git log"])
network_rule(host = "crates.io", protocol = "https", decision = "allow")
"#;
        let policy = Policy::parse_str(source).unwrap();
        assert_eq!(policy.prefix_rules().len(), 3);
        assert_eq!(policy.network_rules().len(), 1);
        assert_eq!(policy.network_rules()[0].host, "crates.io");
        assert_eq!(policy.network_rules()[0].protocol.as_deref(), Some("https"));

        let evaluation = policy.evaluate_command("cargo check --workspace");
        assert_eq!(evaluation.decision, Decision::Allow);
        assert!(evaluation.matched_rule);
        assert_eq!(evaluation.justification.as_deref(), Some("cargo check/test is safe"));

        let evaluation = policy.evaluate_command("npm publish");
        assert_eq!(evaluation.decision, Decision::Forbidden);

        let evaluation = policy.evaluate_command("git status && echo hi");
        assert_eq!(evaluation.decision, Decision::Allow, "strictest of allow+allow");

        // Unknown commands fall back to command-safety classification.
        let evaluation = policy.evaluate_command("npm test");
        assert_eq!(evaluation.decision, Decision::Prompt);
        assert!(!evaluation.matched_rule);

        // Invalid match examples fail parsing.
        assert!(Policy::parse_str(
            "prefix_rule(pattern = [\"cargo\"], decision = \"allow\", match = [\"git status\"])"
        )
        .is_err());
        assert!(Policy::parse_str(
            "prefix_rule(pattern = [\"cargo\"], decision = \"allow\", not_match = [\"cargo build\"])"
        )
        .is_err());
        // Valid examples parse.
        assert!(Policy::parse_str(
            "prefix_rule(pattern = [\"cargo\"], decision = \"allow\", match = [\"cargo check\"], not_match = [\"npm test\"])"
        )
        .is_ok());
    }

    #[test]
    fn amend_appends_allow_rule_and_skips_banned_interpreters() {
        let dir = std::env::temp_dir().join(format!("locagens-rules-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rules_file = dir.join("default.rules");

        let appended = append_allow_prefix_rule(&rules_file, "cargo test --workspace").unwrap();
        assert!(appended);
        let source = std::fs::read_to_string(&rules_file).unwrap();
        assert!(source.contains("prefix_rule(pattern = [\"cargo\", \"test\", \"--workspace\"]"));
        // The appended rule now evaluates as Allow.
        let policy = Policy::parse_str(&source).unwrap();
        assert_eq!(policy.evaluate_command("cargo test --workspace").decision, Decision::Allow);

        // A second amendment is a no-op (already allowed).
        assert!(!append_allow_prefix_rule(&rules_file, "cargo test --workspace").unwrap());
        let source = std::fs::read_to_string(&rules_file).unwrap();
        assert_eq!(source.matches("prefix_rule").count(), 1);

        // Banned interpreters / compound / unsafe commands are never amended.
        for command in [
            "sh -c 'echo hi'",
            "bash -lc 'ls'",
            "python script.py",
            "sudo apt install",
            "rm -rf /",
            "node server.js",
            "cargo build && npm test",
            "echo $(whoami)",
        ] {
            assert!(
                !append_allow_prefix_rule(&rules_file, command).unwrap(),
                "{command} must not be amended"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&rules_file)
                .unwrap()
                .matches("prefix_rule")
                .count(),
            1
        );

        // Path tokens are kept verbatim (basename check strips dirs / .exe).
        let appended = append_allow_prefix_rule(&rules_file, "./scripts/lint.sh --fix").unwrap();
        assert!(appended, "arbitrary binaries are amendable");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
