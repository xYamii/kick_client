use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use rusqlite::{params, Connection, Result};
use serde::Deserialize;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message},
};

#[derive(Deserialize)]
struct ChannelResponse {
    chatroom: Chatroom,
}

#[derive(Deserialize)]
struct Chatroom {
    id: u64,
}

async fn get_chatroom_id(username: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let url = format!("https://kick.com/api/v2/channels/{}", username);
    let client = Client::new();

    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        println!("Failed to retrieve data: {}", response.status());
        return Err("Non-successful status code")?;
    }

    let response_text = response.text().await?;
    println!("Response Text: {}", response_text);

    let response_json: ChannelResponse = serde_json::from_str(&response_text)?;
    Ok(response_json.chatroom.id)
}

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
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    let data = parsed.get("data");
                    if data == None {
                        println!("No data field found");
                        continue;
                    }
                    let data = data.unwrap();
                    if let Some(username) = data["username"].as_str() {
                        if let Some(message) = data["message"].as_str() {
                            println!("{}: {}", username, message);
                            save_message_to_db(&conn, username, message).await?;
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
    let username = "xYamii";
    let chatroom_id = 281473;

    subscribe_and_listen(chatroom_id).await?;
    Ok(())
}
