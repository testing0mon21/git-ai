use serde::{Deserialize, Serialize};

/// Optional token/usage information for a model response.
///
/// This is intentionally flexible to support multiple agent formats.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MessageUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

/// Represents a single message in an AI transcript
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    User {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
    },
    Assistant {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
        /// Optional token usage reported by the agent for this response.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<MessageUsage>,
    },
    ToolUse {
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
    },
}

impl Message {
    /// Create a user message
    pub fn user(text: String, timestamp: Option<String>) -> Self {
        Message::User { text, timestamp }
    }

    /// Create an assistant message
    pub fn assistant(text: String, timestamp: Option<String>) -> Self {
        Message::Assistant {
            text,
            timestamp,
            usage: None,
        }
    }

    /// Create a tool use message
    pub fn tool_use(name: String, input: serde_json::Value) -> Self {
        Message::ToolUse {
            name,
            input,
            timestamp: None,
        }
    }

    /// Get the text content if this is a user or assistant message
    #[allow(dead_code)]
    pub fn text(&self) -> Option<&String> {
        match self {
            Message::User { text, .. } | Message::Assistant { text, .. } => Some(text),
            Message::ToolUse { .. } => None,
        }
    }

    /// Check if this is a tool use message
    #[allow(dead_code)]
    pub fn is_tool_use(&self) -> bool {
        matches!(self, Message::ToolUse { .. })
    }
}

/// Represents a complete AI transcript (collection of messages)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiTranscript {
    pub messages: Vec<Message>,
}

impl AiTranscript {
    /// Create a new empty transcript
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Add a message to the transcript
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Get all messages
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Filter out tool use messages
    #[allow(dead_code)]
    pub fn without_tool_use(&self) -> Self {
        let filtered_messages: Vec<Message> = self
            .messages
            .iter()
            .filter(|msg| !msg.is_tool_use())
            .cloned()
            .collect();

        Self {
            messages: filtered_messages,
        }
    }

    /// Parse a Claude Code JSONL file into a transcript and extract model info
    pub fn from_claude_code_jsonl_with_model(
        jsonl_content: &str,
    ) -> Result<(Self, Option<String>), serde_json::Error> {
        let mut transcript = AiTranscript::new();
        let mut model = None;

        for line in jsonl_content.lines() {
            if !line.trim().is_empty() {
                // Parse the raw JSONL entry
                let raw_entry: serde_json::Value = serde_json::from_str(line)?;
                let timestamp = raw_entry["timestamp"].as_str().map(|s| s.to_string());

                // Extract model from assistant messages if we haven't found it yet
                if model.is_none() && raw_entry["type"].as_str() == Some("assistant") {
                    if let Some(model_str) = raw_entry["message"]["model"].as_str() {
                        model = Some(model_str.to_string());
                    }
                }

                // Extract messages based on the type
                match raw_entry["type"].as_str() {
                    Some("user") => {
                        // Handle user messages
                        if let Some(content) = raw_entry["message"]["content"].as_str() {
                            if !content.trim().is_empty() {
                                transcript.add_message(Message::User {
                                    text: content.to_string(),
                                    timestamp: timestamp.clone(),
                                });
                            }
                        } else if let Some(content_array) =
                            raw_entry["message"]["content"].as_array()
                        {
                            // Handle user messages with content array (like tool results)
                            for item in content_array {
                                if let Some(text) = item["content"].as_str() {
                                    if !text.trim().is_empty() {
                                        transcript.add_message(Message::User {
                                            text: text.to_string(),
                                            timestamp: timestamp.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Some("assistant") => {
                        // Handle assistant messages
                        let usage = raw_entry
                            .get("message")
                            .and_then(|m| m.get("usage"))
                            .and_then(|u| u.as_object())
                            .map(|u| MessageUsage {
                                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).map(|v| v as u32),
                                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).map(|v| v as u32),
                                cache_creation_input_tokens: u
                                    .get("cache_creation_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .map(|v| v as u32),
                                cache_read_input_tokens: u
                                    .get("cache_read_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .map(|v| v as u32),
                            });

                        if let Some(content_array) = raw_entry["message"]["content"].as_array() {
                            for item in content_array {
                                match item["type"].as_str() {
                                    Some("text") => {
                                        if let Some(text) = item["text"].as_str() {
                                            if !text.trim().is_empty() {
                                                transcript.add_message(Message::Assistant {
                                                    text: text.to_string(),
                                                    timestamp: timestamp.clone(),
                                                    usage: usage.clone(),
                                                });
                                            }
                                        }
                                    }
                                    Some("tool_use") => {
                                        if let (Some(name), Some(_input)) =
                                            (item["name"].as_str(), item["input"].as_object())
                                        {
                                            transcript.add_message(Message::ToolUse {
                                                name: name.to_string(),
                                                input: item["input"].clone(),
                                                timestamp: timestamp.clone(),
                                            });
                                        }
                                    }
                                    _ => continue, // Skip unknown content types
                                }
                            }
                        }
                    }
                    _ => continue, // Skip unknown message types
                }
            }
        }

        Ok((transcript, model))
    }
}

impl Default for AiTranscript {
    fn default() -> Self {
        Self::new()
    }
}
