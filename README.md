# KickClient

KickClient is a WebSocket-based library for interacting with Kick chatrooms.

## Features

- Subscribe to chatrooms.
- Receive and process messages in real-time.
- Resolve a channel slug (e.g. `"trainwreckstv"`) to its numeric chatroom ID.
- Automatic keep-alive (responds to server pings, pings back when idle) and
  transparent reconnection if the connection drops.

## Example

```rust
use kick_client::KickClient;

#[tokio::main]
async fn main() {
    let chatroom_id = KickClient::resolve_chatroom_id("roshtein").await.unwrap();
    let mut client = KickClient::new("wss://ws-us2.pusher.com/app/32cbd69e4b950bf97679?protocol=7&client=js&version=8.4.0-rc2&flash=false", vec![chatroom_id]).await.unwrap();

    while let Some(message) = client.read_message().await.unwrap() {
        println!("{:?}", message);
    }
}
