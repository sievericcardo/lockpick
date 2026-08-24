//! `smol.toml` manifest loader.
//!
//! Example manifest:
//! ```toml
//! [project]
//! name = "greenhouse-adapt"
//!
//! [sources]
//! files = ["SMOL/*.smol"]
//!
//! [ontologies]
//! files = ["greenhouse-2.0.ttl"]
//!
//! [entry]
//! main = "SMOL/GreenHouse.smol"
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

// ─── Manifest schema ─────────────────────────────────────────────────────── //

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub project:    ProjectSection,
    pub sources:    SourcesSection,
    pub ontologies: Option<OntologiesSection>,
    pub entry:      Option<EntrySection>,
    /// Optional SHACL shapes files.
    pub shacl:      Option<ShaclSection>,
    /// Optional user-defined SPARQL prefixes.
    pub prefixes:   Option<PrefixesSection>,
    /// Optional CTL properties to model-check.
    pub properties: Option<PropertiesSection>,
}

#[derive(Debug, Deserialize)]
pub struct ProjectSection {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SourcesSection {
    pub files: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct OntologiesSection {
    pub files: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct EntrySection {
    pub main: String,
}

#[derive(Debug, Deserialize)]
pub struct ShaclSection {
    pub files: Vec<String>,
}

/// `[prefixes]` section: arbitrary `name = "IRI"` key-value pairs.
///
/// Example:
/// ```toml
/// [prefixes]
/// domain = "http://example.org/domain/"
/// sensor = "http://example.org/sensor/"
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct PrefixesSection {
    #[serde(flatten)]
    pub map: std::collections::HashMap<String, String>,
}

/// `[properties]` section: CTL formulas to verify against the extracted model.
///
/// Example:
/// ```toml
/// [properties]
/// check = ["AG (Healthy || Unhealthy)", "EF Maintenance"]
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct PropertiesSection {
    pub check: Vec<String>,
}

// ─── Resolved manifest ───────────────────────────────────────────────────── //

/// The fully-resolved file lists for a project.
pub struct ResolvedManifest {
    pub project_name:    String,
    pub smol_files:      Vec<PathBuf>,
    pub ontology_files:  Vec<PathBuf>,
    pub shacl_files:     Vec<PathBuf>,
    pub entry_file:      Option<PathBuf>,
    /// Prefixes declared in `[prefixes]` section of `smol.toml`.
    pub prefixes:        std::collections::HashMap<String, String>,
    /// CTL formulas declared in `[properties]` section of `smol.toml`.
    pub properties:      Vec<String>,
}

/// Load and resolve a `smol.toml` manifest file.
///
/// `manifest_path` must point to the `smol.toml` file itself.  All glob
/// patterns are resolved relative to the directory containing that file.
///
/// # Errors
///
/// Returns an error if the manifest file cannot be read, the TOML cannot be
/// parsed, or glob patterns fail to expand.
pub fn load(manifest_path: &Path) -> Result<ResolvedManifest, Vec<Diagnostic>> {
    let dir = manifest_path.parent().unwrap_or(Path::new("."));

    let content = std::fs::read_to_string(manifest_path).map_err(|e| {
        vec![Diagnostic::error(
            DiagnosticCode::ManifestNotFound,
            format!("cannot read manifest `{}`: {e}", manifest_path.display()),
        )]
    })?;

    let manifest: Manifest = toml::from_str(&content).map_err(|e| {
        vec![Diagnostic::error(
            DiagnosticCode::ManifestParseFailed,
            format!("manifest parse error: {e}"),
        )]
    })?;

    let mut errors = Vec::new();

    let smol_files = expand_globs(&manifest.sources.files, dir, &mut errors);
    let ontology_files = manifest
        .ontologies
        .as_ref()
        .map(|o| expand_globs(&o.files, dir, &mut errors))
        .unwrap_or_default();
    let shacl_files = manifest
        .shacl
        .as_ref()
        .map(|s| expand_globs(&s.files, dir, &mut errors))
        .unwrap_or_default();

    let entry_file = manifest
        .entry
        .as_ref()
        .map(|e| dir.join(&e.main));

    if !errors.is_empty() {
        return Err(errors);
    }

    let prefixes = manifest
        .prefixes
        .map(|p| p.map)
        .unwrap_or_default();

    let properties = manifest
        .properties
        .map(|p| p.check)
        .unwrap_or_default();

    Ok(ResolvedManifest {
        project_name:   manifest.project.name,
        smol_files,
        ontology_files,
        shacl_files,
        entry_file,
        prefixes,
        properties,
    })
}

// ─── Glob expansion ───────────────────────────────────────────────────────── //

fn expand_globs(
    patterns: &[String],
    base: &Path,
    errors: &mut Vec<Diagnostic>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for pat in patterns {
        match globwalk::GlobWalkerBuilder::from_patterns(base, &[pat.as_str()])
            .follow_links(false)
            .build()
        {
            Ok(walker) => {
                for entry in walker {
                    match entry {
                        Ok(e)  => paths.push(e.path().to_path_buf()),
                        Err(e) => errors.push(Diagnostic::error(
                            DiagnosticCode::GlobExpansionFailed,
                            format!("glob `{pat}` entry error: {e}"),
                        )),
                    }
                }
            }
            Err(e) => {
                errors.push(Diagnostic::error(
                    DiagnosticCode::GlobExpansionFailed,
                    format!("invalid glob pattern `{pat}`: {e}"),
                ));
            }
        }
    }
    paths.sort();
    paths
}
