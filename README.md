# <img src="lockpick-logo.webp" alt="Lockpick Logo generated with Google Gemini" width="30"/> Lockpick

A static verification and model-checking tool for **SMOL** (Sensor Modelling and Ontology Language), a domain-specific language for cyber-physical systems that combine **digital twins**, **formal verification**, and **knowledge graphs**.

`lockpick` ships as:

- a **CLI** (`lockpick`) that checks SMOL projects from a manifest or an explicit file list, with both human-readable and JSON output;
- a **library** (`smol-verifier`, re-exported as the `lockpick` crate) that other tools can embed;
- a **Language Server** (`lockpick-lsp`) that gives editors live diagnostics via LSP 3.17.

## Features

### Type system and static analysis

- **Nominal subtyping** with inheritance, abstract classes, and method override contracts
- **List discipline** — transient query results (`List<T>`) vs. persistent model relations (`ExplList<T>`)
- **Field-level access control** — hidden fields, domain-exposed fields, RDF triple generation
- **Flow-sensitive null-safety** — detects definite null-pointer dereferences without false positives
- **Effect system** — `hidden` fields must not leak into `links` fragments; explicit effect contracts (`pure` / `domain-reads` / `domain-writes`) enforced at error level
- **Symbolic execution** — interval abstract interpretation proving definite integer overflow and division-by-zero
- **Termination side conditions** — `while` loops over integers need a visible variant (monotonic update toward the bound)
- **Dead-code warnings** — unused variables, parameters, and unreachable methods
- **Taint analysis** — SPARQL injection detection for query-based knowledge-graph access, with inter-procedural summaries across method calls
- **Resource lifecycle** — path-sensitive FMU (Functional Mock-up Unit) destroy-call validation (every exit path)

### Adaptation and digital-twin verification

- **Adaptation tracking** — methods must declare which state-classes they change via `adapts` modifiers
- **State reachability** — verifies that declared states are actually targeted by adaptation
- **Deadlock detection** — ensures adaptation loops terminate (list advancement, loop variants)

### Knowledge-graph integration

- **OWL reasoning** — loads and checks ontology consistency with a DL reasoner
- **SPARQL validation** — query syntax and schema matching
- **SHACL shapes** — optional shape-based constraint validation
- **IRI reachability** — ensures ontology references are reachable from the root concept

## Installation

Build from source with a Rust toolchain (edition 2024 — Rust 1.87 or newer):

```bash
cargo build --release
```

This produces two binaries:

| Binary | Purpose |
|--------|---------|
| `target/release/lockpick` | Command-line verifier |
| `target/release/lockpick-lsp` | Language Server (LSP 3.17 over stdio) |

Copy them somewhere on your `PATH` if you want to invoke them directly, e.g.:

```bash
cp target/release/lockpick target/release/lockpick-lsp ~/.local/bin/
```

### System Dependencies

OxidOWL uses [Oxigraph](https://github.com/oxigraph/oxigraph) with the `rocksdb` feature, which compiles [RocksDB](https://rocksdb.org/) from source during the build. This requires a **C++17-capable compiler** and **CMake** to be installed on your system.

#### Linux (Ubuntu / Debian)

```bash
sudo apt-get update
sudo apt-get install -y build-essential cmake clang libclang-dev
```

#### Linux (Fedora / RHEL / Rocky)

```bash
sudo dnf install gcc-c++ cmake clang clang-devel
```

#### macOS

Xcode Command Line Tools provide the necessary compiler. CMake can be installed via Homebrew:

```bash
xcode-select --install
brew install cmake
```

#### Windows

Windows users should be sure to use the msvc toolchain when building with Cargo. You can set this as the default with:

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup default stable-x86_64-pc-windows-msvc
```

Then install `llvm`, which provides the `libclang` library required by the `clang-sys` crate. You can download the LLVM installer from the [LLVM releases page](https://releases.llvm.org/download.html). You can also install LLVM via Chocolatey, which is simpler and will set up the necessary environment variables for you:

```powershell
choco install llvm
```

After installing LLVM, set the `LIBCLANG_PATH` environment variable to point to the directory containing `libclang.dll` (usually the `bin` directory of your LLVM installation):


```powershell
$env:LIBCLANG_PATH="C:\Program Files\LLVM\bin"
```

or set it permanently with:

```powershell
setx LIBCLANG_PATH "C:\Program Files\LLVM\bin"
```

You can also install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **"Desktop development with C++"** workload selected. CMake is bundled with Visual Studio, or can be installed separately from [cmake.org](https://cmake.org/download/).

## Docker

You can run lockpick via Docker without installing Rust, CMake, or libclang
locally. The image bundles both binaries (`lockpick` and `lockpick-lsp`) and
builds all native dependencies (RocksDB, clang-sys) inside the builder stage.

Build the image:

```bash
docker build -t lockpick .
```

Run it one-shot against files mounted from your host:

```bash
docker run --rm -v "$PWD:/work" -w /work lockpick lockpick check --manifest examples/odrl/smol.toml
docker run --rm -v "$PWD:/work" -w /work lockpick lockpick check --files examples/error-file.smol --format json
```

For repeated checks, start a long-lived container once and `docker exec` into
it:

```bash
docker run -d --name lockpick -v "$PWD:/work" -w /work lockpick sleep infinity
docker exec lockpick lockpick check --manifest smol.toml
docker exec lockpick lockpick check --files src/*.smol --ontology my-ontology.ttl
docker exec lockpick lockpick check --manifest smol.toml --format json | jq '.error_count'
docker stop lockpick && docker rm lockpick
```

You can also drop into an interactive shell with both binaries on `PATH`:

```bash
docker run --rm -it -v "$PWD:/work" -w /work lockpick bash
```

The container runs as the non-root user `lockpick` (uid 1000). If your mounted
files are owned by a different uid, pass `--user "$(id -u):$(id -g)"` to run as
that user instead.


## Usage (CLI)

```
lockpick check [OPTIONS] [FILE]...
```

The `check` subcommand supports:

| Flag | Meaning |
|------|---------|
| `-m, --manifest <PATH>` | Path to the `smol.toml` manifest (default `smol.toml`) |
| `FILE...` / `--files <FILE>` | `.smol` source files (instead of / in addition to the manifest) |
| `-o, --ontology <FILE>` | Additional ontology files (`.ttl`, `.owx`) |
| `--no-manifest` | Ignore `smol.toml`; only use `--files` / `--ontology` |
| `--prefix NAME=IRI` | Extra SPARQL prefix (repeatable; takes precedence over `[prefixes]`) |
| `--format text\|json` | Output format (default `text`) |

### Text output (default)

```bash
# Check a project described by a smol.toml manifest
lockpick check --manifest examples/odrl/smol.toml

# Check explicit files with an ontology
lockpick check --files examples/error-file.smol
lockpick check src/*.smol --ontology my-ontology.ttl

# Add SPARQL prefix aliases
lockpick check --manifest smol.toml --prefix ast=http://example.org/ast/
```

In `text` mode the verifier prints colour-coded ariadne reports and a summary
line to **stderr**:

```
[E2004] Error: type mismatch in assignment: `Int` vs `String`
    ╭─[MyClass.smol:12:5]
    │
 12 │     x = 42;
    │     ──┬──
    │       ╰── type mismatch in assignment: `Int` vs `String`
────╯
```

**Exit codes:** `0` for a clean project (or one with only warnings), `1` when at
least one error is found. The summary line is printed to stderr in both modes.

### JSON output

```bash
lockpick check --manifest examples/odrl/smol.toml --format json
```

In `json` mode a single JSON document is printed to **stdout** (the summary
line still goes to stderr and the exit codes are unchanged). The output is
machine-readable, so it can be piped to `jq` or consumed by CI:

```bash
lockpick check --files examples/error-file.smol --format json | jq '.error_count'
```

## JSON output

The `--format json` document has this shape:

```json
{
  "diagnostics": [
    {
      "severity": "error",
      "code": "E2004",
      "code_number": 2004,
      "message": "type mismatch in assignment",
      "file": "src/Foo.smol",
      "span": { "start": 0, "end": 5 },
      "labels": [
        { "message": "...", "file": "src/Foo.smol", "span": { "start": 0, "end": 1 } }
      ]
    }
  ],
  "error_count": 1,
  "warning_count": 0,
  "note_count": 0
}
```

- `severity` is `"error"`, `"warning"` or `"note"` (lowercase).
- `code` is the display form (`"E"` + zero-padded 4-digit number); `code_number`
  is the numeric value of the `DiagnosticCode` variant (e.g. `2004`).
- `file` and `span` are `null` when absent; `labels` (secondary labels) may be
  empty.
- `error_count` / `warning_count` / `note_count` are summary counters.

## Using lockpick as a library

The verifier is published as the **`smol-verifier`** crate and re-exported from
the root **`lockpick`** crate. All diagnostics are `serde`-serializable
(`Serialize`/`Deserialize`), so they round-trip to JSON losslessly.

Add it to your `Cargo.toml`:

```toml
[dependencies]
lockpick = { path = "../lockpick" }
# or
smol-verifier = { path = "../lockpick/crates/smol-verifier" }
```

Or import directly from GitHub. The root `lockpick` package re-exports the
`smol-verifier` API, so `use lockpick::verify_manifest;` works:

```toml
[dependencies]
lockpick = { git = "https://github.com/sievericcardo/lockpick" }
# pin to a branch or tag (workspace version is 1.0.0)
lockpick = { git = "https://github.com/sievericcardo/lockpick", branch = "main" }
lockpick = { git = "https://github.com/sievericcardo/lockpick", tag = "v1.0.0" }
```

You can also depend on the underlying library crate directly:

```toml
[dependencies]
smol-verifier = { git = "https://github.com/sievericcardo/lockpick", package = "smol-verifier" }
```

Note that importing from git still compiles the full dependency tree —
including RocksDB and clang-sys — so it requires the same system dependencies
(C++17 compiler, CMake, libclang) as building from source. See
[System Dependencies](#system-dependencies).

### Verify a project from a manifest

```rust
use lockpick::verify_manifest;

let diags = verify_manifest(std::path::Path::new("examples/odrl/smol.toml"));
let errors = diags.iter().filter(|d| d.is_error()).count();
println!("{errors} errors found");
```

### Verify explicit files

```rust
use lockpick::verify_files;

let smol_files = vec!["src/Foo.smol".into(), "src/Bar.smol".into()];
let diags = verify_files(&smol_files, &[]);
```

### Verify in-memory sources (no disk reads)

`verify_sources` takes `(PathBuf, String)` pairs, so you can check text that
lives in an editor buffer, a database, or anywhere else:

```rust
use lockpick::verify_sources;

let sources: Vec<(std::path::PathBuf, String)> = vec![(
    "src/Foo.smol".into(),
    "class Foo () end\n".to_string(),
)];
let diags = verify_sources(&sources, &[]);

for d in &diags {
    println!("{}: {}", d.code, d.message);
}
```

The in-memory API mirrors the file-based one:

| File-based | In-memory |
|------------|-----------|
| `verify_files(&[PathBuf], &[PathBuf])` | `verify_sources(&[(PathBuf, String)], &[PathBuf])` |
| `verify_files_with_prefixes(...)` | `verify_sources_with_prefixes(...)` |
| `verify_files_with_properties(...)` | `verify_sources_with_properties(...)` |

`verify_manifest` continues to work as before.

## Editor integration via LSP

`lockpick-lsp` is a minimal Language Server implementing a subset of **LSP 3.17**
over **stdio** using JSON-RPC with `Content-Length` framing. It has no
`lsp-server` / `lsp-types` / `tower-lsp` dependencies — just `serde_json` and
the standard library.

Launch it as a stdio subprocess from any LSP client:

```bash
target/release/lockpick-lsp
```

Supported features:

- `initialize` / `initialized` handshake (`textDocumentSync: 1`, full-sync)
- `textDocument/didOpen`, `textDocument/didChange` (full sync) and
  `textDocument/didSave` — each triggers a re-verification and pushes
  `textDocument/publishDiagnostics` notifications
- `shutdown` / `exit` (standard LSP lifecycle)
- SMOL diagnostics are mapped to LSP diagnostics: `Error` → severity 1,
  `Warning` → 2, `Note` → 3, with the `E####` code and a source range computed
  from the SMOL byte-offset span
- When a `smol.toml` manifest exists in the workspace root it is used;
  otherwise the server verifies the open documents plus every `.smol` file
  under the workspace root via the in-memory `verify_sources` API
- Resilient to malformed messages (skipped without panicking); unknown methods
  get a standard JSON-RPC `method not found` error (`-32601`)

### Example client configuration

A generic handshake is just three messages:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":"file:///path/to/project"}}
{"jsonrpc":"2.0","method":"initialized","params":{}}
{"jsonrpc":"2.0","method":"textDocument/didOpen",
 "params":{"textDocument":{"uri":"file:///path/to/project/src/Foo.smol","languageId":"smol","version":1,"text":"class Foo () end"}}}
```

Pointing an editor at the binary:

- **Neovim (built-in LSP):**
  ```lua
  vim.api.nvim_create_autocmd("FileType", { pattern = "smol", callback = function()
    vim.lsp.start({
      name = "lockpick",
      cmd = { "/path/to/target/release/lockpick-lsp" },
    })
  end})
  ```
- **VSCode** (`extension.ts` / `clientOptions`):
  ```ts
  { command: "/path/to/target/release/lockpick-lsp" }
  ```

## Quick start

```bash
# Check a single SMOL program with an ontology
./lockpick check --files src/*.smol --ontology my-ontology.ttl

# Check a project using smol.toml manifest
./lockpick check --manifest examples/bedreflyt/smol.toml

# Machine-readable output for CI
./lockpick check --manifest examples/odrl/smol.toml --format json

# Add SPARQL prefix aliases
./lockpick check --manifest smol.toml --prefix ast=http://example.org/ast/
```

## Architecture

```
smol-syntax     ← AST, diagnostic codes, token definitions
smol-parser     ← PEG lexer/parser (chumsky + logos)
smol-checker    ← Type system, null-safety, dead code, taint analysis
smol-ontology   ← OWL reasoning, SPARQL validation, SHACL checking
smol-model      ← Digital-twin extraction, CTL model checking
smol-verifier   ← Public API, orchestrates all checks in parallel
```

## Diagnostic codes

| Range | Owner | Examples |
|-------|-------|----------|
| E1xxx | Parser | UnexpectedToken, UnclosedDelimiter |
| E2xxx | Type checker / static analysis | TypeMismatch, NullDereference, UnusedVariable, SparqlInjection, DomainMutation, IntegerOverflow, DivisionByZero |
| E3xxx | Ontology checker | OntologyInconsistent, UnknownOntologyClass, SparqlSyntaxError |
| E4xxx | Configuration | ManifestNotFound, FileReadFailed |
| E5xxx | Model checker | ContextFieldInvalid, StateNotReachable, InvariantViolation, LoopTerminationUnproven |
| W2xxx, W3xxx | Warnings | DeadMethod, FmoNotDestroyed, SparqlInjection |

## Example programs

Real-world example projects in [examples/](examples/):

- **bedreflyt** — Hospital ward management with beds, rooms, treatments
- **greenhouse-adapt** — Plant monitoring with adaptation cycles and sensor fusion
- **Geological** — Subsurface layer modeling with state transitions
- **odrl** — Privacy-aware data access using ODRL policy checking

## License

BSD 3-Clause License or later (see [LICENSE](LICENSE))