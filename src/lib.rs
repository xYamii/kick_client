use futures_util::{SinkExt, StreamExt};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
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
                match msg {
                    Ok(Message::Text(text)) => {
                        let parsed_message: Option<ChatMessage> = serde_json::from_str(&text).ok();
                        if let Some(parsed_message) = parsed_message {
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
                        eprintln!("Received a non-text message");
                    }
                }
            }
        });

        Ok(rx)
    }
}

/// Enum representing different types of messages received from the WebSocket.
#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum MessageData {
    /// A chat message received in the chatroom.
    ChatMessage(ChatMessageEventData),
    /// A message indicating that user's message was deleted.
    DeletedMessage(DeletedMessageEventData),
    /// A message indicating that a user was banned from the chatroom.
    UserBanned(UserBannedEventData),
    /// A message indicating that a user was unbanned from the chatroom.
    UserUnbanned(UserUnbannedEventData),
    /// A message indicating that the chatroom was updated.
    ChatroomUpdated(ChatroomUpdatedEventData),
    /// A message indicating that the chatroom was cleared.
    ChatroomClear(ChatroomClearEventData),
    /// A message indicating that a poll was updated.
    PollUpdate(PollUpdateEventData),
    /// A message indicating that a poll was deleted.
    PollDelete(PollDeleteEventData),
    /// A message of unknown type.
    Unknown(String),
}

/// Data structure containing the content of a message.
#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessage {
    /// Type of the event send by the server.
    pub event: String,
    /// Data of the message.
    #[serde(deserialize_with = "deserialize_data")]
    pub data: Option<MessageData>,
    /// Channel id.
    pub channel: String,
}

fn deserialize_data<'de, D>(deserializer: D) -> Result<Option<MessageData>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;

    match serde_json::from_value(value) {
        Ok(data) => Ok(Some(data)),
        Err(_) => Ok(None),
    }
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

impl<'de> Deserialize<'de> for MessageData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data_str = String::deserialize(deserializer)?;
        match serde_json::from_str::<ChatMessageEventData>(&data_str) {
            Ok(chat_message) => return Ok(MessageData::ChatMessage(chat_message)),
            Err(_) => {}
        }

        match serde_json::from_str::<DeletedMessageEventData>(&data_str) {
            Ok(deleted_message) => return Ok(MessageData::DeletedMessage(deleted_message)),
            Err(_) => {}
        }

        match serde_json::from_str::<UserBannedEventData>(&data_str) {
            Ok(user_banned) => return Ok(MessageData::UserBanned(user_banned)),
            Err(_) => {}
        }

        match serde_json::from_str::<UserUnbannedEventData>(&data_str) {
            Ok(user_unbanned) => return Ok(MessageData::UserUnbanned(user_unbanned)),
            Err(_) => {}
        }

        match serde_json::from_str::<ChatroomUpdatedEventData>(&data_str) {
            Ok(chatroom_updated) => return Ok(MessageData::ChatroomUpdated(chatroom_updated)),
            Err(_) => {}
        }

        match serde_json::from_str::<ChatroomClearEventData>(&data_str) {
            Ok(chatroom_clear) => return Ok(MessageData::ChatroomClear(chatroom_clear)),
            Err(_) => {}
        }

        match serde_json::from_str::<PollUpdateEventData>(&data_str) {
            Ok(poll_update) => return Ok(MessageData::PollUpdate(poll_update)),
            Err(_) => {}
        }
        match serde_json::from_str::<PollDeleteEventData>(&data_str) {
            Ok(poll_delete) => return Ok(MessageData::PollDelete(poll_delete)),
            Err(_) => {}
        }

        Ok(MessageData::Unknown(data_str))
    }
}
