//! `lockpick-lsp` — a minimal Language Server for SMOL (LSP 3.17 over stdio).
//!
//! Implements a subset of the Language Server Protocol using JSON-RPC 2.0 with
//! `Content-Length` framing, built with **only** `serde_json` and `std`
//! (no `lsp-server` / `lsp-types` / `tower-lsp` dependencies).
//!
//! Supported methods:
//!
//! - `initialize` / `initialized`
//! - `textDocument/didOpen` (full text) and `textDocument/didChange` (FULL
//!   sync: `contentChanges[0].text` replaces the whole document)
//! - `textDocument/didSave`
//! - `shutdown` / `exit`
//!
//! After every open/change/save the server re-verifies the project and pushes
//! `textDocument/publishDiagnostics` notifications for every affected file.
//!
//! Launch it from an editor / LSP client as a stdio subprocess, e.g.:
//!
//! ```json
//! { "command": ["/path/to/target/release/lockpick-lsp"] }
//! ```
//!
//! The server stores open documents in memory keyed by their LSP `uri` and
//! verifies either via a `smol.toml` manifest in the workspace root (when one
//! exists) or via the in-memory `verify_sources` API over the open documents
//! plus any `.smol` files found under the workspace.

use serde_json::{json, Map, Value};
use smol_verifier::verify_manifest;
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process;

// ─── Server state ─────────────────────────────────────────────────────────── //

/// An open document: its on-disk path plus the current full text.
#[derive(Clone)]
struct OpenDocument {
    path: PathBuf,
    text: String,
}

struct Server {
    /// `uri` -> open document.
    open_docs: HashMap<String, OpenDocument>,
    /// Workspace root captured from `initialize` (`rootUri` / `rootPath`),
    /// falling back to the directory of the first opened file, then the cwd.
    workspace_root: Option<PathBuf>,
    /// URIs for which a `publishDiagnostics` notification has already been
    /// sent, so cleared diagnostics can be published as empty arrays.
    previously_published: HashSet<String>,
}

impl Server {
    fn new() -> Self {
        Self {
            open_docs: HashMap::new(),
            workspace_root: None,
            previously_published: HashSet::new(),
        }
    }

    /// The effective workspace root.
    fn workspace_root(&self) -> PathBuf {
        if let Some(ws) = &self.workspace_root {
            return ws.clone();
        }
        if let Some(first) = self.open_docs.values().next()
            && let Some(parent) = first.path.parent() {
                return parent.to_path_buf();
            }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    // ─── JSON-RPC dispatch ──────────────────────────────────────────────── //

    fn handle(&mut self, msg: &Value, out: &mut impl Write) -> io::Result<()> {
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        match method.as_str() {
            "initialize" => {
                // Capture the workspace root for later verification.
                if let Some(params) = msg.get("params") {
                    self.workspace_root = extract_workspace_root(params);
                }
                let result = json!({
                    "capabilities": {
                        "textDocumentSync": 1, // FULL sync
                    },
                    "serverInfo": {
                        "name": "lockpick-lsp",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                });
                if let Some(id) = id {
                    respond(out, id, Some(result), None)?;
                }
            }
            // Protocol-internal notifications we can safely ignore. Same with a message with no recognizable method (e.g. malformed) is skipped.
            "initialized" | "$/cancelRequest" | "$/setTrace" | "workspace/didChangeConfiguration" | "" => {
                // No-op: the client has completed the handshake.
            }
            "textDocument/didOpen" => {
                self.on_did_open(msg);
                self.publish_diagnostics(out)?;
            }
            "textDocument/didChange" => {
                self.on_did_change(msg);
                self.publish_diagnostics(out)?;
            }
            "textDocument/didSave" => {
                // Re-verify from the stored (or on-disk) text.
                self.publish_diagnostics(out)?;
            }
            "shutdown" => {
                if let Some(id) = id {
                    respond(out, id, Some(Value::Null), None)?;
                }
            }
            "exit" => {
                // Standard LSP shutdown sequence: `shutdown` then `exit`.
                process::exit(0);
            }
            _ => {
                // Unknown method → standard JSON-RPC "method not found" error.
                if let Some(id) = id {
                    let error = json!({
                        "code": -32601,
                        "message": format!("method not found: {method}"),
                    });
                    respond(out, id, None, Some(error))?;
                }
            }
        }
        Ok(())
    }

    // ─── Document lifecycle ─────────────────────────────────────────────── //

    fn on_did_open(&mut self, msg: &Value) {
        let Some(params) = msg.get("params") else { return };
        let Some(uri) = params["textDocument"]["uri"].as_str() else {
            return;
        };
        let text = params["textDocument"]["text"].as_str().unwrap_or("").to_string();
        let path = uri_to_path(uri);
        self.open_docs
            .insert(uri.to_string(), OpenDocument { path: path.clone(), text });
        // Fall back the workspace root to the directory of the first opened file.
        if self.workspace_root.is_none()
            && let Some(parent) = path.parent() {
                self.workspace_root = Some(parent.to_path_buf());
            }
    }

    /// FULL-sync change: `contentChanges[0].text` replaces the whole document.
    fn on_did_change(&mut self, msg: &Value) {
        let Some(params) = msg.get("params") else { return };
        let Some(uri) = params["textDocument"]["uri"].as_str() else {
            return;
        };
        let text = params["contentChanges"][0]["text"].as_str().unwrap_or("");
        if let Some(doc) = self.open_docs.get_mut(uri) {
            doc.text = text.to_string();
        }
    }

    // ─── Verification ───────────────────────────────────────────────────── //

    /// Collect the sources to verify: the open documents plus any `.smol`
    /// files found under the workspace root.  When a `smol.toml` manifest
    /// exists in the workspace root it takes precedence (the manifest
    /// determines the full file list, prefixes and CTL properties).
    fn verify(&self) -> Vec<smol_verifier::Diagnostic> {
        let ws = self.workspace_root();
        let manifest = ws.join("smol.toml");
        if manifest.exists() {
            return verify_manifest(&manifest);
        }

        let mut sources: Vec<(PathBuf, String)> = Vec::new();
        for doc in self.open_docs.values() {
            sources.push((doc.path.clone(), doc.text.clone()));
        }
        let mut discovered = Vec::new();
        collect_smol_files(&ws, &mut discovered);
        for path in discovered {
            if sources.iter().any(|(s, _)| s == &path) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                sources.push((path, text));
            }
        }
        smol_verifier::verify_sources(&sources, &[])
    }

    /// Re-verify and publish `textDocument/publishDiagnostics` notifications
    /// for every open document plus any file that previously had diagnostics.
    fn publish_diagnostics(&mut self, out: &mut impl Write) -> io::Result<()> {
        let ws = self.workspace_root();
        let diags = self.verify();

        // Group diagnostics by the LSP uri of the file they belong to.
        let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();
        for d in &diags {
            let (uri, text) = match &d.file {
                Some(file) => {
                    let abs = absolute(Path::new(file), &ws);
                    let text = self.text_for(&abs).unwrap_or_default();
                    (path_uri(&abs), text)
                }
                // No file → publish under the workspace root with an empty range.
                None => (path_uri(&ws), String::new()),
            };
            let lsp = to_lsp_diagnostic(d, &text);
            grouped.entry(uri).or_default().push(lsp);
        }

        // Targets: every open doc, every previously published file, and every
        // file that produced diagnostics in this pass (so cleared diagnostics
        // are sent as empty arrays only for files we have already published).
        let mut targets: HashSet<String> = HashSet::new();
        targets.extend(self.open_docs.keys().cloned());
        targets.extend(self.previously_published.iter().cloned());
        targets.extend(grouped.keys().cloned());

        for uri in &targets {
            let diags = grouped.remove(uri).unwrap_or_default();
            let notification = json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": uri,
                    "diagnostics": diags,
                }
            });
            write_message(out, &notification)?;
        }

        self.previously_published = targets;
        Ok(())
    }

    /// The text of a file, preferring the in-memory open document.
    fn text_for(&self, abs: &Path) -> Option<String> {
        for doc in self.open_docs.values() {
            if doc.path == abs {
                return Some(doc.text.clone());
            }
        }
        std::fs::read_to_string(abs).ok()
    }
}

// ─── Diagnostic conversion ────────────────────────────────────────────────── //

/// Convert a SMOL diagnostic into an LSP `Diagnostic` JSON object.
fn to_lsp_diagnostic(d: &smol_verifier::Diagnostic, text: &str) -> Value {
    let severity = match d.severity {
        smol_verifier::Severity::Error => 1,
        smol_verifier::Severity::Warning => 2,
        smol_verifier::Severity::Note => 3,
    };
    let range = match d.span {
        Some(span) => {
            let (sl, sc) = offset_to_position(text, span.start);
            let (el, ec) = offset_to_position(text, span.end);
            json!({
                "start": { "line": sl, "character": sc },
                "end":   { "line": el, "character": ec },
            })
        }
        // No span → an empty range at the top of the file.
        None => json!({
            "start": { "line": 0, "character": 0 },
            "end":   { "line": 0, "character": 0 },
        }),
    };
    json!({
        "range": range,
        "severity": severity,
        "code": d.code.to_string(),
        "source": "lockpick",
        "message": d.message.clone(),
    })
}

/// Convert a byte offset into a 0-based `(line, character)` position.
///
/// `line` counts `\n` separators; `character` is the number of Unicode scalar
/// values since the start of the line (a UTF-8 approximation of the LSP
/// UTF-16 column — sufficient for SMOL sources, which are ASCII in practice).
fn offset_to_position(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    // Clamp to a valid UTF-8 boundary so slicing never panics on a
    // mid-character offset.
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count();
    let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
    let character = prefix[line_start..].chars().count();
    (line, character)
}

// ─── URIs and paths ───────────────────────────────────────────────────────── //

/// Convert an LSP `file://` uri to a filesystem path.
fn uri_to_path(uri: &str) -> PathBuf {
    match uri.strip_prefix("file://") {
        Some(rest) => PathBuf::from(rest),
        None => PathBuf::from(uri),
    }
}

/// Convert a filesystem path to an LSP `file://` uri.
fn path_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

/// Resolve `p` against the workspace root when it is relative.
fn absolute(p: &Path, ws: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        ws.join(p)
    }
}

/// Extract the workspace root from `initialize` params (`rootUri` preferred,
/// falling back to `rootPath`).
fn extract_workspace_root(params: &Value) -> Option<PathBuf> {
    if let Some(uri) = params["rootUri"].as_str()
        && !uri.is_empty() {
            return Some(uri_to_path(uri));
        }
    if let Some(root) = params["rootPath"].as_str()
        && !root.is_empty() {
            return Some(PathBuf::from(root));
        }
    None
}

/// Recursively collect every `*.smol` file under `dir`, skipping build
/// artifacts and hidden directories.
fn collect_smol_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if path.is_dir() {
            if name_str == "target" || name_str == ".git" || name_str.starts_with('.') {
                continue;
            }
            collect_smol_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("smol") {
            out.push(path);
        }
    }
}

// ─── JSON-RPC framing ─────────────────────────────────────────────────────── //

/// Read a single `Content-Length` framed JSON-RPC message body.
///
/// Returns `Ok(None)` on EOF, `Ok(Some(body))` on success, and `Err` on a
/// framing error (callers should skip gracefully rather than panic).
fn read_message(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None); // EOF
        }
        let line = line.trim_end();
        if line.is_empty() {
            break; // end of headers
        }
        if let Some((key, value)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("content-length")
                && let Ok(len) = value.trim().parse::<usize>() {
                    content_length = Some(len);
                }
    }
    let Some(len) = content_length else {
        return Ok(None); // no Content-Length header: nothing to read
    };
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Write a JSON-RPC message to the client with `Content-Length` framing.
fn write_message(out: &mut impl Write, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_string(msg).map_err(io::Error::other)?;
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

/// Write a JSON-RPC response (either a `result` or an `error`).
fn respond(
    out: &mut impl Write,
    id: Value,
    result: Option<Value>,
    error: Option<Value>,
) -> io::Result<()> {
    let mut msg = Map::new();
    msg.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
    msg.insert("id".to_string(), id);
    if let Some(result) = result {
        msg.insert("result".to_string(), result);
    }
    if let Some(error) = error {
        msg.insert("error".to_string(), error);
    }
    write_message(out, &Value::Object(msg))
}

// ─── Entry point ──────────────────────────────────────────────────────────── //

fn main() {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut server = Server::new();

    loop {
        match read_message(&mut reader) {
            Ok(Some(body)) => {
                match serde_json::from_str::<Value>(&body) {
                    Ok(msg) => {
                        // A malformed-but-parseable message is handled;
                        // unknown methods get a JSON-RPC error response.
                        if let Err(e) = server.handle(&msg, &mut out) {
                            eprintln!("lockpick-lsp: handler error: {e}");
                        }
                    }
                    Err(e) => {
                        // Malformed JSON: skip it gracefully, do not panic.
                        eprintln!("lockpick-lsp: skipping malformed message: {e}");
                    }
                }
            }
            Ok(None) => break, // EOF
            Err(e) => {
                eprintln!("lockpick-lsp: read error: {e}");
                break;
            }
        }
    }
}