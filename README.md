# user-agents-rs

A Rust port of [Intoli's user-agents](https://github.com/intoli/user-agents), with frequency-weighted random sampling, full browser fingerprints, recursive filters, and most-common profiles. BSD-2-Clause; Intoli's original license is retained.

The gzip dataset is bundled into the executable with `include_bytes!`, decoded once, and shared between generators. Sampling uses a precomputed cumulative distribution and binary search. A generator owns the prepared distribution; samples borrow immutable profiles from it. Construction and ranking do not access an RNG.

## Library

Until this crate is published to crates.io, use a Git dependency:

```toml
[dependencies]
user-agents = { git = "https://github.com/nauxliu/user-agents-rs" }
serde_json = "1"
```

```rust
use serde_json::json;
use user_agents::{Filter, Generator};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let all = Generator::new()?;
    let profile = all.sample();
    println!("{}", profile);               // HTTP User-Agent header
    println!("{}", profile.platform);      // Typed fingerprint field
    println!("{}", serde_json::to_string_pretty(profile)?);

    let category = String::from("mobile");
    let mobile = all.filter(|profile| profile.device_category == category)?;
    let another = mobile.sample();        // Reuse the prepared distribution
    assert_eq!(another.device_category, "mobile");
    let top_five = mobile.top(5);          // Borrowed profiles, original weights
    let owned = another.clone();          // Explicitly take an independent copy

    let chrome_regex = user_agents::Regex::new("Chrome")?;
    let chrome = all.filter(|p| chrome_regex.is_match(&p.user_agent))?;

    // Recursive JSON filters are an adapter for configuration and CLI use.
    let nested = all.filter_json(&Filter::from(json!({
        "connection": {"effectiveType": "4g"},
        "deviceCategory": "mobile"
    })))?;
    Ok(())
}
```

`Generator::filter` accepts `FnMut(&UserAgentData) -> bool`. Predicates may borrow local configuration and update local state; they are evaluated during preparation and are not stored. Chained filters narrow the current candidate set. `sample()` uses the thread RNG, while `sample_with_rng(&mut rng)` accepts a caller-supplied RNG for reproducible sampling.

`top(count)` returns references ordered by descending original weight, with dataset order preserved for ties. Pass `usize::MAX` to return all matches. Sampling also returns references without cloning fingerprint strings/maps. Use `.clone()` when ownership is needed. Invalid data, no matching profiles, and all-zero filtered weights return distinct `Result` errors. There are no convenience methods that silently swallow these errors.

`UserAgentData` exposes common fields in Rust's `snake_case` and implements `Display` for the HTTP header. Less common fields such as `connection`, `language`, `oscpu`, and unknown future properties are preserved in `extra` with their original JSON names. Serialization reconstructs the full fingerprint, including nulls.

For external data, use `Dataset::from_gzip` or `Dataset::from_json`, then `dataset.generator()`. Invalid/negative weights are rejected; zero-weight entries can appear in `top()` but are never sampled. Filtered generators require at least one positive weight. Gzip parsing consumes all members, accepts zero padding between/after members like Python's gzip reader, and rejects malformed/truncated streams and other trailing data.

### JSON filter adapter

Strings and regexes at the root match `userAgent`. Within objects they match the selected field. Objects filter recursively, and arrays combine filters with **AND**. Missing properties do not match. `Filter::All` and `Filter::object` can combine regexes and value filters.

JSON integer comparisons preserve `u64` precision, including values above 2^53. Integral floats compare equal to the exact integer they represent. Regexes match **strings only**; numbers, nulls, arrays, and objects are never implicitly converted to strings. The Rust regex engine does not support JavaScript lookaround or backreferences. These explicit semantics replace JavaScript coercion and callable/proxy object behavior.

## Command line

Requires Rust 1.85 or later.

```sh
cargo run --release --features cli -- --count 5
cargo run --release --features cli -- --device mobile --regex Chrome --count 3 --json
cargo run --release --features cli -- --top --count 10 --filter '{"platform":"Win32"}'
cargo install --git https://github.com/nauxliu/user-agents-rs --locked --features cli
user-agents --device desktop --count 5
```

The CLI is enabled by the optional `cli` feature; library users do not pull in `clap`. Without `--json`, output is one HTTP User-Agent string per line. `--json` prints complete fingerprints as an array. Invalid arguments, invalid regexes, and filters without matches exit with a nonzero status.

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
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo check --locked --no-default-features --lib
cargo test --locked --all-features
python3 -m unittest discover -s scripts -v
```

CI covers stable Rust and the minimum supported Rust version. Tests cover weighted sampling, zero weights, typed predicates with borrowed state, nested JSON/regex/AND filters, exact integer comparisons, ranking, fingerprint preservation, the optional CLI, shared Python/Rust gzip fixtures, and data-sync failure behavior.
