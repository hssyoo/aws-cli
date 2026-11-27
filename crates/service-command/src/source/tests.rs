use super::*;

/// A client reads the `s3` block's settings when its rule set takes S3's
/// built-ins, as s3's and s3control's do and sts's does not.
#[test]
fn a_rule_set_with_s3_built_ins_reads_the_s3_settings() -> crate::testing::Result<()> {
    crate::testing::with_models(|models| {
        for (command, reads) in [("s3api", true), ("s3control", true), ("sts", false)] {
            let service = models.service(command).ok_or(command)?;
            let schema = service.model.schema().service();
            let endpoints = EndpointData::load(models.search(), &service.name, schema)
                .map_err(|error| format!("{command}: {error:?}"))?;
            assert_eq!(reads_s3_settings(&endpoints), reads, "{command}");
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?
}

/// The bound invocation names its service's configured-endpoint id.
#[test]
fn a_bound_operation_needs_its_service_s_client() -> crate::testing::Result<()> {
    let files = crate::testing::files("", "");
    let verdict = crate::testing::route(&["sts", "get-caller-identity"], &[], &files)?;
    let cli_router::Verdict::Run(run) = verdict else {
        return Err(format!("not native: {verdict:?}").into());
    };
    assert_eq!(
        run.needs,
        Needs {
            service: "sts".into(),
            reads_s3_settings: false,
        }
    );
    Ok(())
}
