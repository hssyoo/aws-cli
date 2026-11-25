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
        ["sts", "get-caller-identity", "1"]
    );
    Ok(())
}

#[test]
fn a_malformed_row_is_named_by_its_line() {
    assert!(
        failure("{\"id\":\"r\",\"argv\":[\"x\"],\"responses\":[],\"stdin\":\"\"}")
            .starts_with("rows.jsonl:1: unknown field `stdin`")
    );
    assert_eq!(
        failure("\n{\"id\":\"r\",\"argv\":[],\"responses\":[]}"),
        "rows.jsonl:2: row r has no argv"
    );
    assert_eq!(
        failure(
            "{\"id\":\"r\",\"argv\":[\"x\"],\"responses\":[{\"body\":\"\",\"body_base64\":\"\"}]}"
        ),
        "rows.jsonl:1: a reply of row r has both body and body_base64"
    );
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
