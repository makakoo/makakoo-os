//! `TelegramBot` — teloxide driver wired to `ChatStore + IntelligentRouter
//! + LlmClient`.
//!
//! Behaviour contract:
//!
//! 1. The bot replies **only** to incoming messages from allow-listed chats.
//!    There is no unsolicited outbound path — the `send_unsolicited` method
//!    is intentionally not exposed. This is the hard rule from
//!    `core/chat/gateway.py`.
//! 2. On each incoming text message:
//!       * get or create the conversation for `(telegram, chat_id)`,
//!       * append the user message,
//!       * route via `IntelligentRouter`,
//!       * for `DirectAnswer`, call `LlmClient::chat` with the recent
//!         history (8 messages) using the configured model,
//!       * for `ToolCall` / `Command`, return a placeholder that names
//!         the tool or command — the actual dispatcher is the MCP /
//!         handler tier, not this module,
//!       * append the assistant reply and send it back on the same
//!         chat id.
//! 3. Non-text updates (photos, stickers, voice) are dropped silently.
//!    Voice handling is a later mini-task.

use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::ChatId;

use crate::chat::router::{IntelligentRouter, RouteDecision};
use crate::chat::store::ChatStore;
use crate::error::{MakakooError, Result};
use crate::llm::{ChatMessage as LlmChatMessage, LlmClient};

/// Default chat-completion model. Matches the Python gateway default.
const DEFAULT_MODEL: &str = "ail-compound";
/// How many of the most recent messages to feed the LLM as history.
const HISTORY_WINDOW: usize = 8;

/// Telegram bot wrapper. Holds shared references to every collaborator it
/// needs. Cheap to clone thanks to `Arc`s.
#[derive(Clone)]
pub struct TelegramBot {
    bot: Bot,
    store: Arc<ChatStore>,
    router: Arc<IntelligentRouter>,
    llm: Arc<LlmClient>,
    allowlist: Arc<Vec<i64>>,
    model: String,
}

impl TelegramBot {
    /// Construct a new bot. `token` is the raw Telegram HTTP token;
    /// `allowlist` is the set of chat IDs the bot will reply to.
    pub fn new(
        token: &str,
        store: Arc<ChatStore>,
        router: Arc<IntelligentRouter>,
        llm: Arc<LlmClient>,
        allowlist: Vec<i64>,
    ) -> Self {
        Self {
            bot: Bot::new(token),
            store,
            router,
            llm,
            allowlist: Arc::new(allowlist),
            model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Override the default model used for direct answers.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Return the current allowlist. Primarily for diagnostics and tests.
    pub fn allowlist(&self) -> &[i64] {
        &self.allowlist
    }

    /// Return the bot's currently configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Check whether a chat id is allowed to talk to the bot. This is the
    /// single enforcement point for the "never auto-send" rule — every
    /// outbound call in `run_forever` is gated behind this check on the
    /// inbound chat id.
    pub fn is_allowed(&self, chat_id: i64) -> bool {
        self.allowlist.contains(&chat_id)
    }

    /// Drive the bot forever. Uses `teloxide::repl`, so Ctrl-C tears it
    /// down cleanly via the `ctrlc_handler` feature.
    ///
    /// This method reads incoming updates and **only** responds to those
    /// incoming updates. There is no method on this type for sending
    /// unsolicited messages; that deliberate omission is the runtime
    /// guarantee that satisfies the "never auto-send" rule.
    pub async fn run_forever(self) -> Result<()> {
        let store = self.store.clone();
        let router = self.router.clone();
        let llm = self.llm.clone();
        let allowlist = self.allowlist.clone();
        let model = self.model.clone();
        let bot = self.bot.clone();

        teloxide::repl(bot, move |bot: Bot, msg: Message| {
            let store = store.clone();
            let router = router.clone();
            let llm = llm.clone();
            let allowlist = allowlist.clone();
            let model = model.clone();
            async move {
                if !allowlist.contains(&msg.chat.id.0) {
                    return Ok::<(), teloxide::RequestError>(());
                }
                let Some(text) = msg.text() else {
                    return Ok(());
                };
                let display = msg
                    .from
                    .as_ref()
                    .and_then(|u| u.username.clone())
                    .unwrap_or_else(|| "unknown".to_string());

                // All core-store work is synchronous and small; running
                // it on the async executor thread is fine.
                let response = match handle_message(
                    &store, &router, &llm, &model, msg.chat.id.0, &display, text,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => format!("(chat error: {e})"),
                };
                let _ = bot.send_message(ChatId(msg.chat.id.0), response).await?;
                Ok(())
            }
        })
        .await;
        Ok(())
    }
}

/// Pure message-handling pipeline: store user message, route, call LLM,
/// store assistant reply, return it. Separated from `run_forever` so
/// tests can exercise the logic without standing up a Telegram bot.
pub(crate) async fn handle_message(
    store: &ChatStore,
    router: &IntelligentRouter,
    llm: &LlmClient,
    model: &str,
    chat_id: i64,
    user_display: &str,
    text: &str,
) -> Result<String> {
    let conv =
        store.get_or_create_conversation("telegram", &chat_id.to_string(), user_display)?;
    store.append_message(conv.id, "user", text, None)?;

    let decision = router.route(text);
    let response = match decision {
        RouteDecision::DirectAnswer => {
            let history = store.recent_messages(conv.id, HISTORY_WINDOW)?;
            let mut messages: Vec<LlmChatMessage> = history
                .into_iter()
                .map(|m| LlmChatMessage {
                    role: m.role,
                    content: m.content,
                })
                .collect();
            // `recent_messages` already contains the just-appended user
            // turn, so we don't re-push it.
            if messages.is_empty() {
                messages.push(LlmChatMessage::user(text));
            }
            match llm.chat(model, messages).await {
                Ok(content) => content,
                Err(MakakooError::Llm(e)) => format!("(llm error: {e})"),
                Err(e) => return Err(e),
            }
        }
        RouteDecision::ToolCall(tool) => format!("(would call tool: {tool})"),
        RouteDecision::Command(cmd) => format!("(command: {cmd:?})"),
    };

    store.append_message(conv.id, "assistant", &response, None)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fresh_store() -> (TempDir, Arc<ChatStore>) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("chat.db");
        (dir, Arc::new(ChatStore::open(&p).unwrap()))
    }

    #[test]
    fn new_stores_allowlist_and_model() {
        let (_dir, store) = fresh_store();
        let router = Arc::new(IntelligentRouter::new());
        let llm = Arc::new(LlmClient::with_base_url("http://127.0.0.1:1"));
        let bot = TelegramBot::new(
            "123:fake",
            store,
            router,
            llm,
            vec![-100_1234, 42],
        );
        assert_eq!(bot.allowlist(), &[-100_1234, 42]);
        assert_eq!(bot.model(), "ail-compound");
        assert!(bot.is_allowed(42));
        assert!(!bot.is_allowed(999));
    }

    #[test]
    fn with_model_overrides_default() {
        let (_dir, store) = fresh_store();
        let router = Arc::new(IntelligentRouter::new());
        let llm = Arc::new(LlmClient::with_base_url("http://127.0.0.1:1"));
        let bot = TelegramBot::new("t", store, router, llm, vec![1])
            .with_model("ail-mini");
        assert_eq!(bot.model(), "ail-mini");
    }

    #[tokio::test]
    async fn handle_message_direct_answer_stores_both_sides() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": "caveman hello" } }]
            })))
            .mount(&server)
            .await;

        let (_dir, store) = fresh_store();
        let router = IntelligentRouter::new();
        let llm = LlmClient::with_base_url(server.uri());

        let reply = handle_message(
            &store, &router, &llm, "ail-compound", 42, "alice", "hey harvey",
        )
        .await
        .unwrap();
        assert_eq!(reply, "caveman hello");

        let conv = store
            .get_or_create_conversation("telegram", "42", "alice")
            .unwrap();
        let recent = store.recent_messages(conv.id, 10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].role, "user");
        assert_eq!(recent[0].content, "hey harvey");
        assert_eq!(recent[1].role, "assistant");
        assert_eq!(recent[1].content, "caveman hello");
    }

    #[tokio::test]
    async fn handle_message_command_does_not_hit_llm() {
        // No mock server needed — we're going to assert the LLM isn't
        // called. Point the LLM at an unreachable port.
        let (_dir, store) = fresh_store();
        let router = IntelligentRouter::new();
        let llm = LlmClient::with_base_url("http://127.0.0.1:1");
        let reply = handle_message(
            &store, &router, &llm, "ail-compound", 7, "eve", "/status",
        )
        .await
        .unwrap();
        assert!(reply.contains("command"));
        assert!(reply.contains("status"));
    }

    #[tokio::test]
    async fn handle_message_tool_call_does_not_hit_llm() {
        let (_dir, store) = fresh_store();
        let router = IntelligentRouter::new();
        let llm = LlmClient::with_base_url("http://127.0.0.1:1");
        let reply = handle_message(
            &store,
            &router,
            &llm,
            "ail-compound",
            7,
            "eve",
            "please research quantum lattices",
        )
        .await
        .unwrap();
        assert!(reply.contains("tool"));
        assert!(reply.contains("brain_search"));
    }

    #[tokio::test]
    async fn handle_message_persists_across_turns() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{ "message": { "content": "turn-response" } }]
            })))
            .mount(&server)
            .await;

        let (_dir, store) = fresh_store();
        let router = IntelligentRouter::new();
        let llm = LlmClient::with_base_url(server.uri());

        for _ in 0..3 {
            handle_message(
                &store, &router, &llm, "ail-compound", 9, "sam", "ping",
            )
            .await
            .unwrap();
        }
        let conv = store
            .get_or_create_conversation("telegram", "9", "sam")
            .unwrap();
        let recent = store.recent_messages(conv.id, 20).unwrap();
        // 3 turns * 2 roles = 6 messages
        assert_eq!(recent.len(), 6);
    }
}
