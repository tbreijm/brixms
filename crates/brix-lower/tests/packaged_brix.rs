//! The packaged `.brix` sources must keep checking.
//!
//! Without this they are files someone once ran `brix check` on. The examples
//! declared a `base: Money` field for months with no such type existing —
//! exactly the rot this guards against, and the reason a shipped `.brix`
//! source needs a test rather than a good intention.

use brix_lower::check_module;
use brix_syntax::parse;

/// Run `f` on a thread with a large stack.
///
/// ⚠ **This is a workaround for a real cost, not a formality.** Checking
/// `brix.soc` — about 130 lines — overflows a default 2 MiB test-thread stack.
/// The CLI only works because the main thread gets 8 MiB.
///
/// The cause is that derivation building is quadratic in expression depth:
/// every `Ctor` clones and digests its *whole* sub-expression as its `src`
/// atom, so a nested literal is re-hashed once per enclosing level. Raising
/// the stack here keeps the gate honest about what it is measuring — whether
/// the source still checks — rather than turning a depth problem into a
/// spurious failure. The depth problem itself is worth its own fix.
fn with_deep_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("checking must not panic")
}

/// `brix.soc` — SOC's core, written in Brix.
///
/// Every binding must check. The *grades* are deliberately not asserted here:
/// they move as generators are discharged, and pinning them would make this a
/// change-detector rather than a rot-detector.
#[test]
fn brix_soc_checks() {
    with_deep_stack(|| {
        let source = include_str!("../../../packages/brix.soc/src/soc.brix");
        let module = parse(source).expect("brix.soc must parse");
        let results = check_module(&module);

        assert!(
            !results.is_empty(),
            "brix.soc must contain checkable bindings"
        );
        for r in &results {
            r.as_ref().unwrap_or_else(|(name, e)| {
                panic!("brix.soc binding '{name}' failed to check: {e:?}")
            });
        }
    });
}

/// The model's own honesty result is capped, and that is the point of it.
///
/// `honest_outcome` rests on `weaker` (which compares, so `g_cmp`) and on
/// `all_tight` (which recurses, so `g_fix`) — both undischarged. So a
/// statement of the honesty condition is itself graded by that condition.
/// If this ever reports `Proven`, either those generators were discharged or
/// something stopped being honest; both deserve a look.
#[test]
fn brix_soc_is_graded_by_the_rule_it_describes() {
    use brix_semantic::Outcome;

    let outcome = with_deep_stack(|| {
        let source = include_str!("../../../packages/brix.soc/src/soc.brix");
        let module = parse(source).expect("parse");
        check_module(&module)
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .find(|c| c.name == "honest")
            .map(|c| c.outcome)
            .expect("the worked example must be present")
    });

    assert_eq!(
        outcome,
        Outcome::Audited,
        "the model's own result must be capped by the undischarged generators it uses"
    );
}

/// `use brix.soc` brings its declarations into scope without redeclaring them.
#[test]
fn a_package_can_be_imported() {
    use brix_lower::imports::{resolve_imports, ImportError};

    let soc = include_str!("../../../packages/brix.soc/src/soc.brix").to_string();
    let load = move |p: &str| (p == "brix.soc").then(|| soc.clone());

    let module = parse("use brix.soc\nlet best = weaker(Proven, Audited)").expect("parse");
    let resolved = resolve_imports(&module, &load).expect("brix.soc resolves");

    let checked = with_deep_stack(move || {
        check_module(&resolved)
            .into_iter()
            .next()
            .expect("one binding")
            .map(|c| c.outcome)
            .map_err(|(name, e)| format!("{name}: {e:?}"))
    });
    assert!(
        checked.is_ok(),
        "the importing module must check: {checked:?}"
    );

    // A package the loader does not know is named, not silently skipped.
    let module = parse("use brix.nope\nlet x = 1").expect("parse");
    assert_eq!(
        resolve_imports(&module, &load),
        Err(ImportError::NotFound("brix.nope".to_string()))
    );
}

/// An import cycle is refused with the chain, rather than looping.
#[test]
fn an_import_cycle_is_refused() {
    use brix_lower::imports::{resolve_imports, ImportError};

    let load = |p: &str| match p {
        "a" => Some("use b\nconfig A = MkA".to_string()),
        "b" => Some("use a\nconfig B = MkB".to_string()),
        _ => None,
    };
    let module = parse("use a\nlet x = 1").expect("parse");
    match resolve_imports(&module, &load) {
        Err(ImportError::Cycle(chain)) => {
            assert!(
                chain.len() >= 2 && chain.first() == chain.last(),
                "{chain:?}"
            );
        }
        other => panic!("expected a cycle, got {other:?}"),
    }
}

/// Two visible declarations sharing a name are refused rather than shadowed.
///
/// With no qualified-name syntax there is no way for a program to say which
/// one it meant, so picking one would make its meaning depend on import order.
#[test]
fn a_name_conflict_is_refused() {
    use brix_lower::imports::{resolve_imports, ImportError};

    let load = |p: &str| (p == "lib").then(|| "config Outcome = Yes | No".to_string());
    let module = parse("use lib\nconfig Outcome = A | B\nlet x = 1").expect("parse");
    match resolve_imports(&module, &load) {
        Err(ImportError::Conflict { name, .. }) => assert_eq!(name, "Outcome"),
        other => panic!("expected a conflict, got {other:?}"),
    }
}
