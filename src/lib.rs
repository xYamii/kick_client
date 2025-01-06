use futures_util::{SinkExt, StreamExt};
use serde::de::{DeserializeOwned, Deserializer};
use serde::{de, Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

/// A WebSocket client for connecting to and reading messages from Kick chatroom.
pub struct KickClient {
    /// The WebSocket URL used to connect to the Kick server.
    url: String,
}

impl KickClient {
    /// Creates a new instance of `KickClient` with the provided WebSocket URL.
    ///
    /// # Arguments
    ///
    /// * `url` - The WebSocket URL to connect to.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = KickClient::new("wss://example.com");
    /// ```
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
        }
    }
    /// Connects to the WebSocket and subscribes to a Kick chatroom.
    ///
    /// # Arguments
    ///
    /// * `chatroom_id` - The ID of the chatroom to subscribe to.
    ///
    /// # Returns
    ///
    /// A `tokio::sync::mpsc::Receiver` that can be used to receive parsed messages.
    ///
    /// # Errors
    ///
    /// This function will return an error if the WebSocket connection fails.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio_test::block_on(async {
    /// let client = KickClient::new("wss://example.com");
    /// let mut messages = client.connect(12345).await.unwrap();
    /// while let Some(message) = messages.recv().await {
    ///     if let Some(data) = message.data {
    ///         println!("Message: {:?}", data);
    ///     }
    /// }
    /// # });
    /// ```
    pub async fn connect(
        &self,
        chatroom_id: u64,
    ) -> Result<mpsc::Receiver<ChatMessage>, Box<dyn Error>> {
        let (ws_stream, _) = connect_async(self.url.clone()).await?;
        let (mut write, mut read) = ws_stream.split();

        let subscribe_message = serde_json::json!({
            "event": "pusher:subscribe",
            "data": {
                "auth": "",
                "channel": format!("chatrooms.{}.v2", chatroom_id)
            }
        });

        write
            .send(Message::Text(subscribe_message.to_string().into()))
            .await?;

        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                println!("[tokio::spawn] recv msg: {:?}", msg);
                match msg {
                    Ok(Message::Text(text)) => {
                        let parsed_message: serde_json::Result<ChatMessage> =
                            serde_json::from_str(&text)
                                .inspect_err(|e| eprintln!("Error parsing message: {}", e));
                        if let Ok(parsed_message) = parsed_message {
                            if tx.send(parsed_message).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error receiving message: {}", e);
                        break;
                    }
                    _ => {
                        eprintln!("Received a non-text message:\n {:?}", msg);
                    }
                }
            }
        });

        Ok(rx)
    }
}

/// Enum representing different types of messages received from the WebSocket.
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "event", content = "data")]
pub enum MessageData {
    /// A chat message received in the chatroom.
    #[serde(rename = "App\\Events\\ChatMessageEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    ChatMessage(ChatMessageEventData),
    /// A message indicating that user's message was deleted.
    #[serde(rename = "App\\Events\\DeletedMessageEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    DeletedMessage(DeletedMessageEventData),
    /// A message indicating that a user was banned from the chatroom.
    #[serde(rename = "App\\Events\\UserBannedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    UserBanned(UserBannedEventData),
    /// A message indicating that a user was unbanned from the chatroom.
    #[serde(rename = "App\\Events\\UserUnbannedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    UserUnbanned(UserUnbannedEventData),
    /// A message indicating that the chatroom was updated.
    #[serde(rename = "App\\Events\\ChatroomUpdatedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    ChatroomUpdated(ChatroomUpdatedEventData),
    /// A message indicating that the chatroom was cleared.
    #[serde(rename = "App\\Events\\ChatroomClearEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    ChatroomClear(ChatroomClearEventData),
    /// A message indicating that a poll was updated.
    #[serde(rename = "App\\Events\\PollUpdateEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PollUpdate(PollUpdateEventData),
    /// A message indicating that a poll was deleted.
    #[serde(rename = "App\\Events\\PollDeleteEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PollDelete(PollDeleteEventData),
    /// A message of unknown type.
    Unknown(String),
}

/// Data structure containing the content of a message.
#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessage {
    #[serde(flatten)]
    pub data: MessageData,
    /// Channel id.
    pub channel: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageEventData {
    pub id: String,
    pub chatroom_id: u32,
    pub content: Option<String>,
    pub r#type: Option<String>,
    pub created_at: Option<String>,
    pub sender: ChatMessageSender,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSender {
    id: u32,
    username: String,
    slug: String,
    identity: ChatMessageSenderIdentity,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSenderIdentity {
    color: Option<String>,
    badges: Vec<ChatMessageSenderBadge>,
}

#[derive(Serialize, Debug)]
pub enum ChatMessageSenderBadge {
    FullBadge {
        r#type: String,
        text: String,
        count: Option<u32>,
    },
    SimpleBadge {
        r#type: String,
        text: String,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserBannedEventData {
    id: u32,
    user: User,
    banned_by: User,
    pernament: bool,
    duration: Option<u64>,
    expires_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserUnbannedEventData {
    id: u32,
    user: User,
    unbanned_by: User,
    pernament: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct User {
    id: u32,
    username: String,
    slug: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomUpdatedEventData {
    id: u32,
    slow_mode: SlowMode,
    subscribers_mode: SubscribersMode,
    followers_mode: FollowersMode,
    emotes_mode: EmotesMode,
    advanced_bot_protection: AdvancedBotProtection,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SlowMode {
    enabled: bool,
    message_interval: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SubscribersMode {
    enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FollowersMode {
    enabled: bool,
    min_duration: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EmotesMode {
    enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AdvancedBotProtection {
    enabled: bool,
    remaining_time: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomClearEventData {
    id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletedMessageEventData {
    id: String,
    message: DeletedMessage,
    ai_moderated: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletedMessage {
    id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollUpdateEventData {
    poll: Poll,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Poll {
    title: String,
    options: Vec<PollOption>,
    duration: u32,
    remaining: u32,
    result_display_duration: u32,
    has_voted: Option<bool>,
    voted_option_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollOption {
    id: u32,
    label: String,
    votes: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollDeleteEventData {}

impl<'de> Deserialize<'de> for ChatMessageSenderBadge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Debug)]
        struct BadgeHelper {
            r#type: String,
            text: String,
            count: Option<u32>,
        }

        let helper = BadgeHelper::deserialize(deserializer)?;
        if let Some(count) = helper.count {
            Ok(ChatMessageSenderBadge::FullBadge {
                r#type: helper.r#type,
                text: helper.text,
                count: Some(count),
            })
        } else {
            Ok(ChatMessageSenderBadge::SimpleBadge {
                r#type: helper.r#type,
                text: helper.text,
            })
        }
    }
}
fn json_string_to_struct<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let s = String::deserialize(deserializer)?;
    serde_json::from_str(&s).map_err(serde::de::Error::custom)
}
