use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate};
use plotters::prelude::*;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;

const GITHUB_REPO: &str = "evanalyzer/evanalyzer";
const IMAGEJ_STATS_URL: &str = "https://sites.imagej.net/stats.json";
const IMAGEJ_PLUGIN_KEY: &str = "evanalyzer";
const STATS_FILE: &str = "stats.json";
const EXCLUDED_TAG: &str = "app-deps";

#[derive(Serialize, Deserialize, Clone, Default)]
struct DayRecord {
    standalone_accumulated: i64,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    download_count: u64,
}

fn fetch_github_total(token: &str) -> Result<u64> {
    let client = Client::builder()
        .user_agent("evanalyzer-stats")
        .build()?;
    let mut total = 0u64;
    let mut url = format!(
        "https://api.github.com/repos/{}/releases?per_page=100",
        GITHUB_REPO
    );

    loop {
        let response = client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", format!("token {}", token))
            .send()
            .context("GitHub API request failed")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "GitHub API error: {} {}",
                response.status(),
                response.text().unwrap_or_default()
            );
        }

        let link = response
            .headers()
            .get("Link")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        let releases: Vec<GithubRelease> = response.json()?;
        for release in &releases {
            if release.tag_name == EXCLUDED_TAG {
                continue;
            }
            for asset in &release.assets {
                total += asset.download_count;
            }
        }

        match link.as_deref().and_then(parse_next_link) {
            Some(next) => url = next,
            None => break,
        }
    }

    Ok(total)
}

fn parse_next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        if part.contains(r#"rel="next""#) {
            let url_part = part.split(';').next()?.trim();
            Some(url_part.trim_matches(|c| c == '<' || c == '>').to_string())
        } else {
            None
        }
    })
}

fn fetch_imagej_by_day() -> Result<BTreeMap<String, i64>> {
    let client = Client::builder()
        .user_agent("evanalyzer-stats")
        .build()?;
    let response = client
        .get(IMAGEJ_STATS_URL)
        .send()
        .context("ImageJ stats request failed")?;

    let data: Value = response.json()?;
    let mut result = BTreeMap::new();

    if let Some(obj) = data.get(IMAGEJ_PLUGIN_KEY).and_then(|v| v.as_object()) {
        for (date, count) in obj {
            if let Some(n) = count.as_i64() {
                result.insert(date.clone(), n);
            }
        }
    }

    Ok(result)
}

fn build_series(
    imagej_by_day: &BTreeMap<String, i64>,
    stats: &BTreeMap<String, DayRecord>,
    accumulated: bool,
) -> (Vec<NaiveDate>, Vec<i64>, Vec<i64>) {
    let today = Local::now().date_naive();

    let start = imagej_by_day
        .keys()
        .filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .min()
        .unwrap_or(today);

    let mut dates = Vec::new();
    let mut imagej_vals = Vec::new();
    let mut standalone_vals = Vec::new();

    let mut imagej_acc = 0i64;
    let mut standalone_acc = 0i64;
    let mut current = start;

    while current <= today {
        let date_str = current.format("%Y-%m-%d").to_string();

        let imagej_today = imagej_by_day.get(&date_str).copied().unwrap_or(0);
        imagej_acc += imagej_today;

        let new_standalone_acc = stats
            .get(&date_str)
            .map(|r| r.standalone_accumulated)
            .unwrap_or(standalone_acc);
        let standalone_today = (new_standalone_acc - standalone_acc).max(0);
        standalone_acc = new_standalone_acc;

        dates.push(current);
        if accumulated {
            imagej_vals.push(imagej_acc);
            standalone_vals.push(standalone_acc);
        } else {
            imagej_vals.push(imagej_today);
            standalone_vals.push(standalone_today);
        }

        current += Duration::days(1);
    }

    (dates, imagej_vals, standalone_vals)
}

fn generate_chart(
    imagej_by_day: &BTreeMap<String, i64>,
    stats: &BTreeMap<String, DayRecord>,
    filename: &str,
    accumulated: bool,
) -> Result<()> {
    let (dates, imagej_vals, standalone_vals) = build_series(imagej_by_day, stats, accumulated);

    if dates.is_empty() {
        return Ok(());
    }

    let max_val = imagej_vals
        .iter()
        .chain(standalone_vals.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1);

    let min_date = *dates.first().unwrap();
    let max_date = *dates.last().unwrap();

    let root = BitMapBackend::new(filename, (1000, 500)).into_drawing_area();
    root.fill(&WHITE)?;

    let mut chart = ChartBuilder::on(&root)
        .margin(20)
        .x_label_area_size(50)
        .y_label_area_size(70)
        .build_cartesian_2d(min_date..max_date, 0i64..max_val)?;

    chart
        .configure_mesh()
        .x_label_formatter(&|d: &NaiveDate| d.format("%Y").to_string())
        .x_labels(6)
        .y_desc("Downloads")
        .draw()?;

    chart
        .draw_series(LineSeries::new(
            dates.iter().copied().zip(imagej_vals.iter().copied()),
            &BLUE,
        ))?
        .label("EVAnalyzer ImageJ Plugin")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));

    chart
        .draw_series(LineSeries::new(
            dates.iter().copied().zip(standalone_vals.iter().copied()),
            &GREEN,
        ))?
        .label("EVAnalyzer Standalone")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN));

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;

    root.present()?;
    Ok(())
}

fn main() -> Result<()> {
    let token = std::env::args()
        .nth(1)
        .context("Usage: stats <github_token>")?;

    let today_str = Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();

    println!("Fetching GitHub releases for {}...", GITHUB_REPO);
    let standalone_total = fetch_github_total(&token)?;
    println!("Standalone total downloads: {}", standalone_total);

    println!("Fetching ImageJ plugin stats...");
    let imagej_by_day = fetch_imagej_by_day()?;
    let imagej_total: i64 = imagej_by_day.values().sum();
    println!("ImageJ plugin total downloads: {}", imagej_total);

    let mut stats: BTreeMap<String, DayRecord> = fs::read_to_string(STATS_FILE)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    stats.insert(
        today_str,
        DayRecord {
            standalone_accumulated: standalone_total as i64,
        },
    );

    fs::write(STATS_FILE, serde_json::to_string_pretty(&stats)?)?;

    println!("Generating charts...");
    generate_chart(&imagej_by_day, &stats, "downloads_per_day.png", false)?;
    generate_chart(&imagej_by_day, &stats, "downloads_accumulated.png", true)?;

    println!("Done!");
    Ok(())
}
