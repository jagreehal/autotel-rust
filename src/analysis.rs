use crate::Attributes;
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_VALUES_PER_FIELD: usize = 50;
const MAX_UNIQUE_RATIO: f64 = 0.5;
const MIN_DIFFERENCE: f64 = 0.1;
const LIMIT: usize = 20;

#[derive(Clone, Debug)]
pub struct CohortDifference {
    pub field: String,
    pub value: String,
    pub outlier_fraction: f64,
    pub baseline_fraction: f64,
    /// `outlier_fraction - baseline_fraction`: positive when the value is over-represented.
    pub difference: f64,
    pub outlier_count: usize,
    pub baseline_count: usize,
}

/// Rank the field values that separate an outlier cohort from a baseline.
///
/// Fields whose values barely repeat (ids, raw durations) cannot describe a
/// group, so they are skipped; bucket them first. Strongest difference first,
/// over-represented values ahead of absent ones on a tie.
pub fn compare_cohorts(outliers: &[Attributes], baseline: &[Attributes]) -> Vec<CohortDifference> {
    if outliers.is_empty() || baseline.is_empty() {
        return Vec::new();
    }
    let mut tally: BTreeMap<&str, BTreeMap<String, (usize, usize)>> = BTreeMap::new();
    for (events, outlier) in [(outliers, true), (baseline, false)] {
        for event in events {
            for (field, raw) in event {
                let Some(value) = value_key(raw) else {
                    continue;
                };
                let counts = tally.entry(field).or_default().entry(value).or_default();
                if outlier {
                    counts.0 += 1;
                } else {
                    counts.1 += 1;
                }
            }
        }
    }
    let total = (outliers.len() + baseline.len()) as f64;
    let mut output = Vec::new();
    for (field, values) in tally {
        if values.len() > MAX_VALUES_PER_FIELD || values.len() as f64 > total * MAX_UNIQUE_RATIO {
            continue;
        }
        for (value, (outlier_count, baseline_count)) in values {
            let outlier_fraction = outlier_count as f64 / outliers.len() as f64;
            let baseline_fraction = baseline_count as f64 / baseline.len() as f64;
            let difference = outlier_fraction - baseline_fraction;
            if difference.abs() < MIN_DIFFERENCE {
                continue;
            }
            output.push(CohortDifference {
                field: field.to_owned(),
                value,
                outlier_fraction,
                baseline_fraction,
                difference,
                outlier_count,
                baseline_count,
            });
        }
    }
    output.sort_by(|left, right| {
        right
            .difference
            .abs()
            .total_cmp(&left.difference.abs())
            .then(right.difference.total_cmp(&left.difference))
            .then_with(|| left.field.cmp(&right.field))
            .then_with(|| left.value.cmp(&right.value))
    });
    output.truncate(LIMIT);
    output
}

fn value_key(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Label a number with the range it falls in, so a field whose raw values
/// never repeat can still describe a cohort: `bucket(1500, &[1000, 2000])` is `"1000-2000"`.
pub fn bucket(value: f64, boundaries: &[f64]) -> String {
    if !value.is_finite() || boundaries.is_empty() {
        return "unknown".into();
    }
    let mut ordered = boundaries.to_vec();
    ordered.sort_by(f64::total_cmp);
    let mut lower = None;
    for boundary in &ordered {
        if value < *boundary {
            return lower.map_or_else(
                || format!("<{boundary}"),
                |lower| format!("{lower}-{boundary}"),
            );
        }
        lower = Some(boundary);
    }
    format!(">={}", ordered[ordered.len() - 1])
}
