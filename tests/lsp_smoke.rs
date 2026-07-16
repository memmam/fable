//! End-to-end language-server test: spawn `fable lsp`, speak JSON-RPC over
//! its stdio, and check diagnostics, hover, and go-to-definition.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

fn fable_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fable")
}

struct Client {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    next_id: i64,
}

impl Client {
    fn start() -> Client {
        let mut child = Command::new(fable_bin())
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn fable lsp");
        let reader = BufReader::new(child.stdout.take().unwrap());
        Client { child, reader, next_id: 1 }
    }

    fn send(&mut self, body: &str) {
        let stdin = self.child.stdin.as_mut().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: &str) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#
        ));
        id
    }

    fn notify(&mut self, method: &str, params: &str) {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","method":"{method}","params":{params}}}"#
        ));
    }

    fn read_message(&mut self) -> String {
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("read header");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf).expect("read body");
        String::from_utf8(buf).unwrap()
    }

    /// Read messages until one contains `needle`.
    fn read_until(&mut self, needle: &str) -> String {
        for _ in 0..50 {
            let m = self.read_message();
            if m.contains(needle) {
                return m;
            }
        }
        panic!("no message containing {needle:?}");
    }

    fn shutdown(mut self) {
        self.request("shutdown", "null");
        let _ = self.read_until("\"id\":");
        self.notify("exit", "null");
        let status = self.child.wait().expect("wait");
        assert!(status.success(), "lsp exit status: {status:?}");
    }
}

fn uri_for(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join("fable-lsp-smoke");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    (path.clone(), format!("file://{}", path.display()))
}

/// TEMPORARY CI diagnostic (remove once the real Windows hang below is found
/// and fixed): `diagnostics_hover_definition` has hung indefinitely on every
/// `windows-latest` CI run since the Win32/WGL backend first merged --
/// confirmed passing in the same `cargo test` invocation on Linux/macOS, so
/// this is a real, Windows-only bug in this test's stdio child-process
/// protocol, not anything in `window`/`gl`. A plain hang gives no log output
/// at all (see the `gl-windows` job's own `timeout-minutes` comment) and
/// previously cost up to 90 minutes of windows-latest CI time per run just
/// to discover that. This watchdog runs on its own thread (so it isn't
/// subject to libtest's per-test output capture the way the main test
/// thread's `eprintln!`s below are -- `--nocapture` on the CI invocation
/// handles those) and force-exits reporting the last completed step after a
/// short, generous margin: normal execution here is well under a second on
/// every platform that's ever passed this test, so 30s of no progress is
/// unambiguously a hang, not a slow CI runner.
fn arm_watchdog(step: Arc<Mutex<&'static str>>) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(30));
        eprintln!(
            "lsp_smoke WATCHDOG: no progress past step {:?} after 30s -- forcing exit \
             instead of waiting out the CI job timeout",
            *step.lock().unwrap()
        );
        std::process::exit(97);
    });
}

#[test]
fn diagnostics_hover_definition() {
    let step: Arc<Mutex<&'static str>> = Arc::new(Mutex::new("test start"));
    arm_watchdog(Arc::clone(&step));
    macro_rules! at {
        ($s:literal) => {{
            eprintln!("lsp_smoke: {}", $s);
            *step.lock().unwrap() = $s;
        }};
    }

    at!("spawning fable lsp");
    let mut c = Client::start();
    at!("sending initialize");
    c.request("initialize", r#"{"capabilities":{}}"#);
    at!("awaiting initialize response");
    let init = c.read_until("fable-lsp");
    assert!(init.contains("hoverProvider"));
    c.notify("initialized", "{}");

    // A document with a type error produces a diagnostic with its code.
    at!("opening bad.fable");
    let (path, uri) = uri_for("bad.fable");
    std::fs::write(&path, "").unwrap();
    let bad = r#"let x: Int = \"hi\";"#;
    c.notify(
        "textDocument/didOpen",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri}","languageId":"fable","version":1,"text":"{bad}"}}}}"#
        ),
    );
    at!("awaiting diagnostics for bad.fable");
    let diags = c.read_until("publishDiagnostics");
    assert!(diags.contains("E0301"), "diagnostics: {diags}");

    // Fixing the document clears the diagnostics.
    at!("sending didChange fix for bad.fable");
    c.notify(
        "textDocument/didChange",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri}","version":2}},"contentChanges":[{{"text":"let x: Int = 1;"}}]}}"#
        ),
    );
    at!("awaiting cleared diagnostics for bad.fable");
    let diags = c.read_until("publishDiagnostics");
    assert!(diags.contains(r#""diagnostics":[]"#), "diagnostics: {diags}");

    // Hover over a call reports the checked type; definition jumps to the fn.
    at!("opening good.fable");
    let (path2, uri2) = uri_for("good.fable");
    std::fs::write(&path2, "").unwrap();
    let good = r#"fn double(n: Int) -> Int {\n    n * 2\n}\nlet answer = double(21);\n"#;
    c.notify(
        "textDocument/didOpen",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri2}","languageId":"fable","version":1,"text":"{good}"}}}}"#
        ),
    );
    at!("awaiting diagnostics for good.fable");
    let _ = c.read_until("publishDiagnostics");

    // Position on `double` in the call on line 3: `let answer = double(21);`
    at!("sending hover request");
    c.request(
        "textDocument/hover",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri2}"}},"position":{{"line":3,"character":14}}}}"#
        ),
    );
    at!("awaiting hover response");
    let hover = c.read_until("contents");
    assert!(
        hover.contains("fn(Int) -> Int"),
        "hover: {hover}"
    );

    at!("sending definition request");
    c.request(
        "textDocument/definition",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri2}"}},"position":{{"line":3,"character":14}}}}"#
        ),
    );
    at!("awaiting definition response");
    let def = c.read_until("range");
    // The definition is `double` on line 0, character 3.
    assert!(def.contains(r#""line":0"#), "definition: {def}");
    assert!(def.contains(r#""character":3"#), "definition: {def}");

    // Completion after a dot on a struct value: methods and fields.
    at!("opening complete.fable");
    let (path3, uri3) = uri_for("complete.fable");
    std::fs::write(&path3, "").unwrap();
    let src3 = r#"struct Point { x: Float, y: Float }\nimpl Point {\n    fn len(self) -> Float { (self.x * self.x + self.y * self.y).sqrt() }\n}\nlet p = Point { x: 3.0, y: 4.0 };\nlet d = p.len();\n"#;
    c.notify(
        "textDocument/didOpen",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri3}","languageId":"fable","version":1,"text":"{src3}"}}}}"#
        ),
    );
    at!("awaiting diagnostics for complete.fable");
    let _ = c.read_until("publishDiagnostics");
    // Simulate typing `p.` at the end (buffer no longer parses; completion
    // answers from the last good analysis).
    let src4 = format!("{src3}p.");
    at!("sending didChange p. for complete.fable");
    c.notify(
        "textDocument/didChange",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri3}","version":2}},"contentChanges":[{{"text":"{src4}"}}]}}"#
        ),
    );
    at!("awaiting diagnostics after p.");
    let _ = c.read_until("publishDiagnostics");
    at!("sending completion request (struct)");
    c.request(
        "textDocument/completion",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri3}"}},"position":{{"line":6,"character":2}}}}"#
        ),
    );
    at!("awaiting completion response (struct)");
    let comp = c.read_until("label");
    assert!(comp.contains(r#""label":"len""#), "completion: {comp}");
    assert!(comp.contains(r#""label":"x""#), "completion: {comp}");

    // Completion after a module alias dot: std.json's pub members.
    at!("opening complete_mod.fable");
    let (path5, uri5) = uri_for("complete_mod.fable");
    std::fs::write(&path5, "").unwrap();
    let src5 = r#"import std.json;\nlet x = 1;\n"#;
    c.notify(
        "textDocument/didOpen",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri5}","languageId":"fable","version":1,"text":"{src5}"}}}}"#
        ),
    );
    at!("awaiting diagnostics for complete_mod.fable");
    let _ = c.read_until("publishDiagnostics");
    let src6 = r#"import std.json;\nlet x = 1;\njson."#;
    at!("sending didChange json. for complete_mod.fable");
    c.notify(
        "textDocument/didChange",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri5}","version":2}},"contentChanges":[{{"text":"{src6}"}}]}}"#
        ),
    );
    at!("awaiting diagnostics after json.");
    let _ = c.read_until("publishDiagnostics");
    at!("sending completion request (module)");
    c.request(
        "textDocument/completion",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri5}"}},"position":{{"line":2,"character":5}}}}"#
        ),
    );
    at!("awaiting completion response (module)");
    let comp = c.read_until("label");
    assert!(comp.contains(r#""label":"parse""#), "completion: {comp}");
    assert!(comp.contains(r#""label":"stringify""#), "completion: {comp}");

    at!("shutting down");
    c.shutdown();
    at!("done");
}
