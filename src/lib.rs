//! Frequency-weighted browser profiles from Intoli's user-agents dataset.
//!
//! ```
//! use serde_json::json;
//! use user_agents::{Filter, UserAgent};
//!
//! let agent = UserAgent::with_filter(Filter::from(json!({"deviceCategory": "mobile"})))?;
//! assert_eq!(agent.device_category, "mobile");
//! println!("{}", agent.random());
//! let most_common = agent.top(Some(5));
//! # Ok::<(), user_agents::Error>(())
//! ```

mod filter;
pub use filter::Filter;
pub use regex::Regex;

use flate2::read::GzDecoder;
use rand::{Rng, RngExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fmt,
    io::Read,
    ops::Deref,
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
        let mut json = Vec::new();
        // Cap decompression for caller-supplied data as well as downloaded data.
        GzDecoder::new(bytes)
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut json)
            .map_err(|error| Error::InvalidData(error.to_string()))?;
        if json.len() > 64 * 1024 * 1024 {
            return Err(Error::InvalidData(
                "decompressed data exceeds 64 MiB".into(),
            ));
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

    pub fn generator(&self, filter: Option<&Filter>) -> Result<UserAgent, Error> {
        let selection = Arc::new(Selection::new(self.clone(), filter)?);
        Ok(UserAgent::sample(selection, &mut rand::rng()))
    }
}

#[derive(Debug)]
struct Selection {
    dataset: Dataset,
    cumulative: Vec<(f64, usize)>,
}

impl Selection {
    fn new(dataset: Dataset, filter: Option<&Filter>) -> Result<Self, Error> {
        let indices: Vec<usize> = dataset
            .0
            .iter()
            .enumerate()
            .filter_map(|(index, profile)| {
                let matches = filter.is_none_or(|filter| {
                    filter.matches(&serde_json::to_value(profile).expect("validated JSON profile"))
                });
                matches.then_some(index)
            })
            .collect();
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

/// A current sample and a reusable, precomputed filtered distribution.
/// Cloning/random sampling shares the dataset and distribution; profile data is
/// returned by reference or cloned explicitly via `data().clone()` / `top()`.
#[derive(Clone, Debug)]
pub struct UserAgent {
    selection: Arc<Selection>,
    index: usize,
}

impl UserAgent {
    pub fn new() -> Result<Self, Error> {
        static SELECTION: OnceLock<Result<Arc<Selection>, Error>> = OnceLock::new();
        let selection = SELECTION
            .get_or_init(|| Selection::new(Dataset::bundled()?, None).map(Arc::new))
            .clone()?;
        Ok(Self::sample(selection, &mut rand::rng()))
    }

    pub fn with_filter(filter: impl Into<Filter>) -> Result<Self, Error> {
        Dataset::bundled()?.generator(Some(&filter.into()))
    }

    /// Convenience equivalent to upstream's static `random(filters)`.
    pub fn try_random(filter: impl Into<Filter>) -> Option<Self> {
        Self::with_filter(filter).ok()
    }

    pub fn data(&self) -> &UserAgentData {
        &self.selection.dataset.0[self.index]
    }

    pub fn random(&self) -> Self {
        self.random_with_rng(&mut rand::rng())
    }

    pub fn random_with_rng(&self, rng: &mut (impl Rng + ?Sized)) -> Self {
        Self::sample(self.selection.clone(), rng)
    }

    pub fn randomize(&mut self) {
        *self = self.random();
    }

    pub fn matching_len(&self) -> usize {
        self.selection.cumulative.len()
    }

    /// Cumulative probability and original dataset index, in dataset order.
    pub fn cumulative_weight_index_pairs(&self) -> &[(f64, usize)] {
        &self.selection.cumulative
    }

    /// Most frequent matching fingerprints. `None` returns all; `Some(0)` none.
    /// Ties retain dataset order. Returned weights are the original frequencies.
    pub fn top(&self, count: Option<usize>) -> Vec<UserAgentData> {
        let mut indices: Vec<_> = self.selection.cumulative.iter().map(|&(_, i)| i).collect();
        let profiles = &self.selection.dataset.0;
        indices.sort_by(|&a, &b| profiles[b].weight.total_cmp(&profiles[a].weight));
        indices
            .into_iter()
            .take(count.unwrap_or(usize::MAX))
            .map(|i| profiles[i].clone())
            .collect()
    }

    /// Convenience equivalent to upstream's static `top(count, filters)`.
    pub fn top_filtered(count: Option<usize>, filter: impl Into<Filter>) -> Vec<UserAgentData> {
        Self::with_filter(filter)
            .map(|agent| agent.top(count))
            .unwrap_or_default()
    }

    fn sample(selection: Arc<Selection>, rng: &mut (impl Rng + ?Sized)) -> Self {
        let probability: f64 = rng.random();
        let position = selection
            .cumulative
            .partition_point(|&(weight, _)| weight <= probability);
        let index = selection.cumulative[position].1;
        Self { selection, index }
    }
}

impl Deref for UserAgent {
    type Target = UserAgentData;
    fn deref(&self) -> &Self::Target {
        self.data()
    }
}

impl fmt::Display for UserAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.user_agent)
    }
}
