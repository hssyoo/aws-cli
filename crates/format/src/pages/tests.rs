#![expect(
    clippy::panic,
    reason = "a fixture's expression that does not parse must fail the test"
)]
use super::*;

fn value(json: &str) -> Value {
    Value::from_json(json.as_bytes()).unwrap_or(Value::Null)
}

fn expression(text: &str) -> Expression {
    Expression::parse(text).unwrap_or_else(|| panic!("{text} is no expression"))
}

/// Pages from a list, then `fail` as the failure that ends them.
pub(crate) struct Listed {
    pub pages: Vec<Value>,
    pub next: usize,
    pub result_keys: Vec<Expression>,
    pub non_aggregate_keys: Vec<Expression>,
    pub non_aggregate_part: Map,
    pub resume_token: Option<String>,
    pub fail: Option<String>,
}

impl Listed {
    pub(crate) fn new(pages: Vec<Value>, result_keys: &[&str]) -> Self {
        Self {
            pages,
            next: 0,
            result_keys: result_keys.iter().map(|key| expression(key)).collect(),
            non_aggregate_keys: Vec::new(),
            non_aggregate_part: Map::new(),
            resume_token: None,
            fail: None,
        }
    }
}

impl Pages for Listed {
    type Error = String;
    /// `_record_non_aggregate_key_values` on the first page, as
    /// `PageIterator.__iter__` records it before yielding the page.
    fn next_page(&mut self) -> Option<Result<Value, String>> {
        let index = self.next;
        self.next += 1;
        let Some(page) = self.pages.get(index) else {
            return (index == self.pages.len()).then(|| self.fail.clone().map(Err))?;
        };
        if index == 0 {
            let mut part = Value::Map(Map::new());
            for key in &self.non_aggregate_keys {
                let found = key.search(page).cloned().unwrap_or(Value::Null);
                let _ = key.set(&mut part, found);
            }
            if let Value::Map(part) = part {
                self.non_aggregate_part = part;
            }
        }
        Some(Ok(page.clone()))
    }
    fn result_keys(&self) -> &[Expression] {
        &self.result_keys
    }
    fn non_aggregate_part(&self) -> &Map {
        &self.non_aggregate_part
    }
    fn resume_token(&self) -> Option<&str> {
        self.resume_token.as_deref()
    }
}

fn written(pages: &mut Listed) -> (String, Option<String>) {
    let mut bytes = Vec::new();
    let result = text(
        pages,
        pytext::codec::Errors::SurrogateEscape,
        None,
        &mut |text| {
            bytes.extend_from_slice(text);
            Ok(())
        },
    );
    let out = String::from_utf8_lossy(&bytes).into_owned();
    let error = match result {
        Ok(()) => None,
        Err(Stopped::Page(error)) => Some(error),
        Err(Stopped::Format(error)) => Some(error.message),
        Err(Stopped::Write(error)) => Some(error.to_string()),
    };
    (out, error)
}

/// Each page prints only its result keys, the first with the
/// non-aggregate part, and the resume token closes the output.
#[test]
fn each_page_prints_its_result_keys_and_the_resume_token_ends_it() {
    let mut pages = Listed::new(
        vec![
            value(r#"{"Things": ["a", "b"], "NextToken": "t1", "Count": 2}"#),
            value(r#"{"Things": ["c"], "Count": 1}"#),
        ],
        &["Things"],
    );
    pages.non_aggregate_keys = vec![expression("Count")];
    pages.resume_token = Some("token".into());
    let (out, error) = written(&mut pages);
    assert_eq!(error, None);
    assert_eq!(
        out,
        "2\nTHINGS\ta\nTHINGS\tb\nTHINGS\tc\nNEXTTOKEN\ttoken\n"
    );
}

/// A failed page ends the output after the pages before it, with no
/// resume token.
#[test]
fn a_failed_page_ends_the_output_after_the_pages_before_it() {
    let mut pages = Listed::new(vec![value(r#"{"Things": ["a"]}"#)], &["Things"]);
    pages.fail = Some("boom".into());
    pages.resume_token = Some("token".into());
    assert_eq!(
        written(&mut pages),
        ("THINGS\ta\n".to_owned(), Some("boom".to_owned()))
    );
}

/// A path through a name that holds no object raises as Python's `in` or
/// item assignment does on what it holds.
#[test]
fn setting_through_a_value_that_is_no_object_raises_its_type_error() {
    for (source, expression, message) in [
        (
            r#"{"A": null}"#,
            "A.B.C",
            "argument of type 'NoneType' is not a container or iterable",
        ),
        (
            r#"{"A": null}"#,
            "A.B",
            "'NoneType' object does not support item assignment",
        ),
        (
            r#"{"A": "xBy"}"#,
            "A.B.C",
            "string indices must be integers, not 'str'",
        ),
        (
            r#"{"A": "xyz"}"#,
            "A.B.C",
            "'str' object does not support item assignment",
        ),
        (
            r#"{"A": [1]}"#,
            "A.B",
            "list indices must be integers or slices, not str",
        ),
        (
            r#"{"A": 1}"#,
            "A.B",
            "'int' object does not support item assignment",
        ),
    ] {
        let mut source = value(source);
        let error = self::expression(expression)
            .set(&mut source, Value::Null)
            .err();
        assert_eq!(error.as_deref(), Some(message), "{expression}");
    }
    let mut source = value("{}");
    assert_eq!(self::expression("A.B").set(&mut source, value("1")), Ok(()));
    assert_eq!(source, value(r#"{"A": {"B": 1}}"#));
}

/// A dotted search stops at the first missing name or non-object.
#[test]
fn a_dotted_search_reads_through_objects_only() {
    let page = value(r#"{"A": {"B": [1]}, "S": "text"}"#);
    let search = |text: &str| expression(text).search(&page).cloned();
    assert_eq!(search("A.B"), Some(value("[1]")));
    assert_eq!(search("A.C"), None);
    assert_eq!(search("S.B"), None);
}

/// Pages with no rows hand nothing on: `format_text` writes nothing for
/// them, so a lazily started pager never starts.
#[test]
fn pages_with_no_rows_write_nothing() {
    let mut pages = Listed::new(
        vec![value(r#"{"Keys": []}"#), value(r#"{"Keys": []}"#)],
        &["Keys"],
    );
    let mut writes = 0;
    let result = text(
        &mut pages,
        pytext::codec::Errors::SurrogateEscape,
        None,
        &mut |_| {
            writes += 1;
            Ok(())
        },
    );
    assert!(result.is_ok());
    assert_eq!(writes, 0);
    let mut failing = Listed::new(vec![value(r#"{"Mixed": [{"k": 1}, "s"]}"#)], &["Mixed"]);
    let mut writes = 0;
    let result = text(
        &mut failing,
        pytext::codec::Errors::SurrogateEscape,
        None,
        &mut |_| {
            writes += 1;
            Ok(())
        },
    );
    assert!(matches!(result, Err(Stopped::Format(_))));
    assert_eq!(writes, 0);
}

/// `TextFormatter._format_response` searches each value it formats with
/// `--query`: each page's, and the resume token's, which a query of the
/// pages finds nothing in, so Python prints `None` for it.
#[test]
fn the_query_searches_each_page_and_the_resume_token() {
    let mut pages = Listed::new(
        vec![
            value(r#"{"Keys": ["a", "b"]}"#),
            value(r#"{"Keys": ["c"]}"#),
        ],
        &["Keys"],
    );
    pages.resume_token = Some("t".into());
    let query = crate::tests::stub("Keys[0]");
    let mut bytes = Vec::new();
    let result = text(
        &mut pages,
        pytext::codec::Errors::SurrogateEscape,
        Some(&query),
        &mut |text| {
            bytes.extend_from_slice(text);
            Ok(())
        },
    );
    assert!(result.is_ok());
    assert_eq!(String::from_utf8_lossy(&bytes), "a\nc\nNone\n");
}

/// A search that raises stops the pages after the text already written.
#[test]
fn a_raising_search_stops_the_pages() {
    let mut pages = Listed::new(vec![value(r#"{"Keys": ["a"]}"#)], &["Keys"]);
    let mut writes = 0;
    let result = text(
        &mut pages,
        pytext::codec::Errors::SurrogateEscape,
        Some(&crate::tests::Stub(None)),
        &mut |_| {
            writes += 1;
            Ok(())
        },
    );
    assert!(
        matches!(result, Err(Stopped::Format(error)) if error.message.starts_with("In function"))
    );
    assert_eq!(writes, 0);
}
