use rize_core::state::{to_csv_checked, validate_snapshot, Store};
use serde_json::{json, Value};
use std::fs;
const NOW: i64 = 1792227600000;
fn store() -> Store {
    Store::new(None).unwrap()
}
fn directory() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
fn run(store: &mut Store, kind: &str, payload: Value, now: i64) -> Value {
    store
        .mutate(&json!({"type":kind,"payload":payload}), now)
        .unwrap()
}
fn workspace(store: &mut Store) {
    run(store, "workspace.create", json!({"displayName":"Ada"}), NOW);
}
fn entry() -> Value {
    json!({"id":"entry","start":"2026-10-17T09:00:00.123Z","end":"2026-10-17T09:30:00.456Z","description":"Implementation","app":"Code","title":"Project","status":"pending","source":"automatic","category":"productive"})
}
#[test]
fn published_v12_golden_store_parity() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/store-v1.2-golden.json")).unwrap();
    assert_eq!(
        fixture["sourceCommit"],
        "98f56dba6138280b64a7ea57c02b91b73a268b8a"
    );
    let mut s = Store::from_value(fixture["initial"].clone(), None).unwrap();
    for step in fixture["steps"].as_array().unwrap() {
        s.mutate(&step["command"], step["nowMs"].as_i64().unwrap())
            .unwrap();
    }
    assert_eq!(s.get_state(), fixture["expected"]);
}
#[test]
fn published_v12_golden_timer_parity() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/timer-v1.2-golden.json")).unwrap();
    let mut s = Store::from_value(fixture["initial"].clone(), None).unwrap();
    for step in fixture["steps"].as_array().unwrap() {
        s.mutate(&step["command"], step["nowMs"].as_i64().unwrap())
            .unwrap();
    }
    assert_eq!(s.get_state(), fixture["expected"]);
}
#[test]
fn new_workspace_is_empty_opt_in_and_copies_defensive() {
    let mut s = store();
    assert_eq!(s.get_state()["onboarded"], false);
    assert_eq!(s.get_state()["settings"]["trackingEnabled"], false);
    let mut copy = s.get_state();
    copy["entries"].as_array_mut().unwrap().push(entry());
    assert_eq!(s.get_state()["entries"].as_array().unwrap().len(), 0);
    workspace(&mut s);
    assert_eq!(s.get_state()["settings"]["displayName"], "Ada");
}
#[test]
fn explicit_sample_cannot_enable_tracking() {
    let mut s = store();
    let sample = run(&mut s, "workspace.demo", json!({}), NOW);
    assert_eq!(sample["entries"].as_array().unwrap().len(), 15);
    assert!(s
        .mutate(
            &json!({"type":"settings.update","payload":{"trackingEnabled":true}}),
            NOW
        )
        .unwrap_err()
        .contains("own workspace"));
    workspace(&mut s);
    assert_eq!(s.get_state()["entries"], json!([]));
}
#[test]
fn invalid_commands_preserve_everything() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "entry.upsert", entry(), NOW);
    let before = s.get_state();
    for command in [
        json!({"type":"entry.upsert","payload":{"id":"entry","end":"2026-10-17T08:00:00Z"}}),
        json!({"type":"entry.upsert","payload":{"id":"entry","start":"2026-02-30T09:00:00Z"}}),
        json!({"type":"settings.update","payload":{"idleSeconds":-1}}),
        json!({"type":"missing.command"}),
    ] {
        assert!(s.mutate(&command, NOW).is_err());
        assert_eq!(s.get_state(), before)
    }
    assert_eq!(before["entries"][0]["durationMs"], 1800333);
}
#[test]
fn hierarchy_derives_parents_and_reassignment_updates_dependents() {
    let mut s = store();
    workspace(&mut s);
    for id in ["a", "b"] {
        run(&mut s, "client.upsert", json!({"id":id,"name":id}), NOW);
    }
    run(
        &mut s,
        "project.upsert",
        json!({"id":"p","name":"P","clientId":"a"}),
        NOW,
    );
    run(
        &mut s,
        "task.upsert",
        json!({"id":"t","name":"T","projectId":"p"}),
        NOW,
    );
    let mut e = entry();
    e["taskId"] = json!("t");
    run(&mut s, "entry.upsert", e, NOW);
    assert_eq!(s.get_state()["entries"][0]["clientId"], "a");
    run(
        &mut s,
        "project.upsert",
        json!({"id":"p","clientId":"b"}),
        NOW,
    );
    assert_eq!(s.get_state()["entries"][0]["clientId"], "b");
    run(&mut s, "project.delete", json!({"id":"p"}), NOW);
    run(&mut s, "client.delete", json!({"id":"b"}), NOW);
    let e = &s.get_state()["entries"][0];
    assert_eq!(e["projectId"], "");
    assert_eq!(e["clientId"], "");
    assert_eq!(e["taskId"], "t");
}
#[test]
fn reviewed_entries_survive_stale_native_upserts() {
    let mut s = store();
    workspace(&mut s);
    s.upsert_activity(&[entry()], NOW).unwrap();
    run(
        &mut s,
        "entry.upsert",
        json!({"id":"entry","description":"Reviewed","status":"approved","end":"2026-10-17T09:20:00.456Z"}),
        NOW,
    );
    let reviewed = s.get_state()["entries"][0].clone();
    let mut stale = entry();
    stale["end"] = json!("2026-10-17T10:00:00Z");
    s.upsert_activity(&[stale], NOW).unwrap();
    assert_eq!(s.get_state()["entries"][0], reviewed);
}
#[test]
fn native_pending_replaces_ongoing_entry_and_sanitizes_controls() {
    let mut s = store();
    workspace(&mut s);
    s.upsert_activity(&[entry()], NOW).unwrap();
    let mut e = entry();
    e["end"] = json!("2026-10-17T10:00:00.000Z");
    e["title"] = json!("Window\u{0} title\u{7}");
    s.upsert_activity(&[e], NOW).unwrap();
    let state = s.get_state();
    assert_eq!(state["entries"].as_array().unwrap().len(), 1);
    assert_eq!(state["entries"][0]["title"], "Window title");
    assert_eq!(state["entries"][0]["durationMs"], 3599877);
}
#[test]
fn timer_pauses_never_become_recorded_work() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "timer.start", json!({"goal":"Focused"}), NOW);
    run(&mut s, "timer.pause", json!({}), NOW + 600000);
    run(&mut s, "timer.resume", json!({}), NOW + 1800000);
    let final_state = run(&mut s, "timer.stop", json!({}), NOW + 2100000);
    assert_eq!(final_state["timer"], Value::Null);
    assert_eq!(final_state["sessions"][0]["elapsedMs"], 900000);
    assert_eq!(
        final_state["sessions"][0]["segments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        final_state["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["durationMs"].as_i64().unwrap())
            .sum::<i64>(),
        900000
    );
    assert_eq!(final_state["entries"][0]["end"], "2026-10-17T09:10:00.000Z");
    assert_eq!(
        final_state["entries"][1]["start"],
        "2026-10-17T09:30:00.000Z"
    );
}
#[test]
fn backwards_clocks_do_not_create_overlap_or_negative_work() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "timer.start", json!({}), NOW);
    run(&mut s, "timer.pause", json!({}), NOW + 60000);
    run(&mut s, "timer.resume", json!({}), NOW - 1000);
    let end = run(&mut s, "timer.stop", json!({}), NOW - 1000);
    assert_eq!(end["sessions"][0]["elapsedMs"], 60000);
    assert_eq!(end["sessions"][0]["endedAt"], "2026-10-17T09:01:00.000Z");
}
#[test]
fn auto_collection_during_any_part_prevents_duplicate_timer_entries() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "timer.start", json!({}), NOW);
    run(
        &mut s,
        "settings.update",
        json!({"trackingEnabled":true}),
        NOW + 300000,
    );
    run(
        &mut s,
        "settings.update",
        json!({"trackingEnabled":false}),
        NOW + 600000,
    );
    let end = run(&mut s, "timer.stop", json!({}), NOW + 600000);
    assert_eq!(end["sessions"][0]["elapsedMs"], 600000);
    assert_eq!(end["sessions"][0]["autoCollected"], true);
    assert_eq!(end["entries"], json!([]));
}
#[test]
fn break_sessions_do_not_create_calendar_work() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "timer.start", json!({"kind":"break"}), NOW);
    run(&mut s, "timer.extend", json!({"minutes":5}), NOW);
    let end = run(&mut s, "timer.stop", json!({}), NOW + 300000);
    assert_eq!(end["sessions"][0]["elapsedMs"], 300000);
    assert_eq!(end["sessions"][0]["targetMs"], 600000);
    assert_eq!(end["entries"], json!([]));
}
#[test]
fn live_export_checkpoints_copy_and_import_does_not_count_downtime() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "timer.start", json!({}), NOW);
    let copy = s.export_snapshot(NOW + 125123).unwrap();
    assert_eq!(copy["timer"]["status"], "paused");
    assert_eq!(copy["timer"]["elapsedMs"], 125123);
    assert_eq!(s.get_state()["timer"]["status"], "running");
    let mut other = store();
    other
        .import_data(json!(copy.to_string()), NOW + 3600000)
        .unwrap();
    assert_eq!(other.get_state(), copy);
    let mut forged = copy.clone();
    forged["timer"]["elapsedMs"] = json!(60000);
    assert!(other.import_data(forged, NOW).is_err());
    assert_eq!(other.get_state(), copy);
}
#[test]
fn disk_checkpoints_exclude_application_downtime_and_clean_temporary_files() {
    let dir = directory();
    let file = dir.path().join("workspace.json");
    let mut s = Store::new(Some(file.clone())).unwrap();
    workspace(&mut s);
    run(&mut s, "timer.start", json!({}), NOW);
    s.flush(NOW + 420000).unwrap();
    assert_eq!(s.get_state()["timer"]["status"], "running");
    let mut restarted = Store::new(Some(file)).unwrap();
    assert_eq!(restarted.get_state()["timer"]["status"], "paused");
    assert_eq!(restarted.get_state()["timer"]["elapsedMs"], 420000);
    let end = run(&mut restarted, "timer.stop", json!({}), NOW + 7200000);
    assert_eq!(end["sessions"][0]["elapsedMs"], 420000);
    assert!(!fs::read_dir(dir.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}
#[test]
fn corrupt_workspace_and_failed_save_preserve_data() {
    let dir = directory();
    let corrupt = dir.path().join("corrupt.json");
    fs::write(&corrupt, "{original incomplete bytes").unwrap();
    assert!(Store::new(Some(corrupt.clone()))
        .err()
        .unwrap()
        .contains("original file was preserved"));
    assert_eq!(
        fs::read_to_string(corrupt).unwrap(),
        "{original incomplete bytes"
    );
    let blocked = dir.path().join("blocked");
    fs::write(&blocked, "sentinel").unwrap();
    let mut s = Store::new(Some(blocked.join("workspace.json"))).unwrap();
    assert!(s.mutate(&json!({"type":"workspace.create"}), NOW).is_err());
    assert_eq!(s.get_state()["onboarded"], false);
    assert_eq!(fs::read_to_string(blocked).unwrap(), "sentinel");
}
#[test]
fn csv_formulas_quotes_and_amounts_match() {
    let mut s = store();
    workspace(&mut s);
    run(
        &mut s,
        "client.upsert",
        json!({"id":"c","name":"=SUM(1,2)","hourlyRate":120}),
        NOW,
    );
    let mut e = entry();
    e["start"] = json!("2026-10-17T09:00:00Z");
    e["end"] = json!("2026-10-17T09:30:00Z");
    e["clientId"] = json!("c");
    e["description"] = json!("@danger,\"quoted\"");
    e["billable"] = json!(true);
    run(&mut s, "entry.upsert", e, NOW);
    let csv = to_csv_checked(&s.get_state()).unwrap();
    assert!(csv.starts_with('\u{feff}'));
    for expected in [
        "\"'@danger,\"\"quoted\"\"\"",
        "\"'=SUM(1,2)\"",
        "\"60.00\"",
        "\"1800\"",
    ] {
        assert!(csv.contains(expected), "missing {expected}");
    }
}
#[test]
fn imports_reject_duplicate_ids_controls_and_invalid_dates() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "entry.upsert", entry(), NOW);
    let before = s.get_state();
    let mut bad = before.clone();
    bad["entries"]
        .as_array_mut()
        .unwrap()
        .push(before["entries"][0].clone());
    assert!(s
        .import_data(bad, NOW)
        .unwrap_err()
        .contains("Duplicate entry"));
    for payload in [json!({"scheduleStart":"25:00"}), json!({"workDays":[0.5]})] {
        assert!(s
            .mutate(&json!({"type":"settings.update","payload":payload}), NOW)
            .is_err())
    }
    assert!(s.import_data(json!("{"), NOW).is_err());
    assert_eq!(s.get_state(), before);
}
#[test]
fn workspace_id_migration_is_stable_and_new_workspaces_are_distinct() {
    let mut s = store();
    workspace(&mut s);
    let first = s.get_state()["workspaceId"].clone();
    let mut legacy = s.get_state();
    legacy.as_object_mut().unwrap().remove("workspaceId");
    s.import_data(legacy, NOW).unwrap();
    let migrated = s.get_state()["workspaceId"].clone();
    assert_ne!(migrated, first);
    run(
        &mut s,
        "settings.update",
        json!({"displayName":"Stable"}),
        NOW,
    );
    assert_eq!(s.get_state()["workspaceId"], migrated);
    workspace(&mut s);
    assert_ne!(s.get_state()["workspaceId"], migrated);
}
#[test]
fn shared_merge_preserves_unreviewed_native_metadata_and_local_state() {
    let mut s = store();
    workspace(&mut s);
    let mut approved = entry();
    approved["status"] = json!("approved");
    run(&mut s, "entry.upsert", approved, NOW);
    let mut pending = entry();
    pending["id"] = json!("pending");
    pending["title"] = json!("Sensitive unreviewed");
    run(&mut s, "entry.upsert", pending, NOW);
    run(&mut s, "timer.start", json!({"goal":"Keep timer"}), NOW);
    let before = s.get_state();
    let mut approved_data = before["entries"][0].clone();
    approved_data["app"] = json!("Forged");
    approved_data["title"] = json!("Forged");
    approved_data["description"] = json!("Shared approved");
    let mut pending_data = before["entries"][1].clone();
    pending_data["description"] = json!("Overwrite");
    let merged = s
        .merge_shared(
            &[
                json!({"kind":"entries","id":"entry","data":approved_data}),
                json!({"kind":"entries","id":"pending","data":pending_data}),
            ],
            NOW,
        )
        .unwrap();
    assert_eq!(merged["settings"], before["settings"]);
    assert_eq!(merged["timer"], before["timer"]);
    assert_eq!(merged["entries"][0]["title"], "Project");
    assert_eq!(merged["entries"][0]["description"], "Shared approved");
    assert_eq!(merged["entries"][1], before["entries"][1]);
}
#[test]
fn tombstones_only_delete_previously_shared_unprotected_records() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "entry.upsert", entry(), NOW);
    s.merge_shared(&[json!({"kind":"entries","id":"remote","data":{"start":"2026-10-17T11:00:00Z","end":"2026-10-17T12:00:00Z","description":"Remote"}})],NOW).unwrap();
    let remote = s.get_state()["entries"][1].clone();
    assert_eq!(remote["app"], "Cloud sync");
    assert_eq!(remote["title"], "");
    s.merge_shared(
        &[
            json!({"kind":"entries","id":"entry","deleted":true}),
            json!({"kind":"entries","id":"remote","deleted":true}),
        ],
        NOW,
    )
    .unwrap();
    assert_eq!(s.get_state()["entries"].as_array().unwrap().len(), 1);
    assert_eq!(s.get_state()["entries"][0]["id"], "entry");
}
#[test]
fn restore_running_timer_is_rejected() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "timer.start", json!({}), NOW);
    assert!(Store::from_value(s.get_state(), None).is_err());
    let mut target = store();
    assert!(target
        .import_data(s.get_state(), NOW)
        .unwrap_err()
        .contains("paused"));
}
#[test]
fn unknown_snapshot_fields_are_not_persisted() {
    let mut source = store().get_state();
    source["secretUnknown"] = json!("not part of schema");
    let clean = validate_snapshot(&source).unwrap();
    assert!(clean.get("secretUnknown").is_none());
}
#[test]
fn local_midnight_splitting_preserves_milliseconds() {
    use chrono::{Local, TimeZone};
    let start = Local
        .with_ymd_and_hms(2026, 10, 17, 23, 59, 59)
        .earliest()
        .unwrap()
        .timestamp_millis()
        + 500;
    let end = Local
        .with_ymd_and_hms(2026, 10, 18, 0, 0, 0)
        .earliest()
        .unwrap()
        .timestamp_millis()
        + 123;
    let parts=rize_core::timer::split_at_midnight(&json!({"start":rize_core::timer::timestamp(start).unwrap(),"end":rize_core::timer::timestamp(end).unwrap()})).unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts
            .iter()
            .map(|p| rize_core::timer::millis(&p["end"]).unwrap()
                - rize_core::timer::millis(&p["start"]).unwrap())
            .collect::<Vec<_>>(),
        vec![500, 123]
    );
}
#[test]
fn timer_invalid_transitions_do_not_erase_accumulated_work() {
    let mut s = store();
    workspace(&mut s);
    run(&mut s, "timer.start", json!({"kind":"break"}), NOW);
    assert!(s.mutate(&json!({"type":"timer.start"}), NOW).is_err());
    run(&mut s, "timer.extend", json!({"minutes":5}), NOW);
    run(
        &mut s,
        "timer.update",
        json!({"goal":"Take a walk"}),
        NOW + 120000,
    );
    let paused = run(&mut s, "timer.pause", json!({}), NOW + 120000);
    assert_eq!(paused["timer"]["elapsedMs"], 120000);
    assert_eq!(paused["timer"]["targetMs"], 600000);
    for c in [
        json!({"type":"timer.pause"}),
        json!({"type":"timer.extend","payload":{"minutes":-5}}),
    ] {
        assert!(s.mutate(&c, NOW + 120000).is_err());
        assert_eq!(s.get_state(), paused);
    }
}
#[test]
fn legacy_csv_halfway_amounts_use_javascript_rounding() {
    assert_eq!(rize_core::state::fixed_decimal(0.625, 2), "0.63");
    assert_eq!(rize_core::state::fixed_decimal(1.005, 2), "1.00");
    assert_eq!(rize_core::state::fixed_decimal(-0.625, 2), "-0.63");
}
#[test]
fn timestamp_validation_rejects_leap_seconds_and_normalizes_offsets() {
    for bad in [
        "2026-02-30T09:00:00Z",
        "2026-10-17T09:00:60Z",
        "2026-10-17T24:00:00Z",
        "2026-10-17T09:00:00.1234Z",
    ] {
        assert!(
            rize_core::state::iso(&json!(bad), "Timestamp").is_err(),
            "accepted {bad}"
        );
    }
    assert_eq!(
        rize_core::state::iso(&json!("2026-10-17T14:30:00.123+05:30"), "Timestamp").unwrap(),
        "2026-10-17T09:00:00.123Z"
    );
}
