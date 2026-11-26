//! The comparison of the shipped models' census with the fakes', against
//! the ratchet of gaps still open (`gaps.txt`). The ratchet only shrinks:
//! every open gap must be a line of `gaps-baseline.txt`, the list as B7a
//! froze it, so no change can list a feature anew (re-listing a closed gap
//! is the merge guard's to catch).
use super::census::Census;
use std::collections::BTreeSet;

/// `gaps.txt`'s features in file order: one per line, `#` comments and
/// blank lines skipped.
pub(crate) fn listed(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// One line per problem, empty when every shipped feature has a fake or an
/// open gap and every open gap is still one.
pub(crate) fn problems(
    real: &Census,
    fakes: &Census,
    gaps: &[&str],
    baseline: &[&str],
) -> Vec<String> {
    let mut problems = Vec::new();
    let open: BTreeSet<&str> = gaps.iter().copied().collect();
    let frozen: BTreeSet<&str> = baseline.iter().copied().collect();
    for gap in open.difference(&frozen) {
        problems.push(format!(
            "gaps.txt line {gap} is not in gaps-baseline.txt: the list only shrinks; \
             fake the feature instead"
        ));
    }
    for pair in gaps.windows(2) {
        if pair[0] >= pair[1] {
            problems.push(format!(
                "gaps.txt line {} is out of order or repeated; keep the file sorted",
                pair[1]
            ));
        }
    }
    let mut missing: Vec<(&String, &BTreeSet<String>)> = real
        .iter()
        .filter(|(feature, _)| !fakes.contains_key(*feature) && !open.contains(feature.as_str()))
        .collect();
    missing.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (feature, services) in missing {
        let example = services.iter().next().map_or("-", String::as_str);
        problems.push(format!(
            "feature {feature} is used by {} shipped models (e.g. {example}) and no fake in \
             tests/models exercises it; add it to a fake (DEV-LOOP §14)",
            services.len()
        ));
    }
    for gap in &open {
        if fakes.contains_key(*gap) {
            problems.push(format!(
                "gaps.txt line {gap} is stale: a fake now exercises it; delete the line"
            ));
        } else if !real.contains_key(*gap) {
            problems.push(format!(
                "gaps.txt line {gap} is stale: no shipped model uses it; delete the line"
            ));
        }
    }
    problems
}

/// Features per family (the name's first part): shipped, covered by a fake,
/// and open in the ratchet.
pub(crate) fn summary(real: &Census, fakes: &Census, gaps: &[&str]) -> String {
    let open: BTreeSet<&str> = gaps.iter().copied().collect();
    let mut rows: std::collections::BTreeMap<&str, [usize; 3]> = Default::default();
    for feature in real.keys() {
        let family = feature.split('/').next().unwrap_or_default();
        let row = rows.entry(family).or_default();
        row[0] += 1;
        row[1] += usize::from(fakes.contains_key(feature));
        row[2] += usize::from(open.contains(feature.as_str()));
    }
    let mut text = String::from("family    shipped  faked  open\n");
    for (family, [shipped, faked, open]) in &rows {
        text.push_str(&format!("{family:<8} {shipped:>8} {faked:>6} {open:>5}\n"));
    }
    text
}
