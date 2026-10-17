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
    sync::{Arc, Mutex, Weak},
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

// Keep the bulk allocation outside the Arc allocation: a stale Weak cache must
// retain only its small control block, not the entire inline profile array.
type ProfileStorage = Box<[UserAgentData]>;

/// An immutable dataset shared while any dataset or generator owns it.
#[derive(Clone, Debug)]
pub struct Dataset(Arc<ProfileStorage>);

impl Dataset {
    /// Reuse live bundled data. After the last owner is dropped, the decoded
    /// profiles are freed; the next call loads the embedded gzip again.
    pub fn bundled() -> Result<Self, Error> {
        static DATA: Mutex<Weak<ProfileStorage>> = Mutex::new(Weak::new());
        load_shared(&DATA, || {
            Self::from_gzip(include_bytes!("../data/user-agents.json.gz")).map(|data| data.0)
        })
        .map(Self)
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
        Ok(Self(Arc::new(profiles.into_boxed_slice())))
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
    /// Share live bundled data and its prepared distribution. Dropping the last
    /// owner frees its data and indices; construction afterward reloads them.
    pub fn new() -> Result<Self, Error> {
        static SELECTION: Mutex<Weak<Selection>> = Mutex::new(Weak::new());
        load_shared(&SELECTION, || {
            Dataset::bundled()?.generator().map(|generator| generator.0)
        })
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

// The cache holds no strong reference. Serialize loading so simultaneous callers
// share one allocation, then release the lock before sampling/filtering/ranking.
fn load_shared<T>(
    cache: &Mutex<Weak<T>>,
    load: impl FnOnce() -> Result<Arc<T>, Error>,
) -> Result<Arc<T>, Error> {
    let mut cached = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(value) = cached.upgrade() {
        return Ok(value);
    }
    let value = load()?;
    *cached = Arc::downgrade(&value);
    Ok(value)
}

impl fmt::Display for UserAgentData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.user_agent)
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn last_owner_releases_data_and_indices_then_next_use_reloads() {
        // Local caches keep the lifetime assertions independent of parallel tests.
        let data_cache = Mutex::new(Weak::<ProfileStorage>::new());
        let selection_cache = Mutex::new(Weak::<Selection>::new());
        let loads = Cell::new(0);
        let load = || {
            load_shared(&selection_cache, || {
                let dataset = Dataset(load_shared(&data_cache, || {
                    loads.set(loads.get() + 1);
                    Dataset::from_gzip(include_bytes!("../data/user-agents.json.gz"))
                        .map(|data| data.0)
                })?);
                dataset.generator().map(|generator| generator.0)
            })
            .map(Generator)
        };

        let first = load().unwrap();
        let second = load().unwrap();
        assert!(Arc::ptr_eq(&first.0, &second.0));
        assert_eq!(loads.get(), 1);
        let filtered = first.filter(|_| true).unwrap();
        let cloned = filtered.clone();
        let owned_sample = first.sample().clone();
        let selection = Arc::downgrade(&first.0);
        let data = Arc::downgrade(&first.0.dataset.0);

        drop(first);
        drop(second);
        assert!(
            selection.upgrade().is_none(),
            "default indices must be dropped"
        );
        assert!(
            data.upgrade().is_some(),
            "filtered owners keep the dataset alive"
        );
        drop(filtered);
        assert!(data.upgrade().is_some(), "a clone is still an owner");
        drop(cloned);
        assert!(
            data.upgrade().is_none(),
            "the cache must not keep profiles alive"
        );
        assert!(!owned_sample.user_agent.is_empty());

        let reloaded = load().unwrap();
        assert_eq!(loads.get(), 2);
        assert!(!reloaded.is_empty());
    }

    #[test]
    fn default_constructors_share_live_data() {
        let first = Generator::new().unwrap();
        let second = Generator::new().unwrap();
        let dataset = Dataset::bundled().unwrap();
        assert!(Arc::ptr_eq(&first.0, &second.0));
        assert!(Arc::ptr_eq(&first.0.dataset.0, &dataset.0));
    }

    #[test]
    fn concurrent_callers_load_once_while_owners_exist() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };
        let cache = Arc::new(Mutex::new(Weak::new()));
        let barrier = Arc::new(Barrier::new(8));
        let loads = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (cache, barrier, loads) = (cache.clone(), barrier.clone(), loads.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    load_shared(&cache, || {
                        loads.fetch_add(1, Ordering::Relaxed);
                        Ok(Arc::new(42))
                    })
                    .unwrap()
                })
            })
            .collect();
        let owners: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(loads.load(Ordering::Relaxed), 1);
        assert!(owners.iter().all(|owner| Arc::ptr_eq(owner, &owners[0])));
        drop(owners);
        assert!(cache.lock().unwrap().upgrade().is_none());
    }
}
