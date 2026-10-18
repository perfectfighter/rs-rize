use rize_core::{business::*, state::Store};
use serde_json::{json, Value};
fn golden() -> Value {
    serde_json::from_str(include_str!("fixtures/business-v1.2-golden.json")).unwrap()
}
fn now() -> i64 {
    1_792_238_400_000
}
fn canonical(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(canonical),
        Value::Object(m) => m.values_mut().for_each(canonical),
        Value::Number(n) if n.is_f64() && n.as_f64().unwrap().fract() == 0. => {
            *v = json!(n.as_f64().unwrap() as i64)
        }
        _ => (),
    }
}
fn same(a: Value, b: Value) {
    let (mut a, mut b) = (a, b);
    canonical(&mut a);
    canonical(&mut b);
    assert_eq!(a, b)
}
fn command(state: &mut Value, kind: &str, p: Value) {
    assert!(apply_business_command(state, &json!({"type":kind,"payload":p}), now()).unwrap())
}

#[test]
fn exact_money_matches_baseline_bigint_goldens() {
    let g = golden();
    for c in g["money"].as_array().unwrap() {
        let a = &c["input"];
        assert_eq!(
            charge(
                a[0].as_f64().unwrap(),
                a[1].as_f64().unwrap(),
                a[2].as_u64().unwrap() as u32
            )
            .unwrap(),
            c["result"].as_u64().unwrap()
        );
    }
    assert_eq!(currency_digits("JPY").unwrap(), 0);
    assert_eq!(currency_digits("KWD").unwrap(), 3);
    assert!(currency_digits("ZZZ").is_err());
    assert!(charge(f64::INFINITY, 1., 2).is_err());
}
#[test]
fn baseline_invoice_snapshot_validates_without_accounting_changes() {
    let g = golden();
    let invoice = &g["final"]["business"]["invoices"][0];
    same(validate_invoice(invoice).unwrap(), invoice.clone());
    assert_invoice_entries(invoice, &g["final"]).unwrap();
    same(
        validate_business(&g["final"]["business"], &g["final"]).unwrap(),
        g["final"]["business"].clone(),
    );
}
#[test]
fn invoice_creation_and_rebuild_match_baseline_fixed_period_allocation() {
    let g = golden();
    let mut state = g["source"].clone();
    command(&mut state, "invoice.create", g["payload"].clone());
    let mut actual = state["business"]["invoices"][0].clone();
    actual["id"] = g["draft"]["id"].clone();
    same(actual, g["draft"].clone());
    assert_eq!(state["business"]["nextInvoiceNumber"], 2);
    let id = state["business"]["invoices"][0]["id"].clone();
    command(
        &mut state,
        "invoice.update",
        json!({"id":id,"notes":"Rebuilt","taxBps":500}),
    );
    assert_eq!(state["business"]["invoices"][0]["number"], "BILL-00001");
    assert_eq!(state["business"]["invoices"][0]["totalMinor"], 14700);
    assert_eq!(state["business"]["nextInvoiceNumber"], 2);
}
#[test]
fn full_and_narrow_summary_match_node_fixed_fee_allowance_budget_goldens() {
    let g = golden();
    same(
        financial_summary(&g["final"], &json!({})).unwrap(),
        g["summary"].clone(),
    );
    same(
        financial_summary(
            &g["final"],
            &json!({"startDate":"2026-10-18","endDate":"2026-10-18"}),
        )
        .unwrap(),
        g["narrowSummary"].clone(),
    );
}
#[test]
fn html_matches_baseline_and_escapes_editable_fields() {
    let g = golden();
    let html = invoice_html(&g["final"]["business"]["invoices"][0]).unwrap();
    assert_eq!(html, g["html"].as_str().unwrap());
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("default-src 'none'"));
}
#[test]
fn immutable_identity_frozen_fields_and_append_only_payments() {
    let g = golden();
    let draft = &g["draft"];
    let partial = &g["final"]["business"]["invoices"][0];
    assert_invoice_transition(None, Some(draft)).unwrap();
    assert!(assert_invoice_transition(Some(draft), None)
        .unwrap_err()
        .contains("cannot be deleted"));
    let mut forged = draft.clone();
    forged["number"] = json!("NEW-1");
    assert!(assert_invoice_transition(Some(draft), Some(&forged))
        .unwrap_err()
        .contains("identity"));
    let mut sent = draft.clone();
    sent["status"] = json!("sent");
    assert_invoice_transition(Some(&sent), Some(partial)).unwrap();
    forged = partial.clone();
    forged["notes"] = json!("Rewrite");
    assert!(assert_invoice_transition(Some(partial), Some(&forged))
        .unwrap_err()
        .contains("frozen"));
    forged = partial.clone();
    forged["payments"][0]["reference"] = json!("Rewrite");
    assert!(assert_invoice_transition(Some(partial), Some(&forged))
        .unwrap_err()
        .contains("rewritten"));
    let mut void = sent.clone();
    void["status"] = json!("void");
    void["voidReason"] = json!("Canceled");
    assert_invoice_transition(Some(&sent), Some(&void)).unwrap();
    assert!(assert_invoice_transition(Some(&void), Some(&sent))
        .unwrap_err()
        .contains("reopened"));
}
#[test]
fn forged_amounts_precision_payments_and_source_quantities_rejected() {
    let g = golden();
    let invoice = &g["draft"];
    for key in [
        "totalMinor",
        "subtotalMinor",
        "taxMinor",
        "paidMinor",
        "outstandingMinor",
    ] {
        let mut bad = invoice.clone();
        bad[key] = json!(bad[key].as_u64().unwrap() + 1);
        assert!(validate_invoice(&bad).is_err(), "{key}");
    }
    let mut bad = invoice.clone();
    bad["currencyDigits"] = json!(3);
    assert!(validate_invoice(&bad).is_err());
    bad = invoice.clone();
    bad["lines"][1]["amountMinor"] = json!(5);
    assert!(validate_invoice(&bad).is_err());
    for change in [
        json!({"status":"pending"}),
        json!({"billable":false}),
        json!({"clientId":"other"}),
    ] {
        let mut state = g["source"].clone();
        state["entries"][0]
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert!(assert_invoice_entries(invoice, &state).is_err());
    }
    bad = invoice.clone();
    bad["lines"][1]["durationMs"] = json!(1800000);
    bad["lines"][1]["quantityHours"] = json!(0.5);
    assert!(assert_invoice_entries(&bad, &g["source"]).is_err());
}
#[test]
fn invoice_payments_are_transactional_and_void_preserves_number() {
    let g = golden();
    let mut state = g["source"].clone();
    command(&mut state, "invoice.create", g["payload"].clone());
    let id = state["business"]["invoices"][0]["id"].clone();
    command(&mut state, "invoice.markSent", json!({"id":id}));
    let before = state.clone();
    assert!(apply_business_command(
        &mut state,
        &json!({"type":"invoice.payment","payload":{"id":id,"amountMinor":999999}}),
        now()
    )
    .is_err());
    assert_eq!(state, before);
    command(
        &mut state,
        "invoice.void",
        json!({"id":id,"reason":"Reissue"}),
    );
    command(
        &mut state,
        "business.profile.update",
        json!({"invoicePrefix":"NEW-"}),
    );
    command(&mut state, "invoice.create", g["payload"].clone());
    assert_eq!(state["business"]["invoices"][0]["number"], "BILL-00001");
    assert_eq!(state["business"]["invoices"][1]["number"], "NEW-00002");
    assert_eq!(state["business"]["nextInvoiceNumber"], 3);
}
#[test]
fn duplicate_contract_fee_and_overlapping_scope_rejected_atomically() {
    let g = golden();
    let mut state = g["source"].clone();
    let mut contract = state["business"]["contracts"][0].clone();
    contract["id"] = json!("overlap");
    let before = state.clone();
    assert!(apply_business_command(
        &mut state,
        &json!({"type":"contract.upsert","payload":contract}),
        now()
    )
    .unwrap_err()
    .contains("cannot overlap"));
    assert_eq!(state, before);
    contract["projectId"] = json!("");
    command(&mut state, "contract.upsert", contract);
    let mut b = g["final"]["business"].clone();
    let mut duplicate = b["invoices"][0].clone();
    duplicate["id"] = json!("other");
    duplicate["number"] = json!("BILL-00002");
    duplicate["sequence"] = json!(2);
    b["invoices"].as_array_mut().unwrap().push(duplicate);
    b["nextInvoiceNumber"] = json!(3);
    assert!(validate_business(&b, &g["final"])
        .unwrap_err()
        .contains("fixed contract fee"));
}
#[test]
fn allocation_import_capacity_and_monday_checks_are_atomic() {
    let g = golden();
    let mut state = g["source"].clone();
    let a = json!({"id":"a","memberId":"ada","memberName":"Ada","projectId":"p","week":"2026-10-19","hours":20,"capacityHours":40});
    command(&mut state, "allocation.import", json!({"allocations":[a]}));
    let before = state.clone();
    let mut bad = a.clone();
    bad["id"] = json!("bad");
    bad["week"] = json!("2026-10-20");
    assert!(apply_business_command(
        &mut state,
        &json!({"type":"allocation.import","payload":{"allocations":[a,bad]}}),
        now()
    )
    .unwrap_err()
    .contains("Monday"));
    assert_eq!(state, before);
    let mut a2 = a.clone();
    a2["id"] = json!("a2");
    a2["capacityHours"] = json!(32);
    assert!(apply_business_command(
        &mut state,
        &json!({"type":"allocation.import","payload":{"allocations":[a,a2]}}),
        now()
    )
    .unwrap_err()
    .contains("consistent"));
    assert_eq!(state, before);
}
#[test]
fn split_merge_and_bulk_preserve_exact_intervals_and_block_invoiced_work() {
    let mut store = Store::from_value(golden()["source"].clone(), None).unwrap();
    store.mutate(&json!({"type":"entry.upsert","payload":{"id":"e0","start":"2026-10-17T09:00:00.123Z","end":"2026-10-17T09:00:05.456Z"}}),now()).unwrap();
    let mut state = store.get_state();
    state["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["id"] == "e0");
    command(
        &mut state,
        "entry.split",
        json!({"id":"e0","at":"2026-10-17T09:00:02.345Z"}),
    );
    assert_eq!(
        state["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["durationMs"].as_u64().unwrap())
            .sum::<u64>(),
        5333
    );
    let ids = state["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].clone())
        .collect::<Vec<_>>();
    command(&mut state, "entry.merge", json!({"ids":ids}));
    assert_eq!(state["entries"][0]["durationMs"], 5333);
    assert_eq!(state["entries"][0]["start"], "2026-10-17T09:00:00.123Z");
    let id = state["entries"][0]["id"].clone();
    let before = state.clone();
    assert!(apply_business_command(&mut state,&json!({"type":"entry.bulk","payload":{"ids":[id],"patch":{"start":"2026-10-17T00:00:00Z"}}}),now()).unwrap_err().contains("unsupported"));
    assert_eq!(state, before);
    let mut invoiced = golden()["final"].clone();
    assert!(apply_business_command(
        &mut invoiced,
        &json!({"type":"entry.regenerate","payload":{"id":"e0"}}),
        now()
    )
    .unwrap_err()
    .contains("active invoice"));
}
#[test]
fn late_time_does_not_recharge_fixed_fee_or_reset_included_hours() {
    let g = golden();
    let mut state = g["source"].clone();
    state["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["id"] == "e0");
    let p = json!({"clientId":"c","startDate":"2026-10-01","endDate":"2026-10-31"});
    let before = state.clone();
    assert!(apply_business_command(&mut state,&json!({"type":"invoice.create","payload":{"clientId":"c","startDate":"2026-10-17","endDate":"2026-10-17"}}),now()).unwrap_err().contains("exact fixed contract"));
    assert_eq!(state, before);
    command(&mut state, "invoice.create", p.clone());
    assert_eq!(state["business"]["invoices"][0]["totalMinor"], 10000);
    let id = state["business"]["invoices"][0]["id"].clone();
    command(&mut state, "invoice.markSent", json!({"id":id}));
    state["entries"]
        .as_array_mut()
        .unwrap()
        .push(g["source"]["entries"][1].clone());
    command(&mut state, "invoice.create", p);
    assert_eq!(state["business"]["invoices"][1]["totalMinor"], 5000);
    assert!(state["business"]["invoices"][1]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["kind"] == "time"));
}
#[test]
fn labels_regenerate_with_local_rules_and_delete_unlinks_entries() {
    let mut store = Store::from_value(golden()["source"].clone(), None).unwrap();
    for (kind, payload) in [
        (
            "label.upsert",
            json!({"id":"focus","name":"Focus","keywords":"Code, Development","color":"#aa44cc"}),
        ),
        (
            "rule.upsert",
            json!({"id":"rule","name":"Development","keyword":"Code","action":"productive","clientId":"c","projectId":"p"}),
        ),
        (
            "entry.upsert",
            json!({"id":"e0","app":"Code","clientId":"","projectId":"","category":"neutral"}),
        ),
        ("entry.regenerate", json!({"id":"e0"})),
    ] {
        store
            .mutate(&json!({"type":kind,"payload":payload}), now())
            .unwrap();
    }
    let state = store.get_state();
    assert_eq!(state["entries"][0]["clientId"], "c");
    assert_eq!(state["entries"][0]["category"], "productive");
    assert_eq!(state["entries"][0]["labelIds"], json!(["focus"]));
    store
        .mutate(
            &json!({"type":"label.delete","payload":{"id":"focus"}}),
            now(),
        )
        .unwrap();
    assert_eq!(store.get_state()["entries"][0]["labelIds"], json!([]));
}
#[test]
fn gap_overlap_metadata_and_invalid_split_rejections_preserve_state() {
    let mut state = golden()["source"].clone();
    let before = state.clone();
    for p in [json!({"ids":["e0","e1"]}), json!({"ids":["e0","missing"]})] {
        assert!(apply_business_command(
            &mut state,
            &json!({"type":"entry.merge","payload":p}),
            now()
        )
        .is_err());
        assert_eq!(state, before);
    }
    for at in [
        "2026-10-17T09:00:00Z",
        "2026-10-17T10:00:00Z",
        "2026-10-17T09:30:00.1234Z",
    ] {
        assert!(apply_business_command(
            &mut state,
            &json!({"type":"entry.split","payload":{"id":"e0","at":at}}),
            now()
        )
        .is_err());
        assert_eq!(state, before);
    }
    command(
        &mut state,
        "entry.bulk",
        json!({"ids":["e0","e1"],"patch":{"billable":false,"status":"pending"}}),
    );
    for (i, e) in state["entries"].as_array().unwrap().iter().enumerate() {
        assert_eq!(e["start"], before["entries"][i]["start"]);
        assert_eq!(e["durationMs"], before["entries"][i]["durationMs"]);
        assert_eq!(e["status"], "pending");
        assert_eq!(e["billable"], false);
    }
}
