use super::diff::{self, Change, FileLines};
use super::rules::{self, Trailers};
use super::{Inputs, options};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

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

fn lines(path: &str, removed: &[(usize, &str)], added: &[(usize, &str)]) -> FileLines {
    let owned = |items: &[(usize, &str)]| {
        items
            .iter()
            .map(|(line, text)| (*line, (*text).to_owned()))
            .collect()
    };
    FileLines {
        old: Some(path.to_owned()),
        new: Some(path.to_owned()),
        removed: owned(removed),
        added: owned(added),
    }
}

fn rule_names(inputs: &Inputs) -> Vec<String> {
    rules::check(inputs)
        .iter()
        .map(|violation| {
            let words: Vec<_> = violation
                .split_whitespace()
                .take(3)
                .map(|word| word.trim_end_matches(':'))
                .collect();
            words.join(": ")
        })
        .collect()
}

#[test]
fn options_need_a_base_and_default_the_head() {
    let parsed = options(&strings(&["--base", "v2"])).ok();
    assert_eq!(parsed, Some(("v2".to_owned(), "HEAD".to_owned())));
    let both = options(&strings(&["--head", "h", "--base", "b"])).ok();
    assert_eq!(both, Some(("b".to_owned(), "h".to_owned())));
    for bad in [
        &[][..],
        &["--head", "h"],
        &["--base"],
        &["--base", "a", "--base", "b"],
        &["x"],
    ] {
        assert!(options(&strings(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn globs_match_within_and_across_segments() {
    assert!(diff::matches("xtask/**", "xtask/src/guard.rs"));
    assert!(diff::matches("**/Cargo.toml", "Cargo.toml"));
    assert!(diff::matches("**/Cargo.toml", "crates/a/Cargo.toml"));
    assert!(diff::matches(
        "tests/diff/corpus/*.jsonl",
        "tests/diff/corpus/s3.jsonl"
    ));
    assert!(diff::matches("clippy.toml", "clippy.toml"));
    assert!(!diff::matches("clippy.toml", "crates/clippy.toml"));
    assert!(!diff::matches("tests/parity/**", "tests/paritycheck/a"));
    assert!(!diff::matches(
        "tests/diff/corpus/*.jsonl",
        "tests/diff/corpus/a/b.jsonl"
    ));
    assert_eq!(
        diff::patterns("# c\n\na/** # why\nb\n"),
        strings(&["a/**", "b"])
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
    assert_eq!(files[1].added, [(1, "fn n() {}".to_owned())]);
}

#[test]
fn tests_are_counted_in_the_deepest_crate() {
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
    let counts = [
        ("crates/a/src/x.rs", 2),
        ("crates/a/inner/t.rs", 3),
        ("tests/t.rs", 1),
    ]
    .map(|(path, count)| (path.to_owned(), count));
    let totals = diff::per_crate(&crates, counts.into_iter());
    let expected: BTreeMap<_, _> = [(".", 1), ("crates/a", 2), ("crates/a/inner", 3)]
        .map(|(k, v)| (k.to_owned(), v))
        .into();
    assert_eq!(totals, expected);
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
fn protected_paths_are_reported_with_the_pattern() {
    let inputs = Inputs {
        protected: strings(&["tests/parity/**", "**/Cargo.toml"]),
        changes: vec![
            change(
                'M',
                Some("tests/parity/cases.json"),
                Some("tests/parity/cases.json"),
            ),
            change(
                'R',
                Some("crates/a/Cargo.toml"),
                Some("crates/b/Cargo.toml"),
            ),
            change(
                'M',
                Some("crates/a/src/lib.rs"),
                Some("crates/a/src/lib.rs"),
            ),
        ],
        ..Inputs::default()
    };
    let violations = rules::check(&inputs);
    assert_eq!(violations.len(), 3, "{violations:?}");
    assert!(violations[0].starts_with("guard: protected: tests/parity/cases.json (M)"));
    assert!(violations[0].contains("`tests/parity/**`") && violations[0].contains("approved"));
    assert!(violations[2].starts_with("guard: protected: crates/b/Cargo.toml (R)"));
}

#[test]
fn suppressions_are_recognised_outer_inner_and_not_in_text() {
    assert_eq!(rules::suppression("    #[ignore]"), Some("ignore"));
    assert_eq!(rules::suppression(r#"#[ignore = "slow"]"#), Some("ignore"));
    assert_eq!(rules::suppression("#![allow(dead_code)]"), Some("allow"));
    assert_eq!(rules::suppression("#[expect("), Some("expect"));
    assert_eq!(rules::suppression("#[ignored_attr]"), None);
    assert_eq!(rules::suppression("#[allowed(x)]"), None);
    assert_eq!(rules::suppression(r##"let s = "#[ignore]";"##), None);
}

#[test]
fn a_suppression_is_justified_by_a_comment_or_a_reason() {
    let cases: [(&[&str], usize, bool); 11] = [
        (&["// the upstream suite skips it", "#[ignore]"], 1, true),
        (&["#[ignore] // needs network"], 0, true),
        (&[r#"#[ignore = "needs network"]"#], 0, true),
        (
            &[
                "#[expect(",
                "    clippy::panic,",
                r#"    reason = "a test""#,
                ")]",
            ],
            0,
            true,
        ),
        (&[r#"#![allow(dead_code, reason = "fixture")]"#], 0, true),
        (&["#[ignore]"], 0, false),
        (
            &["/// Documents the item.", "#[allow(dead_code)]"],
            1,
            false,
        ),
        (&["//", "#[allow(dead_code)]"], 1, false),
        (&[r#"#[ignore = ""]"#], 0, false),
        (&[r#"#[allow(dead_code, reason = "")]"#], 0, false),
        (&["#[allow(dead_code)]", "// after it"], 0, false),
    ];
    for (source, index, expected) in cases {
        let kind = rules::suppression(source[index]).unwrap_or_default();
        assert_eq!(
            rules::justified(source, index, kind),
            expected,
            "{source:?}"
        );
    }
}

#[test]
fn unjustified_added_suppressions_are_violations() {
    let source =
        "fn a() {}\n#[ignore]\n#[test]\nfn b() {}\n// why\n#[allow(dead_code)]\nfn c() {}\n";
    let inputs = Inputs {
        lines: vec![lines(
            "src/lib.rs",
            &[],
            &[(2, "#[ignore]"), (6, "#[allow(dead_code)]")],
        )],
        head_sources: [("src/lib.rs".to_owned(), source.to_owned())].into(),
        ..Inputs::default()
    };
    assert_eq!(
        rules::suppression_files(&inputs.lines),
        strings(&["src/lib.rs"])
    );
    let violations = rules::check(&inputs);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].starts_with("guard: suppression: src/lib.rs:2: `#[ignore]`"));
    assert!(violations[0].contains("reason = "));
}

#[test]
fn a_drop_in_a_crates_test_count_is_a_violation() {
    let counts = |items: &[(&str, usize)]| -> BTreeMap<String, usize> {
        items
            .iter()
            .map(|(name, count)| ((*name).to_owned(), *count))
            .collect()
    };
    let inputs = Inputs {
        base_tests: counts(&[("crates/a", 5), ("crates/b", 2), ("crates/c", 1)]),
        head_tests: counts(&[("crates/a", 4), ("crates/b", 3), ("crates/d", 9)]),
        ..Inputs::default()
    };
    let violations = rules::check(&inputs);
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations[0]
            .starts_with("guard: test-count: crates/a: #[test] functions fell from 5 to 4")
    );
    assert!(violations[1].starts_with("guard: test-count: crates/c:"));
}

#[test]
fn deleted_test_files_need_a_mention_or_a_retires_trailer() {
    let deleted = |messages: &[&str]| Inputs {
        changes: vec![
            change('D', Some("crates/a/src/tests.rs"), None),
            change('D', Some("crates/a/src/lib.rs"), None),
            change(
                'R',
                Some("crates/a/tests/it.rs"),
                Some("crates/a/tests/moved.rs"),
            ),
        ],
        messages: strings(messages),
        ..Inputs::default()
    };
    assert_eq!(
        rule_names(&deleted(&["refactor"])),
        ["guard: deleted-test: crates/a/src/tests.rs"]
    );
    assert!(rules::check(&deleted(&["drop crates/a/src/tests.rs, now covered"])).is_empty());
    assert!(rules::check(&deleted(&["x\n\nRetires: crates/a/src/**"])).is_empty());
    assert!(!rules::check(&deleted(&["x\n\nAsserts-changed: crates/a/src/*"])).is_empty());
}

#[test]
fn changed_asserts_in_a_test_file_need_a_trailer() {
    let file = lines(
        "src/tests.rs",
        &[
            (3, "    assert_eq!(a, 1);"),
            (7, "    assert!(b);"),
            (9, "let x = 1;"),
        ],
        &[(12, "assert!(b);"), (13, "let x = 2;")],
    );
    let product = lines("src/lib.rs", &[(1, "debug_assert!(x);")], &[]);
    let inputs = |messages: &[&str]| Inputs {
        lines: vec![file.clone(), product.clone()],
        messages: strings(messages),
        ..Inputs::default()
    };
    let violations = rules::check(&inputs(&[]));
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].starts_with("guard: assert: src/tests.rs: 1 removed or changed"));
    assert!(violations[0].contains("(base line 3)"));
    assert!(rules::check(&inputs(&["Asserts-changed: src/tests.rs"])).is_empty());
    assert!(rules::check(&inputs(&["Retires: src/tests.rs, other.rs"])).is_empty());
    assert!(!rules::check(&inputs(&["names src/tests.rs in prose"])).is_empty());
}

#[test]
fn trailers_split_on_commas_and_spaces() {
    let trailers = Trailers::parse(&strings(&[
        "s\n\nRetires: a.rs, b/**\nAsserts-changed: c.rs\n",
    ]));
    assert_eq!(trailers.retires, strings(&["a.rs", "b/**"]));
    assert_eq!(trailers.asserts_changed, strings(&["c.rs"]));
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=guard",
            "-c",
            "user.email=guard@example.invalid",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output();
    let output = output.ok().filter(|output| output.status.success());
    assert!(output.is_some(), "git {args:?}");
    output
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    let made = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&path, text));
    assert!(made.is_ok(), "{}", path.display());
}

#[test]
fn a_seeded_commit_adding_an_ignore_fails_the_guard_on_a_real_repository() {
    let root = std::env::temp_dir().join(format!("xtask-guard-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    write(&root, "xtask/guard/protected.txt", "tests/oracle/**\n");
    write(&root, "Cargo.toml", "[package]\n");
    write(
        &root,
        "src/tests.rs",
        "#[test]\nfn a() {\n    assert!(true);\n}\n",
    );
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "base"]);
    let base = git(&root, &["rev-parse", "HEAD"]);
    write(
        &root,
        "src/tests.rs",
        "#[test]\n#[ignore]\nfn a() {\n    assert!(true);\n}\n",
    );
    write(&root, "tests/oracle/expected.txt", "1\n");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seeded"]);
    let result = super::run(&root, &strings(&["--base", &base]));
    let clean = super::run(&root, &strings(&["--base", &base, "--head", &base]));
    let inputs = super::Git { root: &root }.inputs(&base, "HEAD");
    let _ = fs::remove_dir_all(&root);
    let error = result
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(error.contains("2 violation(s)"), "{error}");
    assert!(clean.is_ok());
    let names = inputs.map(|inputs| rule_names(&inputs)).unwrap_or_default();
    assert_eq!(
        names,
        [
            "guard: protected: tests/oracle/expected.txt",
            "guard: suppression: src/tests.rs:2"
        ]
    );
}
