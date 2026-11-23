use super::*;
use serde_json::json;
use std::fs;

/// `cargo metadata`'s shape for one package and its path dependencies.
fn package(
    name: &str,
    manifest: &Path,
    dependencies: &[(&str, Option<&str>)],
) -> serde_json::Value {
    let dependencies: Vec<_> = dependencies
        .iter()
        .map(|(on, kind)| json!({"name": on, "kind": kind, "rename": null, "path": "/crates"}))
        .collect();
    json!({"name": name, "manifest_path": manifest, "dependencies": dependencies})
}

/// `name`'s row; a crate without one allows nothing.
fn row(name: &str) -> &'static [&'static str] {
    let found = ALLOWED.iter().find(|(crate_name, _)| *crate_name == name);
    assert!(found.is_some(), "{name} has no row");
    found.map_or(&[], |(_, allowed)| *allowed)
}

#[test]
fn a_planted_upward_dependency_fails_on_its_manifest_line() -> Result<(), TaskError> {
    let directory = std::env::temp_dir().join(format!("awscli-layers-{}", std::process::id()));
    fs::create_dir_all(&directory)?;
    let manifest = directory.join("Cargo.toml");
    let text = "[package]\nname = \"format\"\n\n[dependencies]\nmodel.workspace = true\n\
                client = { path = \"../client\" }\n";
    fs::write(&manifest, text)?;
    let metadata = json!({"packages": [
        package("format", &manifest, &[("model", None), ("client", None)]),
        package("client", &manifest, &[("crt", None), ("format", Some("dev"))]),
    ]});
    let diagnostics = check(&metadata);
    fs::remove_dir_all(&directory)?;
    let diagnostics = diagnostics?;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        (diagnostics[0].line, diagnostics[0].rule),
        (6, Rule::Layering)
    );
    assert_eq!(
        diagnostics[0].detail,
        "format may not depend on client (plan/PLAN.md §1.2, §2.2)"
    );
    Ok(())
}

#[test]
fn a_crate_without_a_row_fails() -> Result<(), TaskError> {
    let manifest = Path::new("/absent/Cargo.toml");
    let metadata = json!({"packages": [package("new-crate", manifest, &[])]});
    let diagnostics = check(&metadata)?;
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].detail,
        "crate new-crate has no row in the layer table"
    );
    Ok(())
}

#[test]
fn registry_dependencies_are_not_layered() -> Result<(), TaskError> {
    let manifest = Path::new("/absent/Cargo.toml");
    let metadata = json!({"packages": [{
        "name": "model",
        "manifest_path": manifest,
        "dependencies": [{"name": "serde_json", "kind": null, "rename": null}],
    }]});
    assert!(check(&metadata)?.is_empty());
    Ok(())
}

/// The table keeps the plan's rules (§2.2) that T11a names.
#[test]
fn the_table_keeps_the_plan_s_dependency_rules() {
    let above = ["s3", "cli-router", "aws-cli", "python-fallback"];
    for name in ["model", "client", "format", "command"] {
        assert!(row(name).iter().all(|on| !above.contains(on)), "{name}");
    }
    let added = [
        "model",
        "client",
        "format",
        "command",
        "timestamp",
        "credentials",
    ];
    for name in added.iter().filter(|name| **name != "client") {
        assert!(!row(name).contains(&"crt"), "{name}");
    }
    for name in ["command", "format", "credentials"] {
        assert!(!row(name).contains(&"client"), "{name}");
    }
    assert!(!row("customizations").contains(&"s3"));
    assert!(!row("service-errors").contains(&"s3") && !row("service-errors").contains(&"command"));
}

/// Every crate a row allows has a row of its own.
#[test]
fn every_allowed_crate_has_a_row() {
    for (name, allowed) in ALLOWED {
        for on in *allowed {
            assert!(
                ALLOWED.iter().any(|(crate_name, _)| crate_name == on),
                "{name}: {on}"
            );
        }
    }
}
