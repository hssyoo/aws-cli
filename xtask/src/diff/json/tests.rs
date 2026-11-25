use super::*;

fn kind<'a>(document: &'a Document<'_>) -> &'a Kind<'a> {
    &document.root.kind
}

#[test]
fn a_document_keeps_every_member_scalar_and_space() -> Result<(), String> {
    let text = " {\"B\" : [1.0, \"x\\\"y\" ,NaN], \"A\":{} }\n";
    let document = read(text).ok_or("no document")?;
    assert_eq!((document.before, document.after), (" ", "\n"));
    let Kind::Object(members, close) = kind(&document) else {
        return Err(format!("{document:?}"));
    };
    assert_eq!(*close, " ");
    let keys: Vec<_> = members.iter().map(|member| member.key).collect();
    assert_eq!(keys, ["\"B\"", "\"A\""]);
    let first = members.first().ok_or("no member")?;
    assert_eq!(first.layout, ["", " ", " "]);
    assert_eq!(first.before_comma, Some(""));
    let Kind::Array(elements, _) = &first.value.kind else {
        return Err(format!("{first:?}"));
    };
    let texts: Vec<_> = elements.iter().map(|element| element.value.text).collect();
    assert_eq!(texts, ["1.0", "\"x\\\"y\"", "NaN"]);
    assert_eq!(
        elements.get(1).and_then(|element| element.before_comma),
        Some(" ")
    );
    assert_eq!(members.get(1).map(|member| member.value.text), Some("{}"));
    Ok(())
}

#[test]
fn text_that_is_not_one_value_reads_as_none() {
    for text in [
        "",
        "{",
        "{\"a\" 1}",
        "[1,]",
        "{\"a\":1}x",
        "\"open",
        "a b",
        "[1 2]",
    ] {
        assert_eq!(read(text), None, "{text}");
    }
    let deep = format!("{}{}", "[".repeat(DEPTH_MAX + 1), "]".repeat(DEPTH_MAX + 1));
    assert_eq!(read(&deep), None);
    let deep_enough = format!("{}{}", "[".repeat(DEPTH_MAX), "]".repeat(DEPTH_MAX));
    assert!(read(&deep_enough).is_some());
}
