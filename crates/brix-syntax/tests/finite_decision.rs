use brix_syntax::ast::*;
use brix_syntax::parse;

#[test]
fn test_propose_basic() {
    let source = "propose full_discount() priority 0 when true = 10";
    let module = parse(source).expect("basic propose should parse");
    assert_eq!(module.items.len(), 1);

    match &module.items[0] {
        Item::Propose(ProposeDecl {
            name,
            deps,
            priority,
            guard,
            value,
        }) => {
            assert_eq!(name, "full_discount");
            assert!(deps.is_empty());
            assert_eq!(*priority, 0);
            assert_eq!(*guard, Expr::Bool(true));
            assert_eq!(*value, Expr::Num("10".into()));
        }
        other => panic!("Expected Item::Propose, got {:?}", other),
    }
}

#[test]
fn test_propose_with_dependencies_and_expressions() {
    let source = "propose step_a(cand_x, cand_y) priority 10 when stock > 0 = price * 2";
    let module = parse(source).expect("propose with dependencies and expressions should parse");
    assert_eq!(module.items.len(), 1);

    match &module.items[0] {
        Item::Propose(ProposeDecl {
            name,
            deps,
            priority,
            guard,
            value,
        }) => {
            assert_eq!(name, "step_a");
            assert_eq!(deps, &["cand_x", "cand_y"]);
            assert_eq!(*priority, 10);
            assert_eq!(
                *guard,
                Expr::Bin {
                    op: BinOp::Gt,
                    lhs: Box::new(Expr::Var("stock".into())),
                    rhs: Box::new(Expr::Num("0".into())),
                }
            );
            assert_eq!(
                *value,
                Expr::Bin {
                    op: BinOp::Mul,
                    lhs: Box::new(Expr::Var("price".into())),
                    rhs: Box::new(Expr::Num("2".into())),
                }
            );
        }
        other => panic!("Expected Item::Propose, got {:?}", other),
    }
}

#[test]
fn test_propose_trailing_comma_in_dependencies() {
    let source = "propose cand(dep1,) priority 42 when active = 1";
    let module = parse(source).expect("trailing comma in dependencies should parse");
    assert_eq!(module.items.len(), 1);

    match &module.items[0] {
        Item::Propose(decl) => {
            assert_eq!(decl.name, "cand");
            assert_eq!(decl.deps, &["dep1"]);
            assert_eq!(decl.priority, 42);
        }
        other => panic!("Expected Item::Propose, got {:?}", other),
    }
}

#[test]
fn test_commit_basic() {
    let source = "commit final_choice from (full_discount, step_a)";
    let module = parse(source).expect("basic commit should parse");
    assert_eq!(module.items.len(), 1);

    match &module.items[0] {
        Item::Commit(CommitDecl { name, candidates }) => {
            assert_eq!(name, "final_choice");
            assert_eq!(candidates, &["full_discount", "step_a"]);
        }
        other => panic!("Expected Item::Commit, got {:?}", other),
    }
}

#[test]
fn test_commit_members_accessor_and_trailing_comma() {
    let source = "commit single from (only_one,)";
    let module = parse(source).expect("commit with trailing comma should parse");
    assert_eq!(module.items.len(), 1);

    match &module.items[0] {
        Item::Commit(decl) => {
            assert_eq!(decl.name, "single");
            assert_eq!(decl.candidates, &["only_one"]);
            assert_eq!(decl.members(), &["only_one"]);
        }
        other => panic!("Expected Item::Commit, got {:?}", other),
    }
}

#[test]
fn test_combined_finite_decision_module() {
    let source = "\
config Outcome = Base | Discounted
let base_price = 100
propose disc(base_price) priority 0 when base_price > 50 = Outcome.Discounted
propose regular() priority 1 when true = Outcome.Base
commit chosen from (disc, regular)
show chosen
";
    let module = parse(source).expect("combined module should parse");
    assert_eq!(module.items.len(), 6);

    assert!(matches!(&module.items[0], Item::Config(_)));
    assert!(matches!(&module.items[1], Item::Let(_)));
    assert!(matches!(&module.items[2], Item::Propose(_)));
    assert!(matches!(&module.items[3], Item::Propose(_)));
    assert!(matches!(&module.items[4], Item::Commit(_)));
    assert!(matches!(&module.items[5], Item::Show(_)));
}

#[test]
fn test_newline_separated_propose_commit_and_show() {
    let source = "\
propose alpha() priority 0 when true = 1
propose beta(alpha) priority 1 when x > 0 = 2
commit outcome from (alpha, beta)
show outcome
";
    let module = parse(source).expect("newline separated items should parse");
    assert_eq!(module.items.len(), 4);
    match &module.items[0] {
        Item::Propose(decl) => {
            assert_eq!(decl.name, "alpha");
            assert!(decl.deps.is_empty());
            assert_eq!(decl.priority, 0);
            assert_eq!(decl.guard, Expr::Bool(true));
            assert_eq!(decl.value, Expr::Num("1".into()));
        }
        other => panic!("Expected Propose, got {:?}", other),
    }
    match &module.items[1] {
        Item::Propose(decl) => {
            assert_eq!(decl.name, "beta");
            assert_eq!(decl.deps, &["alpha"]);
            assert_eq!(decl.priority, 1);
        }
        other => panic!("Expected Propose, got {:?}", other),
    }
    match &module.items[2] {
        Item::Commit(decl) => {
            assert_eq!(decl.name, "outcome");
            assert_eq!(decl.members(), &["alpha", "beta"]);
        }
        other => panic!("Expected Commit, got {:?}", other),
    }
    match &module.items[3] {
        Item::Show(Expr::Var(name)) => {
            assert_eq!(name, "outcome");
        }
        other => panic!("Expected Show(Var), got {:?}", other),
    }
}

#[test]
fn test_newline_separated_propose_and_show() {
    let source = "propose cand() priority 0 when true = 1\nshow 1";
    let module = parse(source).expect("newline separated propose and show should parse");
    assert_eq!(module.items.len(), 2);
    assert!(matches!(&module.items[0], Item::Propose(_)));
    assert!(matches!(&module.items[1], Item::Show(_)));
}

#[test]
fn test_newline_separated_commit_and_show() {
    let source = "commit winner from (c1, c2)\nshow winner";
    let module = parse(source).expect("newline separated commit and show should parse");
    assert_eq!(module.items.len(), 2);
    assert!(matches!(&module.items[0], Item::Commit(_)));
    assert!(matches!(&module.items[1], Item::Show(_)));
}

#[test]
fn test_semicolon_bearing_input_fails() {
    // Semicolon after propose
    let err = parse("propose cand() priority 0 when true = 1;").unwrap_err();
    assert!(err.message.contains("Unexpected character ';'"));

    // Semicolon after commit
    let err = parse("commit single from (cand1, cand2);").unwrap_err();
    assert!(err.message.contains("Unexpected character ';'"));

    // Semicolon after show
    let err = parse("show 1;").unwrap_err();
    assert!(err.message.contains("Unexpected character ';'"));

    // Semicolon inside propose dependencies
    let err = parse("propose cand(dep1;) priority 0 when true = 1").unwrap_err();
    assert!(err.message.contains("Unexpected character ';'"));

    // Semicolon between propose clauses
    let err = parse("propose cand() priority 0; when true = 1").unwrap_err();
    assert!(err.message.contains("Unexpected character ';'"));

    // Semicolon inside commit candidate list
    let err = parse("commit single from (cand1; cand2)").unwrap_err();
    assert!(err.message.contains("Unexpected character ';'"));

    // Standalone semicolon
    let err = parse(";").unwrap_err();
    assert!(err.message.contains("Unexpected character ';'"));

    // Semicolon after let
    let err = parse("let x = 1;").unwrap_err();
    assert!(err.message.contains("Unexpected character ';'"));
}

#[test]
fn test_semantic_checks_deferred_to_lowering() {
    // 1. Multiple commit declarations in one module (syntactically permitted)
    let multi_commit = "\
commit first from (c1)
commit second from (c2)
";
    let m1 = parse(multi_commit)
        .expect("multiple commits are syntactically valid (semantic check in lowering)");
    assert_eq!(m1.items.len(), 2);

    // 2. Duplicate candidate names in propose or commit
    let duplicate_names = "\
propose c() priority 0 when true = 1
propose c() priority 1 when true = 2
commit chosen from (c, c)
";
    let m2 = parse(duplicate_names)
        .expect("duplicate names are syntactically valid (semantic check in lowering)");
    assert_eq!(m2.items.len(), 3);

    // 3. Non-boolean guard expression
    let non_bool_guard = "propose c() priority 0 when 100 = 200";
    let m3 = parse(non_bool_guard)
        .expect("non-boolean guard is syntactically valid (semantic check in lowering)");
    match &m3.items[0] {
        Item::Propose(decl) => assert_eq!(decl.guard, Expr::Num("100".into())),
        _ => unreachable!(),
    }

    // 4. Mismatched value types across proposals
    let mismatched_values = "\
propose c1() priority 0 when true = \"text\"
propose c2() priority 1 when true = 42
commit chosen from (c1, c2)
";
    let m4 = parse(mismatched_values)
        .expect("mismatched value types are syntactically valid (semantic check in lowering)");
    assert_eq!(m4.items.len(), 3);

    // 5. Undeclared / forward dependencies and candidates (no earlier rule check in syntax)
    let undeclared = "\
propose c(nonexistent_dep) priority 0 when true = 1
commit chosen from (nonexistent_cand)
";
    let m5 = parse(undeclared)
        .expect("undeclared references are syntactically valid (semantic check in lowering)");
    assert_eq!(m5.items.len(), 2);
}

#[test]
fn test_malformed_propose_diagnostics() {
    // Missing candidate name
    let err = parse("propose (dep) priority 0 when true = 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(9));
    assert!(err.message.contains("Expected identifier"));

    // Missing open paren for dependencies
    let err = parse("propose cand priority 0 when true = 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(14));
    assert!(err.message.contains("Expected OpenParen"));

    // Non-identifier in dependencies
    let err = parse("propose cand(123) priority 0 when true = 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(14));
    assert!(err.message.contains("Expected identifier"));

    // Missing priority keyword
    let err = parse("propose cand() 1 when true = 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(16));
    assert!(err.message.contains("Expected Priority"));

    // Negative priority
    let err = parse("propose cand() priority -1 when true = 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(25));
    assert!(err
        .message
        .contains("Expected nonnegative unsigned integer"));

    // Float priority
    let err = parse("propose cand() priority 1.5 when true = 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(25));
    assert!(err
        .message
        .contains("Expected nonnegative unsigned integer"));

    // Identifier as priority
    let err = parse("propose cand() priority high when true = 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(25));
    assert!(err
        .message
        .contains("Expected nonnegative unsigned integer"));

    // Missing when keyword
    let err = parse("propose cand() priority 0 true = 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(27));
    assert!(err.message.contains("Expected When"));

    // Missing '=' between guard and value
    let err = parse("propose cand() priority 0 when true 1").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(37));
    assert!(err.message.contains("Expected Equals"));
}

#[test]
fn test_malformed_commit_diagnostics() {
    // Missing commit name
    let err = parse("commit from (c1, c2)").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(8));
    assert!(err.message.contains("Expected identifier"));

    // Missing 'from' keyword
    let err = parse("commit decision (c1, c2)").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(17));
    assert!(err.message.contains("Expected From"));

    // Missing open paren
    let err = parse("commit decision from c1, c2").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(22));
    assert!(err.message.contains("Expected OpenParen"));

    // Non-identifier candidate
    let err = parse("commit decision from (123)").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(23));
    assert!(err.message.contains("Expected identifier"));

    // Empty candidate list (nonempty ordered list requirement)
    let err = parse("commit decision from ()").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(22));
    assert!(err.message.contains("cannot be empty"));

    // Unclosed paren
    let err = parse("commit decision from (c1, c2").unwrap_err();
    assert_eq!(err.line, Some(1));
    assert_eq!(err.col, Some(29));
    assert!(err.message.contains("Expected CloseParen"));
}
