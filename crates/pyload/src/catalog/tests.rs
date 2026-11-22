use super::*;
#[test]
fn prefix_upper_bounds_handle_exact_carry_and_terminal_ranges() {
    let mut buffer = [0; 16];
    assert_eq!(
        prefix_upper_bound(&mut buffer, b"abc"),
        Some(b"abd".as_slice())
    );
    assert_eq!(
        prefix_upper_bound(&mut buffer, b"ab\xff"),
        Some(b"ac".as_slice())
    );
    assert_eq!(
        prefix_upper_bound(&mut buffer, b"a\xff\xff"),
        Some(b"b".as_slice())
    );
    assert_eq!(prefix_upper_bound(&mut buffer, b""), None);
    assert_eq!(prefix_upper_bound(&mut buffer, b"\xff\xff"), None);
    assert_eq!(prefix_upper_bound(&mut buffer[..2], b"abc"), None);
}
#[test]
fn immediate_directory_children_reject_misses_and_collapse_descendants() {
    assert_eq!(
        immediate_child(b"pkg/", b"pkg/file.pyc"),
        Some(b"file.pyc".as_slice())
    );
    assert_eq!(
        immediate_child(b"pkg/", b"pkg/child/file.pyc"),
        Some(b"child".as_slice())
    );
    for name in [b"pkg/".as_slice(), b"pkg//file.pyc", b"other/file.pyc"] {
        assert_eq!(immediate_child(b"pkg/", name), None);
    }
}
#[test]
fn model_filename_matching_is_exact() {
    assert!(is_model_file(b"service-2.json", b"service-2"));
    assert!(is_model_file(b"waiters-2.json", b"waiters-2"));
    for name in [
        b"service-2.json.gz".as_slice(),
        b"prefix-service-2.json",
        b"service-2",
    ] {
        assert!(!is_model_file(name, b"service-2"));
    }
}
fn groups(
    view: crate::View<'_>,
    prefix: &[u8],
    type_name: &[u8],
    layout: Layout,
) -> Result<Vec<Vec<u8>>, GroupError> {
    let mut found = Vec::new();
    for_each_model_group(view, prefix, type_name, layout, |group| {
        found.push(group.to_vec());
        Ok::<(), GroupError>(())
    })?;
    Ok(found)
}
#[test]
fn model_groups_list_services_and_versions_without_reading_data()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::format::{flags, testing};
    let entries = [
        b"data/a/2020/service-2.json".as_slice(),
        b"data/a/2021/paginators-1.json",
        b"data/b/2019/waiters-2.json",
        b"data/b/2022/service-2.json",
        b"data/b/2022/service-2.sdk-extras.json",
        b"data/c/2020/service-2.json.gz",
        b"data/partitions.json",
        b"other/z/2020/service-2.json",
    ];
    let entries = entries.map(|name| testing::entry(name, b"", flags::RESOURCE));
    let blob = testing::blob(&entries, false, 2);
    let view = crate::View::new(&blob)?;
    let services = groups(view, b"data/", b"service-2", Layout::Services)?;
    assert_eq!(services, [b"a".to_vec(), b"b".to_vec()]);
    let versions = groups(view, b"data/b/", b"service-2", Layout::Versions)?;
    assert_eq!(versions, [b"2022".to_vec()]);
    let versions = groups(view, b"data/a/", b"paginators-1", Layout::Versions)?;
    assert_eq!(versions, [b"2021".to_vec()]);
    assert!(groups(view, b"data/missing/", b"service-2", Layout::Versions)?.is_empty());
    assert!(groups(view, b"nothing/", b"service-2", Layout::Services)?.is_empty());
    Ok(())
}
#[test]
fn model_group_lookup_names_are_bounded() -> Result<(), Box<dyn std::error::Error>> {
    use crate::format::{flags, testing};
    let mut prefix = vec![b'x'; MODEL_GROUP_NAME_MAX - 6];
    prefix.push(b'/');
    let fits = [prefix.as_slice(), b"abcd/v/service-2.json"].concat();
    let over = [prefix.as_slice(), b"abcde/v/service-2.json"].concat();
    for (name, expected) in [(fits, Ok(1)), (over, Err(()))] {
        let blob = testing::blob(&[testing::entry(&name, b"", flags::RESOURCE)], false, 2);
        let view = crate::View::new(&blob)?;
        let found = groups(view, &prefix, b"service-2", Layout::Services);
        match expected {
            Ok(count) => assert_eq!(found?.len(), count),
            Err(()) => assert!(matches!(found, Err(GroupError::NameTooLong))),
        }
    }
    Ok(())
}
