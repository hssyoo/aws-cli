//! `EnhancedErrorFormatter`'s rules where no captured case reaches them;
//! each expectation is what errorhandler.py 51-103 writes for the value.
use super::*;
use {credentials as _, timestamp as _};

fn info(pairs: Vec<(&str, Value)>) -> Map {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.into(), value);
    }
    map
}

fn shown(pairs: Vec<(&str, Value)>) -> String {
    String::from_utf8(details(&info(pairs)).unwrap_or_default()).unwrap_or_default()
}

fn strings(count: usize) -> Value {
    Value::List((0..count).map(|n| Value::String(n.to_string())).collect())
}

#[test]
fn simple_values_print_as_python_s_str() {
    assert_eq!(
        shown(vec![
            ("Flag", Value::Bool(true)),
            ("Nothing", Value::Null),
            ("Count", Value::Integer(-3)),
            ("Ratio", Value::Float(0.5)),
            ("Name", Value::String("x".into())),
        ]),
        "\nAdditional error details:\nFlag: True\nNothing: None\nCount: -3\nRatio: 0.5\nName: x\n"
    );
}

#[test]
fn collections_under_five_simple_items_print_inline() {
    let mut map = Map::new();
    map.insert("a".into(), Value::Integer(1));
    map.insert("b".into(), Value::Null);
    assert_eq!(
        shown(vec![
            ("List", strings(4)),
            ("Map", Value::Map(map)),
            ("Empty", strings(0))
        ]),
        "\nAdditional error details:\nList: [0, 1, 2, 3]\nMap: {a: 1, b: None}\nEmpty: []\n"
    );
}

#[test]
fn anything_else_is_a_complex_value_with_one_hint() {
    let hint = "Use \"--cli-error-format json\" or another error format to see the full details.\n";
    assert_eq!(
        shown(vec![
            ("Five", strings(5)),
            ("Nested", Value::List(vec![strings(1)])),
            ("Bytes", Value::Blob(b"x".to_vec())),
        ]),
        format!(
            "\nAdditional error details:\nFive: <complex value>\nNested: <complex value>\n\
             Bytes: <complex value>\n{hint}"
        )
    );
}

#[test]
fn code_and_message_in_any_case_are_not_details() {
    assert_eq!(
        shown(vec![
            ("Code", Value::String("X".into())),
            ("message", Value::String("m".into())),
            ("CODE", Value::Integer(1)),
        ]),
        ""
    );
}

/// A writer that keeps what reached it only once flushed, as the binary's
/// buffered stdout does.
#[derive(Default)]
struct Buffered {
    pending: Vec<u8>,
    flushed: Vec<u8>,
}

impl std::io::Write for Buffered {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.flushed.append(&mut self.pending);
        Ok(())
    }
}

/// The interrupt's newline reaches stdout before the exit, which flushes
/// only what a failed write left (`cli_error::flush_pending_stdout`).
#[test]
fn an_interrupt_writes_and_flushes_its_newline() -> std::io::Result<()> {
    let mut stdout = Buffered::default();
    assert_eq!(interrupted(&mut stdout)?, 130);
    assert_eq!(
        (stdout.flushed, stdout.pending),
        (b"\n".to_vec(), Vec::new())
    );
    Ok(())
}

/// A customization's parameter validation is `ParamValidationError` (252)
/// with its text; any other failure of one is the general handler's (255).
#[test]
fn hook_failures_are_validation_or_general() -> Result<(), model::shapes::ModelError> {
    let empty = br#"{"metadata": {"protocol": "json"}, "operations": {}, "shapes": {}}"#;
    let model = model::shapes::Model::from_json(empty)?;
    let text = "Parameter validation failed:\nInvalid length for parameter Name";
    let validation = call_failure(
        &CallError::Hook(client::hooks::HookError::Validation(text.into())),
        &model,
    );
    assert_eq!(validation.category, Category::ParamValidation);
    assert_eq!(validation.message, text.as_bytes());
    let failed = call_failure(
        &CallError::Hook(client::hooks::HookError::Failed("boom".into())),
        &model,
    );
    assert_eq!(failed.category, Category::General);
    assert_eq!(failed.message, b"boom");
    Ok(())
}

/// What the paginator raises prints as `GeneralExceptionHandler` prints
/// `str(exception)`, with 255; each text is the Python route's.
#[test]
fn a_pagination_failure_is_the_general_handler_s() -> Result<(), Box<dyn std::error::Error>> {
    use client::error::PaginationError;
    let model = Model::from_json(br#"{"metadata": {}, "operations": {}, "shapes": {}}"#)?;
    let cases = [
        (
            PaginationError::Pagination(
                "The same next token was received twice: {'ExclusiveStartTableName': 'tok2'}"
                    .into(),
            ),
            "Error during pagination: The same next token was received twice: \
             {'ExclusiveStartTableName': 'tok2'}",
        ),
        (
            PaginationError::Value("Bad starting token: a___b___c".into()),
            "Bad starting token: a___b___c",
        ),
        (
            PaginationError::Other("slice(None, 1, None)".into()),
            "slice(None, 1, None)",
        ),
    ];
    for (error, text) in cases {
        let failure = call_failure(&CallError::Pagination(error), &model);
        let mut rendered = Vec::new();
        cli_core::cli_error::render_command_failure(
            &mut rendered,
            &failure,
            cli_core::cli_error::ErrorFormat::Enhanced,
        )?;
        assert_eq!(
            String::from_utf8(rendered)?,
            format!("\naws: [ERROR]: {text}\n")
        );
        assert_eq!(cli_core::cli_error::exit_status(failure.category), 255);
    }
    Ok(())
}

/// stderr writes with `backslashreplace`: a lone surrogate in a detail, or
/// in a small collection of them, prints as its `\udXXX` escape.
#[test]
fn a_lone_surrogate_in_a_detail_prints_backslash_escaped() {
    let points = "no\u{0} way \u{0}".chars().map(u32::from);
    let points = points.enumerate().map(|(at, point)| match at {
        2 => 0xd800,
        8 => 0xdc80,
        _ => point,
    });
    let text = Value::from_text(model::value::Wtf8Buf::from_code_points(points));
    assert!(matches!(text, Value::PyString(_)), "{text:?}");
    assert_eq!(
        shown(vec![
            ("Reason", text.clone()),
            ("Items", Value::List(vec![text])),
        ]),
        concat!(
            "\nAdditional error details:\n",
            "Reason: no\\ud800 way \\udc80\nItems: [no\\ud800 way \\udc80]\n"
        )
    );
}

/// `modeled_fields` keeps `Code`, `Message` and the members, compared
/// without case, of the last error shape in model order whose code
/// (`error.code` when non-empty, else its name) is the error's; a shape
/// that is no exception, or that the code does not name, models nothing.
#[test]
fn only_the_coded_error_shape_s_members_are_modeled() -> Result<(), Box<dyn std::error::Error>> {
    let model = Model::from_json(
        br#"{"metadata": {}, "operations": {}, "shapes": {
            "S": {"type": "string"},
            "Coded": {"type": "structure", "exception": true,
                      "members": {"Earlier": {"shape": "S"}}},
            "Thrown": {"type": "structure", "exception": true, "error": {"code": "Coded"},
                       "members": {"Reason": {"shape": "S"}}},
            "Blank": {"type": "structure", "exception": true, "error": {"code": ""},
                      "members": {"Zed": {"shape": "S"}}},
            "Choice": {"type": "structure", "exception": true, "union": true,
                       "members": {"Pick": {"shape": "S"}}},
            "Plain": {"type": "structure", "members": {"Kept": {"shape": "S"}}}
        }}"#,
    )?;
    let fields = |code: &str| {
        let mut error = ServiceError::new("Op", 400);
        error
            .error
            .insert("Code".into(), Value::String(code.into()));
        error
            .error
            .insert("message".into(), Value::String("m".into()));
        for name in ["reason", "Earlier", "Zed", "Pick", "Kept"] {
            error.members.insert(name.into(), Value::Integer(1));
        }
        let info = error_info(&error, &model);
        info.iter()
            .map(|(key, _)| key.text().into_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(fields("Coded"), ["Code", "message", "reason"]);
    assert_eq!(fields("Blank"), ["Code", "message", "Zed"]);
    assert_eq!(fields("Choice"), ["Code", "message", "Pick"]);
    assert_eq!(fields("Plain"), ["Code", "message"]);
    assert_eq!(fields("Thrown"), ["Code", "message"]);
    Ok(())
}
