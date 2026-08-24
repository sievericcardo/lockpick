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
fn parse_greenhouse_smol_files() {
    use std::path::Path;

    let smol_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()  // crates/
        .unwrap()
        .parent()  // workspace root
        .unwrap()
        .join("examples/greenhouse-adapt/SMOL");

    if !smol_dir.exists() {
        return; // skip in environments without examples
    }

    for entry in std::fs::read_dir(&smol_dir).unwrap() {
        let entry = entry.unwrap();
        let path  = entry.path();
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
