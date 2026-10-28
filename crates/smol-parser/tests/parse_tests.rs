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

// ─────────────────────────────────────────────────────────────────────────────
// Lowercase type names (Kotlin While.g4 NAME parity) + declaration-prefix
// ambiguity guards
// ─────────────────────────────────────────────────────────────────────────────
//
// Kotlin accepts ANY identifier in a type position (`NAME : LET LOD*`,
// While.g4:112; `type : NAME …`, While.g4:216-219).  The Rust port originally
// restricted `type_name` to uppercase-initial, so ontology-generated classes
// like `newOwlClass_…` failed at their first use as a type.  Allowing
// lowercase types re-introduces the declaration-prefix ambiguity guarded in
// the grammar (`(type_expr ~ !("=" | "."))?`), locked by the tests below.

/// `c.x = 5;` must be an assignment to field `x`.  Without the `.` guard
/// the optional prefix would commit to `type_expr = c`, `expression`
/// cannot start at `.`, and the statement fails to parse outright (PEG
/// commit-then-fail) — while `Int q = c.x;` still declares `q: Int`.
#[test]
fn field_assignment_is_not_misparsed_as_declaration() {
    let source = r#"class C(Int x)
  Int m()
    return 1;
  end
end

main
  C c = new C(0);
  C d = new C(0);
  c.x = 5;
  c.x = d.m();
  Int q = c.x;
end"#;
    let (file, diags) = smol_parser::parse("<test>", source);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error),
        "unexpected parse errors: {diags:#?}"
    );
    use smol_syntax::ast::{Expr, Stmt, Type};

    let stmts = file.main.expect("main block");
    // `c.x = 5;` — plain assignment to a field, no declared type.
    match &stmts[2] {
        Stmt::Assign { lhs, rhs, .. } => {
            match lhs {
                Expr::Field {
                    receiver, field, ..
                } => {
                    assert!(matches!(receiver.as_ref(), Expr::Var(n, _) if n == "c"));
                    assert_eq!(field, "x");
                }
                other => panic!("expected field lhs `c.x`, got {other:?}"),
            }
            assert!(matches!(rhs, Expr::IntLit(5, _)));
        }
        other => panic!("expected an assignment for `c.x = 5;`, got {other:?}"),
    }
    // `c.x = d.m();` — non-declaring call assignment through a dot target.
    match &stmts[3] {
        Stmt::Assign { lhs, rhs, .. } => {
            assert!(matches!(lhs, Expr::Field { field, .. } if field == "x"));
            match rhs {
                Expr::MethodCall {
                    receiver, method, ..
                } => {
                    assert!(matches!(receiver.as_ref(), Expr::Var(n, _) if n == "d"));
                    assert_eq!(method, "m");
                }
                other => panic!("expected method call rhs, got {other:?}"),
            }
        }
        other => panic!("expected an assignment for `c.x = d.m();`, got {other:?}"),
    }
    // `Int q = c.x;` — still a declaration of `Int`.
    match &stmts[4] {
        Stmt::VarDecl { ty, name, .. } => {
            assert_eq!(*ty, Type::Int);
            assert_eq!(name, "q");
        }
        other => panic!("expected a var decl for `Int q = c.x;`, got {other:?}"),
    }
    // Negative guard: no statement anywhere may declare a variable of type
    // `c` — a `VarDecl` of type `c` would mean `type_expr` consumed the
    // receiver as a declared type instead of the prefix failing the parse
    // (the mechanism described in the doc comment above).
    for s in &stmts {
        if let Stmt::VarDecl { ty, .. } = s {
            assert_ne!(
                *ty,
                Type::Named("c".to_owned()),
                "assign misparsed as declaration of type `c`: {s:?}"
            );
        }
    }
}

/// Lowercase classes must work in every type position: declared type,
/// `extends`, field type, parameter type, return type, and `new` target.
#[test]
fn lowercase_class_in_every_type_position() {
    let source = r#"class box(Int v)
end

class child extends box (box inner, box other)
  box ident(box p)
    return p;
  end
end

main
  box b = new box(1);
  child c2 = new child(b, b);
  box b2 = c2.ident(b);
end"#;
    let (file, diags) = smol_parser::parse("<test>", source);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error),
        "unexpected parse errors: {diags:#?}"
    );
    use smol_syntax::ast::{Expr, Stmt, Type};

    let box_ty = Type::Named("box".to_owned());

    // `extends box`.
    assert_eq!(file.classes[1].extends.as_deref(), Some("box"));

    // Field types: `child(box inner, box other)`.
    assert_eq!(file.classes[1].fields[0].ty, box_ty);
    assert_eq!(file.classes[1].fields[1].ty, box_ty);

    // Method return and parameter types.
    let m = &file.classes[1].methods[0];
    assert_eq!(m.ret_type, box_ty);
    assert_eq!(m.params[0].ty, box_ty);

    let stmts = file.main.expect("main block");

    // `box b = new box(1);` — declared lowercase type + lowercase `new`.
    match &stmts[0] {
        Stmt::VarDecl { ty, name, init, .. } => {
            assert_eq!(*ty, box_ty);
            assert_eq!(name, "b");
            match init {
                Expr::New {
                    class,
                    ty_param: None,
                    args,
                    ..
                } => {
                    assert_eq!(class, "box");
                    assert!(matches!(args.as_slice(), [Expr::IntLit(1, _)]));
                }
                other => panic!("expected `new box(1)`, got {other:?}"),
            }
        }
        other => panic!("expected a var decl for `box b = new box(1);`, got {other:?}"),
    }

    // `box b2 = c2.ident(b);` — call declaring a lowercase-typed local.
    match &stmts[2] {
        Stmt::VarDecl { ty, name, init, .. } => {
            assert_eq!(*ty, box_ty);
            assert_eq!(name, "b2");
            assert!(
                matches!(init, Expr::MethodCall { method, .. } if method == "ident"),
                "expected ident() call, got {init:?}"
            );
        }
        other => panic!("expected a var decl for `box b2 = …ident(b);`, got {other:?}"),
    }
}

/// Lowercase class as a generic argument: `List<newOwlClass_1> l0 = …`.
#[test]
fn lowercase_class_as_generic_argument() {
    let source = r#"class newOwlClass_1(String id) end

main
  List<newOwlClass_1> l0 = construct("SELECT ?id WHERE { <inner:x> a <newOwlClass:1> . BIND(STR(<inner:x>) AS ?id) }");
end"#;
    let (file, diags) = smol_parser::parse("<test>", source);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error),
        "unexpected parse errors: {diags:#?}"
    );
    use smol_syntax::ast::{Expr, Stmt, Type};

    let stmts = file.main.expect("main block");
    match &stmts[0] {
        Stmt::VarDecl { ty, init, .. } => {
            assert_eq!(
                *ty,
                Type::List(Box::new(Type::Named("newOwlClass_1".to_owned()))),
                "declared type must be List<newOwlClass_1>"
            );
            assert!(
                matches!(init, Expr::Construct { .. }),
                "expected a construct() init, got {init:?}"
            );
        }
        other => panic!("expected a var decl for the construct, got {other:?}"),
    }
}

/// Minimal equivalent of the RDFMutate mutant cases (case_00017 shape):
/// lowercase class, `List<lowercase>` construct, `.length()`, an
/// iterate-`.get(i)` loop, field read and print — the exact shape that
/// used to produce a lint-vs-execute parse divergence in the aegis
/// mutation pipeline.
#[test]
fn parse_lowercase_class_mutant_shape() {
    let source = r#"class newOwlClass__1604871850(String id) end

main
    List<newOwlClass__1604871850> l1 = construct("SELECT ?id WHERE { <inner:asd272716077> a <newOwlClass:-1604871850> . BIND(STR(<inner:asd272716077>) AS ?id) }");
    Int n1 = l1.length();
    print("BEGIN newOwlClass__1604871850_inner_asd272716077");
    Int i1 = 0;
    while i1 < n1 do
        newOwlClass__1604871850 x1 = l1.get(i1);
        print(x1.id);
        print("ENDROW");
        i1 = i1 + 1;
    end
    print("END newOwlClass__1604871850_inner_asd272716077");
end"#;
    let (file, diags) = smol_parser::parse("<test>", source);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error),
        "unexpected parse errors: {diags:#?}"
    );
    use smol_syntax::ast::{Stmt, Type};

    assert_eq!(file.classes[0].name, "newOwlClass__1604871850");

    let stmts = file.main.expect("main block");
    let mutant_ty = Type::Named("newOwlClass__1604871850".to_owned());
    match &stmts[0] {
        Stmt::VarDecl { ty, .. } => assert_eq!(
            *ty,
            Type::List(Box::new(mutant_ty.clone())),
            "l1 must be List<newOwlClass__1604871850>"
        ),
        other => panic!("expected l1 decl, got {other:?}"),
    }
    match &stmts[1] {
        Stmt::VarDecl { ty, name, .. } => {
            assert_eq!(*ty, Type::Int);
            assert_eq!(name, "n1");
        }
        other => panic!("expected n1 = l1.length();, got {other:?}"),
    }
    match &stmts[4] {
        Stmt::While { body, .. } => match &body[0] {
            Stmt::VarDecl { ty, name, .. } => {
                assert_eq!(*ty, mutant_ty, "x1 must be declared as the lowercase class");
                assert_eq!(name, "x1");
            }
            other => panic!("expected x1 decl in loop, got {other:?}"),
        },
        other => panic!("expected a while loop, got {other:?}"),
    }
}

/// The declaration-prefix guard `!("=" | ".")` exists in EIGHT statement
/// rules plus `classify_assignment`.  `field_assignment_is_not_misparsed_as_declaration`
/// locks the `assign_stmt` limb; this one locks the other seven
/// simultaneously: each line below targets a dot-qualified field through a
/// different statement rule, and every statement must declare nothing with
/// the dot-qualified target intact.  Reverting any single `| "."` limb
/// breaks the corresponding line (hard parse error) and fails this test.
/// (`validate`/`simulate` are parse-only stubs in lockpick — they map to
/// `Stmt::Skip` — but their prefix guards are still exercised: without the
/// guard those two lines would fail the whole parse.)
#[test]
fn dot_target_statements_never_declare() {
    let source = r#"class C(Int x, Int y, Int u, Int v, Int w, Int z, Int s)
  Int m()
    return 1;
  end
end

main
  C c = new C(0, 0, 0, 0, 0, 0, 0);
  C d = new C(0, 0, 0, 0, 0, 0, 0);
  C t = new C(0, 0, 0, 0, 0, 0, 0);
  c.x = d.m();
  c.y = new C(1);
  t.u = access("SELECT ?s WHERE { ?s ?p ?o }");
  t.v = construct("SELECT ?s WHERE { ?s ?p ?o }");
  t.w = member("q");
  t.z = validate("q");
  t.s = simulate("f.fmu");
end"#;
    let (file, diags) = smol_parser::parse("<test>", source);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error),
        "unexpected parse errors: {diags:#?}"
    );
    use smol_syntax::ast::{Expr, Stmt, Type};

    let stmts = file.main.expect("main block");
    // Statements 0-2 are the three `new C(…)` declarations.
    let rest = &stmts[3..];

    /// Assert the statement assigns to `obj.field` and declares nothing.
    fn assert_dot_assign(s: &Stmt, obj: &str, field: &str, what: &str) {
        let Stmt::Assign { lhs, .. } = s else {
            panic!("{what}: expected a non-declaring assignment, got {s:?}");
        };
        match lhs {
            Expr::Field {
                receiver, field: f, ..
            } => {
                assert!(
                    matches!(receiver.as_ref(), Expr::Var(n, _) if n == obj),
                    "{what}: wrong receiver in {lhs:?}"
                );
                assert_eq!(f, field, "{what}: wrong target field");
            }
            other => panic!("{what}: target is not a dot-qualified field access: {other:?}"),
        }
    }

    assert_dot_assign(&rest[0], "c", "x", "call_stmt (`c.x = d.m();`)");
    assert_dot_assign(&rest[1], "c", "y", "create_stmt (`c.y = new C(1);`)");
    assert_dot_assign(&rest[2], "t", "u", "sparql_stmt (`t.u = access(…);`)");
    assert_dot_assign(&rest[3], "t", "v", "construct_stmt (`t.v = construct(…);`)");
    assert_dot_assign(&rest[4], "t", "w", "owl_stmt (`t.w = member(…);`)");
    // validate/simulate parse to Skip (stubs) — but they parsed, i.e. their
    // guarded prefixes took the non-declaring reading.
    assert!(
        matches!(rest[5], Stmt::Skip { .. }),
        "validate_stmt line should parse (stub Skip), got {:?}",
        rest[5]
    );
    assert!(
        matches!(rest[6], Stmt::Skip { .. }),
        "simulate_stmt line should parse (stub Skip), got {:?}",
        rest[6]
    );

    // Negative guard: no statement anywhere may declare a variable of a
    // lowercase receiver type — that would mean a `.`-guard limb is missing.
    for s in &stmts {
        if let Stmt::VarDecl { ty, .. } = s {
            for recv in ["c", "d", "t"] {
                assert_ne!(
                    *ty,
                    Type::Named(recv.to_owned()),
                    "dot-target statement misparsed as declaration of type `{recv}`: {s:?}"
                );
            }
        }
    }
}

/// lockpick-specific guard: `classify_assignment` is
/// `{ (type_expr ~ !("=" | "."))? ~ assignment_target ~ "=" }` and
/// `assignment_target` can start with a field access — with lowercase
/// types, `c.x = classify(q);` would commit `type_expr = c` then fail.
/// The `=` limb additionally preserves the plain `y = classify(1);`
/// non-declaring reading (a `.`-only guard would hard-error there).
#[test]
fn classify_assignment_dot_target_and_plain_target_parse() {
    let source = r#"class C(Int x) end

main
  C c = new C(0);
  c.x = classify(5);
  Int y = 0;
  y = classify(1);
end"#;
    let (file, diags) = smol_parser::parse("<test>", source);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error),
        "unexpected parse errors: {diags:#?}"
    );
    use smol_syntax::ast::{Expr, Stmt, Type};

    let stmts = file.main.expect("main block");

    // `c.x = classify(5);` — non-declaring assignment through a dot target.
    match &stmts[1] {
        Stmt::Assign { lhs, rhs, .. } => {
            match lhs {
                Expr::Field {
                    receiver, field, ..
                } => {
                    assert!(matches!(receiver.as_ref(), Expr::Var(n, _) if n == "c"));
                    assert_eq!(field, "x");
                }
                other => panic!("expected field lhs `c.x`, got {other:?}"),
            }
            assert!(
                matches!(rhs, Expr::Classify { .. }),
                "expected classify() rhs, got {rhs:?}"
            );
        }
        other => panic!("expected an assignment for `c.x = classify(5);`, got {other:?}"),
    }

    // `y = classify(1);` — plain non-declaring classify assignment (this
    // line is what the `=` limb of the classify guard protects).
    match &stmts[3] {
        Stmt::Assign { lhs, rhs, .. } => {
            assert!(
                matches!(lhs, Expr::Var(n, _) if n == "y"),
                "expected var lhs `y`, got {lhs:?}"
            );
            assert!(
                matches!(rhs, Expr::Classify { .. }),
                "expected classify() rhs, got {rhs:?}"
            );
        }
        other => panic!("expected an assignment for `y = classify(1);`, got {other:?}"),
    }

    // Negative guard: nothing may declare a variable of type `c` or `y`.
    for s in &stmts {
        if let Stmt::VarDecl { ty, .. } = s {
            assert_ne!(*ty, Type::Named("c".to_owned()), "misparse: {s:?}");
            assert_ne!(*ty, Type::Named("y".to_owned()), "misparse: {s:?}");
        }
    }
}

/// One compact lock for the probe-verified corners of the widened
/// `type_name`: the uppercase-receiver dot target `C.x = 5;` (bonus fix of
/// the `.` guard — this failed to parse before it), a whitespace-separated
/// dot target `c . x = 5;` (guard is whitespace-insensitive), a lowercase
/// *declared* classify, lowercase inside `Nullable`/`ExplList`/a nested
/// `List<List<…>>`, and a lowercase FMO port type.
#[test]
fn lowercase_type_corners_and_dot_target_variants() {
    let source = r#"class box(Int v)
end

class C(Int x)
end

main
  C c = new C(0);
  C.x = 5;
  c . x = 5;
  box z = classify(1);
  Nullable<box> nb = null;
  ExplList<box> eb = null;
  List<List<box>> ll = null;
  FMO[in box b] fb = null;
end"#;
    let (file, diags) = smol_parser::parse("<test>", source);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != smol_syntax::diagnostic::Severity::Error),
        "unexpected parse errors: {diags:#?}"
    );
    use smol_syntax::ast::{Expr, FmoDirection, Stmt, Type};

    let box_ty = Type::Named("box".to_owned());
    let stmts = file.main.expect("main block");

    /// Assert the statement assigns to `obj.field` and declares nothing.
    fn assert_field_assign(s: &Stmt, obj: &str, field: &str, what: &str) {
        let Stmt::Assign { lhs, .. } = s else {
            panic!("{what}: expected a non-declaring assignment, got {s:?}");
        };
        match lhs {
            Expr::Field {
                receiver, field: f, ..
            } => {
                assert!(
                    matches!(receiver.as_ref(), Expr::Var(n, _) if n == obj),
                    "{what}: wrong receiver in {lhs:?}"
                );
                assert_eq!(f, field, "{what}: wrong target field");
            }
            other => panic!("{what}: not a dot-qualified field target: {other:?}"),
        }
    }

    // `C.x = 5;` and `c . x = 5;` — dot targets parse as assignments.
    assert_field_assign(&stmts[1], "C", "x", "uppercase receiver `C.x = 5;`");
    assert_field_assign(&stmts[2], "c", "x", "whitespace variant `c . x = 5;`");

    // `box z = classify(1);` — lowercase *declared* classify.
    match &stmts[3] {
        Stmt::VarDecl { ty, name, init, .. } => {
            assert_eq!(*ty, box_ty, "declared classify type");
            assert_eq!(name, "z");
            assert!(
                matches!(init, Expr::Classify { .. }),
                "expected classify() init, got {init:?}"
            );
        }
        other => panic!("expected a var decl for `box z = classify(1);`, got {other:?}"),
    }

    // Lowercase nested inside generic wrappers.
    let expectations: [(&str, &Type); 3] = [
        ("Nullable<box>", &Type::Nullable(Box::new(box_ty.clone()))),
        ("ExplList<box>", &Type::ExplList(Box::new(box_ty.clone()))),
        (
            "List<List<box>>",
            &Type::List(Box::new(Type::List(Box::new(box_ty.clone())))),
        ),
    ];
    for (i, (what, expected)) in expectations.into_iter().enumerate() {
        match &stmts[4 + i] {
            Stmt::VarDecl { ty, .. } => assert_eq!(ty, expected, "declared type for {what}"),
            other => panic!("expected a var decl for {what}, got {other:?}"),
        }
    }

    // `FMO[in box b] fb = null;` — lowercase class as an FMO port type.
    match &stmts[7] {
        Stmt::VarDecl { ty, .. } => match ty {
            Type::Fmo(ports) => {
                assert_eq!(ports.len(), 1);
                assert_eq!(ports[0].direction, FmoDirection::In);
                assert_eq!(ports[0].ty, box_ty, "FMO port type");
                assert_eq!(ports[0].name, "b");
            }
            other => panic!("expected an FMO type, got {other:?}"),
        },
        other => panic!("expected a var decl for the FMO line, got {other:?}"),
    }

    // Negative guard: no dot-target line (statements 1..) may have declared
    // a variable of the receiver type (`C`/`c` would mean the prefix
    // swallowed it; statement 0 is the legitimate `C c = …` declaration).
    for s in &stmts[1..] {
        if let Stmt::VarDecl { ty, .. } = s {
            for recv in ["C", "c"] {
                assert_ne!(
                    *ty,
                    Type::Named(recv.to_owned()),
                    "dot-target misparsed as declaration of type `{recv}`: {s:?}"
                );
            }
        }
    }
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
