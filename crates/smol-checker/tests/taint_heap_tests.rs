//! Heap-taint tests (plan §2.8, heap flows): taint stored through fields and
//! `List`/`ExplList` slots must reach later `access`/`construct` query args.

use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

fn check_src(src: &str) -> Vec<Diagnostic> {
    let (file, parse_diags) = smol_parser::parse("<test>", src);
    let mut diags = parse_diags;
    diags.extend(smol_checker::check(&[file]));
    diags
}

fn has_code(diags: &[Diagnostic], code: DiagnosticCode) -> bool {
    diags.iter().any(|d| d.code == code)
}

#[test]
fn expl_list_store_and_read_is_flagged() {
    let src = r#"
class Repo ()
    Unit fetch()
        List<String> results = access("SELECT ?x { ?x a <urn:T> }");
        String first = results.content;
        ExplList<String> store = new ExplList<String>(first, null);
        String back = store.content;
        List<String> more = access("SELECT ?x { ?x <urn:p> \"%1\" }", back);
        print(more);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::SparqlInjection),
        "expected SparqlInjection for ExplList store+read flow, got: {diags:#?}"
    );
}

#[test]
fn field_store_and_read_is_flagged() {
    let src = r#"
class Box (String payload)
end

class Repo ()
    Unit fetch()
        List<String> results = access("SELECT ?x { ?x a <urn:T> }");
        String first = results.content;
        Box b = new Box("clean");
        b.payload = first;
        String back = b.payload;
        List<String> more = access("SELECT ?x { ?x <urn:p> \"%1\" }", back);
        print(more);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::SparqlInjection),
        "expected SparqlInjection for field store+read flow, got: {diags:#?}"
    );
}

#[test]
fn clean_list_is_silent() {
    let src = r#"
class Repo ()
    Unit fetch()
        ExplList<String> store = new ExplList<String>("clean", null);
        String back = store.content;
        List<String> more = access("SELECT ?x { ?x <urn:p> \"%1\" }", back);
        print(more);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlInjection),
        "clean list must not be flagged, got: {diags:#?}"
    );
}
