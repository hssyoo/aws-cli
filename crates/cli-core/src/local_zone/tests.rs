use super::*;

/// January and July offsets, in seconds east, for a probe.
fn probe(january: i64, july: i64) -> impl Fn(i64) -> Option<i64> {
    move |at| {
        Some(if at.rem_euclid(YEAR) < YEAR / 2 {
            january
        } else {
            july
        })
    }
}

#[test]
fn zones_follow_cpython_s_init_timezone() {
    let now = 1_440_938_160;
    let fixed = LocalZone::from_probe(now, probe(19_800, 19_800));
    assert_eq!(
        fixed,
        Some(LocalZone {
            timezone: -19_800,
            altzone: -19_800,
            daylight: false
        })
    );
    assert_eq!(fixed.and_then(LocalZone::fixed_offset), Some(19_800));
    let north = LocalZone::from_probe(now, probe(3_600, 7_200));
    assert_eq!(
        north,
        Some(LocalZone {
            timezone: -3_600,
            altzone: -7_200,
            daylight: true
        })
    );
    assert_eq!(north.and_then(LocalZone::fixed_offset), None);
    let south = LocalZone::from_probe(now, probe(39_600, 36_000));
    assert_eq!(
        south,
        Some(LocalZone {
            timezone: -36_000,
            altzone: -39_600,
            daylight: true
        })
    );
    assert_eq!(LocalZone::from_probe(now, |_| None), None);
}

/// The process's zone agrees with `time.timezone`, `time.altzone` and
/// `time.daylight` in the toolchain's CPython under the same environment.
#[test]
fn the_system_zone_is_python_s() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = std::env::var_os("AWS_NATIVE_CPYTHON_PREFIX")
        .ok_or("AWS_NATIVE_CPYTHON_PREFIX must name the toolchain CPython")?;
    let python = std::path::PathBuf::from(prefix).join("bin/python3");
    let script = "import time; print(time.timezone, time.altzone, int(time.daylight))";
    let output = std::process::Command::new(python)
        .args(["-I", "-c", script])
        .output()?;
    let text = String::from_utf8(output.stdout)?;
    let fields: Vec<i64> = text
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    let zone = LocalZone::system().ok_or("no local zone")?;
    assert_eq!(
        fields,
        [zone.timezone, zone.altzone, i64::from(zone.daylight)]
    );
    Ok(())
}
