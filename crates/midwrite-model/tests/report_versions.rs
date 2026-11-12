//! Report compatibility follows the evidence present, not development phases.

use midwrite_model::source_types::{LossCheck, SourceVerification};
use serde_json::Value;

fn source_report(bytes: &str) -> (u32, SourceVerification) {
    let report: Value = serde_json::from_str(bytes).unwrap();
    (
        report["schemaVersion"].as_u64().unwrap() as u32,
        serde_json::from_value(report["sourceVerification"].clone()).unwrap(),
    )
}

const INVITE: &str =
    include_str!("../../../fixtures/reports/model-check/team-app-invite-double-submit.json");
const CRASH: &str =
    include_str!("../../../fixtures/reports/model-check/checkout-crash-after-charge.json");

#[test]
fn committed_source_reports_keep_their_schema_versions() {
    for bytes in [INVITE, CRASH] {
        let (version, source) = source_report(bytes);
        assert_eq!(source.required_report_schema_version(), version);
        let restored: SourceVerification =
            serde_json::from_value(serde_json::to_value(&source).unwrap()).unwrap();
        assert_eq!(restored.required_report_schema_version(), version);
    }
}

#[test]
fn mixed_evidence_requires_the_highest_schema_version() {
    let (_, mut source) = source_report(CRASH);
    source.wire_log.push("held COMMIT".into());
    assert_eq!(source.required_report_schema_version(), 4);

    source
        .recovery
        .as_mut()
        .unwrap()
        .losses
        .push(LossCheck::default());
    assert_eq!(source.required_report_schema_version(), 5);

    source.recovery.as_mut().unwrap().losses.clear();
    assert_eq!(source.required_report_schema_version(), 4);
    source.wire_log.clear();
    assert_eq!(source.required_report_schema_version(), 3);
}
