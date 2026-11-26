use super::attrs::{self, Source};
use super::diff::{self, Change};
use super::{lex, options, rules};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn change(status: char, old: Option<&str>, new: Option<&str>) -> Change {
    Change {
        status,
        old: old.map(str::to_owned),
        new: new.map(str::to_owned),
    }
}

#[test]
fn options_need_a_base_and_default_the_head() {
    let parsed = options(&strings(&["--base", "v2"])).ok();
    assert_eq!(parsed, Some(("v2".to_owned(), "HEAD".to_owned())));
    let both = options(&strings(&["--head", "h", "--base", "b"])).ok();
    assert_eq!(both, Some(("b".to_owned(), "h".to_owned())));
    let bad: [&[&str]; 5] = [
        &[],
        &["--head", "h"],
        &["--base"],
        &["--base", "a", "--base", "b"],
        &["x"],
    ];
    for bad in bad {
        assert!(options(&strings(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn globs_match_within_and_across_segments() {
    let cases = [
        ("xtask/**", "xtask/src/guard.rs", true),
        ("**/Cargo.toml", "Cargo.toml", true),
        ("**/Cargo.toml", "crates/a/Cargo.toml", true),
        ("**/.cargo/**", ".cargo/config.toml", true),
        ("tests/**/conftest.py", "tests/conftest.py", true),
        ("tests/*.py", "tests/awswire.py", true),
        ("clippy.toml", "crates/clippy.toml", false),
        ("tests/parity/**", "tests/paritycheck/a", false),
        ("tests/*.py", "tests/a/b.py", false),
    ];
    for (pattern, path, expected) in cases {
        assert_eq!(diff::matches(pattern, path), expected, "{pattern} {path}");
    }
    assert_eq!(
        diff::patterns("# c\n\na/** # why\nb\n"),
        strings(&["a/**", "b"])
    );
}

#[test]
fn the_committed_list_covers_what_redirects_or_silences_an_oracle() {
    let list = diff::patterns(include_str!("../../guard/protected.txt"));
    for path in [
        ".cargo/config.toml",
        "crates/a/.cargo/config.toml",
        "rust-toolchain.toml",
        "crates/a/clippy.toml",
        "crates/a/.rustfmt.toml",
        ".gitattributes",
        "crates/a/.gitattributes",
        "tests/blackbox/tests/s3/conftest.py",
        "tests/blackbox/tests/pytest.ini",
        "tests/diff/capture.py",
        "crates/a/Cargo.toml",
        "xtask/src/guard.rs",
    ] {
        assert!(
            list.iter().any(|pattern| diff::matches(pattern, path)),
            "{path}"
        );
    }
    assert!(
        !list
            .iter()
            .any(|pattern| diff::matches(pattern, "crates/a/src/lib.rs"))
    );
}

#[test]
fn name_status_reads_renames_copies_and_deletions() {
    let text = "M\0a.rs\0R087\0old.rs\0new.rs\0D\0gone.rs\0A\0added.rs\0C100\0src.rs\0copy.rs\0";
    assert_eq!(
        diff::name_status(text),
        vec![
            change('M', Some("a.rs"), Some("a.rs")),
            change('R', Some("old.rs"), Some("new.rs")),
            change('D', Some("gone.rs"), None),
            change('A', None, Some("added.rs")),
            change('C', None, Some("copy.rs")),
        ]
    );
    let renamed = change('R', Some("old.rs"), Some("new.rs"));
    assert_eq!(renamed.paths().collect::<Vec<_>>(), ["old.rs", "new.rs"]);
}

#[test]
fn unified_diffs_number_lines_and_keep_header_lookalikes() {
    let text = "diff --git a/x.sql b/x.sql\nindex 1..2 100644\n--- a/x.sql\n+++ b/x.sql\n\
                @@ -3,2 +3 @@ fn\n--- comment\n-two\n+three\n@@ -9,0 +9,2 @@\n+a\n+b\n\
                diff --git a/n.rs b/n.rs\nnew file mode 100644\n--- /dev/null\n+++ b/n.rs\n\
                @@ -0,0 +1 @@\n+fn n() {}\n";
    let files = diff::unified(text);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].old.as_deref(), Some("x.sql"));
    assert_eq!(
        files[0].removed,
        [(3, "-- comment".to_owned()), (4, "two".to_owned())]
    );
    assert_eq!(
        files[0].added,
        [
            (3, "three".to_owned()),
            (9, "a".to_owned()),
            (10, "b".to_owned())
        ]
    );
    assert_eq!(
        (files[1].old.as_deref(), files[1].new.as_deref()),
        (None, Some("n.rs"))
    );
    assert!(rules::unparsed(&files).is_none());
    let unprefixed = diff::unified("diff --git x x\n--- x\n+++ x\n@@ -1 +1 @@\n-a\n+b\n");
    assert!(rules::unparsed(&unprefixed).is_some());
}

#[test]
fn batch_replies_and_logs_are_read_by_spec_and_by_commit() {
    let specs = strings(&["h:a", "h:gone", "h:b"]);
    let reply = b"1 blob 3\nabc\nh:gone missing\n2 blob 0\n\n";
    let blobs = diff::batch(&specs, reply).unwrap_or_default();
    assert_eq!(blobs.get("h:a").map(String::as_str), Some("abc"));
    assert_eq!(blobs.get("h:b").map(String::as_str), Some(""));
    assert!(!blobs.contains_key("h:gone"));
    assert!(diff::batch(&specs, b"1 blob 9\nabc\n").is_err());
    let log = "\u{1}drop it\n\nRetires: a.rs, b.rs\n\u{2}Retires: a.rs, b.rs\n\u{2}\0\na.rs\0c.rs\0\
               \u{1}body Retires: x\n\u{2}\u{2}\0\nd.rs\0";
    let commits = diff::commits(log);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].retires, strings(&["a.rs", "b.rs"]));
    assert_eq!(commits[0].paths, strings(&["a.rs", "c.rs"]));
    assert!(commits[1].retires.is_empty());
    assert_eq!(commits[1].paths, strings(&["d.rs"]));
}

#[test]
fn crates_are_the_deepest_manifest_directory() {
    let crates = diff::crate_directories(
        [
            "Cargo.toml",
            "crates/a/Cargo.toml",
            "crates/a/inner/Cargo.toml",
            "crates/a/src/x.rs",
        ]
        .into_iter(),
    );
    assert_eq!(crates, strings(&["", "crates/a", "crates/a/inner"]));
    assert_eq!(
        diff::crate_of(&crates, "crates/a/inner/src/t.rs"),
        "crates/a/inner"
    );
    assert_eq!(diff::crate_of(&crates, "crates/a/src/x.rs"), "crates/a");
    assert_eq!(diff::crate_of(&crates, "tests/t.rs"), ".");
}

#[test]
fn test_files_are_tests_rs_testing_rs_and_tests_directories() {
    for path in [
        "src/tests.rs",
        "a/testing.rs",
        "tests/x.py",
        "crates/a/tests/it.rs",
    ] {
        assert!(diff::is_test_file(path), "{path}");
    }
    for path in [
        "src/lib.rs",
        "src/test.rs",
        "attests/x.rs",
        "src/tests_util.rs",
    ] {
        assert!(!diff::is_test_file(path), "{path}");
    }
}

#[test]
fn the_mask_blanks_literals_and_comments_and_keeps_offsets() {
    let source = concat!(
        "let a = \"#[ignore] \\\" x\"; // #[ignore]\n",
        "let b = r#\"#[test] \"quoted\" \"#; /* #[test] /* nested */ */\n",
        "let c = '['; let d: &'static str = br\"]\";\n",
        "fn e<'a>(x: &'a str) {}\n",
    );
    let masked = lex::mask(source);
    assert_eq!(masked.len(), source.len());
    assert_eq!(masked.lines().count(), 4);
    assert!(!masked.contains("ignore") && !masked.contains("test"));
    assert!(!masked.contains("nested") && !masked.contains("quoted"));
    assert!(masked.contains("'static") && masked.contains("<'a>") && masked.contains("&'a str"));
    assert_eq!(masked.matches('[').count(), 0, "{masked}");
}

fn kinds(text: &str) -> Vec<(&'static str, bool)> {
    attrs::suppressions(&Source::rust(text))
        .into_iter()
        .map(|found| (found.kind, found.reasoned))
        .collect()
}

#[test]
fn suppressions_are_found_anywhere_and_inside_cfg_attr() {
    let cases: [(&str, &[(&str, bool)]); 18] = [
        ("#[ignore]", &[("ignore", false)]),
        ("    # [ ignore ]", &[("ignore", false)]),
        ("#[ignore = \"slow\"]", &[("ignore", true)]),
        ("#[ignore = \"  \"]", &[("ignore", false)]),
        ("#[test] #[ignore] fn a() {}", &[("ignore", false)]),
        ("#![allow(dead_code)]", &[("allow", false)]),
        ("#[allow (dead_code)]", &[("allow", false)]),
        (
            "#[expect(\n  clippy::panic,\n  reason = \"a test\"\n)]",
            &[("expect", true)],
        ),
        ("#[cfg_attr(miri, ignore)]", &[("ignore", false)]),
        (
            "#[cfg_attr(all(), allow(x, reason = \"r\"), ignore)]",
            &[("allow", true), ("ignore", false)],
        ),
        ("#[should_panic]", &[("should_panic", false)]),
        (
            "#[should_panic(expected = \"boom\")]",
            &[("should_panic", true)],
        ),
        ("#[cfg(any())] mod tests;", &[("cfg-off", false)]),
        ("#[cfg(not(test))]", &[("cfg-off", false)]),
        ("#[cfg(miri)]", &[("cfg", false)]),
        ("#[cfg(all(test, windows))]", &[("cfg", false)]),
        ("#[cfg(feature = \"x\")]", &[("cfg", false)]),
        ("#[path = \"t.rs\"]", &[("path", false)]),
    ];
    for (text, expected) in cases {
        assert_eq!(kinds(text), expected, "{text}");
    }
    for quiet in [
        "let s = \"#[ignore]\";",
        "// #[allow(x)]",
        "#[ignored_attr]",
        "#[allowed(x)]",
        "#[cfg(test)]",
        "#[cfg(all(test, not(miri)))]",
        "#[cfg(unix)]",
    ] {
        assert_eq!(kinds(quiet), [], "{quiet}");
    }
}

#[test]
fn cfg_predicates_are_known_only_when_they_rest_on_test_and_literals() {
    let cases = [
        ("any()", Some(false)),
        ("all()", Some(true)),
        ("not(all())", Some(false)),
        ("not(test)", Some(false)),
        ("false", Some(false)),
        ("any(miri, all())", Some(true)),
        ("all(miri, any())", Some(false)),
        ("miri", None),
        ("any(miri, unix)", None),
        ("nothing", None),
    ];
    for (predicate, expected) in cases {
        assert_eq!(attrs::evaluate(predicate), expected, "{predicate}");
    }
}

#[test]
fn a_suppression_is_justified_by_its_reason_or_a_comment_beside_or_above_it() {
    let justified = |text: &str| {
        let source = Source::rust(text);
        let found = attrs::suppressions(&source);
        assert_eq!(found.len(), 1, "{text}");
        found.iter().all(|found| rules::justified(&source, found))
    };
    for text in [
        "// the upstream suite skips it\n#[ignore]",
        "// flaky upstream\n#[test]\n#[ignore]\nfn a() {}",
        "#[ignore] // needs network",
        "#[ignore = \"needs network\"]",
        "#![allow(dead_code, reason = \"fixture\")]",
    ] {
        assert!(justified(text), "{text}");
    }
    for text in [
        "#[ignore]",
        "/// Documents the item.\n#[allow(dead_code)]",
        "//\n#[allow(dead_code)]",
        "let x = 1; // about x\n#[allow(dead_code)]",
        "#[allow(dead_code)] const U: &str = \"https://example.com\";",
        "#[allow(dead_code)]\n// after it",
    ] {
        assert!(!justified(text), "{text}");
    }
}

#[test]
fn test_functions_are_counted_outside_comments_and_literals() {
    let text = concat!(
        "#[test]\nfn a() {}\n#[tokio::test(flavor = \"current_thread\")]\nasync fn b() {}\n",
        "#[test_case(1)]\nfn c(x: u8) {}\n/* #[test]\nfn d() {} */\n",
        "const E: &str = \"#[test]\";\n#[testing]\nfn f() {}\n",
    );
    assert_eq!(attrs::test_count(&Source::rust(text)), 3);
}

#[test]
fn assert_statements_span_their_arguments() {
    let text = concat!(
        "fn t() {\n    assert_eq!(\n        a,\n        [1, 2],\n    );\n",
        "    debug_assert!(b);\n}\n",
    );
    let found = attrs::statements(&Source::rust(text));
    let spans: Vec<_> = found.iter().map(|s| (s.first_line, s.last_line)).collect();
    assert_eq!(spans, [(2, 5), (6, 6)]);
    assert_eq!(found[0].text, "assert_eq!(a,[1,2])");
    let python = attrs::statements(&Source::plain(
        "    assert x == 1\n    self.assertEqual(\n  a, b)\n",
    ));
    let spans: Vec<_> = python.iter().map(|s| (s.first_line, s.last_line)).collect();
    assert_eq!(spans, [(1, 1), (2, 3)]);
}

#[test]
fn a_changed_assert_is_one_whose_statement_is_gone_at_head() {
    let base = "assert_eq!(\n    a,\n    b,\n);\nassert!(c);\nassert!(c);\n";
    let check = |head: &str, removed: &[(usize, &str)]| {
        let removed: Vec<_> = removed
            .iter()
            .map(|(line, text)| (*line, (*text).to_owned()))
            .collect();
        rules::changed_asserts(&Source::rust(base), &Source::rust(head), &removed)
    };
    let none = Vec::<String>::new();
    let operand = "assert_eq!(\n    a,\n    a,\n);\nassert!(c);\nassert!(c);\n";
    assert_eq!(check(operand, &[(3, "    b,")]), ["1"]);
    let reflowed = "assert_eq!(a, b);\nassert!(c);\nassert!(c);\n";
    assert_eq!(check(reflowed, &[(2, "    a,"), (3, "    b,")]), none);
    let one_copy_left = "assert_eq!(\n    a,\n    b,\n);\nassert!(c);\n";
    assert_eq!(check(one_copy_left, &[(6, "assert!(c);")]), ["6"]);
    let moved = "assert!(c);\nassert_eq!(\n    a,\n    b,\n);\nassert!(c);\n";
    assert_eq!(check(moved, &[(5, "assert!(c);")]), none);
}

#[test]
fn the_ci_build_knows_its_platform_and_not_its_features() {
    let cases = [
        ("unix", Some(true)),
        ("all(test, not(miri))", Some(true)),
        ("target_os = \"linux\"", Some(true)),
        ("target_os = \"macos\"", Some(false)),
        ("all(test, windows)", Some(false)),
        ("feature = \"x\"", None),
    ];
    for (predicate, expected) in cases {
        assert_eq!(attrs::evaluate_ci(predicate), expected, "{predicate}");
    }
}

#[test]
fn module_targets_follow_path_attributes_and_hold_tests_below_them() {
    assert_eq!(rules::module_target("src/lib.rs", "a", None), "src/a.rs");
    assert_eq!(rules::module_target("src/x.rs", "a", None), "src/x/a.rs");
    assert_eq!(
        rules::module_target("tests/it.rs", "t", Some("it/tests.rs")),
        "tests/it/tests.rs"
    );
    let files = strings(&["src/a/b/tests.rs", "src/c.rs"]);
    let sources = [("src/c.rs".to_owned(), "#[test]\nfn c() {}\n".to_owned())].into();
    let none = std::collections::BTreeMap::new();
    assert!(rules::holds_tests(&files, &none, "src/a.rs"));
    assert!(rules::holds_tests(&files, &sources, "src/c.rs"));
    assert!(!rules::holds_tests(&files, &none, "src/c.rs"));
    let modules = super::gates::modules(&Source::rust(
        "#[path = \"t.rs\"]\npub(crate) mod a;\nmod b {}\nmod c;\n",
    ));
    let found: Vec<_> = modules
        .iter()
        .map(|m| (m.line, m.name.as_str(), m.path.as_deref()))
        .collect();
    assert_eq!(found, [(2, "a", Some("t.rs")), (4, "c", None)]);
}

#[test]
fn escapes_are_shadowing_macros_exits_and_passing_returns_before_checks() {
    let text = concat!(
        "macro_rules! assert_eq { ($($t:tt)*) => {}; }\n",
        "use crate::noop as assert;\nuse std::fmt;\n",
        "#[test]\nfn a() {\n    std::process::exit(0);\n    if x { return; }\n    assert!(y);\n}\n",
        "#[test]\nfn b() -> Result<(), E> {\n    let Some(v) = w else { return Err(e) };\n",
        "    let f = |x| x.exit(1);\n    assert!(v);\n    return Ok(());\n}\n",
        "fn helper() { std::process::exit(1); return; }\n",
    );
    let found: Vec<_> = super::gates::escapes(&Source::rust(text), &[])
        .into_iter()
        .map(|found| (found.kind, found.first_line, found.test_item))
        .collect();
    assert_eq!(
        found,
        [
            ("shadow", 1, false),
            ("shadow", 2, false),
            ("exit", 6, true),
            ("exit", 17, false),
            ("return", 7, true),
        ]
    );
    let names = strings(&["assert_parses"]);
    let custom = super::gates::escapes(
        &Source::rust("macro_rules! assert_parses { () => {}; }\n"),
        &names,
    );
    assert_eq!(custom.len(), 1);
}
