use super::*;

#[test]
fn only_the_platform_s_own_baseline_file_may_be_uncommitted() {
    let own = own("macos-aarch64");
    assert_eq!(own, "tests/perf/baselines/macos-aarch64.json");
    assert!(!changed_besides("", &own));
    // The file a calibration or --write-baseline step just rewrote.
    assert!(!changed_besides(" M tests/perf/baselines/macos-aarch64.json\n", &own));
    assert!(!changed_besides("M  tests/perf/baselines/macos-aarch64.json\n", &own));
    // Any other tracked change is a change the binary may be built from.
    let others = [
        " M crates/client/src/paginate.rs\n",
        " M tests/perf/baselines/macos-aarch64.json\n M tests/perf/cases.json\n",
        " M tests/perf/baselines/linux-x86_64.json\n",
        "R  tests/perf/baselines/macos-aarch64.json -> tests/perf/old.json\n",
    ];
    for status in others {
        assert!(changed_besides(status, &own), "{status:?}");
    }
}

#[test]
fn a_binary_under_the_root_is_recorded_relative_to_it() {
    let root = Path::new("/work/aws-cli");
    assert_eq!(
        shown(
            root,
            &root.join("target/aarch64-apple-darwin/release-safe/aws")
        ),
        "target/aarch64-apple-darwin/release-safe/aws"
    );
    assert_eq!(
        shown(root, Path::new("/saved/before/aws")),
        "/saved/before/aws"
    );
}
