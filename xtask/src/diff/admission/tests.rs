use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::diff::{capture, corpus};

const DECLINED: &str = "aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable \
                        (unsupported_option); Python fallback disabled\n";

fn run_of(fields: Value) -> Result<Capture, crate::TaskError> {
    let mut value = json!({"id": "r", "side": "a", "exit": 0, "timed_out": false,
                           "stdout": "", "stderr": "", "requests": []});
    if let (Some(value), Some(fields)) = (value.as_object_mut(), fields.as_object()) {
        value.extend(fields.clone());
    }
    capture::parse(&value.to_string())?
        .pop()
        .ok_or_else(|| crate::deps::error("no capture"))
}

fn declined() -> Result<Capture, crate::TaskError> {
    run_of(json!({"exit": 255, "stderr": DECLINED}))
}

fn side(route: Route) -> Side {
    Side {
        label: "candidate".to_owned(),
        binary: PathBuf::from("aws"),
        route,
    }
}

fn run(a: Route, b: Route) -> Run {
    Run {
        a: side(a),
        b: side(b),
        corpus: Vec::new(),
        filter: None,
        known: None,
        out: PathBuf::new(),
    }
}

/// The fingerprints of row `r`, or none when it is not native, and the
/// refusals counted.
fn classified(
    run: &Run,
    a: &Capture,
    b: &Capture,
) -> Result<(Option<Vec<String>>, Refusals), crate::TaskError> {
    let rows = corpus::parse("rows", "{\"id\":\"r\",\"argv\":[\"x\"],\"responses\":[]}")?;
    let row = rows.first().ok_or_else(|| crate::deps::error("no row"))?;
    let mut refusals = Refusals::new();
    let found = diffs(run, row, (a, b), &mut refusals);
    let names = found.map(|diffs| diffs.into_iter().map(|diff| diff.fingerprint).collect());
    Ok((names, refusals))
}

#[test]
fn a_refusal_is_the_native_routes_one_line_and_exit_255() -> Result<(), crate::TaskError> {
    let native = side(Route::Native);
    assert_eq!(refusal(&native, &declined()?), Some("unsupported_option"));
    assert_eq!(refusal(&side(Route::Unset), &declined()?), None);
    let not_quite = [
        json!({"exit": 1, "stderr": DECLINED}),
        json!({"exit": 255, "stderr": DECLINED, "stdout": "x"}),
        json!({"exit": 255, "stderr": format!("{DECLINED}more\n")}),
        json!({"exit": 255, "stderr": DECLINED.replace("(unsupported_option)", "()")}),
    ];
    for fields in not_quite {
        assert_eq!(refusal(&native, &run_of(fields.clone())?), None, "{fields}");
    }
    Ok(())
}

#[test]
fn a_row_a_native_side_declines_is_not_native() -> Result<(), crate::TaskError> {
    let ran = run_of(json!({"stdout": "{}"}))?;
    let oracle = run(Route::Native, Route::Python);
    let (found, refusals) = classified(&oracle, &declined()?, &ran)?;
    assert_eq!(found, None);
    let rows = refusals
        .get("a")
        .and_then(|reasons| reasons.get("unsupported_option"));
    assert_eq!(rows.map(Vec::as_slice), Some(["r".to_owned()].as_slice()));
    let both = run(Route::Native, Route::Native);
    assert_eq!(classified(&both, &declined()?, &declined()?)?.0, None);
    let hung = run_of(json!({"exit": null, "timed_out": true}))?;
    assert_eq!(
        classified(&oracle, &declined()?, &hung)?.0,
        Some(vec!["timeout:b".to_owned()])
    );
    Ok(())
}

#[test]
fn two_native_sides_that_disagree_changed_the_admission() -> Result<(), crate::TaskError> {
    let ran = run_of(json!({"stdout": "{}"}))?;
    let both = run(Route::Native, Route::Native);
    let changed = |names: &[&str]| Some(names.iter().map(|name| (*name).to_owned()).collect());
    assert_eq!(
        classified(&both, &declined()?, &ran)?.0,
        changed(&["admission-changed:a"])
    );
    assert_eq!(
        classified(&both, &ran, &declined()?)?.0,
        changed(&["admission-changed:b"])
    );
    let other =
        run_of(json!({"exit": 255, "stderr": DECLINED.replace("unsupported_option", "env")}))?;
    assert_eq!(
        classified(&both, &declined()?, &other)?.0,
        changed(&["admission-changed:reason"])
    );
    Ok(())
}
