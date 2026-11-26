use super::super::run;
use super::{Repo, TESTS};
use std::fs;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn a_gitattributes_line_that_hides_rust_diffs_hides_nothing() {
    let repo = Repo::new("attributes");
    repo.write(".gitattributes", "*.rs -diff\n");
    repo.edit(
        "src/tests.rs",
        "#[test]\nfn subtracts()",
        "#[test]\n#[ignore]\nfn subtracts()",
    );
    repo.commit("tidy");
    assert_eq!(
        repo.violations(),
        ["protected .gitattributes", "suppression src/tests.rs"]
    );
}

#[test]
fn diff_settings_in_the_repository_change_nothing() {
    let repo = Repo::new("settings");
    for (key, value) in [
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
        ("diff.relative", "true"),
    ] {
        repo.git(&["config", key, value]);
    }
    repo.edit("src/tests.rs", "        2,\n", "        1 + 1,\n");
    repo.edit(
        "src/tests.rs",
        "#[test]\nfn subtracts()",
        "#[test]\n#[ignore]\nfn subtracts()",
    );
    repo.commit("tidy");
    assert_eq!(
        repo.violations(),
        ["assert src/tests.rs", "suppression src/tests.rs"]
    );
}

#[test]
fn a_changed_operand_of_a_multi_line_assert_is_a_changed_assert() {
    let repo = Repo::new("operand");
    repo.edit("src/tests.rs", "        2,\n", "        1 + 1,\n");
    repo.commit("tidy");
    assert_eq!(repo.violations(), ["assert src/tests.rs"]);
}

#[test]
fn an_assert_removed_while_a_copy_stays_elsewhere_is_removed() {
    let repo = Repo::new("copy");
    repo.edit(
        "src/tests.rs",
        "fn subtracts() {\n    assert!(2 - 1 == 1);\n",
        "fn subtracts() {\n",
    );
    repo.commit("tidy");
    assert_eq!(repo.violations(), ["assert src/tests.rs"]);
}

#[test]
fn ignores_hidden_in_cfg_attr_spacing_or_a_shared_line_are_suppressions() {
    let repo = Repo::new("forms");
    repo.edit(
        "src/tests.rs",
        "#[test]\nfn adds()",
        "#[cfg_attr(all(), ignore)]\n#[test]\nfn adds()",
    );
    repo.edit(
        "src/tests.rs",
        "#[test]\nfn subtracts()",
        "#[test] # [ ignore ]\nfn subtracts()",
    );
    repo.edit(
        "src/lib.rs",
        "pub fn one()",
        "#[allow (dead_code)]\n#[should_panic]\npub fn one()",
    );
    repo.commit("tidy");
    let expected = strings(&[
        "suppression src/lib.rs",
        "suppression src/lib.rs",
        "suppression src/tests.rs",
        "suppression src/tests.rs",
    ]);
    assert_eq!(repo.violations(), expected);
}

#[test]
fn a_test_module_switched_off_by_cfg_is_a_suppression() {
    for (name, gate) in [("any", "#[cfg(any())]"), ("not-test", "#[cfg(not(test))]")] {
        let repo = Repo::new(name);
        repo.edit(
            "src/lib.rs",
            "#[cfg(test)]\nmod tests;",
            &format!("{gate}\nmod tests;"),
        );
        repo.commit("tidy");
        assert_eq!(repo.violations(), ["suppression src/lib.rs"], "{gate}");
    }
}

#[test]
fn renaming_tests_out_and_dropping_the_module_line_is_a_deletion() {
    let repo = Repo::new("rename");
    repo.git(&["mv", "src/tests.rs", "src/old_cases.rs"]);
    repo.edit("src/lib.rs", "\n#[cfg(test)]\nmod tests;\n", "\n");
    repo.commit("move cases");
    assert_eq!(
        repo.violations(),
        ["deleted-test src/tests.rs", "test-module src/lib.rs"]
    );
}

#[test]
fn tests_wrapped_in_a_comment_or_a_string_are_not_counted() {
    for (name, open, close) in [
        ("comment", "/*\n", "*/\n"),
        ("string", "const _: &str = r#\"\n", "\"#;\n"),
    ] {
        let repo = Repo::new(name);
        repo.edit(
            "src/tests.rs",
            "#[test]\nfn subtracts()",
            &format!("{open}#[test]\nfn subtracts()"),
        );
        repo.edit(
            "src/tests.rs",
            "fn subtracts_again",
            &format!("{close}#[test]\nfn subtracts_again"),
        );
        repo.commit("tidy");
        let found = repo.violations();
        assert!(
            found.contains(&"test-count .".to_owned()),
            "{name}: {found:?}"
        );
    }
}

#[test]
fn a_broad_or_unrelated_trailer_retires_nothing() {
    let repo = Repo::new("trailers");
    repo.write("README", "x\n");
    repo.commit("notes\n\nRetires: **\nRetires: src/tests.rs\nAsserts-changed: src/tests.rs");
    fs::remove_file(repo.root.join("src/tests.rs")).unwrap_or_default();
    repo.edit("src/lib.rs", "\n#[cfg(test)]\nmod tests;\n", "\n");
    repo.commit("drop the old cases\n\nRetires: **");
    assert_eq!(
        repo.violations(),
        [
            "deleted-test src/tests.rs",
            "test-count .",
            "test-module src/lib.rs"
        ]
    );
}

#[test]
fn a_comment_or_url_beside_an_allow_is_not_a_justification_unless_it_is_a_comment() {
    let repo = Repo::new("url");
    repo.edit(
        "src/lib.rs",
        "pub fn one()",
        "#[allow(dead_code)] const U: &str = \"https://e.com\";\npub fn one()",
    );
    repo.commit("tidy");
    assert_eq!(repo.violations(), ["suppression src/lib.rs"]);
}

#[test]
fn cargo_configuration_is_protected() {
    let repo = Repo::new("cargo");
    repo.write(
        ".cargo/config.toml",
        "[alias]\nxtask = \"run --package other --\"\n",
    );
    repo.commit("tidy");
    assert_eq!(repo.violations(), ["protected .cargo/config.toml"]);
}

#[test]
fn honest_changes_pass_and_a_protected_change_fails_the_run() {
    let repo = Repo::new("honest");
    let moved = TESTS.replacen(
        "#[test]\nfn adds()",
        "// needs the network in CI\n#[test]\n#[ignore]\nfn adds()",
        1,
    );
    repo.write(
        "src/tests.rs",
        &format!(
            "#[test]\n#[ignore = \"slow\"]\nfn new_case() {{\n    assert!(true);\n}}\n\n{moved}"
        ),
    );
    repo.edit(
        "src/tests.rs",
        "    assert_eq!(\n        1 + 1,\n        2,\n    );",
        "    assert_eq!(1 + 1, 2);",
    );
    repo.commit("tidy");
    repo.edit(
        "src/tests.rs",
        "    assert!(2 - 1 == 1);\n}\n\n#[test]\nfn subtracts_again",
        "    assert!(2 > 1);\n}\n\n#[test]\nfn subtracts_again",
    );
    repo.commit("sharpen\n\nAsserts-changed: src/tests.rs");
    assert_eq!(repo.violations(), Vec::<String>::new());
    let clean = run(&repo.root, &strings(&["--base", &repo.base]));
    assert!(clean.is_ok(), "{clean:?}");
    repo.write("tests/oracle/expected.txt", "1\n");
    repo.commit("oracle");
    let error = run(&repo.root, &strings(&["--base", &repo.base])).err();
    let error = error.map(|error| error.to_string()).unwrap_or_default();
    assert!(error.contains("1 violation(s)"), "{error}");
}

#[test]
fn a_test_module_or_function_gated_on_a_cfg_off_in_ci_is_a_suppression() {
    let cases = [
        (
            "src/lib.rs",
            "#[cfg(test)]\nmod tests;",
            "#[cfg(miri)]\nmod tests;",
        ),
        (
            "src/tests.rs",
            "#[test]\nfn adds()",
            "#[test]\n#[cfg(all(test, windows))]\nfn adds()",
        ),
        (
            "src/tests.rs",
            "#[test]\nfn adds()",
            "#![cfg(miri)]\n#[test]\nfn adds()",
        ),
    ];
    for (path, from, to) in cases {
        let repo = Repo::new("gate");
        repo.edit(path, from, to);
        repo.commit("tidy");
        assert_eq!(repo.violations(), [format!("suppression {path}")], "{to}");
    }
    let repo = Repo::new("gate-ok");
    repo.edit(
        "src/tests.rs",
        "#[test]\nfn adds()",
        "#[cfg(unix)]\n#[test]\nfn adds()",
    );
    repo.edit(
        "src/lib.rs",
        "pub fn one()",
        "#[cfg(windows)]\nmod win;\npub fn one()",
    );
    repo.write("src/win.rs", "pub fn w() {}\n");
    repo.commit("platform code");
    assert_eq!(repo.violations(), Vec::<String>::new());
}

#[test]
fn removing_a_child_module_that_holds_tests_is_a_test_module_removal() {
    let mut repo = Repo::new("child");
    repo.rebase(&[
        (
            "src/lib.rs",
            &format!("mod extra;\nmod inline;\nmod plain;\n{}", super::LIB),
        ),
        ("src/extra.rs", "#[cfg(test)]\nmod tests;\n"),
        ("src/extra/tests.rs", "#[test]\nfn e() {}\n"),
        ("src/inline.rs", "#[test]\nfn i() {}\n"),
        ("src/plain.rs", "pub fn p() {}\n"),
    ]);
    repo.edit("src/lib.rs", "mod extra;\nmod inline;\nmod plain;\n", "");
    repo.commit("tidy");
    assert_eq!(
        repo.violations(),
        ["test-module src/lib.rs", "test-module src/lib.rs"]
    );
}

#[test]
fn a_path_attribute_retargeting_a_test_module_is_a_suppression() {
    let repo = Repo::new("path");
    repo.edit(
        "src/lib.rs",
        "mod tests;",
        "#[path = \"tests_empty.rs\"]\nmod tests;",
    );
    repo.write("src/tests_empty.rs", "\n");
    repo.commit("tidy");
    assert_eq!(repo.violations(), ["suppression src/lib.rs"]);
    let same = Repo::new("path-same");
    same.edit(
        "src/lib.rs",
        "mod tests;",
        "#[path = \"tests.rs\"]\nmod tests;",
    );
    same.commit("tidy");
    assert_eq!(same.violations(), Vec::<String>::new());
}

#[test]
fn a_macro_or_import_shadowing_assert_is_an_escape_unless_a_trailer_says_why() {
    let shadows = [
        "macro_rules! assert_eq { ($($t:tt)*) => {}; }\n",
        "macro_rules! assert { ($($t:tt)*) => {}; }\n",
        "use crate::one as assert_ne;\n",
    ];
    for shadow in shadows {
        let repo = Repo::new("shadow");
        repo.write("src/tests.rs", &format!("{shadow}{TESTS}"));
        repo.commit("tidy");
        assert_eq!(repo.violations(), ["suppression src/tests.rs"], "{shadow}");
        repo.write("README", "x\n");
        repo.edit("src/tests.rs", "fn adds", "fn adds_up");
        repo.commit("rename\n\nAsserts-changed: src/tests.rs");
        assert_eq!(repo.violations(), Vec::<String>::new(), "{shadow}");
    }
}

#[test]
fn an_exit_or_a_passing_return_before_the_checks_of_a_test_is_an_escape() {
    let escapes = [
        "fn adds() {\n    std::process::exit(0);",
        "fn adds() {\n    std::process::abort();",
        "fn adds() {\n    if one() == 1 {\n        return;\n    }",
    ];
    for escape in escapes {
        let repo = Repo::new("escape");
        repo.edit("src/tests.rs", "fn adds() {", escape);
        repo.commit("tidy");
        assert_eq!(repo.violations(), ["suppression src/tests.rs"], "{escape}");
    }
    let failing = Repo::new("escape-err");
    failing.edit(
        "src/tests.rs",
        "fn adds() {",
        concat!(
            "fn adds() -> Result<(), String> {\n",
            "    if one() != 1 {\n        return Err(\"x\".into());\n    }",
        ),
    );
    failing.edit(
        "src/tests.rs",
        "        2,\n    );\n}",
        "        2,\n    );\n    Ok(())\n}",
    );
    failing.commit("tidy");
    assert_eq!(failing.violations(), Vec::<String>::new());
}

#[test]
fn a_test_attribute_made_conditional_is_a_drop_in_the_count() {
    let repo = Repo::new("cfg-attr-test");
    repo.edit(
        "src/tests.rs",
        "#[test]\nfn adds()",
        "#[cfg_attr(not(miri), test)]\nfn adds()",
    );
    repo.commit("tidy");
    assert_eq!(repo.violations(), ["test-count ."]);
}
