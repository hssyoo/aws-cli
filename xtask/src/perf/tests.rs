use super::*;

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
