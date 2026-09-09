//! Connects to a channel's live chat and prints every message as it arrives.
//!
//! Run with:
//!     cargo run --example read_chat -- <channel-slug>
//! (defaults to "xyamii" if no slug is given)

use kick_client::KickClient;
use std::env;

#[tokio::main]
async fn main() {
    let channel = env::args().nth(1).unwrap_or_else(|| "delordione".to_string());

    let chatroom_id = KickClient::resolve_chatroom_id(&channel)
        .await
        .expect("failed to resolve chatroom id");
    println!("resolved '{channel}' -> chatroom id {chatroom_id}");

    let mut client = KickClient::new(
        "wss://ws-us2.pusher.com/app/32cbd69e4b950bf97679?protocol=7&client=js&version=8.4.0-rc2&flash=false",
        vec![chatroom_id],
    )
    .await
    .expect("failed to connect");

    println!("connected, listening for messages (Ctrl+C to stop)...");
    loop {
        match client.read_message().await {
            Ok(Some(message)) => println!("{:?}", message),
            Ok(None) => {
                println!("stream ended");
                break;
            }
            Err(e) => {
                eprintln!("error: {e}");
                break;
            }
        }
    }
}
