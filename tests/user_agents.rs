use rand::{SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use user_agents::{Dataset, Error, Filter, UserAgent};

fn row(ua: &str, weight: f64, category: &str, connection: Value) -> Value {
    json!({
        "appName": "Netscape", "platform": "Test", "pluginsLength": 0,
        "screenHeight": 800, "screenWidth": 1200, "userAgent": ua,
        "vendor": "", "viewportHeight": 700, "viewportWidth": 1100,
        "deviceCategory": category, "weight": weight, "connection": connection,
        "futureProperty": {"preserved": true}
    })
}

fn fixture() -> Dataset {
    Dataset::from_json(
        &serde_json::to_vec(&json!([
            row(
                "Chrome Linux",
                3.0,
                "desktop",
                json!({"effectiveType": "4g", "rtt": 50})
            ),
            row("Safari", 1.0, "mobile", Value::Null),
            row("Zero", 0.0, "tablet", Value::Null)
        ]))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn weighted_sampling_reuses_filter_and_never_selects_zero_weight() {
    let generator = fixture().generator(None).unwrap();
    let mut rng = StdRng::seed_from_u64(42);
    let mut chrome = 0;
    for _ in 0..20_000 {
        let sample = generator.random_with_rng(&mut rng);
        assert_ne!(sample.user_agent, "Zero");
        chrome += usize::from(sample.user_agent == "Chrome Linux");
    }
    assert!(
        (14_400..15_600).contains(&chrome),
        "weighted sample count: {chrome}"
    );
    let filtered = fixture()
        .generator(Some(&Filter::from(json!({"deviceCategory": "mobile"}))))
        .unwrap();
    for _ in 0..20 {
        assert_eq!(filtered.random().user_agent, "Safari");
    }
    assert_eq!(filtered.cumulative_weight_index_pairs(), &[(1.0, 1)]);
}

#[test]
fn nested_regex_array_numeric_and_predicate_filters() {
    let dataset = fixture();
    let filter = Filter::All(vec![
        Filter::object([
            (
                "connection",
                Filter::object([
                    ("effectiveType", Filter::regex("^4g$").unwrap()),
                    ("rtt", Filter::from(json!(50.0))),
                ]),
            ),
            (
                "deviceCategory",
                Filter::All(vec![
                    Filter::regex("desktop|tablet").unwrap(),
                    "desktop".into(),
                ]),
            ),
        ]),
        Filter::regex("Linux").unwrap(),
        Filter::predicate(|profile| profile["screenWidth"].as_u64().unwrap() > 1000),
    ]);
    assert_eq!(
        dataset.generator(Some(&filter)).unwrap().user_agent,
        "Chrome Linux"
    );
    let nested_predicate = Filter::object([("connection", Filter::predicate(|v| v["rtt"] == 50))]);
    assert_eq!(
        dataset
            .generator(Some(&nested_predicate))
            .unwrap()
            .matching_len(),
        1
    );
    assert!(
        Filter::from(json!(["Chrome Linux", "Chrome Linux"]))
            .matches(&json!({"userAgent": "Chrome Linux"}))
    );
    assert!(
        !Filter::from(json!(["Safari", "Chrome Linux"]))
            .matches(&json!({"userAgent": "Chrome Linux"}))
    );
}

#[test]
fn missing_nested_properties_do_not_match() {
    let filter = Filter::from(json!({"connection": {"effectiveType": "4g"}}));
    assert!(!filter.matches(&json!({"connection": null})));
    assert!(!filter.matches(&json!({})));
    assert!(!Filter::from(json!({"missing": null})).matches(&json!({})));
    assert!(Filter::from(json!({"connection": null})).matches(&json!({"connection": null})));
    assert!(Filter::regex("^50$").unwrap().matches(&json!(50)));
}

#[test]
fn no_matches_and_zero_only_filters_return_errors() {
    let dataset = fixture();
    assert_eq!(
        dataset
            .generator(Some(&Filter::from("absent")))
            .unwrap_err(),
        Error::NoMatches
    );
    assert_eq!(
        dataset.generator(Some(&Filter::from("Zero"))).unwrap_err(),
        Error::NoPositiveWeight
    );
    assert!(UserAgent::try_random("absent test user agent").is_none());
    assert!(UserAgent::top_filtered(Some(5), "absent test user agent").is_empty());
}

#[test]
fn top_sorts_unsorted_data_preserves_original_weights_and_clones() {
    let rows = json!([
        row("Safari", 1.0, "mobile", Value::Null),
        row("Chrome", 3.0, "desktop", Value::Null),
        row("Other", 1.0, "mobile", Value::Null)
    ]);
    let dataset = Dataset::from_json(&serde_json::to_vec(&rows).unwrap()).unwrap();
    let agent = dataset.generator(None).unwrap();
    let top = agent.top(None);
    assert_eq!(
        top.iter()
            .map(|p| p.user_agent.as_str())
            .collect::<Vec<_>>(),
        ["Chrome", "Safari", "Other"]
    );
    assert_eq!(top[0].weight, 3.0);
    assert_eq!(agent.top(Some(99)).len(), 3);
    assert!(agent.top(Some(0)).is_empty());
    let mut detached = agent.top(Some(1));
    detached[0].user_agent = "mutated".into();
    assert_eq!(agent.top(Some(1))[0].user_agent, "Chrome");
    let filtered = dataset
        .generator(Some(&Filter::from(json!({"deviceCategory": "mobile"}))))
        .unwrap();
    assert!(
        filtered
            .top(None)
            .iter()
            .all(|p| p.device_category == "mobile")
    );
}

#[test]
fn fingerprint_roundtrip_preserves_unknown_and_null_fields() {
    let original = row("Test", 1.0, "desktop", Value::Null);
    let dataset = Dataset::from_json(&serde_json::to_vec(&vec![&original]).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&dataset.profiles()[0]).unwrap(),
        original
    );
    assert!(dataset.profiles()[0].extra["connection"].is_null());
}

#[test]
fn invalid_data_and_invalid_weights_are_rejected() {
    assert!(Dataset::from_gzip(b"not gzip").is_err());
    assert!(Dataset::from_json(b"[]").is_err());
    assert!(Dataset::from_json(b"[{}]").is_err());
    for weight in [-1.0, 0.0] {
        let bytes = serde_json::to_vec(&vec![row("Test", weight, "desktop", Value::Null)]).unwrap();
        assert!(Dataset::from_json(&bytes).is_err());
    }
    let bytes = serde_json::to_vec(&vec![row("", 1.0, "desktop", Value::Null)]).unwrap();
    assert!(Dataset::from_json(&bytes).is_err());
}

#[test]
fn overflowing_weight_sums_are_scaled() {
    let bytes = serde_json::to_vec(&vec![
        row("A", f64::MAX, "desktop", Value::Null),
        row("B", f64::MAX, "desktop", Value::Null),
    ])
    .unwrap();
    let generator = Dataset::from_json(&bytes).unwrap().generator(None).unwrap();
    assert_eq!(
        generator.cumulative_weight_index_pairs(),
        &[(0.5, 0), (1.0, 1)]
    );
}

#[test]
fn bundled_dataset_display_and_randomize_work() {
    let dataset = Dataset::bundled().unwrap();
    assert!(!dataset.profiles().is_empty());
    let mut agent = UserAgent::new().unwrap();
    assert_eq!(agent.to_string(), agent.user_agent);
    agent.randomize();
    assert_eq!(agent.to_string(), agent.data().user_agent);
    assert!(
        agent
            .top(Some(100))
            .windows(2)
            .all(|p| p[0].weight >= p[1].weight)
    );
    let mobile = UserAgent::with_filter(json!({"deviceCategory": "mobile"})).unwrap();
    assert_eq!(mobile.device_category, "mobile");
}

#[test]
fn cli_outputs_filtered_fingerprints_and_reports_errors() {
    use std::process::Command;
    let executable = env!("CARGO_BIN_EXE_user-agents");
    let output = Command::new(executable)
        .args(["--top", "--count", "3", "--device", "mobile", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|p| p["deviceCategory"] == "mobile"));
    assert!(
        rows.windows(2)
            .all(|p| p[0]["weight"].as_f64() >= p[1]["weight"].as_f64())
    );
    for args in [
        vec!["--regex", "["],
        vec!["--filter", "{}oops"],
        vec!["--filter", "{\"deviceCategory\":\"absent\"}"],
    ] {
        assert!(
            !Command::new(executable)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}
