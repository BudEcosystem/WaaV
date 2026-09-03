//! WaaV's span attribute names must match what budmetrics' `VoiceTurnFact` reads.
//!
//! The two live in different repositories and neither can see the other at build time, so both
//! assert against the same checked-in contract — generated from budmetrics' column registry and
//! copied here verbatim.
//!
//! This matters because the failure is SILENT. Rename an attribute on one side and nothing
//! errors: the ClickHouse column just stays NULL forever, which reads as "nobody uses this
//! feature" rather than "the wire is broken". Exactly the shape of the `voice_table` contract,
//! and caught the same way.
//!
//! Regenerate with:
//! ```text
//! cd services/budmetrics && CLICKHOUSE_HOST=x CLICKHOUSE_PORT=9000 \
//!   python3 -c "..."   # see tests/fixtures/voice_span_contract.json in bud-runtime
//! ```

use std::collections::BTreeSet;

use waav_gateway::observability::voice_attrs;

const CONTRACT: &str = include_str!("voice_span_contract.json");

fn contract_attributes() -> BTreeSet<String> {
    let doc: serde_json::Value = serde_json::from_str(CONTRACT).expect("contract parses");
    doc["attributes"]
        .as_array()
        .expect("attributes array")
        .iter()
        .map(|a| {
            a["attribute"]
                .as_str()
                .expect("attribute is a string")
                .to_string()
        })
        .collect()
}

#[test]
fn waav_emits_exactly_what_voice_turn_fact_reads() {
    let expected = contract_attributes();
    let emitted: BTreeSet<String> = voice_attrs::ALL.iter().map(|s| s.to_string()).collect();

    let missing: Vec<_> = expected.difference(&emitted).collect();
    let extra: Vec<_> = emitted.difference(&expected).collect();

    assert!(
        missing.is_empty(),
        "budmetrics reads these and WaaV never emits them, so the columns stay NULL: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "WaaV emits these and no VoiceTurnFact column reads them, so the data is discarded: {extra:?}"
    );
}

#[test]
fn the_contract_is_not_empty() {
    // Guards the guard: an empty or malformed fixture would make the comparison above pass
    // vacuously, which is the failure mode that makes contract tests untrustworthy.
    assert!(
        contract_attributes().len() >= 20,
        "the contract fixture looks truncated; the comparison would pass vacuously"
    );
}
