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

// --- `adapts` modifier checks (E2014) ---

#[test]
fn method_using_adapt_with_adapts_is_ok() {
    let src = r#"
class Twin() end
class Plant extends Twin() end

class PlantModel extends Twin()
    Unit reclassify() adapts Plant
        Plant p = new Plant();
        adapt(p);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::MissingAdapts),
        "method with adapts must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn method_using_adapt_without_adapts_is_error() {
    let src = r#"
class Twin() end
class Plant extends Twin() end

class PlantModel extends Twin()
    Unit reclassify()
        Plant p = new Plant();
        adapt(p);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::MissingAdapts),
        "method using adapt without adapts must be flagged, got: {diags:#?}"
    );
}

#[test]
fn method_using_classify_without_adapts_is_error() {
    let src = r#"
class Twin() end
class Plant extends Twin() end
class HealthState extends Twin() end

class PlantModel extends Twin()
    Unit reclassify()
        Plant p = new Plant();
        HealthState h = classify(p);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::MissingAdapts),
        "method using classify without adapts must be flagged, got: {diags:#?}"
    );
}

#[test]
fn method_calling_adapt_using_method_needs_adapts() {
    let src = r#"
class Twin() end
class Plant extends Twin() end

class Model extends Twin()
    Unit helper() adapts Plant
        Plant p = new Plant();
        adapt(p);
    end
    Unit caller()
        this.helper();
    end
end
"#;
    let diags = check_src(src);
    let flagged: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::MissingAdapts)
        .collect();
    assert!(
        flagged.iter().any(|d| d.message.contains("caller")),
        "caller of an adapt-using method must declare adapts, got: {diags:#?}"
    );
}

#[test]
fn method_calling_abstract_method_is_not_flagged() {
    // `adaptModel` calls the abstract `adaptAddition` (no body), so it must not
    // be forced to declare `adapts` — only the concrete override is checked.
    let src = r#"
class Twin() end
class Plant extends Twin() end

abstract class Base extends Twin()
    abstract Unit adaptAddition()
    Unit adaptModel()
        this.adaptAddition();
    end
end

class Sub extends Base()
    override Unit adaptAddition() adapts Plant
        Plant p = new Plant();
        adapt(p);
    end
end
"#;
    let diags = check_src(src);
    let flagged: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::MissingAdapts)
        .collect();
    assert!(
        !flagged.iter().any(|d| d.message.contains("adaptModel")),
        "adaptModel must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn override_missing_supermethod_adapts_is_error() {
    let src = r#"
class Twin() end
class Plant extends Twin() end

abstract class Base extends Twin()
    abstract Unit adaptAddition()
end

class Sub extends Base()
    override Unit adaptAddition() adapts Plant
        Plant p = new Plant();
        adapt(p);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::MissingAdapts),
        "override whose supermethod lacks the adapts class must be flagged, got: {diags:#?}"
    );
}

#[test]
fn override_with_supermethod_adapts_is_ok() {
    let src = r#"
class Twin() end
class Plant extends Twin() end

abstract class Base extends Twin()
    abstract Unit adaptAddition() adapts Plant
end

class Sub extends Base()
    override Unit adaptAddition() adapts Plant
        Plant p = new Plant();
        adapt(p);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::MissingAdapts),
        "override whose supermethod lists the same adapts classes must be ok, got: {diags:#?}"
    );
}

// ── Dead-code analysis ────────────────────────────────────────────────────────

#[test]
fn unused_local_variable_is_warned() {
    let src = r#"
class Counter (Int value)
    Unit increment()
        Int unused = 42;
        this.value = this.value;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::UnusedVariable),
        "expected UnusedVariable for `unused`, got: {diags:#?}"
    );
}

#[test]
fn used_local_variable_is_not_warned() {
    let src = r#"
class Counter (Int value)
    Unit double()
        Int v = this.value;
        this.value = v;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::UnusedVariable),
        "used variable must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn unused_parameter_is_warned() {
    let src = r#"
class Printer ()
    Unit printFixed(Int ignored)
        print(42);
    end
end

main
    Printer p = new Printer();
    p.printFixed(0);
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::UnusedParameter),
        "expected UnusedParameter for `ignored`, got: {diags:#?}"
    );
}

#[test]
fn used_parameter_is_not_warned() {
    let src = r#"
class Adder ()
    Int add(Int x, Int y)
        return x + y;
    end
end

main
    Adder a = new Adder();
    Int r = a.add(1, 2);
    print(r);
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::UnusedParameter),
        "used parameters must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn dead_method_is_warned() {
    let src = r#"
class Util ()
    Unit neverCalled()
        print(1);
    end
end

main
    Util u = new Util();
    print(u);
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::DeadMethod),
        "expected DeadMethod for `neverCalled`, got: {diags:#?}"
    );
}

#[test]
fn called_method_is_not_flagged_as_dead() {
    let src = r#"
class Util ()
    Unit doWork()
        print(1);
    end
end

main
    Util u = new Util();
    u.doWork();
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::DeadMethod),
        "called method must not be flagged as dead, got: {diags:#?}"
    );
}

#[test]
fn override_method_is_not_flagged_as_dead() {
    let src = r#"
abstract class Base ()
    abstract Unit run()
end

class Impl extends Base ()
    override Unit run()
        print(1);
    end
end

main
    Base b = new Impl();
    b.run();
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::DeadMethod),
        "override method must not be flagged as dead, got: {diags:#?}"
    );
}

#[test]
fn lhs_plain_assign_does_not_count_as_read() {
    // `result` is assigned on the LHS and only printed — that IS a read (print).
    let src = r#"
class Box (Int value)
    Int compute()
        Int result = this.value;
        result = result + 1;
        return result;
    end
end

main
    Box b = new Box(0);
    Int v = b.compute();
    print(v);
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::UnusedVariable),
        "reassigned-then-returned variable must not be flagged, got: {diags:#?}"
    );
}

// ── Null-safety (E2018) ───────────────────────────────────────────────────────

#[test]
fn null_dereference_on_literal_null_field_access_is_error() {
    let src = r#"
class Wrapper (Int value)
    Int get(Wrapper w)
        Wrapper x = null;
        return x.value;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::NullDereference),
        "expected NullDereference for `x.value` where x is null, got: {diags:#?}"
    );
}

#[test]
fn null_dereference_on_literal_null_method_call_is_error() {
    let src = r#"
class Counter ()
    Unit step()
        skip;
    end
    Unit run()
        Counter c = null;
        c.step();
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::NullDereference),
        "expected NullDereference for `c.step()` where c is null, got: {diags:#?}"
    );
}

#[test]
fn null_guarded_if_no_false_positive() {
    let src = r#"
class Node (Int val)
    Int safeGet(Node n)
        if n != null then
            return n.val;
        else
            return 0;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullDereference),
        "null-guarded access must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn null_guarded_while_no_false_positive() {
    let src = r#"
class Node (Node next, Int val)
    Unit walk(Node cur)
        while cur != null do
            print(cur.val);
            cur = cur.next;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullDereference),
        "while-guarded access must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn reassigned_null_is_flagged_after_assignment() {
    let src = r#"
class Obj (Int v) end

class Test ()
    Unit run()
        Obj o = new Obj(1);
        o = null;
        print(o.v);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::NullDereference),
        "expected NullDereference after explicit null assignment, got: {diags:#?}"
    );
}

#[test]
fn unknown_param_nullness_no_false_positive() {
    // Parameter `other` has unknown nullness; method body accesses it without
    // a null guard.  We must NOT flag this (we can't prove it's null).
    let src = r#"
class Geo (Int size)
    Boolean canMerge(Geo other)
        return other.size == this.size;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullDereference),
        "unknown-nullness parameter must not be flagged, got: {diags:#?}"
    );
}

// ── SPARQL injection (E3016) ──────────────────────────────────────────────────

#[test]
fn sparql_tainted_arg_is_warned() {
    let src = r#"
class Repo ()
    Unit fetch()
        List<Repo> results = access("SELECT ?x WHERE { ?x a <foo:Repo> }");
        Repo first = results.content;
        List<Repo> more = access("SELECT ?x WHERE { ?x a <foo:Repo> . FILTER(?x = %1) }", first);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::SparqlInjection),
        "expected SparqlInjection when access result used as SPARQL arg, got: {diags:#?}"
    );
}

#[test]
fn non_tainted_sparql_arg_no_warning() {
    let src = r#"
class Repo (String id)
    Unit fetch()
        List<Repo> results = access("SELECT ?x WHERE { ?x a <foo:Repo> . FILTER(?id = %1) }", this.id);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlInjection),
        "field arg (non-tainted) must not be flagged, got: {diags:#?}"
    );
}

// ── FMO lifecycle (E2020) ─────────────────────────────────────────────────────

#[test]
fn fmo_not_destroyed_is_warned() {
    let src = r#"
class Sim ()
    Unit run()
        FMO[in Double speed, out Double pos] fmu = null;
        print(fmu);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::FmoNotDestroyed),
        "expected FmoNotDestroyed for undestroyed FMO variable, got: {diags:#?}"
    );
}

#[test]
fn fmo_destroyed_no_warning() {
    let src = r#"
class Sim ()
    Unit run()
        FMO[in Double speed, out Double pos] fmu = null;
        print(fmu);
        destroy(fmu);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::FmoNotDestroyed),
        "destroyed FMO variable must not be warned, got: {diags:#?}"
    );
}

// ── Domain effects (E2019) ──────────────────────────────────────────────────

#[test]
fn domain_write_in_plain_method_is_warned() {
    let src = r#"
class Plant (domain Double moisture)
    Unit sync()
        this.moisture = 1.0;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::DomainMutation),
        "expected DomainMutation for domain write in plain method, got: {diags:#?}"
    );
    assert!(
        diags.iter().filter(|d| d.is_error()).count() == 0,
        "DomainMutation must be warning-level, got: {diags:#?}"
    );
}

#[test]
fn domain_write_in_domain_method_is_clean() {
    let src = r#"
class Plant (domain Double moisture)
    domain Unit sync()
        this.moisture = 1.0;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::DomainMutation),
        "domain method writing domain field must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn transitive_domain_write_is_warned() {
    let src = r#"
class Plant (domain Double moisture)
    domain Unit sync()
        this.moisture = 1.0;
    end
    Unit driver()
        this.sync();
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::DomainMutation),
        "expected DomainMutation for transitive domain write, got: {diags:#?}"
    );
}

#[test]
fn hidden_field_in_links_is_warned() {
    let src = r#"
class Plant (hidden String secret)
links "a ast:Plant ; ast:secret %secret";
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::DomainMutation),
        "expected DomainMutation for hidden field leaked into links, got: {diags:#?}"
    );
}

// ── Path-sensitive FMO lifecycle (W2020) ─────────────────────────────────────

#[test]
fn fmo_destroyed_on_all_branches_no_warning() {
    let src = r#"
class Sim ()
    Unit run(Boolean flag)
        FMO[in Double speed, out Double pos] fmu = null;
        if flag then
            destroy(fmu);
        else
            destroy(fmu);
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::FmoNotDestroyed),
        "destroy on every branch must not be warned, got: {diags:#?}"
    );
}

#[test]
fn fmo_destroyed_on_one_branch_is_warned() {
    let src = r#"
class Sim ()
    Unit run(Boolean flag)
        FMO[in Double speed, out Double pos] fmu = null;
        if flag then
            destroy(fmu);
        else
            print("leak");
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::FmoNotDestroyed),
        "expected FmoNotDestroyed when one branch leaks, got: {diags:#?}"
    );
}

#[test]
fn fmo_destroy_only_in_loop_body_is_warned() {
    let src = r#"
class Sim ()
    Unit run(List<String> items)
        FMO[in Double speed, out Double pos] fmu = null;
        while items != null do
            destroy(fmu);
            items = items.next;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::FmoNotDestroyed),
        "expected FmoNotDestroyed when destroy is only inside a loop body, got: {diags:#?}"
    );
}

// ── Inter-procedural SPARQL taint ────────────────────────────────────────────

#[test]
fn interprocedural_taint_through_query_sink_is_warned() {
    let src = r#"
class Store ()
    Unit lookup(String key)
        List<String> r = access("SELECT ?x { ?x <urn:p> \"%1\" }", key);
        print(r);
    end
    Unit driver()
        List<String> tainted = access("SELECT ?y { ?y a <urn:T> }");
        this.lookup(tainted.content);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::SparqlInjection),
        "expected SparqlInjection across method boundary, got: {diags:#?}"
    );
}

#[test]
fn interprocedural_taint_clean_call_no_warning() {
    let src = r#"
class Store ()
    Unit lookup(String key)
        List<String> r = access("SELECT ?x { ?x <urn:p> \"%1\" }", key);
        print(r);
    end
    Unit driver()
        this.lookup("static-key");
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlInjection),
        "clean call must not be flagged, got: {diags:#?}"
    );
}

// ── Symbolic execution: arithmetic (E2021/E2022) ────────────────────────────

#[test]
fn definite_division_by_zero_is_error() {
    let src = r#"
class Calc ()
    Int div(Int x, Int y)
        Int q = x / 0;
        return q;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::DivisionByZero),
        "expected DivisionByZero, got: {diags:#?}"
    );
}

#[test]
fn guarded_division_no_false_positive() {
    let src = r#"
class Calc ()
    Int div(Int x, Int y)
        if y != 0 then
            Int q = x / y;
            return q;
        else
            return 0;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::DivisionByZero),
        "guarded division must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn zero_guard_then_divide_is_error() {
    let src = r#"
class Calc ()
    Int div(Int x, Int y)
        if y == 0 then
            Int q = x / y;
            return q;
        else
            return 0;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::DivisionByZero),
        "expected DivisionByZero on the y==0 path, got: {diags:#?}"
    );
}

#[test]
fn constant_overflow_is_error() {
    let src = r#"
class Calc ()
    Int big()
        Int x = 9223372036854775807 + 1;
        return x;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::IntegerOverflow),
        "expected IntegerOverflow, got: {diags:#?}"
    );
}

// ── Symbolic execution: termination (E5012) ─────────────────────────────────

#[test]
fn loop_without_variant_is_warned() {
    let src = r#"
class Loop ()
    Unit run()
        Int i = 0;
        while i < 10 do
            print(i);
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::LoopTerminationUnproven),
        "expected LoopTerminationUnproven, got: {diags:#?}"
    );
}

#[test]
fn loop_with_variant_no_warning() {
    let src = r#"
class Loop ()
    Unit run()
        Int i = 0;
        while i < 10 do
            print(i);
            i = i + 1;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::LoopTerminationUnproven),
        "loop with variant must not be flagged, got: {diags:#?}"
    );
}
