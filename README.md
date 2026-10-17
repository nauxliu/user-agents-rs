# user-agents-rs

A Rust port of [Intoli's user-agents](https://github.com/intoli/user-agents), with frequency-weighted random sampling, full browser fingerprints, recursive filters, and most-common profiles. BSD-2-Clause; Intoli's original license is retained.

The gzip dataset is bundled into the executable with `include_bytes!`, decoded once, and shared between generators. Sampling uses a precomputed cumulative distribution and binary search. Generating another profile from an existing instance reuses its filters and distribution.

## Library

Until this crate is published to crates.io, use a Git dependency:

```toml
[dependencies]
user-agents = { git = "https://github.com/nauxliu/user-agents-rs" }
serde_json = "1"
```

```rust
use serde_json::json;
use user_agents::{Filter, UserAgent};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut agent = UserAgent::new()?;
    println!("{}", agent);                      // HTTP User-Agent header
    println!("{}", agent.platform);             // Typed fingerprint field
    println!("{}", serde_json::to_string_pretty(agent.data())?);
    agent.randomize();                         // Replace the current sample

    let mobile = UserAgent::with_filter(json!({"deviceCategory": "mobile"}))?;
    let another = mobile.random();             // Reuse the prepared distribution
    assert_eq!(another.device_category, "mobile");
    let top_five = mobile.top(Some(5));          // Original upstream weights
    let all_mobile = mobile.top(None);

    let chrome = UserAgent::with_filter(Filter::regex("Chrome")?)?;
    let nested = UserAgent::with_filter(Filter::object([
        ("connection", Filter::object([
            ("effectiveType", "4g".into()),
            ("rtt", Filter::predicate(|v| v.as_f64().is_some_and(|n| n < 100.0))),
        ])),
        ("userAgent", Filter::regex("Android")?),
    ]))?;

    let custom = UserAgent::with_filter(Filter::predicate(|profile| {
        profile["screenWidth"].as_u64().is_some_and(|width| width >= 1200)
    }))?;
    Ok(())
}
```

Strings at the root match the exact `userAgent` string. Regexes at the root match that string; regexes inside objects match the selected field. JSON objects filter recursively, and arrays combine filters with **AND**, following upstream semantics. Missing properties do not match. Use `Filter::All` and `Filter::object` to mix regexes, predicates, and JSON filters.

`UserAgentData` exposes common fields in Rust's `snake_case`. Less common fields such as `connection`, `language`, `oscpu`, and unknown future properties are preserved in `data().extra` with their original JSON names. Serializing `data()` reconstructs the full original fingerprint, including nulls. Profiles are immutable shared references; `top()` and `data().clone()` return independent copies.

`UserAgent::try_random(filter)` returns `None` when no profile matches. `UserAgent::top_filtered(count, filter)` returns an empty vector in that case. Fallible constructors expose errors instead. Rust uses methods rather than JavaScript's callable object/proxy syntax, and the Rust regex engine does not support JavaScript lookaround or backreferences. Custom predicates receive the selected JSON value and should not panic.

For external datasets use `Dataset::from_gzip` or `Dataset::from_json`, then `dataset.generator(Some(&filter))`. `random_with_rng` accepts an injected RNG for reproducible sampling. Invalid/negative weights are rejected; zero-weight entries can appear in `top()` but are never sampled. Filtered sets require at least one positive weight.

## Command line

Requires Rust 1.85 or later.

```sh
cargo run --release -- --count 5
cargo run --release -- --device mobile --regex Chrome --count 3 --json
cargo run --release -- --top --count 10 --filter '{"platform":"Win32"}'
cargo install --git https://github.com/nauxliu/user-agents-rs --locked
user-agents --device desktop --count 5
```

Without `--json`, output is one HTTP User-Agent string per line. `--json` prints complete fingerprints as an array. Invalid arguments, invalid regexes, and filters without matches exit with a nonzero status.

## Daily data updates

[Sync upstream data](https://github.com/nauxliu/user-agents-rs/actions/workflows/sync-upstream.yml) runs daily at **08:23 UTC**, and can also be run manually from GitHub Actions. Scheduled runs may be delayed by GitHub.

1. Clone the upstream `main` branch without checking out or executing its code.
2. Compare source, tests, dependencies, build configuration, CI, and license against `upstream.lock.json`. Ignore only the known dataset, named documentation/image assets, and `package.json`'s daily version bump.
3. **If any monitored file is added, deleted, modified, or changes mode, fail the Action before writing any data.** The reviewed baseline never advances automatically.
4. Validate and copy `src/user-agents.json.gz` into `data/`, recording its source commit and SHA-256.
5. Run Rust and sync-guard tests, then commit changed data to `main` as `github-actions[bot]`. An unchanged dataset produces no commit.

The workflow uses the built-in `GITHUB_TOKEN` with `contents: write`; no PAT or AWS credentials are needed. Code drift and download/validation/test/push errors all fail the workflow. GitHub Actions notification preferences control failure emails.

Consumers must pull the updated Git revision and rebuild to pick up new embedded data. This repository does not automatically publish crates.io versions or binary releases. The upstream analytics collector uses Intoli's private AWS data, so this port consumes their published gzip instead of reimplementing that private service.

See [UPSTREAM.md](UPSTREAM.md) for the review/update procedure and the exact monitoring boundary.

## Development

```sh
python3 tools/check_fmt.py
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s scripts -v
```

CI covers stable Rust and the minimum supported Rust version. Tests cover weighted sampling, zero weights, nested/regex/predicate/AND filters, missing fields, top ordering, fingerprint preservation, the CLI, and data-sync failure behavior.
