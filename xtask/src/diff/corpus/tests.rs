use super::*;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

const SEED: &str = "tests/diff/corpus/seed.jsonl";

fn failure(text: &str) -> String {
    parse("rows.jsonl", text)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

#[test]
fn a_row_takes_defaults_for_what_it_leaves_out() -> Result<(), TaskError> {
    let rows = parse(
        "rows.jsonl",
        "\n{\"id\":\"r\",\"argv\":[\"sts\",\"get-caller-identity\"],\
         \"env\":{\"A\":\"1\",\"B\":null},\
         \"responses\":[{\"headers\":[[\"X\",\"1\"],[\"A\",\"2\"]],\"body\":\"{}\"}]}\n",
    )?;
    let [row] = rows.as_slice() else {
        return Err(deps::error(format!("expected one row, not {rows:?}")));
    };
    assert_eq!(row.id, "r");
    assert_eq!(row.data_path, None);
    assert_eq!(row.env.get("B"), Some(&None));
    let reply = row
        .responses
        .first()
        .map(|reply| (reply.status, reply.headers.clone()));
    let headers = vec![
        ("X".to_owned(), "1".to_owned()),
        ("A".to_owned(), "2".to_owned()),
    ];
    assert_eq!(reply, Some((200, headers)));
    assert_eq!(
        row.inputs().collect::<Vec<_>>(),
        ["sts", "get-caller-identity", "1", "1", "2", "{}"]
    );
    Ok(())
}

#[test]
fn a_malformed_row_is_named_by_its_line() {
    let row = |fields: &str| format!("{{\"id\":\"r\",\"argv\":[\"x\"],\"responses\":[]{fields}}}");
    assert!(failure(&row(",\"stdin\":\"\"")).starts_with("rows.jsonl:1: unknown field `stdin`"));
    assert_eq!(
        failure("\n{\"id\":\"r\",\"argv\":[],\"responses\":[]}"),
        "rows.jsonl:2: row r: no argv"
    );
    assert_eq!(
        failure(
            "{\"id\":\"r\",\"argv\":[\"x\"],\"responses\":[{\"body\":\"\",\"body_base64\":\"\"}]}"
        ),
        "rows.jsonl:1: row r: a reply has both body and body_base64"
    );
    assert_eq!(
        failure(&row(",\"timeout_s\":0")),
        "rows.jsonl:1: row r: timeout_s must be at least 1"
    );
    for name in OWNED {
        let owned = row(&format!(",\"env\":{{\"{name}\":\"x\"}}"));
        assert_eq!(
            failure(&owned),
            format!("rows.jsonl:1: row r: the harness owns {name}")
        );
    }
}

#[test]
fn a_reply_the_cli_may_echo_is_an_input() -> Result<(), TaskError> {
    let text = "{\"id\":\"r\",\"argv\":[\"x\"],\"responses\":[\
                {\"headers\":[[\"N\",\"h\"]],\"body\":\"b\"},\
                {\"body_base64\":\"YQ==\"}]}";
    let rows = parse("rows.jsonl", text)?;
    let inputs: Vec<&str> = rows.iter().flat_map(Row::inputs).collect();
    assert_eq!(inputs, ["x", "h", "b"]);
    Ok(())
}

#[test]
fn the_seed_corpus_loads_and_filters_by_id() -> Result<(), TaskError> {
    let files = files(&root(), Path::new("tests/diff/corpus"))?;
    assert!(files.contains(&PathBuf::from(SEED)), "{files:?}");
    let seed = [PathBuf::from(SEED)];
    assert_eq!(load(&root(), &seed, None)?.len(), 3);
    let query = load(&root(), &seed, Some("query"))?;
    assert_eq!(
        query.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["seed-query-get-thing"]
    );
    let failure =
        |filter, files: &[PathBuf]| load(&root(), files, filter).err().map(|e| e.to_string());
    assert_eq!(
        failure(Some("absent"), &seed).as_deref(),
        Some("no corpus row was selected")
    );
    let twice = [PathBuf::from(SEED), PathBuf::from(SEED)];
    assert_eq!(
        failure(None, &twice).as_deref(),
        Some("corpus row id seed-json-get-thing is not unique")
    );
    Ok(())
}

#[test]
fn the_parity_corpus_holds_one_row_per_parity_case() -> Result<(), TaskError> {
    let rows = load(&root(), &[PathBuf::from(crate::diff::PARITY_CORPUS)], None)?;
    assert_eq!(rows.len(), 86);
    let source = |row: &Row| {
        let source = row.source.as_deref().unwrap_or_default();
        let (case, masks) = source.split_once("; argparse: ").unwrap_or((source, ""));
        (case.to_owned(), masks.to_owned())
    };
    let from_cases = |row: &Row| {
        let (case, _) = source(row);
        case.starts_with("tests/parity/cases/") && case.ends_with(&format!("#{}", row.id))
    };
    assert!(rows.iter().all(from_cases));
    // The cases argparse stops before botocore's validator. zjson-measure
    // leaves out its required Unit, so botocore's call fails validation too.
    let masked: Vec<&str> = rows
        .iter()
        .filter(|row| !source(row).1.is_empty())
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(
        masked,
        [
            "dynamodb-validation",
            "dynamodb-validation-type",
            "zjson-measure",
            "zjson-validation-each-kind",
            "zjson11-validation",
            "zquery-validation-each-kind",
            "zquery-validation-missing-required",
            "zquery-validation-get-thing",
            "zrestjson-validation-missing-label"
        ]
    );
    let fake = rows.iter().filter(|row| row.data_path.is_some()).count();
    assert_eq!(fake, 58);
    Ok(())
}
