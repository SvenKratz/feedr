//! Bakes the maintainer's PostHog project token into official builds. Builds without one never send usage statistics.
use std::{collections::HashMap, env, fs, path::Path};

fn main() {
    let dotenv = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join(".env");
    println!("cargo:rerun-if-changed={}", dotenv.display());
    let mut values: HashMap<String, String> = fs::read_to_string(&dotenv)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().into(), clean(value)))
        .collect();
    // CI supplies the same names as environment variables; they take precedence over .env.
    for key in ["POSTHOG_TOKEN", "POSTHOG_REGION"] {
        println!("cargo:rerun-if-env-changed={key}");
        if let Ok(value) = env::var(key)
            && !value.trim().is_empty()
        {
            values.insert(key.into(), clean(&value));
        }
    }
    let token = values.get("POSTHOG_TOKEN").map_or("", String::as_str);
    if token.is_empty() {
        return;
    }
    // Only a write-only project token may be embedded; personal API keys grant account access.
    if !token.starts_with("phc_")
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        println!(
            "cargo:warning=POSTHOG_TOKEN is not a PostHog project token (phc_…); usage statistics stay disabled"
        );
        return;
    }
    let region = values
        .get("POSTHOG_REGION")
        .map_or("us", String::as_str)
        .to_ascii_lowercase();
    let host = if region.starts_with("https://") {
        region.trim_end_matches('/').to_owned()
    } else if region.starts_with("eu") {
        "https://eu.i.posthog.com".into()
    } else {
        "https://us.i.posthog.com".into()
    };
    println!("cargo:rustc-env=FEEDR_POSTHOG_TOKEN={token}");
    println!("cargo:rustc-env=FEEDR_POSTHOG_HOST={host}");
}

fn clean(value: &str) -> String {
    value.trim().trim_matches(['"', '\'']).trim().to_owned()
}
