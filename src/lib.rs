//! Frequency-weighted browser profiles from Intoli's user-agents dataset.
//!
//! ```
//! use user_agents::Generator;
//!
//! let mobile = Generator::new()?.filter(|profile| profile.device_category == "mobile")?;
//! assert_eq!(mobile.sample().device_category, "mobile");
//! println!("{}", mobile.sample());
//! let most_common = mobile.top(5);
//! # Ok::<(), user_agents::Error>(())
//! ```

mod filter;
pub use filter::Filter;
pub use regex::Regex;

use flate2::bufread::GzDecoder;
use rand::{Rng, RngExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fmt,
    io::Read,
    sync::{Arc, OnceLock},
};

/// A full browser fingerprint. Less common and future fields are preserved in
/// `extra`, including nested `connection`, `language`, and `oscpu` values.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserAgentData {
    pub app_name: String,
    pub platform: String,
    pub plugins_length: u64,
    pub screen_height: u64,
    pub screen_width: u64,
    pub user_agent: String,
    pub vendor: String,
    pub viewport_height: u64,
    pub viewport_width: u64,
    pub device_category: String,
    /// Original upstream frequency, before filtering and renormalization.
    pub weight: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidData(String),
    NoMatches,
    NoPositiveWeight,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(reason) => write!(f, "Invalid user-agent data: {reason}"),
            Self::NoMatches => f.write_str("No user agents matched your filters."),
            Self::NoPositiveWeight => f.write_str("Matching user agents have no positive weight."),
        }
    }
}

impl std::error::Error for Error {}

/// An immutable, shareable dataset, loaded once for the bundled data.
#[derive(Clone, Debug)]
pub struct Dataset(Arc<[UserAgentData]>);

impl Dataset {
    pub fn bundled() -> Result<Self, Error> {
        static DATA: OnceLock<Result<Dataset, Error>> = OnceLock::new();
        DATA.get_or_init(|| Self::from_gzip(include_bytes!("../data/user-agents.json.gz")))
            .clone()
    }

    pub fn from_gzip(bytes: &[u8]) -> Result<Self, Error> {
        const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;
        let mut json = Vec::new();
        let mut remaining = bytes;
        while !remaining.is_empty() {
            // The BufRead decoder preserves bytes after each member, allowing
            // us to skip gzip zero padding just as Python's gzip.GzipFile does.
            let mut decoder = GzDecoder::new(remaining);
            decoder
                .by_ref()
                .take((MAX_JSON_BYTES + 1 - json.len()) as u64)
                .read_to_end(&mut json)
                .map_err(|error| Error::InvalidData(error.to_string()))?;
            if json.len() > MAX_JSON_BYTES {
                return Err(Error::InvalidData(
                    "decompressed data exceeds 64 MiB".into(),
                ));
            }
            remaining = decoder.into_inner();
            let padding = remaining.iter().take_while(|&&byte| byte == 0).count();
            remaining = &remaining[padding..];
        }
        Self::from_json(&json)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let profiles: Vec<UserAgentData> =
            serde_json::from_slice(bytes).map_err(|error| Error::InvalidData(error.to_string()))?;
        if profiles.is_empty() {
            return Err(Error::InvalidData("empty dataset".into()));
        }
        for (index, profile) in profiles.iter().enumerate() {
            if !profile.weight.is_finite() || profile.weight < 0.0 {
                return Err(Error::InvalidData(format!(
                    "invalid weight at profile {index}"
                )));
            }
            if profile.user_agent.is_empty() {
                return Err(Error::InvalidData(format!(
                    "empty userAgent at profile {index}"
                )));
            }
        }
        if !profiles.iter().any(|profile| profile.weight > 0.0) {
            return Err(Error::NoPositiveWeight);
        }
        Ok(Self(profiles.into()))
    }

    pub fn profiles(&self) -> &[UserAgentData] {
        &self.0
    }

    /// Prepare the full distribution without drawing a sample or accessing an RNG.
    pub fn generator(&self) -> Result<Generator, Error> {
        Generator::from_indices(self.clone(), (0..self.0.len()).collect())
    }
}

#[derive(Debug)]
struct Selection {
    dataset: Dataset,
    cumulative: Vec<(f64, usize)>,
}

impl Selection {
    fn new(dataset: Dataset, indices: Vec<usize>) -> Result<Self, Error> {
        if indices.is_empty() {
            return Err(Error::NoMatches);
        }
        // Scaling by the maximum avoids overflow for unnormalized external data.
        let max = indices
            .iter()
            .map(|&i| dataset.0[i].weight)
            .fold(0.0, f64::max);
        if max == 0.0 {
            return Err(Error::NoPositiveWeight);
        }
        let total: f64 = indices.iter().map(|&i| dataset.0[i].weight / max).sum();
        let mut sum = 0.0;
        let cumulative = indices
            .into_iter()
            .map(|index| {
                sum += (dataset.0[index].weight / max) / total;
                (sum.min(1.0), index)
            })
            .collect::<Vec<_>>();
        let mut selection = Self {
            dataset,
            cumulative,
        };
        // Remove rounding gaps without giving zero-weight trailing rows a chance.
        let last_positive = selection
            .cumulative
            .iter()
            .rposition(|&(_, index)| selection.dataset.0[index].weight > 0.0)
            .expect("positive weight checked above");
        for pair in &mut selection.cumulative[last_positive..] {
            pair.0 = 1.0;
        }
        Ok(selection)
    }
}

/// A reusable weighted distribution, independent of any particular sample.
///
/// Construction, filtering, and ranking never access an RNG. Samples and ranked
/// entries borrow immutable profiles; call `.clone()` when ownership is needed.
#[derive(Clone, Debug)]
pub struct Generator(Arc<Selection>);

impl Generator {
    /// Use the bundled dataset and a distribution prepared once per process.
    pub fn new() -> Result<Self, Error> {
        static SELECTION: OnceLock<Result<Arc<Selection>, Error>> = OnceLock::new();
        SELECTION
            .get_or_init(|| {
                let dataset = Dataset::bundled()?;
                Selection::new(dataset.clone(), (0..dataset.0.len()).collect()).map(Arc::new)
            })
            .clone()
            .map(Self)
    }

    fn from_indices(dataset: Dataset, indices: Vec<usize>) -> Result<Self, Error> {
        Selection::new(dataset, indices).map(Arc::new).map(Self)
    }

    /// Restrict the current candidate set with a typed Rust predicate.
    /// The predicate is evaluated only during preparation and may borrow locals.
    pub fn filter(&self, mut predicate: impl FnMut(&UserAgentData) -> bool) -> Result<Self, Error> {
        let indices = self
            .0
            .cumulative
            .iter()
            .map(|&(_, i)| i)
            .filter(|&i| predicate(&self.0.dataset.0[i]))
            .collect();
        Self::from_indices(self.0.dataset.clone(), indices)
    }

    /// Adapter for recursive JSON/regex filters, primarily for configuration/CLI.
    /// Prefer `filter` for typed field access without JSON serialization.
    pub fn filter_json(&self, filter: &Filter) -> Result<Self, Error> {
        self.filter(|profile| {
            filter
                .matches(&serde_json::to_value(profile).expect("validated immutable JSON profile"))
        })
    }

    pub fn sample(&self) -> &UserAgentData {
        self.sample_with_rng(&mut rand::rng())
    }

    pub fn sample_with_rng(&self, rng: &mut (impl Rng + ?Sized)) -> &UserAgentData {
        let probability: f64 = rng.random();
        let position = self
            .0
            .cumulative
            .partition_point(|&(weight, _)| weight <= probability);
        &self.0.dataset.0[self.0.cumulative[position].1]
    }

    pub fn len(&self) -> usize {
        self.0.cumulative.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.cumulative.is_empty()
    }

    /// Most frequent matching profiles, borrowing data without cloning fields.
    /// Ties retain dataset order. Weights retain their original upstream values.
    /// Pass `usize::MAX` to return all matching profiles.
    pub fn top(&self, count: usize) -> Vec<&UserAgentData> {
        let mut indices: Vec<_> = self.0.cumulative.iter().map(|&(_, i)| i).collect();
        let profiles = &self.0.dataset.0;
        indices.sort_by(|&a, &b| profiles[b].weight.total_cmp(&profiles[a].weight));
        indices
            .into_iter()
            .take(count)
            .map(|i| &profiles[i])
            .collect()
    }
}

impl fmt::Display for UserAgentData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.user_agent)
    }
}
