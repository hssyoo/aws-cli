use super::defines;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `succeeded` and `select` are each defined twice in the client; a path
/// resolves only to the module and owner that define it.
#[test]
fn a_name_defined_twice_resolves_only_where_its_path_points() -> std::io::Result<()> {
    let workspace = workspace();
    assert!(defines(&workspace, "client::retry::Standard::succeeded")?);
    assert!(defines(&workspace, "client::call::Client::succeeded")?);
    assert!(defines(&workspace, "client::auth::select")?);
    assert!(defines(&workspace, "client::protocol::select")?);
    assert!(!defines(&workspace, "client::auth::succeeded")?);
    assert!(!defines(&workspace, "client::retry::succeeded")?);
    assert!(!defines(&workspace, "client::retry::Standard::select")?);
    assert!(!defines(&workspace, "client::retry::select")?);
    Ok(())
}

/// A method resolves through the type its `impl` is for, or the trait that
/// declares it, and not through another type in the same module.
#[test]
fn a_method_resolves_through_its_impl_owner() -> std::io::Result<()> {
    let workspace = workspace();
    assert!(defines(&workspace, "client::call::Client::prepare")?);
    assert!(defines(&workspace, "command::models::ModelSource::roots")?);
    assert!(!defines(&workspace, "client::call::Prepared::prepare")?);
    assert!(!defines(&workspace, "client::call::prepare")?);
    assert!(defines(&workspace, "command::spec::Invocation::run")?);
    Ok(())
}

/// An item re-exported by a `pub use` resolves where it is re-exported.
#[test]
fn a_reexported_item_resolves() -> std::io::Result<()> {
    let workspace = workspace();
    assert!(defines(
        &workspace,
        "cli_core::client_config::resolve_settings"
    )?);
    assert!(!defines(
        &workspace,
        "cli_core::client_config::settings::nope"
    )?);
    Ok(())
}

/// A path whose module, owner or item no longer exists does not resolve.
#[test]
fn a_stale_path_does_not_resolve() -> std::io::Result<()> {
    let workspace = workspace();
    assert!(!defines(&workspace, "client::no_such_module::select")?);
    assert!(!defines(
        &workspace,
        "client::retry::NoSuchType::succeeded"
    )?);
    assert!(!defines(
        &workspace,
        "client::retry::Standard::no_such_method"
    )?);
    assert!(!defines(
        &workspace,
        "client::retry::Standard::succeeded::more"
    )?);
    assert!(!defines(&workspace, "client")?);
    Ok(())
}

/// A workspace of one crate, `fixture`, with `files` under its `src`.
fn fixture(name: &str, files: &[(&str, &str)]) -> std::io::Result<PathBuf> {
    let root = std::env::temp_dir().join(format!("defines-{}-{name}", std::process::id()));
    for (file, source) in files {
        let path = root.join("crates/fixture/src").join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, source)?;
    }
    Ok(root)
}

/// A path that could mean two items fails with an error naming it.
#[test]
fn an_ambiguous_path_fails_naming_it() -> std::io::Result<()> {
    let two_files = fixture(
        "two-files",
        &[
            ("lib.rs", "mod a;\n"),
            ("a.rs", "pub fn f() {}\n"),
            ("a/mod.rs", "pub fn f() {}\n"),
        ],
    )?;
    let error = defines(&two_files, "fixture::a::f").err();
    let message = error.map(|error| error.to_string()).unwrap_or_default();
    assert!(message.contains("`fixture::a::f`"), "{message}");
    assert!(message.contains("module `a` has two files"), "{message}");

    let item_and_method = fixture(
        "item-and-method",
        &[
            (
                "lib.rs",
                "mod a;\npub struct a;\nimpl a {\n    pub fn f() {}\n}\n",
            ),
            ("a.rs", "pub fn f() {}\n"),
        ],
    )?;
    let error = defines(&item_and_method, "fixture::a::f").err();
    let message = error.map(|error| error.to_string()).unwrap_or_default();
    assert!(message.contains("`fixture::a::f`"), "{message}");
    assert!(message.contains("names an item and a method"), "{message}");
    std::fs::remove_dir_all(two_files)?;
    std::fs::remove_dir_all(item_and_method)
}
