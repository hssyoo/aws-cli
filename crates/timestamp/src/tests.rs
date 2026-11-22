use super::*;

/// 2015-08-30T12:36:00Z, the AWS SigV4 test suite's date: a Sunday.
const SUITE: i64 = 1_440_938_160;
/// 2024-02-29T12:34:56Z, a leap day: a Thursday.
const LEAP_DAY: i64 = 1_709_210_096;
/// 0001-01-01T00:00:00Z and 9999-12-31T23:59:59Z, the ends of Python's
/// `datetime` range.
const FIRST: i64 = -62_135_596_800;
const LAST: i64 = 253_402_300_799;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn at(seconds: i64) -> Result<Timestamp, String> {
    Timestamp::from_unix_seconds(seconds).ok_or(format!("{seconds} is out of range"))
}

fn at_micros(micros: i64) -> Result<Timestamp, String> {
    Timestamp::from_unix_micros(micros).ok_or(format!("{micros} µs is out of range"))
}

fn civil(year: i32, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Civil {
    Civil {
        year,
        month,
        day,
        hour,
        minute,
        second,
        microsecond: 0,
    }
}

#[test]
fn instants_have_their_civil_date_and_time() -> TestResult {
    assert_eq!(at(0)?.civil(), civil(1970, 1, 1, 0, 0, 0));
    assert_eq!(at(SUITE)?.civil(), civil(2015, 8, 30, 12, 36, 0));
    assert_eq!(at(LEAP_DAY)?.civil(), civil(2024, 2, 29, 12, 34, 56));
    assert_eq!(at(-1)?.civil(), civil(1969, 12, 31, 23, 59, 59));
    assert_eq!(at(FIRST)?.civil(), civil(1, 1, 1, 0, 0, 0));
    assert_eq!(at(LAST)?.civil(), civil(9999, 12, 31, 23, 59, 59));
    let before = at_micros(-1)?.civil();
    assert_eq!(
        before,
        Civil {
            microsecond: 999_999,
            ..civil(1969, 12, 31, 23, 59, 59)
        }
    );
    Ok(())
}

#[test]
fn the_range_is_python_s_datetime_range() {
    assert!(Timestamp::from_unix_seconds(FIRST - 1).is_none());
    assert!(Timestamp::from_unix_seconds(LAST + 1).is_none());
    assert!(Timestamp::from_unix_micros(LAST * 1_000_000 + 999_999).is_some());
    assert!(Timestamp::from_unix_micros(LAST * 1_000_000 + 1_000_000).is_none());
    assert!(Timestamp::from_unix_seconds(i64::MAX).is_none());
    assert!(Timestamp::from_unix_seconds(i64::MIN).is_none());
}

/// Every day of the range, sampled, survives the trip to a civil date and
/// back, and consecutive days are consecutive dates.
#[test]
fn civil_dates_round_trip_across_the_range() -> TestResult {
    let mut seconds = FIRST;
    while seconds <= LAST {
        let instant = at(seconds)?;
        assert_eq!(Timestamp::from_civil(instant.civil()), Some(instant));
        let next = at(seconds + 86_400).map(|next| next.civil());
        if let Ok(next) = next {
            let today = instant.civil();
            let advanced = (next.year, next.month, next.day) > (today.year, today.month, today.day);
            assert!(advanced, "{today:?} then {next:?}");
        }
        seconds += 86_400 * 97 + 3_601;
    }
    Ok(())
}

#[test]
fn only_dates_and_times_that_exist_are_civil() {
    for valid in [
        civil(2024, 2, 29, 0, 0, 0),
        civil(2000, 2, 29, 23, 59, 59),
        civil(2023, 12, 31, 0, 0, 0),
    ] {
        assert!(Timestamp::from_civil(valid).is_some(), "{valid:?}");
    }
    for invalid in [
        civil(2023, 2, 29, 0, 0, 0),
        civil(1900, 2, 29, 0, 0, 0),
        civil(2023, 13, 1, 0, 0, 0),
        civil(2023, 0, 1, 0, 0, 0),
        civil(2023, 4, 31, 0, 0, 0),
        civil(2023, 1, 0, 0, 0, 0),
        civil(2023, 1, 1, 24, 0, 0),
        civil(2023, 1, 1, 0, 60, 0),
        civil(2023, 1, 1, 0, 0, 60),
        civil(0, 1, 1, 0, 0, 0),
        civil(10_000, 1, 1, 0, 0, 0),
        Civil {
            microsecond: 1_000_000,
            ..civil(2023, 1, 1, 0, 0, 0)
        },
    ] {
        assert_eq!(Timestamp::from_civil(invalid), None, "{invalid:?}");
    }
}

#[test]
fn the_basic_form_is_sigv4_s_and_drops_the_fraction() -> TestResult {
    assert_eq!(at(0)?.iso8601_basic(), "19700101T000000Z");
    assert_eq!(at(SUITE)?.iso8601_basic(), "20150830T123600Z");
    assert_eq!(at(LEAP_DAY)?.basic_date(), "20240229");
    assert_eq!(
        at_micros(SUITE * 1_000_000 + 999_999)?.iso8601_basic(),
        "20150830T123600Z"
    );
    assert_eq!(at(FIRST)?.iso8601_basic(), "00010101T000000Z");
    Ok(())
}

#[test]
fn the_basic_form_parses_only_instants_that_exist() -> TestResult {
    for seconds in [0, SUITE, LEAP_DAY, FIRST, LAST] {
        let instant = at(seconds)?;
        assert_eq!(
            Timestamp::parse_iso8601_basic(&instant.iso8601_basic()),
            Some(instant)
        );
    }
    for text in [
        "nope",
        "",
        "20161318T143303Z",
        "20160832T143303Z",
        "20230229T000000Z",
        "20160818T243303Z",
        "20160818T146003Z",
        "20160818T143360Z",
        "00000101T000000Z",
        "+0160818T143303Z",
        "20160818 143303Z",
        "20160818T143303",
        "20160818T143303z",
        "20160818T14330３Z",
        "20160818T143303Z ",
    ] {
        assert_eq!(Timestamp::parse_iso8601_basic(text), None, "{text}");
    }
    Ok(())
}

/// botocore's `_timestamp_iso8601`: a fraction only when there are
/// microseconds, and then six digits.
#[test]
fn the_extended_form_is_botocore_s() -> TestResult {
    assert_eq!(at(SUITE)?.iso8601(), "2015-08-30T12:36:00Z");
    assert_eq!(
        at_micros(SUITE * 1_000_000 + 123_456)?.iso8601(),
        "2015-08-30T12:36:00.123456Z"
    );
    assert_eq!(
        at_micros(SUITE * 1_000_000 + 500_000)?.iso8601(),
        "2015-08-30T12:36:00.500000Z"
    );
    assert_eq!(at_micros(-1)?.iso8601(), "1969-12-31T23:59:59.999999Z");
    Ok(())
}

#[test]
fn the_extended_form_parses_with_a_zone_and_a_fraction() -> TestResult {
    let suite = at(SUITE)?;
    let micros = |extra: i64| at_micros(SUITE * 1_000_000 + extra).map(Some);
    for (text, expected) in [
        ("2015-08-30T12:36:00Z", Some(suite)),
        ("2015-08-30t12:36:00z", Some(suite)),
        ("2015-08-30 12:36:00Z", Some(suite)),
        ("2015-08-30T12:36:00+00:00", Some(suite)),
        ("2015-08-30T13:36:00+01:00", Some(suite)),
        ("2015-08-30T11:06:00-0130", Some(suite)),
        ("2015-08-31T00:36:00+12:00", Some(suite)),
        ("2015-08-30T12:36:00.123456Z", micros(123_456)?),
        ("2015-08-30T12:36:00.1Z", micros(100_000)?),
        ("2015-08-30T12:36:00.1234567Z", micros(123_456)?),
        ("2015-08-30T12:36:00.000001+00:00", micros(1)?),
    ] {
        assert_eq!(Timestamp::parse_iso8601(text), expected, "{text}");
    }
    for text in [
        "2015-08-30T12:36:00",
        "2015-08-30T12:36Z",
        "2015-08-30T12:36:00.Z",
        "2015-02-29T12:36:00Z",
        "2015-08-30T12:36:00+24:00",
        "2015-08-30T12:36:00+0:00",
        "2015-08-30T12:36:00Zjunk",
        "20150830T123600Z",
        "0001-01-01T00:00:00+00:01",
        "9999-12-31T23:59:59-00:01",
    ] {
        assert_eq!(Timestamp::parse_iso8601(text), None, "{text}");
    }
    Ok(())
}

/// botocore's `_timestamp_unixtimestamp`: whole seconds as an integer,
/// else the float Python computes, as `repr` prints it.
#[test]
fn epoch_seconds_print_as_python_prints_them() -> TestResult {
    for (micros, text) in [
        (0, "0"),
        (SUITE * 1_000_000, "1440938160"),
        (-1_000_000, "-1"),
        (SUITE * 1_000_000 + 500_000, "1440938160.5"),
        (SUITE * 1_000_000 + 123_456, "1440938160.123456"),
        // The case botocore's comment names: 1 + 3691 / 10**6 would not do.
        (1_003_691, "1.003691"),
        (-1_500_000, "-1.5"),
        // Python's repr turns to an exponent below 1e-4.
        (1, "1e-06"),
        (-1, "-1e-06"),
        (99, "9.9e-05"),
        (100, "0.0001"),
        // Past 2**33 seconds a float no longer holds every microsecond.
        (LAST * 1_000_000 + 999_999, "253402300800.0"),
    ] {
        assert_eq!(at_micros(micros)?.epoch_seconds(), text, "{micros}");
    }
    Ok(())
}

#[test]
fn epoch_seconds_parse_to_the_microsecond() -> TestResult {
    for (text, micros) in [
        ("0", 0),
        ("1440938160", SUITE * 1_000_000),
        ("+1440938160", SUITE * 1_000_000),
        ("1440938160.5", SUITE * 1_000_000 + 500_000),
        ("1440938160.", SUITE * 1_000_000),
        (".5", 500_000),
        ("-1.5", -1_500_000),
        ("1.003691", 1_003_691),
        // Past six digits, to the nearest microsecond, ties to even.
        ("1.0000005", 1_000_000),
        ("1.0000015", 1_000_002),
        ("1.00000051", 1_000_001),
        ("0.9999999", 1_000_000),
    ] {
        assert_eq!(
            Timestamp::parse_epoch_seconds(text),
            Some(at_micros(micros)?),
            "{text}"
        );
    }
    for text in [
        "", "-", ".", "abc", "1e9", " 1", "1 ", "1.5.5", "0x10", "1_000",
    ] {
        assert_eq!(Timestamp::parse_epoch_seconds(text), None, "{text}");
    }
    let past_the_range = (LAST + 1).to_string();
    assert_eq!(Timestamp::parse_epoch_seconds(&past_the_range), None);
    assert_eq!(Timestamp::parse_epoch_seconds("99999999999999999999"), None);
    Ok(())
}

/// botocore's `_timestamp_rfc822`: `formatdate(..., usegmt=True)`, whole
/// seconds.
#[test]
fn rfc_822_is_email_utils_formatdate_in_gmt() -> TestResult {
    assert_eq!(at(0)?.rfc822(), "Thu, 01 Jan 1970 00:00:00 GMT");
    assert_eq!(at(SUITE)?.rfc822(), "Sun, 30 Aug 2015 12:36:00 GMT");
    assert_eq!(at(LEAP_DAY)?.rfc822(), "Thu, 29 Feb 2024 12:34:56 GMT");
    assert_eq!(
        at_micros(SUITE * 1_000_000 + 999_999)?.rfc822(),
        "Sun, 30 Aug 2015 12:36:00 GMT"
    );
    assert_eq!(at(FIRST)?.rfc822(), "Mon, 01 Jan 0001 00:00:00 GMT");
    Ok(())
}

#[test]
fn rfc_822_parses_the_dates_services_send() -> TestResult {
    let suite = Some(at(SUITE)?);
    for text in [
        "Sun, 30 Aug 2015 12:36:00 GMT",
        "30 Aug 2015 12:36:00 GMT",
        "Sun, 30 Aug 2015 12:36:00 UTC",
        "Sun, 30 Aug 2015 12:36:00 +0000",
        "Sun, 30 Aug 2015 13:36:00 +0100",
        "Sun, 30 Aug 2015 12:36:00 -0000",
    ] {
        assert_eq!(Timestamp::parse_rfc822(text), suite, "{text}");
    }
    assert_eq!(
        Timestamp::parse_rfc822("Mon, 3 Aug 2015 00:00:00 GMT"),
        Some(at(1_438_560_000)?)
    );
    for text in [
        "",
        "Sun, 30 Aug 2015 12:36:00",
        "Sun, 31 Feb 2015 12:36:00 GMT",
        "Sun, 30 Foo 2015 12:36:00 GMT",
        "Sunday, 30 Aug 2015 12:36:00 GMT",
        "Sun 30 Aug 2015 12:36:00 GMT",
        "Sun, 30 Aug 15 12:36:00 GMT",
        "Sun, 30 Aug 2015 12:36 GMT",
        "Sun, 30 Aug 2015 12:36:00 EST",
        "Sun, 30 Aug 2015 12:36:00 GMT junk",
    ] {
        assert_eq!(Timestamp::parse_rfc822(text), None, "{text}");
    }
    Ok(())
}

/// Each form reads back what it writes.
#[test]
fn every_form_reads_back_what_it_writes() -> TestResult {
    for micros in [
        0,
        SUITE * 1_000_000,
        SUITE * 1_000_000 + 123_456,
        LEAP_DAY * 1_000_000 + 1,
        -1,
        -1_500_000,
        FIRST * 1_000_000,
        LAST * 1_000_000 + 999_999,
    ] {
        let instant = at_micros(micros)?;
        let whole = at(instant.unix_seconds())?;
        assert_eq!(Timestamp::parse_iso8601(&instant.iso8601()), Some(instant));
        assert_eq!(
            Timestamp::parse_iso8601_basic(&instant.iso8601_basic()),
            Some(whole)
        );
        assert_eq!(Timestamp::parse_rfc822(&instant.rfc822()), Some(whole));
        // Python's float holds every microsecond to 2**33 seconds.
        if instant.unix_seconds().abs() < 1 << 33 {
            assert_eq!(
                Timestamp::parse_epoch_seconds(&instant.epoch_seconds()),
                Some(instant),
                "{}",
                instant.epoch_seconds()
            );
        }
    }
    Ok(())
}

#[test]
fn seconds_and_microseconds_split_toward_the_past() -> TestResult {
    let instant = at_micros(-1_500_000)?;
    assert_eq!(instant.unix_seconds(), -2);
    assert_eq!(instant.subsec_micros(), 500_000);
    assert_eq!(instant.unix_micros(), -1_500_000);
    assert!(Timestamp::now().unix_seconds() > SUITE);
    Ok(())
}
