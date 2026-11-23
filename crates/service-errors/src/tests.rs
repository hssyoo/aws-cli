//! `EnhancedErrorFormatter`'s rules where no captured case reaches them;
//! each expectation is what errorhandler.py 51-103 writes for the value.
use super::*;
use timestamp as _;

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
