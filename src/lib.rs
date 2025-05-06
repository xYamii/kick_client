use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_tungstenite::{tungstenite, MaybeTlsStream, WebSocketStream};

/// A WebSocket client for connecting to and reading messages from Kick chatroom.
pub struct KickClient {
    #[allow(dead_code)]
    /// The WebSocket URL used to connect to the Kick server.
    url: String,
    #[allow(dead_code)]
    /// The channel ID for the subscribed chatroom.
    channel_id: u64,
    /// The WebSocket read stream for receiving messages.
    read_stream: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl KickClient {
    /// Creates a new instance of `KickClient` and automatically establishes a WebSocket connection.
    ///
    /// # Arguments
    ///
    /// * `url` - The WebSocket URL to connect to.
    /// * `channel_id` - The ID of the chatroom to subscribe to.
    ///
    /// # Returns
    ///
    /// A `KickClient` instance ready to receive messages.
    ///
    /// # Errors
    ///
    /// This function will return an error if the WebSocket connection fails.
    /// # Examples
    ///
    /// ```
    /// let mut client = KickClient::new("wss://ws-us2.pusher.com/app/32cbd69e4b950bf97679?protocol=7&client=js&version=8.4.0-rc2&flash=false", 281473).await.unwrap();
    /// while let Some(message) = client.read_message().await.unwrap() {
    ///     println!("{:?}", message);
    /// }
    /// ```
    pub async fn new(url: &str, channel_id: u64) -> Result<Self, Box<dyn Error>> {
        let request = url.into_client_request()?;
        let (ws_stream, _) = connect_async(request).await?;
        let (mut write, read) = ws_stream.split();

        // Create a subscription message
        let subscribe_message = serde_json::json!({
            "event": "pusher:subscribe",
            "data": {
                "auth": "",
                "channel": format!("chatrooms.{}.v2", channel_id)
            }
        });

        write
            .send(Message::Text(subscribe_message.to_string().into()))
            .await?;

        Ok(Self {
            url: url.to_string(),
            channel_id,
            read_stream: read,
        })
    }

    /// Reads the next message from the WebSocket stream and returns a parsed `KickChatMessage`.
    ///
    /// # Returns
    ///
    /// A `KickChatMessage` if a valid message is received, or `None` if the stream ends.
    ///
    /// # Errors
    ///
    /// This function will return an error if the WebSocket stream encounters an error.
    pub async fn read_message(&mut self) -> Result<Option<KickChatMessage>, KickError> {
        if let Some(msg) = self.read_stream.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let parsed_message =
                        serde_json::from_str(&text).map_err(KickError::MessageParseError)?;
                    return Ok(Some(parsed_message));
                }
                Err(e) => {
                    return Err(KickError::WebSocketError(e));
                }
                _ => {
                    return Ok(Some(KickChatMessage {
                        data: MessageData::Unknown(None),
                        channel: None,
                    }));
                }
            }
        }
        println!("something broke lol");
        Err(KickError::StreamEnded)
    }

    /// If the `tokio-handling` feature is enabled, this function spawns a task that handles
    /// incoming messages and invokes the provided callback for each message.
    #[cfg(feature = "tokio-handling")]
    pub fn start_handling<F>(mut self, callback: F)
    where
        F: Fn(KickMessage) + Send + Sync + 'static,
    {
        tokio::spawn(async move {
            while let Ok(Some(message)) = self.read_message().await {
                callback(message);
            }
        });
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
    /// A message indicating that the connection was established.
    #[serde(rename = "pusher:connection_established")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PusherConnectionEstablished(PusherConnectionEstablishedEventData),
    /// A message indicating that a subscription was pushed and was successful.
    #[serde(rename = "pusher_internal:subscription_succeeded")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PusherSubscriptionSucceeded(PusherSubscriptionSucceededEventData),
    /// A messenge indicating that the connection is still alive.
    #[serde(rename = "pusher:pong")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PusherPong(PusherPongEventData),
    /// A message of unknown type.
    Unknown(Option<String>),
}

/// Data structure containing the content of a message.
#[derive(Serialize, Deserialize, Debug)]
pub struct KickChatMessage {
    #[serde(flatten)]
    pub data: MessageData,
    /// Channel id.
    pub channel: Option<String>,
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
    pub id: u32,
    pub username: String,
    pub slug: String,
    pub identity: ChatMessageSenderIdentity,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSenderIdentity {
    pub color: Option<String>,
    pub badges: Vec<ChatMessageSenderBadge>,
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
    pub id: String,
    pub user: User,
    pub banned_by: User,
    pub permanent: bool,
    pub duration: Option<u64>,
    pub expires_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserUnbannedEventData {
    pub id: String,
    pub user: User,
    pub unbanned_by: User,
    pub permanent: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct User {
    pub id: u32,
    pub username: String,
    pub slug: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomUpdatedEventData {
    pub id: u32,
    pub slow_mode: SlowMode,
    pub subscribers_mode: SubscribersMode,
    pub followers_mode: FollowersMode,
    pub emotes_mode: EmotesMode,
    pub advanced_bot_protection: AdvancedBotProtection,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SlowMode {
    pub enabled: bool,
    pub message_interval: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SubscribersMode {
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FollowersMode {
    pub enabled: bool,
    pub min_duration: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EmotesMode {
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AdvancedBotProtection {
    pub enabled: bool,
    pub remaining_time: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomClearEventData {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletedMessageEventData {
    pub id: String,
    pub message: DeletedMessage,
    pub ai_moderated: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletedMessage {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollUpdateEventData {
    pub poll: Poll,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Poll {
    pub title: String,
    pub options: Vec<PollOption>,
    pub duration: u32,
    pub remaining: u32,
    pub result_display_duration: u32,
    pub has_voted: Option<bool>,
    pub voted_option_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollOption {
    pub id: u32,
    pub label: String,
    pub votes: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollDeleteEventData {}

#[derive(Serialize, Deserialize, Debug)]
pub struct PusherConnectionEstablishedEventData {
    pub socket_id: String,
    pub activity_timeout: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PusherSubscriptionSucceededEventData {}

#[derive(Serialize, Deserialize, Debug)]
pub struct PusherPongEventData {}

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

/// Enum representing possible errors in KickClient.
#[derive(Debug)]
pub enum KickError {
    WebSocketError(tungstenite::Error),
    MessageParseError(serde_json::Error),
    StreamEnded,
}

impl fmt::Display for KickError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KickError::WebSocketError(err) => write!(f, "WebSocket error: {}", err),
            KickError::MessageParseError(err) => write!(f, "Message parse error: {}", err),
            KickError::StreamEnded => write!(f, "WebSocket stream ended unexpectedly"),
        }
    }
}

impl std::error::Error for KickError {}

impl From<tungstenite::Error> for KickError {
    fn from(err: tungstenite::Error) -> Self {
        KickError::WebSocketError(err)
    }
}

impl From<serde_json::Error> for KickError {
    fn from(err: serde_json::Error) -> Self {
        KickError::MessageParseError(err)
    }
}
