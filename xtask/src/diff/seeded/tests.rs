use std::process::Command;

use super::*;
use crate::diff::capture::{Capture, Request};
use crate::diff::{corpus, report};

fn capture(stdout: &str, max: &str, exit: i32) -> Capture {
    Capture {
        id: "r".to_owned(),
        side: String::new(),
        exit: Some(exit),
        timed_out: false,
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
        requests: vec![Request {
            method: "POST".to_owned(),
            target: "/".to_owned(),
            version: "1.1".to_owned(),
            headers: vec![(
                "amz-sdk-request".to_owned(),
                format!("attempt=1; max={max}"),
            )],
            body: Vec::new(),
            complete: true,
        }],
    }
}

fn report_of(b: Capture) -> Result<Report, TaskError> {
    let side = |label: &str| Side {
        label: label.to_owned(),
        binary: PathBuf::from(label),
        route: Route::Native,
    };
    let run = Run {
        a: side("candidate"),
        b: side("seeded"),
        corpus: Vec::new(),
        filter: None,
        known: None,
        out: PathBuf::new(),
    };
    let rows = corpus::parse("rows", "{\"id\":\"r\",\"argv\":[\"x\"],\"responses\":[]}")?;
    let a = capture("{\n    \"ThingId\": \"thing-0\"\n}\n", "3", 0);
    Ok(report::build(&run, &rows, &[(a, b)], &[]))
}

#[test]
fn the_seeded_defect_must_be_named_by_its_two_fingerprints_alone() -> Result<(), TaskError> {
    let seeded = "{\n    \"ThingId\": \"thing-X\"\n}\n";
    assert!(check_caught(&report_of(capture(seeded, "7", 0))?).is_ok());
    let cases = [
        capture("{\n    \"ThingId\": \"thing-0\"\n}\n", "3", 0),
        capture("{\n    \"ThingId\": \"thing-0\"\n}\n", "7", 0),
        capture(seeded, "7", 255),
    ];
    for case in cases {
        let report = report_of(case)?;
        let error = check_caught(&report)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            error.starts_with("diff self-test: the seeded defect must make"),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn the_wrapper_quotes_the_binary() {
    let script = wrapper_script(Path::new("/a b/it's/aws"));
    assert!(script.starts_with("#!/bin/bash\n"), "{script}");
    assert!(
        script.contains("\n'/a b/it'\\''s/aws' \"$@\" | sed "),
        "{script}"
    );
}

/// The wrapper, around a stand-in CLI, changes one stdout byte and the
/// attempts, and keeps the exit status.
#[cfg(unix)]
#[test]
fn the_wrapper_changes_one_byte_and_the_attempts() -> Result<(), TaskError> {
    let directory = std::env::temp_dir().join(format!("xtask-diff-seeded-{}", std::process::id()));
    let cli = directory.join("cli");
    fs::create_dir_all(&directory)?;
    let printed = "{\n    \"ThingId\": \"thing-0\",\n    \"Name\": \"n1\"\n}\n";
    fs::write(
        &cli,
        format!(
            "#!/bin/bash\nprintf '%s' '{printed}'\necho \"$AWS_MAX_ATTEMPTS $*\" >&2\nexit 3\n"
        ),
    )?;
    executable(&cli)?;
    let wrapper = directory.join("seeded");
    write_wrapper(&wrapper, &cli)?;
    let output = Command::new(&wrapper).args(["a", "b c"]).output()?;
    fs::remove_dir_all(&directory)?;
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "7 a b c\n");
    let expected = printed.replacen("thing-0", "thing-X", 1);
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    Ok(())
}
