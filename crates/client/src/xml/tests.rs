use super::*;

/// The tree as `xml.etree.ElementTree` gives it: tag, attributes in order,
/// text, children.
fn dump(element: &Element) -> serde_json::Value {
    let attributes: Vec<serde_json::Value> = element
        .attributes
        .iter()
        .map(|(name, value)| serde_json::json!([name, value]))
        .collect();
    serde_json::json!({
        "tag": element.tag,
        "attrib": attributes,
        "text": element.text,
        "children": element.children.iter().map(dump).collect::<Vec<_>>(),
    })
}

fn outcome(document: &[u8]) -> serde_json::Value {
    match parse(document) {
        Ok(root) => serde_json::json!({ "ok": dump(&root) }),
        Err(error) => serde_json::json!({ "error": error.message }),
    }
}

const ORACLE: &str = r#"
import base64, json, sys
import xml.etree.ElementTree as ET
def dump(e):
    return {'tag': e.tag, 'attrib': [[k, v] for k, v in e.attrib.items()],
            'text': e.text, 'children': [dump(c) for c in e]}
out = []
for doc in json.loads(sys.stdin.read()):
    parser = ET.XMLParser(target=ET.TreeBuilder(), encoding='utf-8')
    try:
        parser.feed(base64.b64decode(doc))
        out.append({'ok': dump(parser.close())})
    except ET.ParseError as e:
        out.append({'error': str(e)})
sys.stdout.write(json.dumps(out))
"#;

fn corpus() -> Vec<Vec<u8>> {
    let mut documents: Vec<Vec<u8>> = [
        &b""[..],
        b"  ",
        b"<?xml version=\"1.0\"?>",
        b"<a>",
        b"<a",
        b"<a></b>",
        b"<a/><b/>",
        b"x<a/>",
        b"<a>&foo;</a>",
        b"<a>&#0;</a>",
        b"<a b=\"1\" b=\"2\"/>",
        b"<p:a/>",
        b"<a/> <?xml version=\"1.0\"?>",
        b"<a>\xff</a>",
        b"<a>\xc3\xa9\xff</a>",
        b"\n\n<a></b>",
        b"<a>\xc3\xa9</b>",
        b"<a b=1/>",
        b"<a><!-- c -- d --></a>",
        b"<a>]]></a>",
        b"<1a/>",
        b"<a>&#x110000;</a>",
        b"<a>\x01</a>",
        b"<a>text</a>trailing",
        b"<a></a >",
        b"<a ></a>",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<R xmlns=\"https://x/\"><A>1</A></R>\n",
        b"<a x='1' y=\"&lt;&amp;&#65;&#x42;\" z=\"t\tn\nr\r\n.\">t<!-- c -->u<![CDATA[<v>&]]>w\
          <b/>tail</a>",
        b"<a xmlns:p=\"u:p\" p:k=\"1\" xml:lang=\"en\"><p:b>x</p:b><c xmlns=\"u:d\"/></a>",
        b"<a>\r\nline\rtwo</a>",
        b"<a><b><c>deep</c></b><b/></a>",
        b"<a><?pi data?>x</a>",
        // A processing instruction's `?>` must follow its `<?` (found by
        // the client_xml fuzz target).
        b"<?><a/>",
        b"<a><?></a>",
        b"<??><a/>",
        b"<?x?><a/>",
        b"<a>\xe4\xb8\xad\xf0\x9f\x98\x80</a>",
        b"<a>&quot;&apos;&gt;</a>",
        b"<a></a>\n<!-- end -->\n",
        b"<a b=\"<\"/>",
        b"<a>&#xD800;</a>",
        b"<a><![CDATA[x]]</a>",
        b"<a/",
        b"</a>",
    ]
    .iter()
    .map(|document| document.to_vec())
    .collect();
    documents.push([b"<a>".repeat(20), b"x".to_vec(), b"</a>".repeat(20)].concat());
    documents
}

/// Trees and malformed-document messages agree with the toolchain
/// CPython's ElementTree over expat, as botocore parses responses.
#[test]
fn parses_as_elementtree_does() -> Result<(), Box<dyn std::error::Error>> {
    let documents = corpus();
    let encoded: Vec<String> = documents.iter().map(|d| model::base64::encode(d)).collect();
    let prefix = std::env::var_os("AWS_NATIVE_CPYTHON_PREFIX")
        .ok_or("AWS_NATIVE_CPYTHON_PREFIX must name the toolchain CPython")?;
    let mut child =
        std::process::Command::new(std::path::PathBuf::from(prefix).join("bin/python3"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["-I", "-c", ORACLE])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
    let input = serde_json::to_vec(&encoded)?;
    std::io::Write::write_all(&mut child.stdin.take().ok_or("stdin")?, &input)?;
    let expected: Vec<serde_json::Value> =
        serde_json::from_slice(&child.wait_with_output()?.stdout)?;
    assert_eq!(expected.len(), documents.len());
    let mut differences = Vec::new();
    for (document, wanted) in documents.iter().zip(&expected) {
        let actual = outcome(document);
        if &actual != wanted {
            differences.push(format!(
                "{document:?}\n  rust   {actual}\n  python {wanted}"
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    Ok(())
}

#[test]
fn local_names_drop_the_namespace_as_botocore_does() -> Result<(), XmlError> {
    let root = parse(b"<R xmlns=\"https://x/\"><A>1</A></R>")?;
    assert_eq!(root.tag, "{https://x/}R");
    assert_eq!(root.local_name(), "R");
    assert_eq!(root.children[0].local_name(), "A");
    Ok(())
}

/// Adversarial input fails without panicking or recursing without bound.
#[test]
fn deep_huge_and_truncated_documents_fail_cleanly() {
    let deep = [b"<a>".repeat(100_000), b"</a>".repeat(100_000)].concat();
    let error = parse(&deep).err().map(|error| error.message);
    assert_eq!(error.as_deref(), Some(DEPTH_MESSAGE));
    let long = [b"<a>".to_vec(), b"9".repeat(1_000_000), b"</a>".to_vec()].concat();
    assert!(parse(&long).is_ok());
    let document = b"<a x=\"1\"><b>t</b><![CDATA[z]]></a>";
    for end in 0..document.len() {
        assert!(parse(&document[..end]).is_err(), "{end}");
    }
    for byte in 0..=255_u8 {
        let _ = parse(&[b'<', b'a', b'>', byte, b'<', b'/', b'a', b'>']);
    }
}
