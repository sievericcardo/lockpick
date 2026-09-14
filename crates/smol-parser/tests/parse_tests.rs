//! Round-trip parse tests for smol-parser.

use smol_parser::parse;

fn assert_no_errors(src: &str) {
    let (_, diags) = parse("<test>", src);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == smol_syntax::diagnostic::Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "unexpected parse errors:\n{errors:#?}\nsource:\n{src}"
    );
}

#[test]
fn parse_empty_class() {
    assert_no_errors("class Foo () end");
}

#[test]
fn parse_abstract_class() {
    assert_no_errors(
        r#"
abstract class Base ()
    abstract Unit doIt()
end
"#,
    );
}

#[test]
fn parse_class_with_fields() {
    assert_no_errors(
        r#"
class Plant extends Twin (
    hidden String plantId,
    domain Double moisture
)
end
"#,
    );
}

#[test]
fn parse_class_with_classifies() {
    assert_no_errors(
        r#"
class Healthy extends HealthState (String name)
classifies "<ast:Healthy>";
    override Unit printInfo()
        print("hello");
    end
end
"#,
    );
}

#[test]
fn parse_main_block() {
    assert_no_errors(
        r#"
class Dog ()
end

main
    Dog d = new Dog();
    print("done");
end
"#,
    );
}

#[test]
fn parse_if_while() {
    assert_no_errors(
        r#"
class X ()
    Unit foo(Int n)
        if n == 0 then
            print("zero");
        else
            print("nonzero");
        end
        while n != 0 do
            n = n - 1;
        end
    end
end
"#,
    );
}

#[test]
fn parse_construct_access_classify() {
    assert_no_errors(
        r#"
class Model ()
    Unit rebuildModel()
        List<Object> items = construct("SELECT ?obj { ?obj a <foo:Bar> }");
        List<Object> more  = access("SELECT ?obj { ?obj a <foo:Baz> }");
        Object x = more.content;
        Object y = classify(x);
    end
end
"#,
    );
}

#[test]
fn parse_expl_list_field_and_new() {
    assert_no_errors(
        r#"
class Model (ExplList<Object> items)
    Unit rebuild()
        this.items = new ExplList<Object>(null, null);
        ExplList<Object> cursor = this.items;
        cursor.content = null;
        cursor = cursor.next;
    end
end
"#,
    );
}

#[test]
fn parse_requires_ensures_clauses() {
    // Contracts travel as JML comments (the `smol.pest` grammar is fixed).
    let (file, diags) = parse(
        "<test>",
        r#"
class X ()
    /*@ requires n != null @*/
    /*@ ensures n == n @*/
    Unit foo(Int n)
        skip;
    end
end
"#,
    );
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error)
    );
    let m = &file.classes[0].methods[0];
    assert_eq!(m.requires.len(), 1);
    assert_eq!(m.ensures.len(), 1);
}

#[test]
fn parse_requires_ensures_line_comments() {
    // `//@` line comments work as well, including inside the method body.
    let (file, diags) = parse(
        "<test>",
        r#"
class X ()
    Unit foo(Int n)
        //@ requires n != null
        //@ ensures n == n
        skip;
    end
end
"#,
    );
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error)
    );
    let m = &file.classes[0].methods[0];
    assert_eq!(m.requires.len(), 1);
    assert_eq!(m.ensures.len(), 1);
}

#[test]
fn parse_nullable_type_suffix() {
    // Nullable types use the `Nullable<T>` sugar (fixed grammar).
    let (file, diags) = parse(
        "<test>",
        r#"
class X (Nullable<String> name)
    Nullable<String> greet(Nullable<String> who)
        return who;
    end
end
"#,
    );
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error)
    );
    assert!(matches!(
        file.classes[0].fields[0].ty,
        smol_syntax::ast::Type::Nullable(_)
    ));
    assert_eq!(
        format!("{}", file.classes[0].fields[0].ty),
        "Nullable<String>"
    );
    let m = &file.classes[0].methods[0];
    assert!(matches!(m.ret_type, smol_syntax::ast::Type::Nullable(_)));
    assert!(matches!(
        m.params[0].ty,
        smol_syntax::ast::Type::Nullable(_)
    ));
}

#[test]
fn parse_while_decreases_clause() {
    // The variant travels as a JML comment inside the loop body.
    let (file, diags) = parse(
        "<test>",
        r#"
class X ()
    Unit foo(Int n)
        while n != 0 do
            /*@ decreases n @*/
            n = n - 1;
        end
    end
end
"#,
    );
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error)
    );
    let body = &file.classes[0].methods[0].body;
    match &body[0] {
        smol_syntax::ast::Stmt::While { decreases, .. } => assert!(decreases.is_some()),
        other => panic!("expected while stmt, got {other:?}"),
    }
}

#[test]
fn parse_while_without_decreases_still_passes() {
    let (file, diags) = parse(
        "<test>",
        r#"
class X ()
    Unit foo(Int n)
        while n != 0 do
            n = n - 1;
        end
    end
end
"#,
    );
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error)
    );
    let body = &file.classes[0].methods[0].body;
    match &body[0] {
        smol_syntax::ast::Stmt::While { decreases, .. } => assert!(decreases.is_none()),
        other => panic!("expected while stmt, got {other:?}"),
    }
}

#[test]
fn parse_effect_prefixes() {
    // Effects travel as JML comments (fixed grammar — no method prefixes).
    for (annotation, check) in [
        ("pure", smol_syntax::ast::Effect::Pure),
        ("domain-reads", smol_syntax::ast::Effect::DomainReads),
        ("domain-writes", smol_syntax::ast::Effect::DomainWrites),
    ] {
        let src = format!(
            "class X ()\n    /*@ {annotation} @*/\n    Unit foo()\n        skip;\n    end\nend\n"
        );
        let (file, diags) = parse("<test>", &src);
        assert!(
            diags
                .iter()
                .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error),
            "annotation {annotation}: {diags:#?}"
        );
        let m = &file.classes[0].methods[0];
        assert_eq!(m.flags.effect, Some(check), "annotation {annotation}");
    }
    // `pure` convenience accessor.
    let (file, _) = parse(
        "<test>",
        "class X ()\n    /*@ pure @*/\n    Unit foo()\n        skip;\n    end\nend\n",
    );
    assert!(file.classes[0].methods[0].flags.is_pure());
    // Legacy method without annotation keeps `effect: None`.
    let (file, _) = parse(
        "<test>",
        "class X ()\n    Unit foo()\n        skip;\n    end\nend\n",
    );
    assert_eq!(file.classes[0].methods[0].flags.effect, None);
    assert!(file.classes[0].methods[0].requires.is_empty());
    assert!(file.classes[0].methods[0].ensures.is_empty());
}

#[test]
fn parse_greenhouse_smol_files() {
    use std::path::Path;

    let smol_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .unwrap()
        .parent() // workspace root
        .unwrap()
        .join("examples/greenhouse-adapt/SMOL");

    if !smol_dir.exists() {
        return; // skip in environments without examples
    }

    for entry in std::fs::read_dir(&smol_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("smol") {
            let src = std::fs::read_to_string(&path).unwrap();
            let (_, diags) = parse(&path.to_string_lossy(), &src);
            let errors: Vec<_> = diags
                .iter()
                .filter(|d| d.severity == smol_syntax::diagnostic::Severity::Error)
                .collect();
            assert!(
                errors.is_empty(),
                "parse errors in {}:\n{errors:#?}",
                path.display()
            );
        }
    }
}
