use crate::plugins;
use anyhow::anyhow;
use async_openai::types::{
    ChatCompletionFunctions, ChatCompletionFunctionsArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestMessageArgs, CreateChatCompletionRequestArgs, Role,
};
use chrono::Local;
use futures::Future;
use serde_json::json;
use serenity::{model::channel::Message as DiscordMessage, prelude::Context as DiscordContext};
use std::{collections::HashMap, pin::Pin};

const USER_EMOJI: &str = "💬 ";
const BOT_EMOJI: &str = "☑️ ";

#[derive(Debug)]
struct Func {
    name: String,
    description: String,
    parameters: Vec<Param>,
    call_func: FuncType,
}

impl Func {
    fn new(name: &str, description: &str, parameters: Vec<Param>, call_func: FuncType) -> Self {
        Func {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters,
            call_func,
        }
    }
}

type FuncType =
    fn(HashMap<String, String>) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send>>;

#[derive(Debug, Clone)]
struct Param {
    name: String,
    description: String,
    required: bool,
    enum_values: Option<Vec<String>>,
}
impl Param {
    fn new(name: &str, description: &str) -> Self {
        Param {
            name: name.to_owned(),
            description: description.to_owned(),
            required: true,
            enum_values: None,
        }
    }

    fn with_enum_values(mut self, enum_values: &[&str]) -> Self {
        self.enum_values = Some(enum_values.iter().map(|&val| val.to_owned()).collect());
        self
    }

    fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("description".to_owned(), json!(self.description));
        map.insert("type".to_owned(), json!("string"));
        if let Some(ref enum_values) = self.enum_values {
            map.insert("enum".to_owned(), json!(enum_values));
        }
        json!(map)
    }
}

fn func_to_chat_completion(func: &Func) -> ChatCompletionFunctions {
    let properties: serde_json::Map<String, _> = func
        .parameters
        .iter()
        .map(|param| (param.name.clone(), param.to_json()))
        .collect();

    let required: Vec<_> = func
        .parameters
        .iter()
        .filter(|&param| param.required)
        .map(|param| param.name.clone())
        .collect();

    ChatCompletionFunctionsArgs::default()
        .name(&func.name)
        .description(&func.description)
        .parameters(json!({
            "type": "object",
            "properties": properties,
            "required": required,
        }))
        .build()
        .unwrap()
}

fn get_chat_completions() -> Vec<ChatCompletionFunctions> {
    get_functions()
        .iter()
        .map(func_to_chat_completion)
        .collect()
}

/// Get available functions data
fn get_functions() -> Vec<Func> {
    // Common parameters for the functions
    let format_param = Param::new("format", "The format of the media to be searched for")
        .with_enum_values(&["movie", "series"]);
    let quality_param = Param::new(
        "quality",
        "The quality to set the media to, default to 1080p if not specified",
    )
    .with_enum_values(&["SD", "720p", "1080p", "2160p", "720p/1080p", "Any"]);
    let id_param = Param::new("id", "The id of the media item");

    // Create the functions
    vec![
        Func::new(
            "web_search",
            "Search web for query",
            vec![Param::new(
                "query",
                "A query for information to be answered",
            )],
            plugins::websearch::ai_search_args,
        ),
        Func::new(
            "media_lookup",
            "Search the media server for query information about a piece of media",
            vec![
                format_param.clone(),
                Param::new(
                    "searches",
                    "List of movies/series to search for separated by pipe |, for example \"Game of Thrones|Watchmen|Cats\"",
                ),
                Param::new(
                    "query",
                    "A query for information to be answered, query should be phrased as a question, for example \"Available on the server?\" \"Is series Watchmen available on the server in the Ultimate Cut?\" \"What is Cats movie tmdbId/tvdbId?\" \"Who added series Game of Thrones to the server?\" \"What is series Game of Thrones tmdbId/tvdbId?\", if multiple results are returned, ask user for clarification",
                ),
            ],
            plugins::media::lookup_args,
        ),
        Func::new(
            "media_add",
            "Adds media to the server, perform a lookup first to get the tmdbId/tvdbId",
            vec![
                format_param.clone(),
                Param::new("db_id", "The tmdbId/tvdbId of the media item"),
                quality_param.clone(),
            ],
            plugins::media::add_args,
        ),
        Func::new(
            "media_setres",
            "Change the targeted resolution of a piece of media on the server, perform a lookup first to get the id",
            vec![format_param.clone(), id_param.clone(), quality_param.clone()],
            plugins::media::setres_args,
        ),
        Func::new(
            "media_remove",
            "Removes media from users requests, media items remain on the server if another user has requested also, perform a lookup first to get the id",
            vec![format_param.clone(), id_param.clone()],
            plugins::media::remove_args,
        ),
        Func::new(
            "media_wanted",
            "Returns a list of series that user or noone has requested ... Aim for the most condensed list while retaining clarity knowing that the user can always request more specific detail.",
            vec![
                format_param.clone(),
                Param::new(
                    "user",
                    "Self for the user that spoke, none to get a list of movies or series that noone has requested",
                )
                .with_enum_values(&["self", "none"]),
            ],
            plugins::media::wanted_args,
        ),
        Func::new(
            "media_downloads",
            "Returns a list of series or movies that are downloading and their status, if user asks how long until a series is on etc",
            Vec::new(),
            plugins::media::downloads_args,
        ),
    ]
}

/// Run function
async fn run_function(
    name: String,
    args: serde_json::Value,
    user_name: &str,
) -> anyhow::Result<String> {
    let functions = get_functions();

    for func in functions {
        if func.name == name {
            let mut args_map = HashMap::new();
            args_map.insert("user_name".to_string(), user_name.to_string());
            for (key, value) in args.as_object().unwrap() {
                args_map.insert(key.clone(), value.as_str().unwrap().to_string());
            }
            return (func.call_func)(args_map).await;
        }
    }

    Err(anyhow!("Function not found"))
}

/// Run the chat completition
pub async fn run_chat_completition(
    ctx: DiscordContext,
    mut bot_message: DiscordMessage,
    message_history_text: String,
    user_name: String,
    chat_query: Vec<ChatCompletionRequestMessage>,
) {
    // The initial messages to send to the API
    let mut chat_query: Vec<ChatCompletionRequestMessage> = chat_query;

    // Rerun the chat completition until either no function calls left, or max iterations reached
    let mut extra_history_text: String = String::new();
    let mut final_response: String = String::new();
    let mut counter = 0;
    while counter < 10 {
        let request = CreateChatCompletionRequestArgs::default()
            .max_tokens(512u16)
            .model("gpt-4-0613")
            .messages(chat_query.clone())
            .functions(get_chat_completions())
            .function_call("auto")
            .build()
            .unwrap();

        let response_message = async_openai::Client::new()
            .chat()
            .create(request)
            .await
            .unwrap()
            .choices
            .get(0)
            .unwrap()
            .message
            .clone();

        if let Some(function_call) = response_message.function_call {
            let function_name = function_call.name;
            let function_args: serde_json::Value = function_call.arguments.parse().unwrap();

            // Edit the discord message with function call in progress
            let ctx_c = ctx.clone();
            let mut bot_message_c = bot_message.clone();
            let new_message = format!(
                "{message_history_text}{extra_history_text}⌛ Running function {function_name} with arguments {function_args}"
            );
            tokio::spawn(async move {
                bot_message_c
                    .edit(&ctx_c.http, |msg| msg.content(new_message))
                    .await
                    .unwrap();
            });

            let function_response =
                run_function(function_name.clone(), function_args, &user_name).await;
            // Get function response as string if either ok or error
            let function_response_message =
                function_response.map_or_else(|error| error.to_string(), |response| response);
            // Truncate the function response to 100 characters
            let function_response_short = if function_response_message.len() > 150 {
                let trimmed_message = function_response_message
                    .chars()
                    .take(150)
                    .collect::<String>();
                format!("{trimmed_message}...")
            } else {
                function_response_message.clone()
            };

            // Edit the discord message with function call results
            extra_history_text.push_str(
                format!("🎬 Ran function {function_name} {function_response_short}\n",).as_str(),
            );
            let ctx_c = ctx.clone();
            let mut bot_message_c = bot_message.clone();
            let new_message = format!("{message_history_text}{extra_history_text}");
            tokio::spawn(async move {
                bot_message_c
                    .edit(&ctx_c.http, |msg| msg.content(new_message))
                    .await
                    .unwrap();
            });

            chat_query.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::Function)
                    .name(function_name)
                    .content(function_response_message)
                    .build()
                    .unwrap(),
            );
            counter += 1;
        } else {
            final_response = response_message.content.unwrap();
            break;
        }
    }

    // Edit the discord message finalised
    bot_message
        .edit(&ctx.http, |msg| {
            msg.content(format!(
                "{message_history_text}{extra_history_text}✅ {final_response}"
            ))
        })
        .await
        .unwrap();
}

/// Process the chat message from the user
pub async fn process_chat(
    user_name: String,            // The users name
    users_text: String,           // Users text to bot
    ctx: DiscordContext,          // The discord context
    bot_message: DiscordMessage,  // The message reply to the user
    message_history_text: String, // The message history text, each starts with emoji identifying role
) {
    // Go through each line of message_history_text, if it starts with 💬 add it to user_text_total
    let mut user_text_total = String::new();
    for line in message_history_text.lines() {
        if line.starts_with(USER_EMOJI) {
            user_text_total.push_str(line.replace(USER_EMOJI, "").as_str());
        }
    }
    // Add the users latest message
    user_text_total.push_str(&users_text);

    // Get current date and time in DD/MM/YYYY and HH:MM:SS format
    let date = Local::now().format("%d/%m/%Y").to_string();
    let time = Local::now().format("%H:%M").to_string();

    let mut chat_query: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestMessageArgs::default()
            .role(Role::System)
            .content(format!("You are media management assistant called CineMatic, enthusiastic, knowledgeable and passionate about all things media\nYou always run lookups to ensure correct id, do not rely on chat history, if the data you have received does not contain what you need you reply with the truthful answer of unknown, responses should all be on one line (with comma separation) and compact language, use emojis to express emotion to the user. The current date is {date}, the current time is {time}"))
            .build()
            .unwrap(),
    ];
    // Add message history minus the most recent line
    let mut just_history = if message_history_text.is_empty() {
        String::new()
    } else {
        message_history_text[..message_history_text.len() - 1].to_string()
    };
    // If it contains a \n then it has history
    if just_history.contains('\n') {
        // Remove the last line
        just_history =
            just_history[..just_history.rfind('\n').unwrap_or(just_history.len())].to_string();
        chat_query.push(
            ChatCompletionRequestMessageArgs::default()
                .role(Role::System)
                .content(format!(
                    "Message history:\n{}",
                    just_history
                        .replace(USER_EMOJI, "User: ")
                        .replace(BOT_EMOJI, "CineMatic: ")
                ))
                .build()
                .unwrap(),
        );
    }
    // Add users message
    chat_query.push(
        ChatCompletionRequestMessageArgs::default()
            .role(Role::User)
            .content(users_text.clone())
            .build()
            .unwrap(),
    );

    // Run chat completion
    run_chat_completition(
        ctx,
        bot_message,
        message_history_text,
        user_name,
        chat_query,
    )
    .await;
}
