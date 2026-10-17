use rand::{SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use user_agents::{Dataset, Error, Filter, Generator, UserAgentData};

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
fn weighted_sampling_reuses_filters_and_skips_zero_weight() {
    let generator = fixture().generator().unwrap();
    let mut rng = StdRng::seed_from_u64(42);
    let mut chrome = 0;
    for _ in 0..20_000 {
        let sample = generator.sample_with_rng(&mut rng);
        assert_ne!(sample.user_agent, "Zero");
        chrome += usize::from(sample.user_agent == "Chrome Linux");
    }
    assert!(
        (14_400..15_600).contains(&chrome),
        "weighted sample count: {chrome}"
    );
    let filtered = generator
        .filter(|profile| profile.device_category == "mobile")
        .unwrap();
    assert_eq!(filtered.len(), 1);
    for _ in 0..20 {
        assert_eq!(filtered.sample().user_agent, "Safari");
    }
    // Preparation/filtering/top do not require an initial sample or an RNG.
    let first = filtered.top(1)[0];
    assert!(std::ptr::eq(first, filtered.sample()));
}

#[test]
fn native_predicates_can_borrow_non_send_state_and_mutate_locals() {
    use std::{cell::Cell, rc::Rc};
    let category = String::from("desktop");
    let calls = Rc::new(Cell::new(0));
    let mut visited = Vec::new();
    let generator = fixture().generator().unwrap();
    let desktop = generator
        .filter(|profile| {
            calls.set(calls.get() + 1);
            visited.push(profile.user_agent.clone());
            profile.device_category == category
        })
        .unwrap();
    assert_eq!(calls.get(), 3);
    assert_eq!(visited.len(), 3);
    assert_eq!(desktop.sample().user_agent, "Chrome Linux");
}

#[test]
fn nested_regex_and_array_filters_work_as_a_json_adapter() {
    let generator = fixture().generator().unwrap();
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
    ]);
    assert_eq!(
        generator.filter_json(&filter).unwrap().sample().user_agent,
        "Chrome Linux"
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
fn numeric_equality_preserves_u64_precision_and_checks_float_cast_bounds() {
    let a = 9_007_199_254_740_992u64;
    assert!(!Filter::from(json!(a)).matches(&json!(a + 1)));
    assert!(!Filter::from(json!(a as f64)).matches(&json!(a + 1)));
    assert!(Filter::from(json!(a as f64)).matches(&json!(a)));
    for (a, b) in [
        (json!(50), json!(50.0)),
        (json!(0), json!(-0.0)),
        (json!(-50), json!(-50.0)),
        (json!(i64::MIN), json!(i64::MIN as f64)),
    ] {
        assert!(Filter::from(a.clone()).matches(&b));
        assert!(Filter::from(b).matches(&a));
    }
    for (a, b) in [
        (json!(u64::MAX), json!(u64::MAX as f64)),
        (json!(i64::MAX), json!(i64::MAX as f64)),
        (json!(50), json!(50.5)),
        (json!(-1), json!(u64::MAX)),
    ] {
        assert!(!Filter::from(a.clone()).matches(&b));
        assert!(!Filter::from(b).matches(&a));
    }
}

#[test]
fn regex_matches_strings_without_javascript_coercion() {
    let regex = Filter::regex("^50$").unwrap();
    assert!(regex.matches(&json!("50")));
    for other in [
        json!(50),
        json!(50.0),
        json!(null),
        json!(["50"]),
        json!({}),
    ] {
        assert!(!regex.matches(&other));
    }
}

#[test]
fn missing_nested_properties_do_not_match() {
    let filter = Filter::from(json!({"connection": {"effectiveType": "4g"}}));
    assert!(!filter.matches(&json!({"connection": null})));
    assert!(!filter.matches(&json!({})));
    assert!(!Filter::from(json!({"missing": null})).matches(&json!({})));
    assert!(Filter::from(json!({"connection": null})).matches(&json!({"connection": null})));
}

#[test]
fn failed_filters_return_specific_errors() {
    let generator = fixture().generator().unwrap();
    assert_eq!(
        generator.filter(|p| p.user_agent == "absent").unwrap_err(),
        Error::NoMatches
    );
    assert_eq!(
        generator.filter(|p| p.user_agent == "Zero").unwrap_err(),
        Error::NoPositiveWeight
    );
}

#[test]
fn top_sorts_unsorted_data_and_borrows_profiles_with_original_weights() {
    let dataset = Dataset::from_json(
        &serde_json::to_vec(&json!([
            row("Safari", 1.0, "mobile", Value::Null),
            row("Chrome", 3.0, "desktop", Value::Null),
            row("Other", 1.0, "mobile", Value::Null)
        ]))
        .unwrap(),
    )
    .unwrap();
    let generator = dataset.generator().unwrap();
    let top = generator.top(usize::MAX);
    assert_eq!(
        top.iter()
            .map(|p| p.user_agent.as_str())
            .collect::<Vec<_>>(),
        ["Chrome", "Safari", "Other"]
    );
    assert_eq!(top[0].weight, 3.0);
    assert!(std::ptr::eq(top[0], &dataset.profiles()[1]));
    assert_eq!(generator.top(99).len(), 3);
    assert!(generator.top(0).is_empty());
    let mut detached: UserAgentData = top[0].clone();
    detached.user_agent = "mutated".into();
    assert_eq!(generator.top(1)[0].user_agent, "Chrome");
    let filtered = generator.filter(|p| p.device_category == "mobile").unwrap();
    assert!(
        filtered
            .top(usize::MAX)
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
    for bytes in [b"[]".as_slice(), b"[{}]"] {
        assert!(Dataset::from_json(bytes).is_err());
    }
    for weight in [-1.0, 0.0] {
        assert!(
            Dataset::from_json(
                &serde_json::to_vec(&vec![row("Test", weight, "desktop", Value::Null)]).unwrap()
            )
            .is_err()
        );
    }
    assert!(
        Dataset::from_json(
            &serde_json::to_vec(&vec![row("", 1.0, "desktop", Value::Null)]).unwrap()
        )
        .is_err()
    );
}

#[test]
fn overflowing_weight_sums_are_scaled_without_bias() {
    let dataset = Dataset::from_json(
        &serde_json::to_vec(&vec![
            row("A", f64::MAX, "desktop", Value::Null),
            row("B", f64::MAX, "desktop", Value::Null),
        ])
        .unwrap(),
    )
    .unwrap();
    let generator = dataset.generator().unwrap();
    let mut rng = StdRng::seed_from_u64(7);
    let count = (0..10_000)
        .filter(|_| generator.sample_with_rng(&mut rng).user_agent == "A")
        .count();
    assert!((4_700..5_300).contains(&count));
}

#[test]
fn injected_rng_produces_reproducible_sequences() {
    let generator = fixture().generator().unwrap();
    let mut first = StdRng::seed_from_u64(17);
    let mut second = StdRng::seed_from_u64(17);
    for _ in 0..100 {
        assert_eq!(
            generator.sample_with_rng(&mut first),
            generator.sample_with_rng(&mut second)
        );
    }
}

#[test]
fn bundled_dataset_and_profile_display_work() {
    assert!(!Dataset::bundled().unwrap().profiles().is_empty());
    let generator = Generator::new().unwrap();
    assert!(!generator.is_empty());
    let profile = generator.sample();
    assert_eq!(profile.to_string(), profile.user_agent);
    assert!(
        generator
            .top(100)
            .windows(2)
            .all(|p| p[0].weight >= p[1].weight)
    );
}

#[test]
fn rust_and_python_share_gzip_acceptance_fixtures() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gzip");
    let expectations: std::collections::BTreeMap<String, bool> =
        serde_json::from_slice(&std::fs::read(directory.join("expectations.json")).unwrap())
            .unwrap();
    for (name, accepted) in expectations {
        let content = std::fs::read(directory.join(&name)).unwrap();
        assert_eq!(Dataset::from_gzip(&content).is_ok(), accepted, "{name}");
    }
}

#[cfg(feature = "cli")]
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
