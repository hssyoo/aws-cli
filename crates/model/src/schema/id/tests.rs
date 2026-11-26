use super::*;
use crate::testing::Result;

#[test]
fn ids_parse_and_print() -> Result<()> {
    let shape = ShapeId::parse("c2j.my-service#Input")?;
    assert_eq!(
        (shape.namespace(), shape.name(), shape.member()),
        ("c2j.my-service", "Input", None)
    );
    assert_eq!(shape.to_string(), "c2j.my-service#Input");
    let member = ShapeId::parse("smithy.api#Input$Name")?;
    assert_eq!(
        (member.namespace(), member.name(), member.member()),
        ("smithy.api", "Input", Some("Name"))
    );
    assert_eq!(member.as_str(), "smithy.api#Input$Name");
    assert_eq!(member.root(), ShapeId::parse("smithy.api#Input")?);
    assert_eq!(member.root().member(), None);
    Ok(())
}

#[test]
fn malformed_ids_are_refused() {
    for text in ["", "Name", "a#b#c", "a#b$c$d", "a$b#c", "a#b$c#d"] {
        let refused = matches!(ShapeId::parse(text), Err(ShapeIdError::Malformed(_)));
        assert!(refused, "{text}");
    }
    for text in ["#Name", "ns#", "ns#$m", "ns#Name$"] {
        let refused = matches!(ShapeId::parse(text), Err(ShapeIdError::EmptyPart(_)));
        assert!(refused, "{text}");
    }
    let long = format!("ns#{}", "a".repeat(usize::from(u16::MAX)));
    assert_eq!(ShapeId::parse(&long), Err(ShapeIdError::TooLong));
}
