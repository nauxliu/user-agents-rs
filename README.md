# User Agents for Rust

[Installation](#installation) | [Examples](#examples) | [API](#api) | [Data](#data) | [Contributing](#contributing)

Generate random User-Agent headers and browser fingerprints from [Intoli's user-agents](https://github.com/intoli/user-agents) dataset. Profiles are sampled according to their observed frequency, so common configurations appear more often than rare ones.

This Rust port provides reusable generators, typed predicates, recursive JSON filters, and frequency rankings. The dataset is bundled with the crate and refreshed daily in this repository. Sampling works offline.

## Installation

Requires **Rust 1.85 or later**. The crate is not yet published to crates.io; add it as a Git dependency:

```toml
[dependencies]
user-agents = { git = "https://github.com/nauxliu/user-agents-rs" }
```

To serialize complete fingerprints as JSON, also add:

```toml
serde_json = "1"
```

For the optional command-line tool:

```sh
cargo install --git https://github.com/nauxliu/user-agents-rs --locked --features cli
user-agents --device mobile --count 5
```

The `cli` feature enables `clap`; library users do not need it.

## Examples

### Generating a Random User Agent

Create a generator, then draw a profile. `Display` prints its HTTP User-Agent header; serialization includes the full browser fingerprint.

```rust
use user_agents::Generator;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let generator = Generator::new()?;
    let profile = generator.sample();

    println!("{}", profile);
    println!("{}", serde_json::to_string_pretty(profile)?);
    Ok(())
}
```

For example, a fingerprint can look like this. Values change with each sample and dataset refresh:

```json
{
  "appName": "Netscape",
  "connection": {
    "downlink": 2.75,
    "effectiveType": "4g",
    "rtt": 200
  },
  "deviceCategory": "desktop",
  "language": "zh-CN",
  "platform": "Win32",
  "pluginsLength": 5,
  "screenHeight": 1205,
  "screenWidth": 1341,
  "userAgent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36",
  "vendor": "Google Inc.",
  "viewportHeight": 1076,
  "viewportWidth": 1341,
  "weight": 0.0003358385458910402
}
```

The shorter Rust examples below belong inside a `main` function returning `Result`, as above. Each example starts with a fresh generator.

### Restricting Device Categories

Filter typed fields directly with a Rust closure:

```rust
use user_agents::Generator;

let mobile = Generator::new()?.filter(|profile| profile.device_category == "mobile")?;
let profile = mobile.sample();
assert_eq!(profile.device_category, "mobile");
println!("{}", profile);
```

Use `"desktop"` or `"tablet"` to select another category. A filter with no matching profiles returns `Error::NoMatches`.

### Generating Multiple User Agents With the Same Filters

Prepare filters once, then reuse the generator for repeated draws:

```rust
use user_agents::Generator;

let windows = Generator::new()?.filter(|profile| profile.platform == "Win32")?;
let profiles: Vec<_> = (0..1000).map(|_| windows.sample()).collect();

assert!(profiles.iter().all(|profile| profile.platform == "Win32"));
```

These profiles borrow from `windows`, so no fingerprint strings or maps are copied. If the results must outlive the generator, collect `windows.sample().clone()` instead.

### Regular Expression Matching

Use a regex in a typed predicate to match User-Agent strings:

```rust
use user_agents::{Generator, Regex};

let pattern = Regex::new("Safari")?;
let matching = Generator::new()?.filter(|profile| pattern.is_match(&profile.user_agent))?;

println!("{}", matching.sample());
```

This selects strings containing `Safari`, including strings from browsers that share that token. To identify a browser family precisely, use your application's User-Agent parser in a custom predicate.

### Custom Filter Functions

Predicates receive `&UserAgentData` and can borrow local configuration. For example, select desktop profiles with a minimum screen width:

```rust
use user_agents::Generator;

let minimum_width = 1920;
let wide_desktop = Generator::new()?.filter(|profile| {
    profile.device_category == "desktop" && profile.screen_width >= minimum_width
})?;

println!("{}", wide_desktop.sample());
```

The predicate is evaluated while preparing the filtered generator and is not stored. It accepts `FnMut`, so it can also update local state without requiring `Send`, `Sync`, or `'static` captures.

### Combining Filters With Arrays

For filters supplied as configuration, combine regex and nested JSON conditions with `Filter::All`:

```rust
use serde_json::json;
use user_agents::{Filter, Generator};

let filters = Filter::All(vec![
    Filter::regex("Chrome")?,
    Filter::from(json!({
        "platform": "Win32",
        "connection": { "effectiveType": "4g" }
    })),
]);
let matching = Generator::new()?.filter_json(&filters)?;

println!("{}", matching.sample());
```

All conditions must match. JSON arrays also mean **AND**, and nested objects inspect nested fingerprint properties. Missing properties do not match.

For native Rust predicates, successive `.filter(...)` calls narrow the current candidate set in the same way.

### Filtering Browser Versions

Apply your own version policy in a predicate. This example selects strings with a `Chrome/` major version of at least 130:

```rust
use user_agents::{Generator, Regex};

let minimum_major = 130;
let version = Regex::new(r"Chrome/(\d+)\.")?;
let matching = Generator::new()?.filter(|profile| {
    version
        .captures(&profile.user_agent)
        .and_then(|captures| captures[1].parse::<u32>().ok())
        .is_some_and(|major| major >= minimum_major)
})?;

println!("{}", matching.sample());
```

The cutoff is an application policy, not an automatically maintained list of supported browsers. You can use a browser parser or your own compatibility rules in the same closure.

### Getting the Most Common User Agents

Use `top` to inspect the most frequent matching profiles without drawing a random sample:

```rust
use user_agents::Generator;

let mobile = Generator::new()?.filter(|profile| profile.device_category == "mobile")?;
for profile in mobile.top(5) {
    println!("{:.6} {}", profile.weight, profile);
}
```

Results borrow the dataset and retain their original upstream weights. Pass `usize::MAX` to return all matching profiles.

### Reproducible Sampling

Add `rand = "0.10"` to your dependencies, then supply a seeded RNG:

```rust
use rand::{SeedableRng, rngs::StdRng};
use user_agents::Generator;

let generator = Generator::new()?;
let mut rng = StdRng::seed_from_u64(42);
let profile = generator.sample_with_rng(&mut rng);

println!("{}", profile);
```

A fixed dataset, generator, RNG implementation, and seed reproduce the same sequence. Updating the dataset can change it.

### Keeping a Header and Releasing Memory

Clone the value you want to keep, then let the generator leave its scope:

```rust
use user_agents::Generator;

let header = {
    let generator = Generator::new()?;
    generator.sample().user_agent.clone()
};

// The independent header is still usable after the generator is dropped.
println!("{}", header);
```

You can also call `drop(generator)` explicitly. Decoded data is released when the last dataset or generator that owns it is dropped; other active callers can continue using their shared data. See [Storage and Memory](#storage-and-memory).

## API

### `Generator::new() -> Result<Generator, Error>`

Create a generator over the bundled dataset. Live generators share decoded profiles and the default prepared distribution. Construction does not draw a sample or access an RNG.

### `generator.filter(predicate) -> Result<Generator, Error>`

Prepare a new generator from the current candidates using `FnMut(&UserAgentData) -> bool`. The original generator is unchanged. Filtering renormalizes sampling probabilities over the matching profiles; their stored `weight` values remain unchanged.

### `generator.filter_json(&filter) -> Result<Generator, Error>`

Apply a recursive `Filter` to the current candidates. JSON uses the original `camelCase` fingerprint names:

- `Filter::from(json!(...))` converts JSON objects and arrays recursively.
- Objects require every named property to match; arrays and `Filter::All` require every child filter to match.
- Strings and regexes at the root match `userAgent`; inside an object they match the selected property.
- Regexes match strings only. Numbers, nulls, arrays, and objects are not coerced to strings.
- Numeric equality preserves integer precision, including `u64` values above 2^53. An integral float matches the exact integer it represents.

`Filter::object` builds nested filters containing regexes. Prefer typed `.filter(...)` closures when working directly with Rust fields; the JSON adapter serializes each candidate during preparation. Rust regexes do not support lookaround or backreferences.

### `generator.sample() -> &UserAgentData`

Draw one profile using the thread RNG. The returned reference borrows the generator. Sampling reuses the prepared cumulative distribution and performs a binary search.

### `generator.sample_with_rng(&mut rng) -> &UserAgentData`

Draw a profile using a supplied `rand::Rng`. Use this when the caller controls randomness, including seeded tests.

### `generator.top(count) -> Vec<&UserAgentData>`

Return up to `count` profiles in descending weight order. Ties retain dataset order. `top(0)` returns an empty vector; `top(usize::MAX)` returns all candidates. Ranking does not access an RNG or clone profile contents.

### `generator.len()` / `generator.is_empty()`

Inspect the number of matching profiles. Successful generator construction always has at least one match with positive weight.

### `UserAgentData`

A full browser fingerprint with typed fields and `Display` for its HTTP header. Rust field names use `snake_case`; serialization restores the original JSON names.

| Rust field | JSON field | Meaning |
| --- | --- | --- |
| `user_agent` | `userAgent` | HTTP User-Agent string |
| `app_name` | `appName` | Navigator application name |
| `platform` | `platform` | Navigator platform string |
| `device_category` | `deviceCategory` | Device category, typically `desktop`, `mobile`, or `tablet` |
| `plugins_length` | `pluginsLength` | Reported plugin count |
| `vendor` | `vendor` | Navigator vendor string |
| `screen_width`, `screen_height` | `screenWidth`, `screenHeight` | Screen dimensions |
| `viewport_width`, `viewport_height` | `viewportWidth`, `viewportHeight` | Viewport dimensions |
| `weight` | `weight` | Original upstream sampling weight |
| `extra` | Original property names | Additional properties, such as `connection`, `language`, and `oscpu` |

`extra` is a `serde_json::Map<String, Value>`. Serialization merges its entries into the fingerprint object, preserving unknown properties and explicit nulls. Use `.clone()` to obtain an owned profile or an owned header string.

### `Dataset`

Load and inspect data independently of a generator:

- `Dataset::bundled()` shares live bundled data.
- `Dataset::from_json(bytes)` parses an external JSON dataset.
- `Dataset::from_gzip(bytes)` decompresses and parses an external gzip dataset.
- `dataset.profiles()` borrows all profiles in dataset order.
- `dataset.generator()` prepares the full weighted distribution.

Gzip loading supports multiple members and zero padding between/after members. It rejects malformed, truncated, or otherwise trailing data, and caps total decompressed JSON at 64 MiB.

### `Error`

Fallible constructors and filters return one of:

- `InvalidData(reason)`: malformed data, invalid fields, an empty dataset, or negative/nonfinite weights.
- `NoMatches`: no profiles satisfy a filter.
- `NoPositiveWeight`: a dataset or filtered set has no positive weight.

Zero-weight profiles can appear in rankings but are never sampled.

## Data

Intoli produces the source dataset from a recent window of observed browser traffic. Frequencies describe that snapshot; profiles are weighted rather than sampled uniformly. This port consumes the published gzip dataset without requiring access to Intoli's private analytics service.

### Daily Updates

[Sync upstream data](https://github.com/nauxliu/user-agents-rs/actions/workflows/sync-upstream.yml) runs daily at **08:23 UTC** and supports manual runs:

1. Compare upstream source, tests, dependencies, build configuration, CI, and license against the reviewed baseline.
2. If a monitored file changes, **fail the Action before syncing data**. The baseline never advances automatically.
3. Otherwise validate the published gzip, run tests, and commit changed data to `main`.

Daily `package.json` version bumps and the named documentation/image files are excluded from source comparison. See [UPSTREAM.md](UPSTREAM.md) for the complete boundary and the source-review procedure. `data/upstream.json` records the source commit, gzip SHA-256, and profile count.

Consumers pick up data updates by updating their Git dependency with `cargo update -p user-agents`, then rebuilding. A local checkout can pull and rebuild. Running programs keep their embedded dataset until replaced by a rebuilt executable.

### Storage and Memory

The executable embeds gzip bytes. Loading creates a temporary JSON buffer, parses owned Rust profiles, then releases that buffer. Live datasets and generators share the parsed data; sampling does not repeat decompression or parsing.

The global caches hold weak references. The last owner dropping releases decoded profiles and indices, including ownership held by cloned or filtered generators. Later use reloads the gzip. Borrowed samples cannot outlive their generator; explicitly cloned profiles or strings remain independently owned.

The embedded gzip remains in the program's read-only data. Heap allocations return to the allocator when released; process RSS need not fall immediately. Keep a generator alive while repeatedly sampling to avoid unloading and reloading the same dataset.

## Command Line

With the `cli` feature enabled:

```sh
user-agents --count 5
user-agents --device mobile --regex Chrome --count 3 --json
user-agents --top --count 10 --filter '{"platform":"Win32"}'
```

Without `--json`, output is one User-Agent header per line. `--json` prints complete fingerprints as an array. Invalid arguments, regexes, or filters without matches exit with a nonzero status.

To run from a checkout:

```sh
cargo run --release --features cli -- --device desktop --count 5
```

## Versioning

The Rust crate version is independent of upstream's npm version. Automated dataset updates create Git commits without changing the crate version. This repository does not automatically publish crates.io packages or binary releases.

## Acknowledgements

The dataset and original library come from [Intoli](https://github.com/intoli/user-agents). This README follows the upstream examples and organization, adapted to Rust's generator, borrowing, and ownership APIs. This is an independent port.

## Contributing

Contributions are welcome. Run the development checks before committing:

```sh
python3 tools/check_fmt.py
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo check --locked --no-default-features --lib
cargo test --locked --all-features
python3 -m unittest discover -s scripts -v
```

CI covers stable Rust and Rust 1.85. Changes to upstream behavior should include a reviewed baseline update and relevant Rust tests; see [UPSTREAM.md](UPSTREAM.md).

## License

[BSD-2-Clause](LICENSE). The original Intoli copyright notice and disclaimer are retained.
