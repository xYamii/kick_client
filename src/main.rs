use events::chat_events::{ChatMessage, MessageData};
use futures_util::{SinkExt, StreamExt};
use rusqlite::{params, Connection, Result};
use std::fs::OpenOptions;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message},
};
mod events;

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

// async fn save_message_to_db(conn: &Connection, username: &str, message: &str) -> Result<()> {
//     conn.execute(
//         "INSERT INTO chat_messages (username, message) VALUES (?1, ?2)",
//         params![username, message],
//     )?;
//     Ok(())
// }

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
                println!("Received a text message: {}", text);
                let parsed_message: Option<ChatMessage> = serde_json::from_str(&text).ok();
                println!("parsed_message: {:?}", parsed_message);
                if let Some(parsed_message) = parsed_message {
                    match parsed_message.data {
                        Some(MessageData::ChatMessage(data)) => {
                            println!("data: {:?}", data);
                        }
                        Some(MessageData::DeletedMessage(data)) => {
                            println!("Deleted message: {:?}", data);
                        }
                        Some(MessageData::Unknown(data)) => {
                            // println!("Unknown message: {:?}", data);
                        }
                        None => {
                            // println!("No data in message");
                        }
                        _ => {
                            // println!("No data in message");
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
