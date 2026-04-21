use reqwest::Client;
use scraper::{Html, Selector};
use serde_json::Value;
use tokio::sync::Semaphore;
use futures::future::join_all;
use std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct ServerConfig {
    speed: String,
    config: String,
}

async fn get_text(client: &Client, url: String,semaphore: Arc<Semaphore>) -> Option<String> {
    let _permit = semaphore.acquire().await.unwrap();
    
    let response = client
        .get(url)
        .header("user-agent", "love_linux_mint")
        .header("x-requested-with", "XMLHttpRequest")
        .header("Connection", "keep-alive")
        .header("host", "ru.v2nodes.com")
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    
    match response {
        Ok(resp) => {
            if resp.status().is_success() {
                return resp.text().await.ok();
            }
            None
        }
        Err(_) => None,
    }
}

async fn post_json(
    client: &Client,
    req_id: &str,
    semaphore: Arc<Semaphore>,
) -> Option<Value> {
    let _permit = semaphore.acquire().await.unwrap();
    
    let params = HashMap::from([("id", req_id)]);
    
    let response = client
        .post("https://ru.v2nodes.com/checkServers.json")
        .form(&params)
        .header("user-agent", "love_linux_mint")
        .header("x-requested-with", "XMLHttpRequest")
        .header("Connection", "keep-alive")
        .header("host", "ru.v2nodes.com")
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    
    match response {
        Ok(resp) => {
            if resp.status().is_success() {
                return resp.json().await.ok();
            }
            None
        }
        Err(_) => None,
    }
}

async fn get_servers(
    client: &Client, 
    semaphore: Arc<Semaphore>,
) -> Vec<String> {
    let mut tasks = vec![];
    
    for page in 1..84 {
        let url = format!("https://ru.v2nodes.com/?page={}", page);
        tasks.push(get_text(client, url, semaphore.clone()));
    }
    
    let pages_content = join_all(tasks).await;
    let mut servers = Vec::new();
    
    let link_selector = Selector::parse("a.text-decoration-none").unwrap();
    
    for content in pages_content {
        if let Some(html) = content {
            let document = Html::parse_document(&html);
            
            for element in document.select(&link_selector) {
                if let Some(href) = element.value().attr("href") {
                    servers.push(href.to_string());
                }
            }
        }
    }
    
    servers
}

async fn process_server(
    client: &Client,
    server: &str,
    semaphore: Arc<Semaphore>,
) -> Option<ServerConfig> {
    let url = format!("https://ru.v2nodes.com{}", server);
    
    let req_id = server
        .split("servers/")
        .nth(1)
        .and_then(|s| s.split('/').next());
    
    let req_id = match req_id {
        Some(id) => id,
        None => return None,
    };
    
    let html = match get_text(client, url, semaphore.clone()).await {
        Some(h) => h,
        None => return None,
    };
    
    let speed_data = match post_json(client, req_id, semaphore.clone()).await {
        Some(data) => data,
        None => return None,
    };
    
    let speed = speed_data
        .get("response")?.to_string();
    
    let document = Html::parse_document(&html);
    let textarea_selector = Selector::parse("textarea").unwrap();
    
    let config = document
        .select(&textarea_selector)
        .next()
        .map(|el| el.inner_html())
        .unwrap_or_default();
    
    if !config.is_empty() {
        Some(ServerConfig { speed, config })
    } else {
        None
    }
}

#[tokio::main]
async fn main() {
    let max_concurrent = 10;
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to create HTTP client");
    
    println!("Fetching server list...");
    let servers = get_servers(&client, semaphore.clone()).await;
    println!("Found {} servers", servers.len());
    
    println!("Testing server speeds...");
    let mut tasks = vec![];
    
    for server in &servers {
        let task = process_server(&client, server, semaphore.clone());
        tasks.push(task);
    }
    
    let results = join_all(tasks).await;
    
    let mut configs: Vec<ServerConfig> = results.into_iter().flatten().collect();
    
    configs.sort_by_key(|cfg| cfg.speed.clone());
    
    println!("\n--- TOP 10 FASTEST CONFIGS ---");
    for cfg in configs.iter().take(10) {
        println!("Speed: {} | Config: {}", cfg.speed, cfg.config.clone());
    }
    
    println!("\nTotal valid configs: {}", configs.len());
}
