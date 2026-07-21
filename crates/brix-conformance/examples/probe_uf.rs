use brix_ast::parse_file;
use brix_ir::reflect::{analyze, ConflictKind, Fact};
use brixc::lower_file;
fn run(label: &str, src: &str) {
    let (file, diags) = parse_file(src);
    let lowered = lower_file(&file, &diags);
    let report = analyze(&lowered.source, &lowered.resolver);
    let lows: Vec<_> = lowered.diags.iter().map(|d| d.message.clone()).collect();
    let uf = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::UnknownField { .. }))
        .count();
    let fa = report
        .facts
        .iter()
        .filter(|f| matches!(f.fact, Fact::FieldAccess { .. }))
        .count();
    eprintln!("[{label}] lowdiags={lows:?} FieldAccessFacts={fa} UnknownFieldConflicts={uf}");
}
fn main() {
    run("A let-record r.b", "package t @ 1.0.0\nrel Output { n: Int } key(n)\nderive C: Output(n: 1) from {\n  let r = { a: 1 }\n  let bad = r.b\n}\n");
    run("B let-int r.x", "package t @ 1.0.0\nrel Output { n: Int } key(n)\nderive C: Output(n: 1) from {\n  let r = 1\n  let bad = r.x\n}\n");
    run("C let-from-role m.x", "package t @ 1.0.0\nrel Input { n: Int } key(n)\nrel Output { n: Int } key(n)\nderive C: Output(n: n) from {\n  Input(n: n)\n  let m = n\n  let bad = m.x\n}\n");
    run("D record path Foo{}", "package t @ 1.0.0\nrel Output { n: Int } key(n)\nderive C: Output(n: 1) from {\n  let r = { a: 1 }\n  let bad = r.a\n}\n");
}
