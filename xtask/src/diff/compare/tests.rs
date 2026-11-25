use super::*;

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn request(headers: &[(&str, &str)], body: &str) -> Normal {
    Normal {
        method: "POST".to_owned(),
        version: "1.1".to_owned(),
        host: "zjson.us-west-2.amazonaws.com".to_owned(),
        path: "/".to_owned(),
        headers: pairs(headers),
        body: body.as_bytes().to_vec(),
        complete: true,
        ..Normal::default()
    }
}

fn outcome<'a>(stdout: &'a str, requests: Vec<Normal>) -> Outcome<'a> {
    Outcome {
        exit: Some(0),
        timed_out: false,
        stdout: stdout.as_bytes(),
        stderr: b"",
        requests,
    }
}

fn named(a: &Outcome<'_>, b: &Outcome<'_>) -> Vec<String> {
    diffs(a, b)
        .into_iter()
        .map(|diff| diff.fingerprint)
        .collect()
}

/// The fingerprints of two runs that differ in one request alone.
fn named_requests(x: Normal, y: Normal) -> Vec<String> {
    named(&outcome("", vec![x]), &outcome("", vec![y]))
}

#[test]
fn identical_runs_do_not_differ() {
    let run = || outcome("{}", vec![request(&[("Host", "h")], "{}")]);
    assert_eq!(diffs(&run(), &run()), []);
}

#[test]
fn the_status_names_timeouts_and_exit_codes() {
    let a = outcome("", Vec::new());
    let b = Outcome {
        exit: None,
        timed_out: true,
        ..outcome("", Vec::new())
    };
    assert_eq!(named(&a, &b), ["timeout:b", "exit:0->none"]);
    let failed = Outcome {
        exit: Some(255),
        ..outcome("", Vec::new())
    };
    let found = diffs(&a, &failed);
    assert_eq!(
        found,
        [Diff {
            fingerprint: "exit:0->255".to_owned(),
            request: None,
            a: "0".to_owned(),
            b: "255".to_owned()
        }]
    );
}

#[test]
fn json_output_is_named_by_path() {
    let a = outcome(
        r#"{"Items": [{"When": 1}, {"When": 2}], "Gone": 1, "Same": "s"}"#,
        Vec::new(),
    );
    let b = outcome(
        r#"{"Items": [{"When": 1}, {"When": 3}, {}], "New": 1, "Same": "s"}"#,
        Vec::new(),
    );
    assert_eq!(
        named(&a, &b),
        [
            "stdout:$.Gone:missing",
            "stdout:$.Items:length",
            "stdout:$.Items[].When",
            "stdout:$.New:extra"
        ]
    );
    let spaced = outcome("{\"Same\":  \"s\"}", Vec::new());
    assert_eq!(
        named(&outcome("{\"Same\": \"s\"}", Vec::new()), &spaced),
        ["stdout:json-format"]
    );
}

#[test]
fn text_output_is_named_by_its_first_differing_line_masked() {
    let a = outcome(
        "same\nthing-01 is 2025 long and goes on past forty-eight characters\n",
        Vec::new(),
    );
    let b = outcome("same\nthing-02 is 2025 long\n", Vec::new());
    let found = diffs(&a, &b);
    assert_eq!(
        found
            .first()
            .map(|diff| (diff.fingerprint.as_str(), diff.b.as_str())),
        Some((
            "stdout:text:thing-## is #### long and goes on past forty-eig",
            "thing-02 is 2025 long"
        ))
    );
    assert_eq!(
        named(&outcome("", Vec::new()), &outcome("x\n", Vec::new())),
        ["stdout:text:x"]
    );
}

#[test]
fn requests_are_named_by_count_and_line() {
    let base = request(&[], "");
    let more = outcome("", vec![base.clone(), base.clone()]);
    assert_eq!(
        named(&outcome("", vec![base.clone()]), &more),
        ["requests:count"]
    );
    let moved = Normal {
        method: "PUT".to_owned(),
        path: "/x".to_owned(),
        query: pairs(&[("b", "2"), ("a", "1")]),
        ..base.clone()
    };
    let with_query = Normal {
        query: pairs(&[("a", "1"), ("b", "2")]),
        ..base.clone()
    };
    assert_eq!(
        named_requests(with_query.clone(), moved),
        [
            "request[*].line:method",
            "request[*].line:path",
            "request[*].line:query-order"
        ]
    );
    let other = Normal {
        query: pairs(&[("a", "9"), ("b", "2"), ("c", "")]),
        ..base
    };
    assert_eq!(
        named_requests(with_query, other),
        ["request[*].line:query:a", "request[*].line:query:c"]
    );
}

#[test]
fn headers_are_named_by_name_and_kind() {
    let x = request(
        &[
            ("Host", "h"),
            ("X-Amz-Target", "T.A"),
            ("Gone", "1"),
            ("Same", "s"),
        ],
        "",
    );
    let y = request(
        &[
            ("Host", "h"),
            ("X-Amz-Target", "T.B"),
            ("same", "s"),
            ("New", "1"),
        ],
        "",
    );
    assert_eq!(
        named_requests(x, y),
        [
            "request[*].header:x-amz-target:value",
            "request[*].header:gone:missing",
            "request[*].header:same:case",
            "request[*].header:new:extra",
        ]
    );
    let x = request(&[("A", "1"), ("B", "2")], "");
    let y = request(&[("B", "2"), ("A", "1")], "");
    assert_eq!(named_requests(x, y), ["request[*].header-order"]);
}

#[test]
fn the_user_agent_is_named_by_token() {
    let agent = |value: &str| request(&[("User-Agent", value)], "");
    let x = agent("aws-cli/2 md/awscrt#0.38.1 lang/rust#1.96.0 m/E,Z md/prompt#off");
    let y = agent(
        "aws-cli/2 md/awscrt#1.0.0 lang/python#3.14.8 md/pyimpl#CPython m/E,Z md/prompt#off \
         md/fallback#x",
    );
    assert_eq!(
        named_requests(x, y),
        [
            "request[*].header:user-agent:md/awscrt",
            "request[*].header:user-agent:lang",
            "request[*].header:user-agent:md/pyimpl",
            "request[*].header:user-agent:md/fallback",
        ]
    );
    let reordered = named_requests(agent("ua/2.1 m/E"), agent("m/E ua/2.1"));
    assert_eq!(reordered, ["request[*].header:user-agent:order"]);
}

#[test]
fn bodies_are_named_by_format() {
    let json = |body| request(&[], body);
    assert_eq!(
        named_requests(json(r#"{"A": {"B": 1}}"#), json(r#"{"A": {"B": 2}}"#)),
        ["request[*].body:$.A.B"]
    );
    let form = |body| {
        request(
            &[(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=utf-8",
            )],
            body,
        )
    };
    assert_eq!(
        named_requests(
            form("Action=A&Tags.member.1.Key=k"),
            form("Action=A&Tags.member.1.Key=v")
        ),
        ["request[*].body:Tags.member.#.Key"]
    );
    let xml = |body| request(&[], body);
    assert_eq!(
        named_requests(
            xml("<?xml?><R><!-- c --><E/><C><D>1</D></C></R>"),
            xml("<?xml?><R><!-- c --><E/><C><D>2</D></C></R>")
        ),
        ["request[*].body:/R/C/D"]
    );
    let bytes = named_requests(request(&[], "\u{1}ab"), request(&[], "\u{1}ac"));
    assert_eq!(bytes, ["request[*].body:bytes"]);
    let cut = Normal {
        complete: false,
        ..request(&[], "")
    };
    assert_eq!(
        named_requests(request(&[], ""), cut),
        ["request[*].body:complete"]
    );
}

#[test]
fn a_fingerprint_appears_once_a_row_with_its_first_request() {
    let x = request(&[("A", "1")], "");
    let y = request(&[("A", "2")], "");
    let found = diffs(
        &outcome("", vec![x.clone(), x]),
        &outcome("", vec![y.clone(), y]),
    );
    let shown: Vec<_> = found
        .iter()
        .map(|diff| (diff.fingerprint.as_str(), diff.request))
        .collect();
    assert_eq!(shown, [("request[*].header:a:value", Some(0))]);
}

#[test]
fn excerpts_are_cut() {
    let long = "x".repeat(EXCERPT_CHARS + 5);
    assert_eq!(excerpt(&long), format!("{}...", "x".repeat(EXCERPT_CHARS)));
    assert_eq!(token_key("cfg/retry-mode#standard"), "cfg/retry-mode");
    assert_eq!(token_key("md/arch"), "md/arch");
    assert_eq!(token_key("Mozilla"), "Mozilla");
}
