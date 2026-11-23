use super::*;
use crate::testing::{Result, with_models};

/// sts's caller identity is admitted with no arguments, a paginated
/// operation is refused, and the census over every service writes its
/// counts to `target/service-eligibility.txt` for the report.
#[test]
fn every_operation_has_a_standing() -> Result<()> {
    with_models(|models| {
        let table = &customizations::TABLE[..];
        assert_eq!(
            standing(&models, table, "sts", "get-caller-identity"),
            Standing::NoArguments
        );
        assert_eq!(
            standing(&models, table, "zjson", "list-things"),
            Standing::Refused(PythonReasonName("unsupported_trait"))
        );
        let started = std::time::Instant::now();
        let counts = census(&models, table);
        let total: usize = counts.values().sum();
        assert!(total > 10_000, "{total} operations");
        let mut report = format!("{total} operations in {:?}\n", started.elapsed());
        for (standing, count) in &counts {
            report.push_str(&format!("{count:>6} {standing:?}\n"));
        }
        let written = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/service-eligibility.txt");
        assert!(
            std::fs::write(&written, report).is_ok(),
            "{}",
            written.display()
        );
    })
}
