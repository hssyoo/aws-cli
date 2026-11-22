use super::*;

fn civil(text: &str) -> Civil {
    let number = |range: std::ops::Range<usize>| text[range].parse().unwrap_or(u32::MAX);
    Civil {
        year: i32::try_from(number(0..4)).unwrap_or(i32::MAX),
        month: number(5..7),
        day: number(8..10),
        hour: number(11..13),
        minute: number(14..16),
        second: number(17..19),
        micros: 0,
    }
}

#[test]
fn civil_days_round_trip_across_the_python_range() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    assert_eq!(days_from_civil(1, 1, 1), -719_162);
    assert_eq!(days_from_civil(9999, 12, 31), 2_932_896);
    for days in (-719_162..=2_932_896).step_by(97) {
        let (year, month, day) = civil_from_days(days);
        assert_eq!(days_from_civil(year, month, day), days);
    }
    assert_eq!(civil_from_days(-1), (1969, 12, 31));
    assert_eq!(civil_from_days(59), (1970, 3, 1));
}

#[test]
fn instants_stay_within_python_datetime_years() {
    assert_eq!(Timestamp::MIN.civil(), civil("0001-01-01T00:00:00"));
    let max = Timestamp::MAX.civil();
    assert_eq!((max.year, max.second, max.micros), (9999, 59, 999_999));
    assert!(Timestamp::from_unix(Timestamp::MIN.unix_seconds() - 1, 0).is_none());
    assert!(Timestamp::from_unix(Timestamp::MAX.unix_seconds() + 1, 0).is_none());
    assert!(Timestamp::from_unix(0, 1_000_000).is_none());
    for bad in [
        civil("2021-02-29T00:00:00"),
        civil("2020-13-01T00:00:00"),
        civil("2020-01-01T24:00:00"),
        civil("2020-01-01T00:60:00"),
        civil("2020-01-01T00:00:60"),
        civil("0000-12-31T00:00:00"),
    ] {
        assert!(Timestamp::from_civil(bad).is_none(), "{bad:?}");
    }
    let leap = Timestamp::from_civil(civil("2020-02-29T12:34:56"));
    assert_eq!(leap.map(Timestamp::unix_seconds), Some(1_582_979_696));
}

#[test]
fn formats_match_botocore_serializers() {
    let at = |seconds, micros| Timestamp::from_unix(seconds, micros).unwrap_or(Timestamp::MIN);
    assert_eq!(at(0, 0).iso8601(), "1970-01-01T00:00:00Z");
    assert_eq!(at(1_582_979_696, 5).iso8601(), "2020-02-29T12:34:56.000005Z");
    assert_eq!(at(1_582_979_696, 0).rfc822(), "Sat, 29 Feb 2020 12:34:56 GMT");
    assert_eq!(at(1_582_979_696, 999_999).rfc822(), "Sat, 29 Feb 2020 12:34:56 GMT");
    assert_eq!(at(-1, 0).rfc822(), "Wed, 31 Dec 1969 23:59:59 GMT");
    assert_eq!(Timestamp::MIN.weekday(), 0);
    assert_eq!(at(0, 0).weekday(), 3);
}

#[test]
fn iso8601_parsing_accepts_offsets_fractions_and_dates() {
    let seconds = |text: &str| Timestamp::parse_iso8601(text).map(|t| (t.unix_seconds(), t.micros()));
    assert_eq!(seconds("2020-02-29T12:34:56Z"), Some((1_582_979_696, 0)));
    assert_eq!(seconds("2020-02-29t12:34:56z"), Some((1_582_979_696, 0)));
    assert_eq!(seconds("2020-02-29 12:34:56"), Some((1_582_979_696, 0)));
    assert_eq!(seconds("2020-02-29T12:34:56.5Z"), Some((1_582_979_696, 500_000)));
    assert_eq!(seconds("2020-02-29T12:34:56.1234567Z"), Some((1_582_979_696, 123_456)));
    assert_eq!(seconds("2020-02-29T13:34:56+01:00"), Some((1_582_979_696, 0)));
    assert_eq!(seconds("2020-02-29T11:04:56-0130"), Some((1_582_979_696, 0)));
    assert_eq!(seconds("2020-02-29T14:34:56+02"), Some((1_582_979_696, 0)));
    assert_eq!(seconds("2020-02-29T12:34Z"), Some((1_582_979_640, 0)));
    assert_eq!(seconds("2020-02-29"), Some((1_582_934_400, 0)));
    for bad in [
        "",
        "2020-02-30",
        "2020-2-29",
        "2020-02-29T12",
        "2020-02-29T12:34:56.Z",
        "2020-02-29T12:34:56+1:00",
        "2020-02-29T12:34:56Zjunk",
        "0001-01-01T00:00:00+00:01",
    ] {
        assert_eq!(seconds(bad), None, "{bad}");
    }
}

#[test]
fn rfc822_parsing_reads_the_http_date_form() {
    let seconds = |text: &str| Timestamp::parse_rfc822(text).map(Timestamp::unix_seconds);
    assert_eq!(seconds("Sat, 29 Feb 2020 12:34:56 GMT"), Some(1_582_979_696));
    assert_eq!(seconds("29 Feb 2020 12:34:56 GMT"), Some(1_582_979_696));
    assert_eq!(seconds("Sat, 29 Feb 2020 12:34:56 UTC"), Some(1_582_979_696));
    assert_eq!(seconds("Sat, 29 Feb 2020 13:34:56 +0100"), Some(1_582_979_696));
    assert_eq!(seconds("Sat, 9 Feb 2020 12:34:56 GMT"), Some(1_581_251_696));
    for bad in [
        "",
        "Sat, 30 Feb 2020 12:34:56 GMT",
        "Sat, 29 Fob 2020 12:34:56 GMT",
        "Sat, 29 Feb 2020 12:34 GMT",
        "Sat, 29 Feb 2020 12:34:56",
    ] {
        assert_eq!(seconds(bad), None, "{bad}");
    }
}

#[test]
fn epoch_floats_round_half_even_to_microseconds_as_python_does() {
    // datetime.datetime.fromtimestamp(x, datetime.timezone.utc) in CPython 3.14.
    let at = |value: f64| Timestamp::from_epoch(value).map(|t| (t.unix_seconds(), t.micros()));
    assert_eq!(at(1.5), Some((1, 500_000)));
    assert_eq!(at(-1.5), Some((-2, 500_000)));
    assert_eq!(at(0.000_000_5), Some((0, 0)));
    assert_eq!(at(0.000_001_5), Some((0, 2)));
    assert_eq!(at(1_582_979_696.999_999_9), Some((1_582_979_697, 0)));
    assert_eq!(at(-0.000_000_1), Some((0, 0)));
    assert_eq!(at(f64::NAN), None);
    assert_eq!(at(f64::INFINITY), None);
    assert_eq!(at(1e20), None);
}
