//! Typed input of exec sessions (`write_stdin`) that runs as commands
//! (QA-012).
//!
//! The rule:
//!
//! - A session *reads commands* when its approved command starts a known
//!   shell or REPL — `cmd`, `powershell`/`pwsh`, `bash`/`sh`/`zsh`/…,
//!   `python`, `node`, database and remote shells, … — without a script or
//!   a one-shot command operand (`bash`, `python -i`, `cmd /k`,
//!   `pwsh -NoExit`, `cd sub && python`). See [`reads_commands_from_stdin`].
//! - In such a session every *submitted line* — text up to `\n` or `\r` — is
//!   a command and goes through the same approval as `exec_command`. Text
//!   after the last line break is held back (not written) until a later
//!   write completes the line, so a command cannot be smuggled in pieces.
//!   See [`split_submitted_input`].
//! - Interrupt / end-of-input keys on their own (Ctrl-C `\u{3}`, Ctrl-D
//!   `\u{4}`, Ctrl-Z `\u{1a}`, Ctrl-\ `\u{1c}`) are always sent and drop the
//!   held text. See [`is_signal_input`].
//! - Input for any other program (answering a prompt of a running build,
//!   feeding data to a script) is plain data and is sent as-is.

use crate::command_safety::program_name;
use crate::rules::{split_shell_commands_in, ShellDialect};

/// Shells: interactive unless they got a script or `-c` command.
const POSIX_SHELLS: [&str; 13] =
    ["sh", "bash", "zsh", "dash", "ksh", "mksh", "ash", "fish", "csh", "tcsh", "nu", "elvish", "xonsh"];

/// Database and remote shells: every line is a command whatever the
/// operands (`psql mydb`, `ssh host`, `sqlite3 app.db`).
/// (Names are matched without a version suffix: `sqlite3` → `sqlite`.)
const ALWAYS_INTERACTIVE: [&str; 19] = [
    "ssh", "telnet", "mosh", "wsl", "su", "sqlite", "psql", "mysql", "mariadb", "mongo", "mongosh", "redis-cli",
    "sqlcmd", "gdb", "lldb", "ftp", "sftp", "jshell", "ghci",
];

/// REPL interpreters with a generic "script or eval flag" rule.
const REPLS: [&str; 22] = [
    "irb", "ruby", "perl", "php", "lua", "luajit", "julia", "scala", "sbcl", "clojure", "clj", "iex", "erl", "ocaml",
    "utop", "guile", "racket", "tclsh", "wish", "groovysh", "bc", "dc",
];

/// Wrapper programs that run their operand (`env X=1 bash`, `nohup python`).
const WRAPPERS: [&str; 14] = [
    "env", "exec", "command", "builtin", "nohup", "time", "nice", "stdbuf", "winpty", "start", "call", "busybox", "sudo",
    "doas",
];

/// Wrapper flags followed by a separate value (`nice -n 5`, `sudo -u bob`).
const WRAPPER_VALUE_FLAGS: [&str; 5] = ["-n", "-u", "-g", "-C", "-p"];

/// Keys that are always sent, even to a shell: interrupt / end of input.
const SIGNAL_KEYS: [char; 4] = ['\u{3}', '\u{4}', '\u{1a}', '\u{1c}'];

/// Whether typed input to a session started with `command` is a stream of
/// commands (a shell or REPL reading its stdin) rather than plain data.
///
/// Errs on the side of "commands": a false positive only costs a prompt.
pub fn reads_commands_from_stdin(command: &str) -> bool {
    let segments = match split_shell_commands_in(command, ShellDialect::platform()) {
        Ok(segments) => segments,
        Err(_) => naive_segments(command),
    };
    segments.iter().any(|segment| segment_reads_commands(segment))
}

/// Separator split for commands the shell grammar rejects (redirections,
/// substitutions): quotes are dropped, every piece is a segment.
fn naive_segments(command: &str) -> Vec<Vec<String>> {
    command
        .split([';', '&', '|', '(', ')', '\n', '<', '>', '`'])
        .map(|part| {
            part.split_whitespace()
                .map(|token| token.trim_matches(['"', '\'', '^']).to_string())
                .filter(|token| !token.is_empty())
                .collect::<Vec<String>>()
        })
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Interpreter name without version suffix (`python3.12` → `python`).
fn interpreter_name(token: &str) -> String {
    let name = program_name(token);
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if trimmed.is_empty() {
        name
    } else {
        trimmed.to_string()
    }
}

/// Skips leading `VAR=value` assignments and wrapper programs (with their
/// own flags) to the program that really runs. The flag says whether a
/// wrapper itself opens a shell (`sudo -i`, `doas -s`).
fn strip_wrappers(tokens: &[String]) -> (&[String], bool) {
    let mut rest = tokens;
    let mut opens_shell = false;
    loop {
        let Some(first) = rest.first() else { return (rest, opens_shell) };
        let is_assignment = first
            .split_once('=')
            .is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        if is_assignment {
            rest = &rest[1..];
            continue;
        }
        let wrapper = program_name(first);
        if !WRAPPERS.contains(&wrapper.as_str()) {
            return (rest, opens_shell);
        }
        let escalates = wrapper == "sudo" || wrapper == "doas";
        rest = &rest[1..];
        while let Some(flag) = rest.first() {
            let is_flag = flag.starts_with('-') || (flag.starts_with('/') && flag.len() <= 3);
            if !is_flag {
                break;
            }
            opens_shell |= escalates && !flag.starts_with("--") && (flag.contains('i') || flag.contains('s'));
            let takes_value = WRAPPER_VALUE_FLAGS.contains(&flag.as_str());
            rest = &rest[if takes_value { 2 } else { 1 }.min(rest.len())..];
        }
    }
}

fn segment_reads_commands(tokens: &[String]) -> bool {
    let (tokens, wrapper_opens_shell) = strip_wrappers(tokens);
    let Some(first) = tokens.first() else { return wrapper_opens_shell };
    let program = interpreter_name(first);
    let args = &tokens[1..];
    let lower: Vec<String> = args.iter().map(|arg| arg.to_lowercase()).collect();
    match program.as_str() {
        "cmd" => cmd_is_interactive(&lower),
        "powershell" | "pwsh" | "powershell_ise" => powershell_is_interactive(&lower),
        "python" | "py" | "pypy" | "ipython" => python_is_interactive(&lower),
        "node" | "nodejs" => node_is_interactive(&lower),
        "deno" | "bun" => lower.is_empty() || lower[0] == "repl",
        "docker" | "podman" | "nerdctl" | "kubectl" | "oc" => container_shell_is_interactive(&lower),
        name if ALWAYS_INTERACTIVE.contains(&name) => true,
        name if POSIX_SHELLS.contains(&name) => posix_shell_is_interactive(&lower),
        name if REPLS.contains(&name) => repl_is_interactive(&lower),
        _ => false,
    }
}

/// `cmd` reads commands unless it got `/c` (run and exit) without `/k`.
fn cmd_is_interactive(args: &[String]) -> bool {
    let runs_once = args.iter().any(|arg| arg.starts_with("/c"));
    let keeps_open = args.iter().any(|arg| arg.starts_with("/k"));
    keeps_open || !runs_once
}

/// PowerShell reads commands unless `-Command` / `-File` /
/// `-EncodedCommand` names something other than `-` (stdin), or with
/// `-NoExit`. A bare positional script is treated as interactive (safe
/// side: only costs prompts).
fn powershell_is_interactive(args: &[String]) -> bool {
    if args.iter().any(|arg| arg.starts_with("-noe")) {
        return true;
    }
    let run_flag = |arg: &str| {
        arg == "-c" || arg == "/c" || arg.starts_with("-com") || arg == "--command" || arg == "-f" || arg.starts_with("-fi")
            || arg == "-e" || arg == "-ec" || arg.starts_with("-en")
    };
    match args.iter().position(|arg| run_flag(arg)) {
        Some(index) => args.get(index + 1).is_none_or(|value| value == "-"),
        None => true,
    }
}

/// `-o opt`, `+o opt`, `-O shopt`, `--rcfile f`, `--init-file f` take a value.
fn posix_shell_is_interactive(args: &[String]) -> bool {
    let mut explicit = false;
    let mut runs_once = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if matches!(arg, "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file") {
            index += 2;
            continue;
        }
        if arg == "-" || arg == "--" {
            explicit |= arg == "-";
            index += 1;
            continue;
        }
        if arg.starts_with("--") {
            explicit |= arg == "--interactive";
        } else if arg.starts_with('-') || arg.starts_with('+') {
            explicit |= arg[1..].contains('i') || arg[1..].contains('s');
            runs_once |= arg[1..].contains('c');
        } else {
            // A script operand (or the `-c` command string).
            runs_once = true;
        }
        index += 1;
    }
    explicit || !runs_once
}

fn python_is_interactive(args: &[String]) -> bool {
    let mut explicit = false;
    let mut runs_once = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-" {
            explicit = true;
            break;
        }
        // `-W arg`, `-X opt` and `--check-hash-based-pycs mode` take a value.
        if matches!(arg, "-w" | "-x" | "--check-hash-based-pycs") {
            index += 2;
            continue;
        }
        if arg.starts_with("--") {
            index += 1;
            continue;
        }
        if let Some(flags) = arg.strip_prefix('-') {
            // `py -3`, `py -3.12`, `py -V:3.12` select a version.
            if flags.starts_with(|c: char| c.is_ascii_digit()) || flags.starts_with("v:") {
                index += 1;
                continue;
            }
            explicit |= flags.contains('i');
            if flags.contains('c') || flags.contains('m') {
                // The rest of the argv belongs to the command / module.
                runs_once = true;
                break;
            }
            index += 1;
            continue;
        }
        runs_once = true;
        break;
    }
    explicit || !runs_once
}

fn node_is_interactive(args: &[String]) -> bool {
    const VALUE_FLAGS: [&str; 8] = ["-r", "--require", "--import", "--loader", "--experimental-loader", "-c", "--conditions", "--input-type"];
    let mut explicit = false;
    let mut runs_once = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-" || arg == "-i" || arg == "--interactive" {
            explicit = true;
        } else if matches!(arg, "-e" | "--eval" | "-p" | "--print") {
            runs_once = true;
            index += 2;
            continue;
        } else if VALUE_FLAGS.contains(&arg) {
            index += 2;
            continue;
        } else if !arg.starts_with('-') {
            runs_once = true;
            break;
        }
        index += 1;
    }
    explicit || !runs_once
}

/// `docker exec -it …`, `kubectl exec -i …`: an interactive container
/// session reads commands; anything else (`docker build .`) does not.
fn container_shell_is_interactive(args: &[String]) -> bool {
    let attaches = args.iter().any(|arg| matches!(arg.as_str(), "exec" | "run" | "attach" | "debug" | "rsh"));
    let interactive = args.iter().any(|arg| {
        arg == "--interactive" || arg == "--stdin" || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('i'))
    });
    attaches && interactive
}

/// Generic REPLs read commands unless they got a script file (an operand
/// with a `.` or a path separator) or an eval flag, without `-i` / `-`.
fn repl_is_interactive(args: &[String]) -> bool {
    let explicit = args.iter().any(|arg| arg == "-i" || arg == "-a" || arg == "-" || arg == "--interactive");
    let evals = args.iter().any(|arg| matches!(arg.as_str(), "-e" | "-c" | "-r" | "-E" | "--eval"));
    let script = args.iter().any(|arg| !arg.starts_with('-') && arg.contains(['.', '/', '\\']));
    explicit || !(evals || script)
}

/// Whether `chars` consists only of interrupt / end-of-input keys.
pub fn is_signal_input(chars: &str) -> bool {
    !chars.is_empty() && chars.chars().all(|c| SIGNAL_KEYS.contains(&c))
}

/// Typed input of a command-reading session split at line breaks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmittedInput {
    /// Every complete line (without its terminator), held text first.
    pub lines: Vec<String>,
    /// What to write now: the complete lines with their terminators.
    pub to_send: String,
    /// Text after the last line break, held until a later write ends it.
    pub held: String,
}

/// Splits `held` + `chars` into submitted lines. `\n`, `\r` and `\r\n` end
/// a line (a terminal submits on either).
pub fn split_submitted_input(held: &str, chars: &str) -> SubmittedInput {
    let text = format!("{held}{chars}");
    let mut input = SubmittedInput::default();
    let mut line_start = 0usize;
    let mut chars_iter = text.char_indices().peekable();
    while let Some((index, c)) = chars_iter.next() {
        if c != '\n' && c != '\r' {
            continue;
        }
        let mut end = index + 1;
        if c == '\r' {
            if let Some((next_index, '\n')) = chars_iter.peek().copied() {
                chars_iter.next();
                end = next_index + 1;
            }
        }
        input.lines.push(text[line_start..index].to_string());
        input.to_send.push_str(&text[line_start..end]);
        line_start = end;
    }
    input.held = text[line_start..].to_string();
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_and_repls_without_operands_read_commands() {
        for command in [
            "cmd",
            "cmd.exe /k",
            "cmd /q",
            "powershell",
            "powershell -NoProfile -NoLogo",
            "pwsh -NoExit -Command Get-Date",
            "pwsh -Command -",
            "powershell -ExecutionPolicy Bypass",
            "bash",
            "bash -i",
            "/bin/bash --login",
            "bash -s arg",
            "sh -",
            "zsh -o vi",
            "python",
            "python3.12",
            "python -i script.py",
            "py -3",
            "python -",
            "node",
            "node -i",
            "node --require ts-node/register",
            "ipython",
            "irb",
            "ruby -I lib",
            "perl",
            "php -a",
            "deno",
            "bun repl",
            "psql mydb",
            "sqlite3 app.db",
            "ssh build-host",
            "wsl -d Ubuntu",
            "sudo -i",
            "sudo -u bob bash",
            "busybox sh",
            "docker exec -it web sh",
            "kubectl exec -i pod -- bash",
            "env FOO=1 bash",
            "nohup python",
            "cd sub && python",
            "echo start | bash",
        ] {
            assert!(reads_commands_from_stdin(command), "{command}");
        }
    }

    #[test]
    fn programs_with_a_script_or_command_operand_take_data() {
        for command in [
            "npm run build",
            "cargo test",
            "cmd /c echo hi",
            "powershell -File build.ps1",
            "pwsh -Command Get-Date",
            "powershell -EncodedCommand ZQBjAGgAbwA=",
            "bash script.sh",
            "bash -c \"read x; echo $x\"",
            "sh ./configure",
            "python script.py",
            "python -m http.server",
            "python -c \"input()\"",
            "node server.js",
            "node -e \"process.stdin.pipe(process.stdout)\"",
            "ruby app.rb",
            "perl -e 'print <STDIN>'",
            "deno run main.ts",
            "docker build .",
            "git commit",
            "ping -n 30 127.0.0.1",
        ] {
            assert!(!reads_commands_from_stdin(command), "{command}");
        }
    }

    #[test]
    fn signal_keys_are_recognized_alone() {
        assert!(is_signal_input("\u{3}"));
        assert!(is_signal_input("\u{4}\u{4}"));
        assert!(!is_signal_input(""));
        assert!(!is_signal_input("\u{3}rm -rf x\n"));
        assert!(!is_signal_input("\u{1b}[A"));
    }

    #[test]
    fn lines_are_split_and_the_tail_is_held() {
        let input = split_submitted_input("", "echo a\nmkdir b\r\ncd c\rpart");
        assert_eq!(input.lines, vec!["echo a", "mkdir b", "cd c"]);
        assert_eq!(input.to_send, "echo a\nmkdir b\r\ncd c\r");
        assert_eq!(input.held, "part");

        // A command typed in pieces is only submitted (and checked) whole.
        let first = split_submitted_input("", "rm -rf ");
        assert!(first.lines.is_empty());
        assert_eq!(first.to_send, "");
        let second = split_submitted_input(&first.held, "build\n");
        assert_eq!(second.lines, vec!["rm -rf build"]);
        assert_eq!(second.to_send, "rm -rf build\n");
        assert_eq!(second.held, "");

        let empty = split_submitted_input("", "\n");
        assert_eq!(empty.lines, vec![""]);
    }
}
