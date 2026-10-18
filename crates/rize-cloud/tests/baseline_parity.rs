use rize_cloud::local_records;
use serde_json::Value;

#[test]
fn reviewed_work_matches_the_published_javascript_baseline() {
    // Generated once from cloud-client.cjs at the published reference commit.
    // Running or distributing Node is unnecessary for this parity check.
    let fixture: Value = serde_json::from_str(include_str!("fixtures/reviewed-work.json")).unwrap();
    assert_eq!(
        serde_json::to_value(local_records(&fixture["input"])).unwrap(),
        fixture["expected"]
    );
}
