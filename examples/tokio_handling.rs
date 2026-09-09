//! Demonstrates the `tokio-handling` feature's callback-based API.
//!
//! Run with:
//!     cargo run --example tokio_handling --features tokio-handling -- <channel-slug>
//! (defaults to "xyamii" if no slug is given)

use kick_client::KickClient;
use std::env;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let channel = env::args().nth(1).unwrap_or_else(|| "xyamii".to_string());

    let chatroom_id = KickClient::resolve_chatroom_id(&channel)
        .await
        .expect("failed to resolve chatroom id");

    let client = KickClient::new(
        "wss://ws-us2.pusher.com/app/32cbd69e4b950bf97679?protocol=7&client=js&version=8.4.0-rc2&flash=false",
        vec![chatroom_id],
    )
    .await
    .expect("failed to connect");

    client.start_handling(|message| {
        println!("{:?}", message);
    });

    // `start_handling` spawns a background task and returns immediately, so
    // keep the process alive to actually observe messages.
    println!("listening for 30s (Ctrl+C to stop earlier)...");
    tokio::time::sleep(Duration::from_secs(30)).await;
}
