//! Unit tests for `smol-checker`, focused on the `List` vs `ExplList` rules.

use smol_syntax::ast::File;
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

fn check_src(src: &str) -> Vec<Diagnostic> {
    let (file, parse_diags) = smol_parser::parse("<test>", src);
    let mut diags = parse_diags;
    diags.extend(smol_checker::check(&[file]));
    diags
}

fn files_from_sources(sources: &[&str]) -> Vec<File> {
    sources
        .iter()
        .enumerate()
        .map(|(i, src)| {
            let (file, _) = smol_parser::parse(&format!("<test{i}>"), src);
            file
        })
        .collect()
}

fn has_code(diags: &[Diagnostic], code: DiagnosticCode) -> bool {
    diags.iter().any(|d| d.code == code)
}

// ExplList is a distinct, usable type
#[test]
fn expl_list_field_resolves_content_and_next() {
    let src = r#"
class Action (String type) end

class Model (ExplList<Action> actions)
    Unit walk()
        ExplList<Action> cursor = this.actions;
        Action a = cursor.content;
        ExplList<Action> rest = cursor.next;
        print(a.type);
        print(rest);
    end
end
"#;
    let diags = check_src(src);
    let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "expected no errors, got: {errors:#?}");
}

#[test]
fn expl_list_has_length_method() {
    let src = r#"
class Action (String type) end

class Model (ExplList<Action> actions)
    Int count()
        return this.actions.length();
    end
end
"#;
    let diags = check_src(src);
    let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "expected no errors, got: {errors:#?}");
}

#[test]
fn new_expl_list_types_as_expl_list() {
    let src = r#"
class Action (String type) end

class Model ()
    Unit build()
        ExplList<Action> actions = new ExplList<Action>(null, null);
        print(actions);
    end
end
"#;
    let diags = check_src(src);
    let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "expected no errors, got: {errors:#?}");
}

// List field is flagged; ExplList field is not
#[test]
fn list_field_is_flagged() {
    let src = r#"
class Policy (String id, List<String> permissions)
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::ListFieldNotExplicit),
        "expected ListFieldNotExplicit, got: {diags:#?}"
    );
}

#[test]
fn expl_list_field_is_not_flagged() {
    let src = r#"
class Policy (String id, ExplList<String> permissions)
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::ListFieldNotExplicit),
        "ExplList field must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn init_list_field_is_flagged() {
    let src = r#"
class Model (List<String> items)
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::ListFieldNotExplicit),
        "expected ListFieldNotExplicit for init field, got: {diags:#?}"
    );
}

// List vs ExplList are invariant / incompatible
#[test]
fn list_is_not_assignable_to_expl_list() {
    let src = r#"
class Action (String type) end

class Model (ExplList<Action> actions)
    Unit assign()
        List<Action> xs = access("SELECT ?obj { ?obj a <foo:Action> }");
        this.actions = xs;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::TypeMismatch),
        "expected TypeMismatch assigning List to ExplList, got: {diags:#?}"
    );
}

// access()/construct() keep returning List
#[test]
fn access_returns_list_and_traverses() {
    let src = r#"
class Action (String type) end

class Model ()
    Unit walk()
        List<Action> acts = access("SELECT ?obj { ?obj a <foo:Action> }");
        while acts != null do
            Action a = acts.content;
            print(a.type);
            acts = acts.next;
        end
    end
end
"#;
    let diags = check_src(src);
    let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "expected no errors, got: {errors:#?}");
}

// error-file.smol pattern: List fields + local traversal
#[test]
fn error_file_pattern_flags_all_list_fields() {
    // Two files so the cross-class references resolve.
    let sources = [
        r#"
abstract class Party() end
class DataSubject (String id) end
class DataController (String id) end
"#,
        r#"
class Policy (String id, DataSubject ds, List<PolicyPermission> permissions)
    Unit setPermissions()
        List<PolicyPermission> perms = access("SELECT ?obj { ?obj a <foo:PP> }", this.id);
        if perms != null then
            this.permissions = perms;
            while perms != null do
                PolicyPermission newPermission = perms.content;
                perms = perms.next;
            end
        end
    end
end

class PolicyPermission (Policy policy, String name) end
"#,
    ];
    let files = files_from_sources(&sources);
    let diags = smol_checker::check(&files);
    assert!(
        has_code(&diags, DiagnosticCode::ListFieldNotExplicit),
        "expected ListFieldNotExplicit, got: {diags:#?}"
    );
}
