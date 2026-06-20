use reqwest::Client;
use scraper::{Html, Selector};
use serde_json::Value;
use std::fs::File;
use std::io::{Write, BufWriter,stdin, stdout};
use tokio::sync::Semaphore;
use futures::future::join_all;
use std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;
use regex::Regex;
use indicatif::{ProgressBar, ProgressStyle};

const BASE_URL: &str = "https://ru.v2nodes.com";
const MAX_CONCURRENT: usize = 15;

async fn get_text(client: &Client, url: String, semaphore: Arc<Semaphore>) -> Option<String> {
    let _permit = semaphore.acquire().await.unwrap();
    client.get(&url)
        .header("user-agent", "Mozilla/5.0 (Macintosh; PPC Mac OS X 10_12_6 rv:4.0; sr-ME) AppleWebKit/533.21.6 (KHTML, like Gecko) Version/4.0 Safari/533.21.6")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()
}

async fn post_json(client: &Client, req_id: &str, semaphore: Arc<Semaphore>) -> Option<Value> {
    let _permit = semaphore.acquire().await.unwrap();
    client.post(format!("{}/checkServers.json", BASE_URL))
        .form(&HashMap::from([("id", req_id)]))
        .header("user-agent", "Mozilla/5.0 (Macintosh; PPC Mac OS X 10_12_6 rv:4.0; sr-ME) AppleWebKit/533.21.6 (KHTML, like Gecko) Version/4.0 Safari/533.21.6")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()
}

async fn parse_countries(client: &Client, semaphore: Arc<Semaphore>) -> Vec<(String, String)> {
    let html = get_text(client, format!("{}/?page=1", BASE_URL), semaphore).await;
    
    if let Some(html_content) = html {
        let doc = Html::parse_document(&html_content);
        let country_selector = Selector::parse("a.btn.btn-default").unwrap();
        let mut countries = Vec::new();
        
        for el in doc.select(&country_selector) {
            if let Some(href) = el.value().attr("href") {
                if href.starts_with("/country/") {
                    if let Some(code) = href.split('/').nth(2) {
                        let code = code.to_uppercase();
                        let name = el.text().collect::<String>().trim().to_string();
                        if !code.is_empty() && !name.is_empty() {
                            countries.push((name, code));
                        }
                    }
                }
            }
        }
        
        countries.sort_by(|a, b| a.0.cmp(&b.0));
        countries.dedup_by(|a, b| a.1 == b.1);
        return countries;
    }
    
    Vec::new()
}

fn display_countries(countries: &[(String, String)]) {
    println!("\n📋 Available countries:");
    println!("{:-^60}", "");
    for (i, (name, code)) in countries.iter().enumerate() {
        println!("{:2}. {} ({})", i + 1, name, code);
        if (i + 1) % 20 == 0 && i + 1 < countries.len() {
            println!("{:-^60}", "");
        }
    }
    println!("{:-^60}", "");
    println!("0. All countries (no filter)\n");
}

fn get_user_choice(max: usize) -> usize {
    loop {
        print!("👉 Select country number (0 for all): ");
        stdout().flush().unwrap();
        
        let mut input = String::new();
        stdin().read_line(&mut input).unwrap();
        
        if let Ok(num) = input.trim().parse::<usize>() {
            if num == 0 || (num >= 1 && num <= max) {
                return num;
            }
        }
        println!("❌ Invalid input. Please try again.");
    }
}

async fn get_servers(client: &Client, semaphore: Arc<Semaphore>, pb: &ProgressBar, country_code: Option<&str>) -> Vec<String> {
    pb.set_message("Fetching pages...");
    
    let base_url = if let Some(code) = country_code {
        format!("{}/country/{}/", BASE_URL, code.to_lowercase())
    } else {
        format!("{}/", BASE_URL)
    };
    
    let html = get_text(client, format!("{}?page=1", base_url), semaphore.clone()).await;
    let total_pages = html.and_then(|h| {
        let doc = Html::parse_document(&h);
        let selector = Selector::parse("ul.pagination li.page-item:last-child a").unwrap();
        doc.select(&selector).next().and_then(|el| el.value().attr("href"))
            .and_then(|href| Regex::new(r"page=(\d+)").unwrap().captures(href))
            .and_then(|caps| caps[1].parse::<usize>().ok())
    }).unwrap_or(1);
    
    pb.set_length(total_pages as u64);
    
    let mut tasks = vec![];
    for page in 1..=total_pages {
        tasks.push(get_text(client, format!("{}?page={}", base_url, page), semaphore.clone()));
    }
    
    let mut servers = Vec::new();
    let selector = Selector::parse("a.text-decoration-none").unwrap();
    
    for content in join_all(tasks).await {
        if let Some(html) = content {
            let doc = Html::parse_document(&html);
            for el in doc.select(&selector) {
                if let Some(href) = el.value().attr("href") {
                    if href.contains("/servers/") {
                        servers.push(href.to_string());
                    }
                }
            }
        }
        pb.inc(1);
    }
    
    servers.sort(); 
    servers.dedup();
    
    let message = if let Some(code) = country_code {
        format!("Found {} servers in {}", servers.len(), code.to_uppercase())
    } else {
        format!("Found {} servers (all countries)", servers.len())
    };
    pb.finish_with_message(message);
    
    servers
}

async fn process_server(client: &Client, server: &str, semaphore: Arc<Semaphore>) -> Option<String> {
    let req_id = server.split("servers/").nth(1)?.split('/').next()?;
    
    let (html_result, data_result) = tokio::join!(
        get_text(client, format!("{}{}", BASE_URL, server), semaphore.clone()),
        post_json(client, req_id, semaphore.clone())
    );
    
    let html = html_result?;
    let data = data_result?;
    let speed_value = data.get("response")?;
    let speed_str = speed_value.as_str()?;
    
    let _speed_ms = speed_str
        .trim_matches('"')
        .trim_end_matches("ms")
        .trim()
        .parse::<u32>()
        .ok()?;
    
    let doc = Html::parse_document(&html);
    let textarea_selector = Selector::parse("textarea").unwrap();
    let config = doc.select(&textarea_selector).next()?.inner_html();
    
    if config.is_empty() {
        return None;
    }
    
    Some(config)
}

#[tokio::main]
async fn main() {
    let pb = ProgressBar::new(100);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} - {msg}")
        .unwrap()
        .progress_chars("#>-"));
    
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let client = Client::new();
    
    pb.set_message("Loading countries...");
    let countries = parse_countries(&client, semaphore.clone()).await;
    
    if countries.is_empty() {
        println!("❌ Failed to load country list!");
        return;
    }
    
    display_countries(&countries);
    let choice = get_user_choice(countries.len());
    
    let country_code = if choice > 0 {
        Some(countries[choice - 1].1.as_str())
    } else {
        None
    };
    
    if let Some(code) = country_code {
        println!("🔍 Filtering by country: {}", code);
    } else {
        println!("🌍 No filter (all countries)");
    }
    
    let servers = get_servers(&client, semaphore.clone(), &pb, country_code).await;
    
    if servers.is_empty() {
        println!("❌ No servers found!");
        return;
    }
    
    let pb = ProgressBar::new(servers.len() as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} - {msg}")
        .unwrap()
        .progress_chars("#>-"));
    pb.set_message("Downloading configs...");
    
    let mut tasks = vec![];
    for server in &servers {
        tasks.push(process_server(&client, server, semaphore.clone()));
    }
    
    let configs: Vec<String> = join_all(tasks).await.into_iter().flatten().collect();
    pb.finish_with_message("Done!");
    
    if !configs.is_empty() {
        let filename = if let Some(code) = country_code {
            format!("configs_{}.txt", code.to_lowercase())
        } else {
            "configs_all.txt".to_string()
        };
        
        let mut file = BufWriter::new(File::create(&filename).unwrap());
        for cfg in &configs {
            let _ = writeln!(file, "{}", cfg);
        }
        println!("✅ Saved {} configs to {}", configs.len(), filename);
    } else {
        println!("❌ No configs found!");
    }
}
