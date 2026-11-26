use super::defines;
use std::path::{Path, PathBuf};

/// Asserts that each path under `workspace` resolves to `Ok(defined)`,
/// reporting every one that does not.
fn check(workspace: &Path, cases: &[(&str, bool)]) {
    let mut wrong = Vec::new();
    for &(path, defined) in cases {
        let found = defines(workspace, path);
        if !matches!(found, Ok(found) if found == defined) {
            wrong.push(format!("{path}: {found:?}"));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `succeeded` and `select` are each defined twice in the client; a path
/// resolves only to the module and owner that define it.
#[test]
fn a_name_defined_twice_resolves_only_where_its_path_points() {
    check(
        &workspace(),
        &[
            ("client::retry::Standard::succeeded", true),
            ("client::call::Client::succeeded", true),
            ("client::auth::select", true),
            ("client::protocol::select", true),
            ("client::auth::succeeded", false),
            ("client::retry::succeeded", false),
            ("client::retry::Standard::select", false),
            ("client::retry::select", false),
        ],
    );
}

/// A method resolves through the type its `impl` is for, or the trait that
/// declares it, and not through another type in the same module.
#[test]
fn a_method_resolves_through_its_impl_owner() {
    check(
        &workspace(),
        &[
            ("client::call::Client::prepare", true),
            ("command::models::ModelSource::roots", true),
            ("command::spec::Invocation::run", true),
            ("client::call::Prepared::prepare", false),
            ("client::call::prepare", false),
        ],
    );
}

/// An item re-exported by a `pub use` resolves where it is re-exported;
/// stale paths and doc examples (awscli-argparse's `bind` shows types it
/// does not define) do not resolve.
#[test]
fn a_stale_path_or_doc_example_does_not_resolve() {
    check(
        &workspace(),
        &[
            ("cli_core::client_config::resolve_settings", true),
            ("cli_core::client_config::settings::nope", false),
            ("client::no_such_module::select", false),
            ("client::retry::NoSuchType::succeeded", false),
            ("client::retry::Standard::no_such_method", false),
            ("client::retry::Standard::succeeded::more", false),
            ("client", false),
            ("awscli_argparse::bind::ListArgs", false),
            ("awscli_argparse::bind::Bad", false),
        ],
    );
}

/// A workspace of one crate, `fixture`, with `files` under its `src`,
/// removed when dropped.
struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str, files: &[(&str, &str)]) -> std::io::Result<Self> {
        let root = std::env::temp_dir().join(format!("defines-{}-{name}", std::process::id()));
        let fixture = Self(root);
        for (file, source) in files {
            let path = fixture.0.join("crates/fixture/src").join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, source)?;
        }
        Ok(fixture)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A path that could mean two items fails with an error naming it: a
/// module with two files, a name that is a module and a type (not valid
/// Rust, but still refused), and a method two trait impls define.
#[test]
fn an_ambiguous_path_fails_naming_it() -> std::io::Result<()> {
    let f_in = |owner: &str| format!("impl {owner} {{\n    fn f() {{}}\n}}\n");
    let cases = [
        ("mod a;\n".into(), "a/mod.rs", "two files"),
        (
            "mod a;\n".to_owned() + &f_in("a"),
            "x.rs",
            "item and a method",
        ),
        (f_in("A for a") + &f_in("B for a"), "x.rs", "2 trait impls"),
    ];
    for (index, (lib, extra, why)) in cases.into_iter().enumerate() {
        let name = format!("ambiguous-{index}");
        let files = [("lib.rs", &*lib), ("a.rs", "pub fn f() {}"), (extra, "")];
        let fixture = Fixture::new(&name, &files)?;
        let message = defines(&fixture.0, "fixture::a::f").map_err(|error| error.to_string());
        let message = message.err().unwrap_or_default();
        assert!(
            message.contains("Basis path `fixture::a::f` is ambiguous"),
            "{name}: {message}"
        );
        assert!(message.contains(why), "{name}: {message}");
    }
    Ok(())
}

/// Paths that a looser reading would pass, each a way a removed, moved or
/// unreachable item could still look defined, and the near misses that
/// must still resolve.
#[test]
fn an_item_only_mentioned_or_out_of_the_tree_does_not_resolve() -> std::io::Result<()> {
    let lib = "\
//! The crate keeps fn in_crate_docs only in prose.
mod a;
// mod commented;
pub use a::{kept as renamed, gone};
use a::kept as private;
pub struct C;
impl C {
    /// Unlike the fn retired, this one stays.
    pub fn inherent() {
        fn nested() {}
    }
}
impl Shown for C {
    fn shown() {}
}
pub fn takes(a: impl Foo) {
    fn inner() {}
}
pub fn real() {} // fn trailing
";
    let a = "\
/// Replaces the old fn moved_away, now in another module.
// fn commented_out() {}
/// ```
/// pub struct Example;
/// ```
pub fn kept() {}
";
    let fixture = Fixture::new(
        "evasions",
        &[
            ("lib.rs", lib),
            ("a.rs", a),
            ("b.rs", "pub fn moved_away() {}\npub fn gone() {}\n"),
            ("orphan.rs", "pub fn orphaned() {}\n"),
            ("commented.rs", "pub fn hidden() {}\n"),
        ],
    )?;
    let cases = [
        ("fixture::in_crate_docs", false),
        ("fixture::a::moved_away", false),
        ("fixture::a::commented_out", false),
        ("fixture::a::Example", false),
        ("fixture::C::retired", false),
        ("fixture::C::nested", false),
        ("fixture::Foo::inner", false),
        ("fixture::trailing", false),
        ("fixture::orphan::orphaned", false),
        ("fixture::commented::hidden", false),
        ("fixture::gone", false),
        ("fixture::private", false),
        ("fixture::b::moved_away", false),
        ("fixture::renamed", true),
        ("fixture::a::kept", true),
        ("fixture::C::inherent", true),
        ("fixture::C::shown", true),
        ("fixture::takes", true),
        ("fixture::real", true),
    ];
    check(&fixture.0, &cases);
    Ok(())
}
