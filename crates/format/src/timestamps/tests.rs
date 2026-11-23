use super::*;
use model::value::Value;

fn iso(zone: Option<i64>) -> TimestampParser {
    TimestampParser::new(TimestampFormat::Iso8601, zone)
}

fn parsed(parser: &TimestampParser, wire: Value) -> Result<Value, TimestampError> {
    parser.parse(&wire)
}

#[test]
fn the_setting_reads_as_add_timestamp_parser_reads_it() {
    assert_eq!(TimestampFormat::from_setting(None), Ok(TimestampFormat::Iso8601));
    assert_eq!(TimestampFormat::from_setting(Some("wire")), Ok(TimestampFormat::Wire));
    assert_eq!(TimestampFormat::from_setting(Some("iso8601")), Ok(TimestampFormat::Iso8601));
    let error = TimestampFormat::from_setting(Some("ISO8601")).err().map(|e| e.to_string());
    let expected = r#"Unknown cli_timestamp_format value: ISO8601, valid values are "wire" or "iso8601""#;
    assert_eq!(error.as_deref(), Some(expected));
}

#[test]
fn wire_keeps_the_parsed_value() {
    let wire = TimestampParser::new(TimestampFormat::Wire, None);
    for value in [Value::Float(1.5), Value::String("2015-08-30T12:36:00Z".into())] {
        assert_eq!(parsed(&wire, value.clone()), Ok(value));
    }
}

/// `iso_format`: `parse_timestamp(value).isoformat()`.
#[test]
fn iso8601_prints_isoformat_of_what_parse_timestamp_reads() {
    let utc = iso(Some(0));
    let ist = iso(Some(19_800));
    let text = |value: &str| Value::String(value.into());
    let cases = [
        (&utc, Value::Integer(1_440_938_160), "2015-08-30T12:36:00+00:00"),
        (&ist, Value::Float(1_440_938_160.5), "2015-08-30T18:06:00.500000+05:30"),
        (&ist, text("1440938160"), "2015-08-30T18:06:00+05:30"),
        (&ist, text("2015-08-30T12:36:00Z"), "2015-08-30T12:36:00+00:00"),
        (&ist, text("2015-08-30T13:36:00.1+01:00"), "2015-08-30T13:36:00.100000+01:00"),
        (&ist, text("2015-08-30T12:36:00"), "2015-08-30T12:36:00"),
        (&ist, text("Sun, 30 Aug 2015 12:36:00 GMT"), "2015-08-30T12:36:00+00:00"),
    ];
    for (parser, wire, expected) in cases {
        assert_eq!(parsed(parser, wire.clone()), Ok(text(expected)), "{wire:?}");
    }
}

/// An epoch needs the local zone, which a zone with daylight-saving time
/// leaves to Python; a string with its own zone does not.
#[test]
fn what_native_cannot_print_is_an_error_naming_why() {
    let dst = iso(None);
    let epoch = parsed(&dst, Value::Integer(0));
    assert!(matches!(epoch, Err(TimestampError::LocalZoneHasDaylightSaving)));
    let zoned = parsed(&dst, Value::String("2015-08-30T12:36:00Z".into()));
    assert_eq!(zoned, Ok(Value::String("2015-08-30T12:36:00+00:00".into())));
    let utc = iso(Some(0));
    for wire in [Value::String("next tuesday".into()), Value::String("nan".into()), Value::Float(1e300)] {
        assert!(matches!(parsed(&utc, wire.clone()), Err(TimestampError::Unsupported(_))), "{wire:?}");
    }
}
