use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use rusqlite::{params, Connection, Result};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message},
};
use serde::{Deserialize, Serialize};
use serde::de::Deserializer;
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug)]
struct ChatMessage {
    event: String,
    #[serde(deserialize_with = "deserialize_data")]
    data: Option<MessageData>,
    channel: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
enum MessageData {
    ChatMessage(ChatMessageData),
    DeletedMessage(DeletedMessageData),
    Unknown(Value),
}

#[derive(Serialize, Deserialize, Debug)]
struct ChatMessageData {
    id: String,
    chatroom_id: Option<u32>,
    content: Option<String>,
    r#type: Option<String>,
    created_at: Option<String>,
    sender: Option<Sender>,
}

#[derive(Serialize, Deserialize, Debug)]
struct DeletedMessageData {
    id: String,
    message: DeletedMessage,
    ai_moderated: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct DeletedMessage {
    id: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Sender {
    id: u32,
    username: String,
    slug: String,
    identity: Identity,
}

#[derive(Serialize, Deserialize, Debug)]
struct Identity {
    color: String,
    badges: Vec<String>,
}

fn deserialize_data<'de, D>(deserializer: D) -> Result<Option<MessageData>, D::Error>
where
    D: Deserializer<'de>,
{
    let data_str = String::deserialize(deserializer)?;
    
    match serde_json::from_str(&data_str) {
        Ok(data) => Ok(Some(data)),
        Err(_) => Ok(None),
    }
}

// async fn get_chatroom_id(username: &str) -> Result<u64, Box<dyn std::error::Error>> {
//     let url = format!("https://kick.com/api/v2/channels/{}", username);
//     let client = Client::new();

//     let response = client.get(&url).send().await?;

//     if !response.status().is_success() {
//         println!("Failed to retrieve data: {}", response.status());
//         return Err("Non-successful status code")?;
//     }

//     let response_text = response.text().await?;
//     println!("Response Text: {}", response_text);

//     let response_json: ChannelResponse = serde_json::from_str(&response_text)?;
//     Ok(response_json.chatroom.id)
// }

async fn save_message_to_db(conn: &Connection, username: &str, message: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO chat_messages (username, message) VALUES (?1, ?2)",
        params![username, message],
    )?;
    Ok(())
}

async fn subscribe_and_listen(chatroom_id: u64) -> Result<(), Box<dyn std::error::Error>> {
    let request = "wss://ws-us2.pusher.com/app/32cbd69e4b950bf97679?protocol=7&client=js&version=8.4.0-rc2&flash=false".into_client_request().unwrap();
    let (ws_stream, _) = connect_async(request).await?;
    println!("Connected to the WebSocket server");

    let (mut write, mut read) = ws_stream.split();
    let subscribe_message = serde_json::json!({
        "event": "pusher:subscribe",
        "data": {
            "auth": "",
            "channel": format!("chatrooms.{}.v2", chatroom_id)
        }
    });

    write
        .send(Message::Text(subscribe_message.to_string()))
        .await?;
    let conn = Connection::open("chat_messages.db")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS chat_messages (
                  id INTEGER PRIMARY KEY,
                  username TEXT NOT NULL,
                  message TEXT NOT NULL
              )",
        [],
    )?;

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let parsed_message: Option<ChatMessage> = serde_json::from_str(&text).ok();
                if let Some(parsed_message) = parsed_message {
                    println!("{:?}", parsed_message);
                    match parsed_message.data {
                        Some(MessageData::ChatMessage(data)) => {
                        }
                        Some(MessageData::DeletedMessage(data)) => {
                        }
                        Some(MessageData::Unknown(data)) => {
                        }
                        None => {
                        }
                    }
                }
            }
            Err(e) => {
                println!("Error receiving message: {}", e);
            }
            _ => {
                println!("Received a non-text message");
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let chatroom_id = 281473;

    subscribe_and_listen(chatroom_id).await?;
    Ok(())
}
