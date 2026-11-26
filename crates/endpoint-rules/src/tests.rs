use super::*;
use serde_json::json;

type Error = Box<dyn std::error::Error>;

/// A rule set of `params` and `rules`.
fn program(params: serde_json::Value, rules: serde_json::Value) -> Result<Program, DecodeError> {
    let doc = json!({"version": "1.0", "parameters": params, "rules": rules});
    Program::decode(doc.to_string().as_bytes())
}

fn evaluate(program: &Program, args: &[(&str, EvalValue)]) -> Result<Outcome, EvalError> {
    let mut params = Params::new(program);
    for (name, value) in args {
        if let Some(param) = program.param(name) {
            params.set(param.slot(), value.clone());
        }
    }
    let partitions = Partitions::empty();
    program.evaluate(
        &params,
        &Library {
            partitions: &partitions,
        },
    )
}

fn region() -> serde_json::Value {
    json!({"Region": {"type": "String"}})
}

fn endpoint_rule(conditions: serde_json::Value, url: serde_json::Value) -> serde_json::Value {
    json!({"type": "endpoint", "conditions": conditions, "endpoint": {"url": url}})
}

fn url(outcome: Result<Outcome, EvalError>) -> Result<String, Error> {
    match outcome? {
        Outcome::Endpoint(endpoint) => Ok(endpoint.url),
        Outcome::Error(message) => Err(format!("rule error {message:?}").into()),
    }
}

/// The url `template` renders to with `Region` set to `r`.
fn render(template: &str) -> Result<String, Error> {
    let program = program(region(), json!([endpoint_rule(json!([]), json!(template))]))?;
    url(evaluate(&program, &[("Region", EvalValue::str("r"))]))
}

/// `{{` is `{`; before a reference `}}` is `}`; after the last `{` the
/// text stays as written, as the CRT engine leaves it.
#[test]
fn templates_unescape_as_the_crt_engine_does() -> Result<(), Error> {
    assert_eq!(render("a{{b}}{Region}c}}")?, "a{b}rc}}");
    assert_eq!(render("{{x}}")?, "{x}}");
    assert_eq!(render("{Region}{Region}")?, "rr");
    assert_eq!(render("plain}text")?, "plain}text");
    for bad in ["a}{Region}", "{Region", "{Other}", "{#x}"] {
        let decoded = program(region(), json!([endpoint_rule(json!([]), json!(bad))]));
        assert!(decoded.is_err(), "{bad}");
    }
    Ok(())
}

/// Properties are the JSON text with the references in its strings
/// resolved; braces outside strings are the document's.
#[test]
fn properties_resolve_inside_their_strings() -> Result<(), Error> {
    let rule = json!({"type": "endpoint", "conditions": [], "endpoint": {
        "url": "https://x",
        "properties": {"authSchemes": [{"name": "sigv4", "signingRegion": "{Region}",
                                        "literal": "{{a}}", "flag": true}]},
        "headers": {"b": ["{Region}", "two"], "a": ["one"]}}});
    let program = program(region(), json!([rule]))?;
    let Outcome::Endpoint(endpoint) = evaluate(&program, &[("Region", EvalValue::str("r"))])?
    else {
        return Err("no endpoint".into());
    };
    // No `{` follows `{{a}}`, so its `}}` stays as written.
    let expected = json!({"authSchemes": [{"name": "sigv4", "signingRegion": "r",
                                           "literal": "{a}}", "flag": true}]});
    assert_eq!(endpoint.properties, expected);
    let headers = [("b", vec!["r", "two"]), ("a", vec!["one"])];
    let headers = headers.map(|(n, v)| (n.to_owned(), v.into_iter().map(String::from).collect()));
    assert_eq!(endpoint.headers, headers);
    // A quote in a value would break the CRT engine's JSON: it fails.
    let quoted = evaluate(&program, &[("Region", EvalValue::str("a\"b"))]);
    assert!(quoted.is_err());
    Ok(())
}

/// A tree whose conditions hold is final: when none of its rules matches,
/// evaluation fails instead of trying the next rule.
#[test]
fn a_matched_tree_is_final() -> Result<(), Error> {
    let never = json!([{"fn": "booleanEquals", "argv": [true, false]}]);
    let rules = json!([
        {"type": "tree", "conditions": [],
         "rules": [endpoint_rule(never.clone(), json!("https://a"))]},
        endpoint_rule(json!([]), json!("https://b")),
    ]);
    let error = evaluate(&program(region(), rules)?, &[])
        .err()
        .ok_or("matched")?;
    assert_eq!(error.to_string(), "endpoint resolution failed");
    let rules = json!([
        {"type": "tree", "conditions": never,
         "rules": [endpoint_rule(json!([]), json!("https://a"))]},
        endpoint_rule(json!([]), json!("https://b")),
    ]);
    assert_eq!(url(evaluate(&program(region(), rules)?, &[]))?, "https://b");
    Ok(())
}

/// A condition's assignment is visible to the rule's later conditions and
/// its result; a sibling rule reuses the slot for its own name.
#[test]
fn assignments_scope_to_their_rule() -> Result<(), Error> {
    let assign =
        |name: &str| json!({"fn": "parseURL", "argv": [{"ref": "Region"}], "assign": name});
    let not_http = json!({"fn": "stringEquals",
                          "argv": [{"fn": "getAttr", "argv": [{"ref": "u"}, "scheme"]}, "http"]});
    let rules = json!([
        endpoint_rule(json!([assign("u"), not_http]), json!("{u#authority}-first")),
        endpoint_rule(
            json!([assign("v")]),
            json!("{v#authority}{v#normalizedPath}")
        ),
    ]);
    let program = program(region(), rules)?;
    let Program::Tree(rules) = &program;
    assert_eq!(rules.scope_len, 2);
    let got = evaluate(&program, &[("Region", EvalValue::str("https://h:1/p"))]);
    assert_eq!(url(got)?, "h:1/p/");
    let got = evaluate(&program, &[("Region", EvalValue::str("http://h"))]);
    assert_eq!(url(got)?, "h-first");
    Ok(())
}

/// Only a required parameter takes its default, and a required parameter
/// with neither value nor default fails.
#[test]
fn defaults_apply_to_required_parameters() -> Result<(), Error> {
    let params = json!({"Fips": {"type": "Boolean", "required": true, "default": true},
                        "Region": {"type": "string", "required": true}});
    let fips = json!([{"fn": "booleanEquals", "argv": [{"ref": "Fips"}, true]}]);
    let fips_rules = program(
        params,
        json!([endpoint_rule(fips, json!("https://{Region}"))]),
    )?;
    assert_eq!(
        url(evaluate(&fips_rules, &[("Region", EvalValue::str("r"))]))?,
        "https://r"
    );
    assert!(evaluate(&fips_rules, &[]).is_err());
    let unrequired = json!({"Region": {"type": "string", "default": "d"}});
    let set = json!([{"fn": "isSet", "argv": [{"ref": "Region"}]}]);
    let rules = json!([
        endpoint_rule(set, json!("https://set")),
        endpoint_rule(json!([]), json!("https://unset"))
    ]);
    assert_eq!(
        url(evaluate(&program(unrequired, rules)?, &[]))?,
        "https://unset"
    );
    Ok(())
}

/// The value of `call` as a condition: `Ok(true)` when it holds.
fn holds(call: serde_json::Value, args: &[(&str, EvalValue)]) -> Result<bool, EvalError> {
    let params = json!({"Region": {"type": "String"}, "List": {"type": "stringArray"}});
    let rules = json!([
        endpoint_rule(json!([call]), json!("https://yes")),
        endpoint_rule(json!([]), json!("https://no"))
    ]);
    let program = program(params, rules).map_err(|e| EvalError::new(e.detail()))?;
    match evaluate(&program, args)? {
        Outcome::Endpoint(endpoint) => Ok(endpoint.url == "https://yes"),
        Outcome::Error(_) => Err(EvalError::new("rule error")),
    }
}

/// An unset or mistyped argument fails evaluation; `isSet` alone takes
/// an unset value.
#[test]
fn functions_take_their_exact_types() -> Result<(), Error> {
    let r = || json!({"ref": "Region"});
    assert!(!holds(json!({"fn": "isSet", "argv": [r()]}), &[])?);
    assert!(holds(json!({"fn": "stringEquals", "argv": [r(), "x"]}), &[]).is_err());
    assert!(holds(json!({"fn": "not", "argv": [r()]}), &[]).is_err());
    let flag = [("Region", EvalValue::Bool(true))];
    assert!(holds(json!({"fn": "stringEquals", "argv": [r(), "x"]}), &flag).is_err());
    assert!(holds(
        json!({"fn": "not", "argv": [{"fn": "isSet", "argv": [r()]}]}),
        &[]
    )?);
    Ok(())
}

/// `getAttr`: a missing name fails, an index past the end (or into a value
/// that is not an array) is unset, a negative index counts from the end.
#[test]
fn get_attr_walks_as_the_crt_engine_does() -> Result<(), Error> {
    let list = [(
        "List",
        EvalValue::Array([EvalValue::str("a"), EvalValue::str("b")].into()),
    )];
    let at = |path: &str, want: &str| {
        let attr = json!({"fn": "getAttr", "argv": [{"ref": "List"}, path]});
        holds(json!({"fn": "stringEquals", "argv": [attr, want]}), &list)
    };
    assert!(at("[1]", "b")?);
    assert!(at("[-1]", "b")?);
    assert!(at("[2]", "b").is_err(), "unset into stringEquals");
    assert!(at("[-3]", "b").is_err());
    assert!(at("x", "b").is_err());
    let url = [("Region", EvalValue::str("https://h"))];
    let parsed = json!({"fn": "parseURL", "argv": [{"ref": "Region"}]});
    let attr = |path: &str| json!({"fn": "getAttr", "argv": [parsed.clone(), path]});
    assert!(holds(
        json!({"fn": "isSet", "argv": [attr("scheme")]}),
        &url
    )?);
    assert!(!holds(
        json!({"fn": "isSet", "argv": [attr("scheme[0]")]}),
        &url
    )?);
    assert!(holds(json!({"fn": "isSet", "argv": [attr("nope")]}), &url).is_err());
    let string = json!({"fn": "getAttr", "argv": [{"ref": "Region"}, "x"]});
    assert!(holds(json!({"fn": "isSet", "argv": [string]}), &url).is_err());
    Ok(())
}

/// What the decoder refuses, each with the CRT engine's text for an
/// invalid rule set.
#[test]
fn the_decoder_refuses_what_cannot_evaluate() {
    let call = |call: serde_json::Value| {
        program(
            region(),
            json!([endpoint_rule(json!([call]), json!("https://x"))]),
        )
    };
    let cases = [
        call(json!({"fn": "isSet", "argv": [{"ref": "Nope"}]})),
        call(json!({"fn": "isSet", "argv": [{"ref": "Region"}], "assign": "Region"})),
        call(json!({"fn": "aws.nothing", "argv": []})),
        call(json!({"fn": "isSet", "argv": []})),
        call(json!({"fn": "getAttr", "argv": [{"ref": "Region"}, {"ref": "Region"}]})),
        call(json!({"fn": "getAttr", "argv": [{"ref": "Region"}, "a[0].b"]})),
        call(json!({"fn": "getAttr", "argv": [{"ref": "Region"}, "a..b"]})),
        call(json!({"fn": "getAttr", "argv": [{"ref": "Region"}, "[+1]"]})),
        call(json!({"fn": "substring", "argv": [{"ref": "Region"}, 0.5, 2, false]})),
        program(json!({"R": {"type": "Number"}}), json!([])),
        program(json!({"R": {"type": "String", "default": true}}), json!([])),
        program(region(), json!([{"type": "other", "conditions": []}])),
        Program::decode(b"not json"),
    ];
    for (index, case) in cases.into_iter().enumerate() {
        let error = case.err();
        assert_eq!(
            error.map(|e| e.to_string()).as_deref(),
            Some("invalid endpoint ruleset"),
            "{index}"
        );
    }
}

#[test]
fn host_labels() {
    use stdlib::is_valid_host_label as label;
    assert!(label("a", false) && label("a-1", false) && label(&"a".repeat(63), false));
    assert!(!label("", false) && !label("-a", false) && !label("a-", false));
    assert!(!label(&"a".repeat(64), false) && !label("a.b", false) && !label("a_b", false));
    assert!(
        label("a.b-c.d", true) && !label("a..b", true) && !label(".a", true) && !label("a.", true)
    );
    assert!(!label("a-.b", true) && !label("a.-b", true));
}

/// `parseURL`'s fields for URLs the CRT parser splits its own way.
#[test]
fn parse_url_splits_as_the_crt_parser_does() -> Result<(), Error> {
    let fields = |url: &str| -> Option<Vec<String>> {
        let EvalValue::Record(fields) = stdlib::call(
            ir::FnId::ParseUrl,
            &[Some(EvalValue::str(url))],
            &Partitions::empty(),
        )
        .ok()??
        else {
            return None;
        };
        Some(
            fields
                .iter()
                .map(|(_, value)| format!("{value:?}"))
                .collect(),
        )
    };
    let joined = |url: &str| fields(url).map(|f| f.join(" "));
    assert_eq!(
        joined("https://h:8443/a/b").as_deref(),
        Some(r#"Str("https") Str("h:8443") Str("/a/b") Str("/a/b/") Bool(false)"#)
    );
    assert_eq!(
        joined("http://1.2.3.4").as_deref(),
        Some(r#"Str("http") Str("1.2.3.4") Str("") Str("/") Bool(true)"#)
    );
    assert_eq!(
        joined("https://[fe80::1%25en0]:443/").as_deref(),
        Some(r#"Str("https") Str("[fe80::1%25en0]:443") Str("/") Str("/") Bool(true)"#)
    );
    assert_eq!(
        joined("https://h?/p").as_deref(),
        Some(r#"Str("https") Str("h?") Str("/p") Str("/p/") Bool(false)"#)
    );
    assert!(
        fields("https://h/p?").is_some(),
        "an empty query is no query"
    );
    for none in [
        "https://h/p?q=1",
        "ftp://h",
        "h:80/p",
        "https://",
        "https:///p",
        "https://h:x",
        "https://h:4294967296",
        "https://[::1",
        "http:/h",
    ] {
        assert!(fields(none).is_none(), "{none}");
    }
    // The CRT parser drops an IPv6 literal's last byte, whatever it is.
    assert_eq!(
        fields("https://[::1]é").map(|f| f[4].clone()).as_deref(),
        Some("Bool(false)")
    );
    Ok(())
}

#[test]
fn ip_addresses() {
    use stdlib::{is_ipv4, is_ipv6};
    for yes in [
        "1.2.3.4",
        "255.255.255.255",
        "001.2.3.4",
        " 1.2.3.4",
        "+1.2.3.4",
        "-0.1.2.3",
        "1.2.3.4 ",
    ] {
        assert!(is_ipv4(yes.as_bytes()), "{yes}");
    }
    for no in [
        "256.1.1.1",
        "1.2.3",
        "1.2.3.4.5",
        "1.2.3.4x",
        "1234.1.1.1",
        "-1.2.3.4",
        "a.b.c.d",
        "",
    ] {
        assert!(!is_ipv4(no.as_bytes()), "{no}");
    }
    assert!(is_ipv4(b"1.2.3.4\0x"), "sscanf stops at the NUL");
    for yes in ["::1", "fe80::1%25en0", "1:2:3:4:5:6:7:8", "1::"] {
        assert!(is_ipv6(yes.as_bytes()), "{yes}");
    }
    for no in [
        "",
        ":1",
        "1:2:3:4:5:6:7",
        "1::2::3",
        "12345::",
        "fe80::1%en0",
        "fe80::1%25",
        "g::1",
    ] {
        assert!(!is_ipv6(no.as_bytes()), "{no}");
    }
}

#[test]
fn substring_and_uri_encode() -> Result<(), Error> {
    let sub = |text: &str, start, stop, reverse| {
        let args = [
            Some(EvalValue::str(text)),
            Some(EvalValue::Int(start)),
            Some(EvalValue::Int(stop)),
            Some(EvalValue::Bool(reverse)),
        ];
        stdlib::call(ir::FnId::Substring, &args, &Partitions::empty())
    };
    assert_eq!(sub("abcdef", 1, 3, false)?, Some(EvalValue::str("bc")));
    assert_eq!(sub("abcdef", 1, 3, true)?, Some(EvalValue::str("de")));
    for (start, stop) in [(3, 3), (0, 7), (-1, 2)] {
        assert_eq!(sub("abcdef", start, stop, false)?, None);
    }
    assert_eq!(sub("abcdé", 0, 1, false)?, None);
    let encoded = stdlib::call(
        ir::FnId::UriEncode,
        &[Some(EvalValue::str("a b/é~-_."))],
        &Partitions::empty(),
    )?;
    assert_eq!(encoded, Some(EvalValue::str("a%20b%2F%C3%A9~-_.")));
    Ok(())
}

/// Engine failures display the CRT engine's texts.
#[test]
fn errors_display_the_crt_texts() {
    assert_eq!(
        DecodeError::rule_set("x").to_string(),
        "invalid endpoint ruleset"
    );
    assert_eq!(
        EvalError::new("x").to_string(),
        "endpoint resolution failed"
    );
    assert_eq!(EvalError::new("x").detail(), "x");
}

fn aws(
    id: ir::FnId,
    args: &[EvalValue],
    partitions: &Partitions,
) -> Result<Option<EvalValue>, EvalError> {
    let args: Vec<_> = args.iter().cloned().map(Some).collect();
    stdlib::call(id, &args, partitions)
}

#[test]
fn arns_parse_as_the_crt_engine_does() -> Result<(), Error> {
    let parse = |arn: &str| {
        aws(
            ir::FnId::AwsParseArn,
            &[EvalValue::str(arn)],
            &Partitions::empty(),
        )
    };
    let Some(parsed) = parse("arn:aws:s3:us-west-2:123:accesspoint/a:b//c")? else {
        return Err("no arn".into());
    };
    let field = |key: &str| parsed.field(key).cloned();
    assert_eq!(field("partition"), Some(EvalValue::str("aws")));
    assert_eq!(field("region"), Some(EvalValue::str("us-west-2")));
    assert_eq!(field("accountId"), Some(EvalValue::str("123")));
    let ids = ["accesspoint", "a", "b", "", "c"].map(EvalValue::str);
    assert_eq!(field("resourceId"), Some(EvalValue::Array(ids.into())));
    assert!(
        parse("arn:aws:s3:::r")?.is_some(),
        "region and account may be empty"
    );
    for none in [
        "arn:aws:s3:r:a",
        "arx:aws:s3:r:a:x",
        "arn::s3:r:a:x",
        "arn:aws::r:a:x",
        "arn:aws:s3:r:a:",
    ] {
        assert_eq!(parse(none)?, None, "{none}");
    }
    Ok(())
}

#[test]
fn virtual_hostable_buckets() -> Result<(), Error> {
    let hostable = |bucket: &str, subdomains| {
        let args = [EvalValue::str(bucket), EvalValue::Bool(subdomains)];
        aws(
            ir::FnId::AwsIsVirtualHostableS3Bucket,
            &args,
            &Partitions::empty(),
        )
    };
    let yes = Some(EvalValue::Bool(true));
    let no = Some(EvalValue::Bool(false));
    assert_eq!(hostable("abc", false)?, yes);
    assert_eq!(hostable(&"a".repeat(63), false)?, yes);
    assert_eq!(
        hostable("a.b.c", true)?,
        yes,
        "labels may be short; the whole is 3 or more"
    );
    for (bucket, subdomains) in [
        ("ab", false),
        (&"a".repeat(64)[..], false),
        ("aBc", false),
        ("a.bc", false),
        ("1.2.3.4", true),
        ("a..b", true),
        (&format!("a.{}", "b".repeat(62))[..], true),
    ] {
        assert_eq!(hostable(bucket, subdomains)?, no, "{bucket}");
    }
    Ok(())
}

/// A partitions document with a region override, a description and two
/// regexes.
fn partitions() -> Result<Partitions, DecodeError> {
    let doc = json!({"version": "1.1", "partitions": [
        {"id": "aws", "regionRegex": "^(us|eu)\\-\\w+\\-\\d+$",
         "outputs": {"name": "aws", "dnsSuffix": "amazonaws.com", "supportsFIPS": true},
         "regions": {"us-east-1": {"description": "N. Virginia"},
                     "aws-global": {"description": "global", "dnsSuffix": "global.example",
                                    "extra": "x"}}},
        {"id": "other", "regionRegex": "^o\\-\\w+\\-\\d+$",
         "outputs": {"name": "other", "dnsSuffix": "o.example"}},
    ]});
    Partitions::decode(doc.to_string().as_bytes())
}

#[test]
fn partitions_match_regions_as_the_crt_engine_does() -> Result<(), Error> {
    let partitions = partitions()?;
    let suffix = |region: &str| -> Result<Option<EvalValue>, EvalError> {
        let found = aws(
            ir::FnId::AwsPartition,
            &[EvalValue::str(region)],
            &partitions,
        )?;
        Ok(found.and_then(|p| p.field("dnsSuffix").cloned()))
    };
    let s = EvalValue::str;
    assert_eq!(
        suffix("aws-global")?,
        Some(s("global.example")),
        "a region's override"
    );
    assert_eq!(
        suffix("us-east-1")?,
        Some(s("amazonaws.com")),
        "a description is not an output"
    );
    assert_eq!(suffix("o-west-2")?, Some(s("o.example")));
    assert_eq!(
        suffix("o-west-2x")?,
        Some(s("o.example")),
        "a match need not reach the end"
    );
    assert_eq!(
        suffix("o-we5t-2")?,
        Some(s("amazonaws.com")),
        "\\w is a letter: falls back to aws"
    );
    assert_eq!(
        suffix(&format!("o-{}-1", "w".repeat(46)))?,
        Some(s("o.example")),
        "50 bytes"
    );
    assert_eq!(
        suffix(&format!("o-{}-1", "w".repeat(47)))?,
        Some(s("amazonaws.com")),
        "51 bytes"
    );
    assert_eq!(suffix("")?, Some(s("amazonaws.com")));
    let global = aws(ir::FnId::AwsPartition, &[s("aws-global")], &partitions)?.ok_or("none")?;
    let EvalValue::Record(fields) = global else {
        return Err("not a record".into());
    };
    let keys: Vec<&str> = fields.iter().map(|(k, _)| &**k).collect();
    assert_eq!(
        keys,
        ["name", "supportsFIPS", "dnsSuffix", "extra"],
        "an override moves to the end"
    );
    let none = aws(
        ir::FnId::AwsPartition,
        &[s("us-east-1")],
        &Partitions::empty(),
    );
    assert!(none.is_err(), "no aws partition to fall back to");
    Ok(())
}

fn one_partition(regex: &str) -> serde_json::Value {
    json!({"version": "1.1", "partitions": [{"id": "aws", "regionRegex": regex, "outputs": {}}]})
}

/// Regexes outside the CRT engine's subset are refused, with its text.
#[test]
fn partitions_refuse_unsupported_regexes() {
    for regex in [
        "us-east",
        "^us$x",
        "^us\\-\\w+\\-\\d+",
        "^(us|use)\\-\\d+$",
        "^(us||eu)$",
        "^()$",
        "^+a$",
        "^a*b$",
        "^(a-b)$",
        "^[a]$",
        "^a\\$",
        &format!("^{}$", "a".repeat(59)),
    ] {
        let error = Partitions::decode(one_partition(regex).to_string().as_bytes()).err();
        assert_eq!(
            error.map(|e| e.to_string()).as_deref(),
            Some("invalid endpoint partitions"),
            "{regex}"
        );
    }
    for regex in [
        "^a*$",
        "^a.+\\-b$",
        "^(a|b)+\\-c$",
        &format!("^{}$", "a".repeat(58)),
    ] {
        assert!(
            Partitions::decode(one_partition(regex).to_string().as_bytes()).is_ok(),
            "{regex}"
        );
    }
    for bad in [
        json!({"partitions": []}),
        json!({"version": "1", "partitions": [{"outputs": {}}]}),
        json!({"version": "1", "partitions": [{"id": "a", "outputs": {"x": null}}]}),
    ] {
        assert!(
            Partitions::decode(bad.to_string().as_bytes()).is_err(),
            "{bad}"
        );
    }
}

/// The shipped partitions decode.
#[test]
fn the_shipped_partitions_decode() -> Result<(), Error> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../aws-cli/awscli/botocore/data/partitions.json"
    );
    let partitions = Partitions::decode(&std::fs::read(path)?)?;
    let name = |region: &str| -> Result<Option<EvalValue>, EvalError> {
        let found = aws(
            ir::FnId::AwsPartition,
            &[EvalValue::str(region)],
            &partitions,
        )?;
        Ok(found.and_then(|p| p.field("name").cloned()))
    };
    for (region, partition) in [
        ("us-gov-west-1", "aws-us-gov"),
        ("cn-north-9", "aws-cn"),
        ("eusc-de-east-9", "aws-eusc"),
        ("us-isob-east-9", "aws-iso-b"),
        ("mars-east-1", "aws"),
    ] {
        assert_eq!(name(region)?, Some(EvalValue::str(partition)), "{region}");
    }
    Ok(())
}
