use std::process::Command;

use serde_json::{Value, json};

use super::*;
use crate::diff::{corpus, report};

const PRINTED: &str = "{\n    \"ThingId\": \"thing-0\"\n}\n";
const SEEDED: &str = "{\n    \"ThingId\": \"thing-X\"\n}\n";

/// A run of row `id` as the capture writes it.
fn run_of(id: &str, fields: Value) -> Result<Capture, TaskError> {
    let mut value = json!({"id": id, "side": "a", "exit": 0, "timed_out": false,
                           "stdout": "", "stderr": "", "requests": []});
    if let (Some(value), Some(fields)) = (value.as_object_mut(), fields.as_object()) {
        value.extend(fields.clone());
    }
    capture::parse(&value.to_string())?
        .pop()
        .ok_or_else(|| deps::error("no capture"))
}

/// A run of row `id` that printed `stdout` and sent one request with
/// `amz-sdk-request` max=`max`.
fn sent(id: &str, stdout: &str, max: &str) -> Result<Capture, TaskError> {
    let header = ["amz-sdk-request", &format!("attempt=1; max={max}")];
    let request = json!({"method": "POST", "target": "/", "version": "1.1", "headers": [header],
                         "body": "", "complete": true});
    run_of(id, json!({"stdout": stdout, "requests": [request]}))
}

fn report_of(pairs: Vec<(Capture, Capture)>) -> Result<Report, TaskError> {
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
    let lines: Vec<String> = pairs
        .iter()
        .map(|(a, _)| format!("{{\"id\":\"{}\",\"argv\":[\"x\"],\"responses\":[]}}", a.id))
        .collect();
    let rows = corpus::parse("rows", &lines.join("\n"))?;
    Ok(report::build(&run, &rows, &pairs, &[]))
}

fn caught_error(report: &Report) -> String {
    check_caught(report)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

#[test]
fn the_seeded_defect_must_be_named_by_its_two_fingerprints_alone() -> Result<(), TaskError> {
    assert!(
        check_caught(&report_of(vec![(
            sent("r", PRINTED, "3")?,
            sent("r", SEEDED, "7")?
        )])?)
        .is_ok()
    );
    let cases = [sent("r", PRINTED, "3")?, sent("r", PRINTED, "7")?, {
        let mut failed = sent("r", SEEDED, "7")?;
        failed.exit = Some(255);
        failed
    }];
    for case in cases {
        let error = caught_error(&report_of(vec![(sent("r", PRINTED, "3")?, case)])?);
        assert!(
            error.starts_with("diff self-test: the seeded defect must make"),
            "{error}"
        );
    }
    Ok(())
}

/// G-018: each row must show both fingerprints, not the rows between them.
#[test]
fn each_row_must_show_both_fingerprints() -> Result<(), TaskError> {
    let header_only = (sent("r1", PRINTED, "3")?, sent("r1", PRINTED, "7")?);
    let stdout_only = (sent("r2", PRINTED, "3")?, sent("r2", SEEDED, "3")?);
    let error = caught_error(&report_of(vec![header_only, stdout_only])?);
    assert!(
        error.starts_with("diff self-test: the seeded defect must make"),
        "{error}"
    );
    Ok(())
}

/// The stand-in's runs on A, each row's as `fields` give it.
fn standin(rows: &[(&str, Value)]) -> Result<Vec<Capture>, TaskError> {
    rows.iter()
        .map(|(id, fields)| run_of(id, fields.clone()))
        .collect()
}

fn standin_runs(env: &str, limit: &str) -> Result<Vec<Capture>, TaskError> {
    let get = json!({"method": "GET", "target": "/", "version": "1.1", "headers": [], "body": "",
                     "complete": true});
    let requests = |count: usize| vec![get.clone(); count];
    let chunked = json!({"method": "PUT", "target": "/chunked", "version": "1.1", "headers": [],
                         "body": "hello", "framing": ["3", "2", "0"], "complete": true});
    standin(&[
        ("standin-chunked", json!({"requests": [chunked]})),
        ("standin-env", json!({"stdout": env})),
        (
            "standin-replies",
            json!({"stdout": "200 one\n201 two\n201 two\n", "requests": requests(3)}),
        ),
        (
            "standin-limit",
            json!({"stdout": limit, "requests": requests(66)}),
        ),
    ])
}

#[test]
fn the_stand_in_must_see_what_the_capture_promises() -> Result<(), TaskError> {
    let env = format!("{}\nHOME=/h\n", ENV_HOLDS.join("\n"));
    let limit = format!("{}{}", "200 x\n".repeat(64), "closed\n".repeat(2));
    let unparsed = json!({"harness": ["protocol_error: a connection closed in request_headers"]});
    let hung = json!({"exit": null, "timed_out": true});
    let pairs = vec![
        (
            run_of("standin-malformed", unparsed.clone())?,
            run_of("standin-malformed", unparsed)?,
        ),
        (
            run_of("standin-timeout", hung.clone())?,
            run_of("standin-timeout", hung)?,
        ),
    ];
    let report = report_of(pairs)?;
    assert!(check_standin(&report, &standin_runs(&env, &limit)?).is_ok());
    let failures = [
        standin_runs(&format!("{env}AWS_DEFAULT_REGION=us-west-2\n"), &limit)?,
        standin_runs(&env.replace("AWS_PAGER=\n", ""), &limit)?,
        standin_runs(&env, &"200 x\n".repeat(66))?,
    ];
    for runs in failures {
        let error = check_standin(&report, &runs)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(error.starts_with("diff self-test: "), "{error}");
    }
    let quiet = report_of(vec![(
        run_of("standin-timeout", json!({}))?,
        run_of("standin-timeout", json!({}))?,
    )])?;
    assert!(check_standin(&quiet, &standin_runs(&env, &limit)?).is_err());
    Ok(())
}

#[test]
fn the_scripts_quote_their_paths() {
    let script = wrapper_script(Path::new("/a b/it's/aws"));
    assert!(script.starts_with("#!/bin/bash\n"), "{script}");
    assert!(
        script.contains("\n'/a b/it'\\''s/aws' \"$@\" | sed "),
        "{script}"
    );
    let script = standin_script(Path::new("/a b/it's/standin.py"));
    assert_eq!(
        script,
        "#!/bin/bash\nexec python3 -I -B '/a b/it'\\''s/standin.py' \"$@\"\n"
    );
}

/// The wrapper, around a stand-in CLI, changes one stdout byte and the
/// attempts, and keeps the exit status.
#[cfg(unix)]
#[test]
fn the_wrapper_changes_one_byte_and_the_attempts() -> Result<(), TaskError> {
    let directory = std::env::temp_dir().join(format!("xtask-diff-seeded-{}", std::process::id()));
    let cli = directory.join("cli");
    let printed = "{\n    \"ThingId\": \"thing-0\",\n    \"Name\": \"n1\"\n}\n";
    let script = format!(
        "#!/bin/bash\nprintf '%s' '{printed}'\necho \"$AWS_MAX_ATTEMPTS $*\" >&2\nexit 3\n"
    );
    // Both scripts run right after they're written, while other tests fork.
    fs::create_dir_all(&directory)?;
    deps::write_executable_out_of_process(&cli, &script)?;
    let wrapper = directory.join("seeded");
    deps::write_executable_out_of_process(&wrapper, &wrapper_script(&cli))?;
    let output = Command::new(&wrapper).args(["a", "b c"]).output()?;
    fs::remove_dir_all(&directory)?;
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "7 a b c\n");
    let expected = printed.replacen("thing-0", "thing-X", 1);
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    Ok(())
}
