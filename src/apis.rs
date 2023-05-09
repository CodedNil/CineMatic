//! Access to the apis for getting api keys, making requests to openai, and sonarr/radarr

use reqwest::Method;
use std::fs::File;
use std::io::prelude::*;

use async_openai::types::{
    ChatCompletionRequestMessageArgs, CreateChatCompletionRequestArgs, Role,
};
use async_openai::Client as OpenAiClient;

#[derive(Clone)]
pub enum ArrService {
    Sonarr,
    Radarr,
}
impl std::fmt::Display for ArrService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sonarr => write!(f, "sonarr"),
            Self::Radarr => write!(f, "radarr"),
        }
    }
}

#[derive(Debug)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

fn get_credentials() -> toml::Value {
    // Read credentials.toml file to get keys
    let mut file = File::open("credentials.toml").expect("Failed to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .expect("Failed to read credentials file");
    let cred: toml::Value = contents.parse().expect("Failed to parse credentials TOML");

    cred
}

/// Get discord token
pub fn get_discord_token() -> String {
    let cred = get_credentials();

    // Configure the client with your Discord bot token
    let discord_token: String = cred["discord_token"]
        .as_str()
        .expect("Expected a discord_token in the credentials.toml file")
        .to_owned();
    discord_token
}

/// Get openai client
pub fn get_openai() -> OpenAiClient {
    let cred = get_credentials();

    // Configure the client with your openai api key
    let openai_api_key = cred["openai_api_key"]
        .as_str()
        .expect("Expected a openai_api_key in the credentials.toml file")
        .to_string();
    OpenAiClient::new().with_api_key(openai_api_key)
}

/// Use gpt to query information
pub async fn gpt_info_query(model: String, data: String, prompt: String) -> Result<String, String> {
    let openai = get_openai();

    // Search with gpt through the memories to answer the query
    let request = CreateChatCompletionRequestArgs::default()
        .model(model)
        .messages([
            ChatCompletionRequestMessageArgs::default()
                .role(Role::System)
                .content(data)
                .build()
                .unwrap(),
            ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .content(prompt)
                .build()
                .unwrap(),
        ])
        .build()
        .unwrap();

    // Retry the request if it fails
    let mut tries = 0;
    let response = loop {
        let response = openai.chat().create(request.clone()).await;
        if let Ok(response) = response {
            break Ok(response);
        }
        tries += 1;
        if tries >= 3 {
            break response;
        }
    };
    // Return from errors
    if response.is_err() {
        return Err("Failed to get response from openai".to_string());
    }
    let result = response
        .unwrap()
        .choices
        .first()
        .unwrap()
        .message
        .content
        .clone();
    Ok(result)
}

/// Make a request to an arr service
pub async fn arr_request(
    method: HttpMethod,
    service: ArrService,
    url: String,
    data: Option<String>,
) -> serde_json::Value {
    let cred = get_credentials();
    let arr = cred[&service.to_string()]
        .as_table()
        .expect("Expected a section in credentials.toml");
    let arr_api_key = arr["api"]
        .as_str()
        .expect("Expected an api in credentials.toml")
        .to_string();
    let arr_url = arr["url"]
        .as_str()
        .expect("Expected a url in credentials.toml")
        .to_string();
    let username = arr["authuser"]
        .as_str()
        .expect("Expected an authuser in credentials.toml")
        .to_string();
    let password = arr["authpass"]
        .as_str()
        .expect("Expected an authpass in credentials.toml")
        .to_string();

    let client = reqwest::Client::new();
    let request = client
        .request(
            match method {
                HttpMethod::Get => Method::GET,
                HttpMethod::Post => Method::POST,
                HttpMethod::Put => Method::PUT,
                HttpMethod::Delete => Method::DELETE,
            },
            format!("{arr_url}{url}"),
        )
        .basic_auth(username, Some(password))
        .header("X-Api-Key", arr_api_key);

    let request = if let Some(data) = data {
        request
            .header("Content-Type", "application/json")
            .body(data)
    } else {
        request
    };

    let response = request
        .send()
        .await
        .expect("Failed to send request")
        .text()
        .await
        .expect("Failed to get response");

    serde_json::from_str(&response).expect("Failed to parse json")
}

async fn get_tags_with_prefix(prefix: &str) -> Vec<String> {
    let tags_result = arr_request(HttpMethod::Get, ArrService::Sonarr, format!("/api/v3/tag"), None).await;
    let mut matched_tags = Vec::new();

    if let Ok(tags) = tags_result {
        if let Some(tags_array) = tags.as_array() {
            for tag in tags_array {
                if let Some(label) = tag["label"].as_str() {
                    if label.starts_with(prefix) {
                        matched_tags.push(label.to_string());
                    }
                }
            }
        }
    }

    matched_tags
}

pub async fn sync_user_tags(user_names: Vec<&str>) {
    let mut current_tags = get_tags_with_prefix("added-").await;
    let mut desired_tags: Vec<String> = user_names
        .iter()
        .map(|user_name| format!("added-{}", user_name))
        .collect();

    // Remove extra tags
    for tag in &current_tags {
        if !desired_tags.contains(tag) {
            if let Some(tag_id) = arr_request(HttpMethod::Get, ArrService::Sonarr, format!("/api/v3/tag?label={}", tag), None).await.unwrap().as_array().unwrap().get(0).map(|t| t["id"].as_i64().unwrap()) {
                arr_request(HttpMethod::Delete, ArrService::Sonarr, format!("/api/v3/tag/{}", tag_id), None).await.unwrap();
            }
        }
    }

    // Add missing tags
    for tag in desired_tags {
        if !current_tags.contains(&tag) {
            let body = json!({
                "label": tag
            });
            arr_request(HttpMethod::Post, ArrService::Sonarr, format!("/api/v3/tag"), Some(body)).await.unwrap();
        }
    }
}