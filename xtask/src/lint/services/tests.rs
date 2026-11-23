use super::*;
use std::path::PathBuf;

fn names() -> Vec<String> {
    ["sts", "dynamodb", "location", "s3"]
        .map(String::from)
        .to_vec()
}

fn found(relative: &str, source: &str) -> Result<Vec<String>, TaskError> {
    let mut report = Report::default();
    check_file(Path::new(relative), relative, source, &names(), &mut report)?;
    Ok(report
        .diagnostics
        .iter()
        .map(|diagnostic| format!("{} {}", diagnostic.line, diagnostic.detail))
        .collect())
}

/// A planted name is found in a plain, a raw and a byte string, with its
/// line, and not in a comment, a character literal or an identifier.
#[test]
fn a_planted_service_name_is_found() -> Result<(), TaskError> {
    let source = concat!(
        "// \"sts\" in a comment is fine\n",
        "let quote = '\"';\n",
        "let service = \"dynamodb\";\n",
        "let raw = r#\"sts\"#;\n",
        "let bytes = b\"s3\";\n",
        "let sts = \"not-a-service\";\n",
    );
    assert_eq!(
        found("crates/client/src/x.rs", source)?,
        [
            "3 names the service \"dynamodb\"",
            "4 names the service \"sts\"",
            "5 names the service \"s3\""
        ]
    );
    Ok(())
}

/// An exemption holds for its own file and literal only.
#[test]
fn an_exemption_names_its_file_and_literal() -> Result<(), TaskError> {
    let source = "let key = \"location\";\nlet other = \"sts\";\n";
    assert_eq!(
        found("crates/model/src/shapes/traits.rs", source)?,
        ["2 names the service \"sts\""]
    );
    assert_eq!(found("crates/model/src/shapes/build.rs", source)?.len(), 2);
    Ok(())
}

#[test]
fn the_scope_is_the_generic_code_without_its_tests() {
    for inside in [
        "crates/model/src/shapes.rs",
        "crates/service-errors/src/lib.rs",
        "crates/cli-router/src/generic.rs",
        "crates/cli-router/src/effective_config/settings.rs",
    ] {
        assert!(in_scope(inside), "{inside}");
    }
    for outside in [
        "crates/customizations/src/lib.rs",
        "crates/cli-router/src/lib.rs",
        "crates/client/src/call/tests.rs",
        "crates/client/src/testing.rs",
        "crates/client/src/testing/fuzz.rs",
        "crates/client/tests/protocol_suite.rs",
        "crates/s3/src/lib.rs",
    ] {
        assert!(!in_scope(outside), "{outside}");
    }
}

/// Every exemption sits in scope, so none is stale or excuses nothing.
#[test]
fn every_exemption_is_in_scope() {
    for (file, literal, reason) in EXEMPT {
        assert!(in_scope(file), "{file}");
        assert!(!literal.is_empty() && !reason.is_empty());
    }
    assert!(!in_scope(&PathBuf::from("x").to_string_lossy()));
}
