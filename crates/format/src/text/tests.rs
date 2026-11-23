use super::*;
use model::value::Value;

fn text(json: &str) -> Result<String, String> {
    let value = Value::from_json(json.as_bytes()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    format_text(&value, &mut out).map_err(|error| error.message)?;
    Ok(out)
}

#[test]
fn scalars_sort_into_one_row_and_children_follow_by_key() {
    let output = text(r#"{"b":2,"a":"x","z":{"k":true,"n":null},"l":["p","q"],"e":[]}"#);
    assert_eq!(output.as_deref(), Ok("x\t2\nL\tp\nL\tq\nZ\tTrue\tNone\n"));
}

#[test]
fn lists_of_structures_share_one_set_of_columns() {
    let output = text(r#"{"Items":[{"b":1,"a":2},{"c":3,"d":[4]}]}"#);
    assert_eq!(output.as_deref(), Ok("ITEMS\t2\t1\t\nITEMS\t\t\t3\nD\t4\n"));
}

#[test]
fn bare_lists_and_nested_lists_print_as_text_format_does() {
    assert_eq!(text(r#"["a",1,1.5]"#).as_deref(), Ok("a\t1\t1.5\n"));
    assert_eq!(
        text(r#"[["a"],"b",["c","d"]]"#).as_deref(),
        Ok("b\na\nc\td\n")
    );
    assert_eq!(text(r#""plain""#).as_deref(), Ok("plain\n"));
    assert_eq!(text("{}").as_deref(), Ok(""));
}

/// `_all_scalar_keys` calls `.items()` on every element of a list that
/// holds a dict, so a list mixing dicts with anything else fails as
/// Python fails, after the output written before it.
#[test]
fn a_list_mixing_structures_and_scalars_fails_as_python_does() {
    let value = Value::from_json(br#"{"a":1,"x":[{"k":1},"s"]}"#).unwrap_or(Value::Null);
    let mut out = String::new();
    let error = format_text(&value, &mut out).err();
    assert_eq!(out, "1\n");
    let message = error.map(|error| error.message);
    assert_eq!(
        message.as_deref(),
        Some("'str' object has no attribute 'items'")
    );
}
