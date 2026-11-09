//! Declaration pass: build the `ClassEnv` from all parsed files.
//!
//! Errors detected:
//! - Duplicate class name
//! - Unknown superclass
//! - Cyclic inheritance
//! - Abstract method with body / non-abstract without body
//! - `override` without a parent method

use std::collections::HashSet;

use smol_syntax::ast::{ClassDecl, File, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

use crate::class_env::{ClassEnv, ClassInfo};

/// Build the class environment from all parsed files.
///
/// # Panics
///
/// Panics if a class was inserted but its entry is missing from `ClassEnv`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn build_class_env(files: &[File]) -> (ClassEnv, Vec<Diagnostic>) {
    let mut env = ClassEnv::default();
    let mut diags = Vec::new();

    // Insert all class declarations
    for file in files {
        for cls in &file.classes {
            if env.contains(&cls.name) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::DuplicateClass,
                        format!("duplicate class `{}`", cls.name),
                    )
                    .with_file(&file.path)
                    .with_span(cls.span),
                );
                continue;
            }
            env.insert(ClassInfo {
                name: cls.name.clone(),
                extends: cls.extends.clone(),
                is_abstract: cls.is_abstract,
                fields: cls.fields.clone(),
                methods: cls.methods.clone(),
                classifies: cls.classifies.as_ref().map(|c| c.iri.clone()),
                span: cls.span,
                file: file.path.clone(),
            });
        }
    }

    // Validate superclass references
    for file in files {
        for cls in &file.classes {
            if let Some(parent) = &cls.extends
                && !env.contains(parent)
            {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::UnknownClass,
                        format!("class `{}` extends unknown class `{}`", cls.name, parent),
                    )
                    .with_file(&file.path)
                    .with_span(cls.span),
                );
            }
        }
    }

    // Detect inheritance cycles
    let names: Vec<String> = env.iter().map(|c| c.name.clone()).collect();
    for name in &names {
        if has_cycle(&env, name) {
            let info = env.get(name).unwrap();
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::CyclicInheritance,
                    format!("cyclic inheritance involving class `{name}`"),
                )
                .with_file(&info.file)
                .with_span(info.span),
            );
        }
    }

    // Method-level checks
    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                // abstract method must have empty body
                if method.flags.is_abstract && !method.body.is_empty() {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::MissingAbstractBody,
                            format!(
                                "abstract method `{}::{}` must not have a body",
                                cls.name, method.name
                            ),
                        )
                        .with_file(&file.path)
                        .with_span(method.span),
                    );
                }

                // non-abstract, non-Unit method in a concrete class — we'll check returns in body pass
                // override must have a parent method
                if method.flags.is_override {
                    let parent_has = cls
                        .extends
                        .as_deref()
                        .and_then(|p| env.resolve_method(p, &method.name))
                        .is_some();
                    if !parent_has {
                        diags.push(
                            Diagnostic::error(
                                DiagnosticCode::OverrideWithoutParent,
                                format!(
                                    "`{}::{}` is marked `override` but no parent method found",
                                    cls.name, method.name
                                ),
                            )
                            .with_file(&file.path)
                            .with_span(method.span),
                        );
                    }
                }
            }

            // Concrete class must implement all inherited abstract methods
            if !cls.is_abstract {
                check_abstract_unimplemented(cls, &env, &mut diags, &file.path);
            }
        }
    }

    // List fields must be ExplList
    // `List<T>` is hidden by default and not exposed in the RDF model.  A model
    // relation that must be persisted and traversed must be declared `ExplList<T>`.
    for file in files {
        for cls in &file.classes {
            for field in &cls.fields {
                if matches!(field.ty, Type::List(_)) {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::ListFieldNotExplicit,
                            format!(
                                "`List` field `{}` is hidden by default and is not \
                                 exposed in the RDF model; use `ExplList<...>` instead",
                                field.name
                            ),
                        )
                        .with_file(&file.path)
                        .with_span(field.span),
                    );
                }
            }
            for field in &cls.init_fields {
                if matches!(field.ty, Type::List(_)) {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::ListFieldNotExplicit,
                            format!(
                                "`List` field `{}` is hidden by default and is not \
                                 exposed in the RDF model; use `ExplList<...>` instead",
                                field.name
                            ),
                        )
                        .with_file(&file.path)
                        .with_span(field.span),
                    );
                }
            }
        }
    }

    (env, diags)
}

fn has_cycle(env: &ClassEnv, start: &str) -> bool {
    let mut seen = HashSet::new();
    let mut current = start;
    loop {
        if seen.contains(current) {
            return true;
        }
        seen.insert(current);
        match env.get(current).and_then(|i| i.extends.as_deref()) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// Collect the set of abstract method names that are not overridden in `cls` or
/// its concrete ancestors.
fn check_abstract_unimplemented(
    cls: &ClassDecl,
    env: &ClassEnv,
    diags: &mut Vec<Diagnostic>,
    file: &str,
) {
    // Gather methods that are declared abstract in any ancestor.
    let ancestors = env.ancestors(&cls.name);
    let mut required: HashSet<String> = HashSet::new();
    for ancestor_name in &ancestors {
        if let Some(info) = env.get(ancestor_name) {
            for m in &info.methods {
                if m.flags.is_abstract {
                    required.insert(m.name.clone());
                }
            }
        }
    }
    // Remove methods that have a concrete implementation anywhere in the chain.
    for ancestor_name in &ancestors {
        if let Some(info) = env.get(ancestor_name) {
            for m in &info.methods {
                if !m.flags.is_abstract {
                    required.remove(&m.name);
                }
            }
        }
    }
    for missing in required {
        diags.push(
            Diagnostic::error(
                DiagnosticCode::MissingAbstractBody,
                format!(
                    "concrete class `{}` does not implement abstract method `{missing}`",
                    cls.name
                ),
            )
            .with_file(file)
            .with_span(cls.span),
        );
    }
}
