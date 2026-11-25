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
        target: "/".to_owned(),
        path: "/".to_owned(),
        headers: pairs(headers),
        body: body.as_bytes().to_vec(),
        complete: true,
        ..Normal::default()
    }
}

fn outcome<'a>(stdout: &'a str, requests: Vec<Normal>) -> Outcome<'a> {
    Outcome {
        harness: &[],
        exit: Some(0),
        timed_out: false,
        stdout: stdout.as_bytes(),
        stderr: b"",
        connects: &[],
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

fn printed(a: &str, b: &str) -> Vec<String> {
    named(&outcome(a, Vec::new()), &outcome(b, Vec::new()))
}

#[test]
fn identical_runs_do_not_differ() {
    let run = || outcome("{}", vec![request(&[("Host", "h")], "{}")]);
    assert_eq!(diffs(&run(), &run()), []);
}

#[test]
fn the_status_names_harness_errors_timeouts_and_exit_codes() {
    let a = outcome("", Vec::new());
    let errors = ["dropped_requests: 3".to_owned()];
    let b = Outcome {
        harness: &errors,
        exit: None,
        timed_out: true,
        ..outcome("", Vec::new())
    };
    assert_eq!(
        named(&a, &b),
        ["harness:b:dropped_requests", "timeout:b", "exit:0->none"]
    );
    let failed = Outcome {
        exit: Some(255),
        ..outcome("", Vec::new())
    };
    let found = diffs(&a, &failed);
    let exit = Diff {
        fingerprint: "exit:0->255".to_owned(),
        key: None,
        request: None,
        a: "0".to_owned(),
        b: "255".to_owned(),
    };
    assert_eq!(found, [exit]);
}

#[test]
fn json_output_is_named_by_path_order_and_layout() {
    let a = r#"{"Items": [{"When": 1}, {"When": 2}], "Gone": 1, "Same": "s"}"#;
    let b = r#"{"Items": [{"When": 1}, {"When": 3}, {}], "New": 1, "Same": "s"}"#;
    assert_eq!(
        printed(a, b),
        [
            "stdout:$.Items:length",
            "stdout:$.Items[].When",
            "stdout:$.Gone:missing",
            "stdout:$.New:extra"
        ]
    );
    assert_eq!(
        printed("{\"A\": 1, \"B\": 2}", "{\"B\": 2, \"A\": 1}"),
        ["stdout:$:order", "stdout:json-format"]
    );
    assert_eq!(
        printed("{\"S\": \"s\"}", "{\"S\":  \"s\"}"),
        ["stdout:json-format"]
    );
    assert_eq!(printed("[1.0]", "[1]"), ["stdout:$[]"]);
    assert_eq!(
        printed("{\"A\": [1]}", "{\"A\": [1,2]}"),
        ["stdout:$.A:length"]
    );
}

#[test]
fn text_output_names_each_differing_line_both_sides_masked() {
    let a = "same\nthing-01 is 2025 long and goes on past forty-eight characters\nend\n";
    let b = "same\nthing-02 is 2025 long\nend\n";
    let found = diffs(&outcome(a, Vec::new()), &outcome(b, Vec::new()));
    let shown: Vec<_> = found
        .iter()
        .map(|diff| (diff.fingerprint.as_str(), diff.b.as_str()))
        .collect();
    assert_eq!(
        shown,
        [(
            "stdout:text:thing-## is #### long and goes on past forty-eig->thing-## is #### long",
            "thing-02 is 2025 long"
        )]
    );
    assert_eq!(printed("", "x\n"), ["stdout:text:->x"]);
    let many: String = (0..20).map(|line| format!("{line}\n")).collect();
    let found = printed(&many, &many.replace('\n', "x\n"));
    assert_eq!(found.len(), SPAN_MAX + 1, "{found:?}");
    assert_eq!(found.first().map(String::as_str), Some("stdout:text:#->#x"));
    assert_eq!(found.last().map(String::as_str), Some("stdout:text:..."));
}

#[test]
fn requests_are_named_by_count_line_query_and_target() {
    let base = request(&[], "");
    let more = outcome("", vec![base.clone(), base.clone()]);
    assert_eq!(
        named(&outcome("", vec![base.clone()]), &more),
        ["requests:count"]
    );
    let query = |text: &str| Normal {
        target: format!("/?{text}"),
        query: Some(text.to_owned()),
        ..base.clone()
    };
    let moved = Normal {
        method: "PUT".to_owned(),
        path: "/x".to_owned(),
        ..query("b=2&a=1")
    };
    assert_eq!(
        named_requests(query("a=1&b=2"), moved),
        [
            "request[*].line:method",
            "request[*].line:path",
            "request[*].line:query-order"
        ]
    );
    assert_eq!(
        named_requests(query("a=1&b=2&c"), query("a=9&b=2&c=")),
        ["request[*].line:query:a", "request[*].line:query:c"]
    );
    assert_eq!(
        named_requests(query("a=1"), query("a=1&")),
        ["request[*].line:query-format"]
    );
    let bare = Normal {
        target: "/?".to_owned(),
        query: Some(String::new()),
        ..base.clone()
    };
    assert_eq!(
        named_requests(base.clone(), bare),
        ["request[*].line:query-format"]
    );
    let only_target = Normal {
        target: "/elsewhere".to_owned(),
        ..base.clone()
    };
    assert_eq!(
        named_requests(base, only_target),
        ["request[*].line:target:format"]
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
    let x = request(&[("M", "1, 2"), ("M", "3")], "");
    let y = request(&[("M", "1"), ("M", "2, 3")], "");
    assert_eq!(named_requests(x, y), ["request[*].header:m:split"]);
    let x = request(&[("A", "1"), ("B", "2"), ("A", "3")], "");
    let y = request(&[("A", "1"), ("A", "3"), ("B", "2")], "");
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
    let plain = |body| request(&[], body);
    assert_eq!(
        named_requests(plain(r#"{"A": {"B": 1}}"#), plain(r#"{"A": {"B": 2}}"#)),
        ["request[*].body:$.A.B"]
    );
    let form_type = (
        "Content-Type",
        "application/x-www-form-urlencoded; charset=utf-8",
    );
    let form = |body| request(&[form_type], body);
    assert_eq!(
        named_requests(
            form("Action=A&Tags.member.1.Key=k"),
            form("Action=A&Tags.member.1.Key=v")
        ),
        ["request[*].body:Tags.member.#.Key"]
    );
    assert_eq!(
        named_requests(form("Action=A&x=1"), form("Action=A&&x=1")),
        ["request[*].body-format"]
    );
    assert_eq!(
        named_requests(
            plain("<?xml?><R><!-- c --><E/><C><D>1</D></C></R>"),
            plain("<?xml?><R><!-- c --><E/><C><D>2</D></C></R>")
        ),
        ["request[*].body:/R/C/D"]
    );
    let bytes = named_requests(plain("\u{1}ab"), plain("\u{1}ac"));
    assert_eq!(bytes, ["request[*].body:bytes"]);
    let cut = Normal {
        complete: false,
        ..plain("")
    };
    assert_eq!(named_requests(plain(""), cut), ["request[*].body:complete"]);
    let chunked = |sizes: &[&str]| Normal {
        framing: Some(sizes.iter().map(|size| (*size).to_owned()).collect()),
        ..plain("hello")
    };
    assert_eq!(
        named_requests(chunked(&["5", "0"]), chunked(&["2", "3", "0"])),
        ["request[*].body:framing"]
    );
}

#[test]
fn the_proxy_connects_are_compared() {
    let hosts = |hosts: &[&str]| {
        hosts
            .iter()
            .map(|host| (*host).to_owned())
            .collect::<Vec<_>>()
    };
    let (one, other) = (hosts(&["a.example"]), hosts(&["b.example", "b.example"]));
    let a = Outcome {
        connects: &one,
        ..outcome("", Vec::new())
    };
    let b = Outcome {
        connects: &other,
        ..outcome("", Vec::new())
    };
    assert_eq!(named(&a, &b), ["connects:count", "connects:host"]);
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
    assert_eq!(
        excerpt(&long, ""),
        format!("{}...", "x".repeat(EXCERPT_CHARS))
    );
    let tail = format!("{long}a{long}");
    let shown = excerpt(&tail, &format!("{long}b"));
    assert!(shown.starts_with("...") && shown.contains('a'), "{shown}");
    assert_eq!(token_key("cfg/retry-mode#standard"), "cfg/retry-mode");
    assert_eq!(token_key("md/arch"), "md/arch");
    assert_eq!(token_key("Mozilla"), "Mozilla");
}
