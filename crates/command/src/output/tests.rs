use super::*;
use cli_core::SnapshotEnv;

fn files(text: &str) -> ParsedFiles {
    ParsedFiles::parse(awscli_config::Inputs {
        config: Some(awscli_config::Contents {
            source: "config",
            bytes: text.as_bytes(),
        }),
        ..Default::default()
    })
}

fn resolve(
    pairs: &[(&str, &str)],
    config: &str,
    globals: OutputGlobals<'_>,
) -> Result<OutputSettings, PythonReason> {
    let env = SnapshotEnv::from_pairs(pairs.iter().copied());
    OutputSettings::resolve(&env, &files(config), &globals, Some(0))
}

fn format(
    pairs: &[(&str, &str)],
    config: &str,
    output: Option<Option<&str>>,
) -> Result<OutputFormat, PythonReason> {
    let globals = OutputGlobals {
        output,
        ..OutputGlobals::default()
    };
    resolve(pairs, config, globals).map(|settings| settings.format)
}

/// `--output`, then the variable whenever it is set, then the profile,
/// then json; only the three native formats resolve, and a value that is
/// no Unicode text or a nested block is none of them.
#[test]
fn the_format_follows_the_cli_s_output_chain() {
    let text = "[default]\noutput = text\n";
    assert_eq!(format(&[], "", None), Ok(OutputFormat::Json));
    assert_eq!(format(&[], text, None), Ok(OutputFormat::Text));
    let variable = [("AWS_DEFAULT_OUTPUT", "off")];
    assert_eq!(format(&variable, text, None), Ok(OutputFormat::Off));
    assert_eq!(
        format(&variable, "", Some(Some("text"))),
        Ok(OutputFormat::Text)
    );
    for refused in [
        format(&[], "", Some(Some("table"))),
        format(&[], "", Some(None)),
        format(&[("AWS_DEFAULT_OUTPUT", "yaml")], "", None),
        format(&[], "[default]\noutput =\n  nested = x\n", None),
    ] {
        assert_eq!(refused, Err(PythonReason::OutputFormat));
    }
}

/// The timestamp format is parsed here, once; a value Python rejects is
/// Python's. The pager chain's inputs are carried as found.
#[test]
fn the_timestamp_format_and_the_pager_inputs_are_carried() {
    let globals = OutputGlobals {
        no_cli_pager: true,
        ..OutputGlobals::default()
    };
    let config = "[default]\ncli_timestamp_format = wire\ncli_pager = more\n";
    let resolved = resolve(&[("AWS_PAGER", "less -R")], config, globals);
    assert!(resolved.is_ok(), "{resolved:?}");
    let Ok(settings) = resolved else { return };
    let wire = TimestampParser::new(TimestampFormat::Wire, Some(0));
    assert_eq!(settings.timestamps, wire);
    let aws_pager = settings.pager.aws_pager.as_deref();
    assert_eq!(aws_pager, Some(std::ffi::OsStr::new("less -R")));
    assert_eq!(settings.pager.cli_pager, PagerProfile::Text("more".into()));
    assert!(settings.pager.chain().no_cli_pager);
    let bogus = "[default]\ncli_timestamp_format = bogus\n";
    let refused = resolve(&[], bogus, OutputGlobals::default());
    assert_eq!(refused, Err(PythonReason::OutputFormat));
}
