use clap::Parser;
use serde_json::json;
use user_agents::{Filter, UserAgent};

#[derive(Parser)]
#[command(
    version,
    about = "Generate frequency-weighted user agents from Intoli's dataset"
)]
struct Args {
    /// Number of samples, or top entries with --top
    #[arg(short, long, default_value_t = 1)]
    count: usize,
    /// Restrict device category
    #[arg(long, value_parser = ["desktop", "mobile", "tablet"])]
    device: Option<String>,
    /// Match the user-agent string with a Rust regex
    #[arg(long)]
    regex: Option<String>,
    /// Recursive JSON filter, combined with other filters using AND
    #[arg(long)]
    filter: Option<String>,
    /// Return the most frequent profiles instead of random samples
    #[arg(long)]
    top: bool,
    /// Print full browser fingerprints as a JSON array
    #[arg(long)]
    json: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let mut filters = Vec::new();
    if let Some(device) = args.device {
        filters.push(Filter::from(json!({"deviceCategory": device})));
    }
    if let Some(pattern) = args.regex {
        filters.push(Filter::regex(&pattern)?);
    }
    if let Some(filter) = args.filter {
        filters.push(Filter::from(serde_json::from_str::<serde_json::Value>(
            &filter,
        )?));
    }
    let agent = if filters.is_empty() {
        UserAgent::new()?
    } else {
        UserAgent::with_filter(Filter::All(filters))?
    };
    let profiles = if args.top {
        agent.top(Some(args.count))
    } else {
        (0..args.count)
            .map(|_| agent.random().data().clone())
            .collect()
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&profiles)?);
    } else {
        for profile in profiles {
            println!("{}", profile.user_agent);
        }
    }
    Ok(())
}
