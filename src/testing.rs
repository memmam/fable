//! The golden-test runner: any `.fable` file with expectation directives in
//! its comments is a test. Shared by the `fable test` CLI command and the
//! interpreter's own spec suite.
//!
//! Directives (each on its own line, anywhere in the file):
//!   //? expect: <line>     — expected stdout, one directive per line, in order.
//!   //? error: <substring> — compilation must fail; some diagnostic
//!                            (message or code) must contain <substring>.
//!   //? panic: <substring> — the program must panic at runtime; the panic
//!                            message must contain <substring>.
//!
//! A directive must begin the line's comment: `//?` inside a string literal,
//! or later in the text of an ordinary `//` comment, is not a directive (so
//! prose *about* directives can't inject phantom expectations). Expected and
//! actual lines are compared ignoring trailing whitespace — trailing spaces
//! in a directive are invisible in an editor and can't be pinned reliably.
//!
//! A file with no directives must merely run to completion with no output.
//! `expect` cannot be combined with `error`; `expect` + `panic` means the
//! output-so-far must match before the panic.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::RunOutcome;

#[derive(Default)]
pub struct Directives {
    pub expect: Vec<String>,
    pub errors: Vec<String>,
    pub panics: Vec<String>,
}

/// Where the line's comment starts, if that comment is a directive.
///
/// Scans left-to-right with just enough string-literal awareness to skip a
/// `//` inside quotes (`"http://.."`). The first real comment decides: if it
/// starts with `//?` it's a directive, otherwise the rest of the line is
/// prose and cannot contain one.
fn directive_start(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        match b[i] {
            b'\\' if in_str => {
                i += 2;
                continue;
            }
            b'"' => in_str = !in_str,
            b'/' if !in_str && b.get(i + 1) == Some(&b'/') => {
                return if b.get(i + 2) == Some(&b'?') { Some(i) } else { None };
            }
            _ => {}
        }
        i += 1;
    }
    None
}

pub fn parse_directives(text: &str) -> Directives {
    let mut d = Directives::default();
    for line in text.lines() {
        let Some(idx) = directive_start(line) else { continue };
        let rest = line[idx + 3..].trim_start();
        if let Some(v) = rest.strip_prefix("expect:") {
            d.expect.push(v.strip_prefix(' ').unwrap_or(v).to_string());
        } else if let Some(v) = rest.strip_prefix("error:") {
            d.errors.push(v.trim().to_string());
        } else if let Some(v) = rest.strip_prefix("panic:") {
            d.panics.push(v.trim().to_string());
        }
    }
    d
}

/// Recursively collect `.fable` files under `dir` (sorted for determinism).
pub fn collect_fable_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            collect_fable_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "fable") {
            out.push(p);
        }
    }
}

/// Run one test file and check it against its directives.
pub fn check_one(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read failed: {e}"))?;
    let d = parse_directives(&text);
    // Path-based so `import` resolves sibling files and FABLE_PATH.
    let outcome = crate::run_capture_path(path);

    match outcome {
        RunOutcome::CompileError(diags) => {
            if d.errors.is_empty() {
                let mut msg = String::from("unexpected compile error(s):\n");
                for di in diags.iter().filter(|di| di.is_error()) {
                    let _ = writeln!(msg, "  [{}] {}", di.code, di.message);
                }
                return Err(msg);
            }
            for want in &d.errors {
                let hit = diags.iter().any(|di| {
                    di.is_error() && (di.message.contains(want) || di.code.contains(want))
                });
                if !hit {
                    let mut msg =
                        format!("expected a compile error containing {want:?}; got:\n");
                    for di in diags.iter().filter(|di| di.is_error()) {
                        let _ = writeln!(msg, "  [{}] {}", di.code, di.message);
                    }
                    return Err(msg);
                }
            }
            Ok(())
        }
        RunOutcome::Panic { stdout, error } => {
            if d.panics.is_empty() {
                return Err(format!(
                    "unexpected runtime panic: {}\n--- stdout so far ---\n{stdout}",
                    error.msg
                ));
            }
            for want in &d.panics {
                if !error.msg.contains(want) {
                    return Err(format!(
                        "expected panic containing {want:?}, got: {}",
                        error.msg
                    ));
                }
            }
            check_stdout(&d.expect, &stdout)
        }
        RunOutcome::Ok { stdout, .. } => {
            if !d.errors.is_empty() {
                return Err(format!(
                    "expected a compile error containing {:?}, but the program compiled",
                    d.errors
                ));
            }
            if !d.panics.is_empty() {
                return Err(format!(
                    "expected a panic containing {:?}, but the program succeeded",
                    d.panics
                ));
            }
            check_stdout(&d.expect, &stdout)
        }
    }
}

fn check_stdout(expect: &[String], stdout: &str) -> Result<(), String> {
    let got: Vec<&str> = stdout.lines().collect();
    if got.len() != expect.len()
        || !got
            .iter()
            .zip(expect)
            .all(|(g, e)| g.trim_end() == e.trim_end())
    {
        let mut msg = String::new();
        let _ = writeln!(msg, "output mismatch");
        let _ = writeln!(msg, "--- expected ({} lines) ---", expect.len());
        for e in expect {
            let _ = writeln!(msg, "{e}");
        }
        let _ = writeln!(msg, "--- actual ({} lines) ---", got.len());
        for g in &got {
            let _ = writeln!(msg, "{g}");
        }
        return Err(msg);
    }
    Ok(())
}

pub struct TestReport {
    pub total: usize,
    pub failures: Vec<(PathBuf, String)>,
}

/// Run every `.fable` file under the given paths (files are taken as-is,
/// directories are walked) and report failures.
pub fn run_test_paths(paths: &[PathBuf]) -> TestReport {
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            collect_fable_files(p, &mut files);
        } else {
            files.push(p.clone());
        }
    }
    let mut failures = Vec::new();
    for f in &files {
        if let Err(msg) = check_one(f) {
            failures.push((f.clone(), msg));
        }
    }
    TestReport { total: files.len(), failures }
}
