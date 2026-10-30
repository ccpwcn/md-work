//! Reserved AI provider interface.
//!
//! v1 ships no provider and makes no network requests. The trait exists so a
//! later provider can be added without touching the editor or MCP boundaries.

use std::sync::Arc;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiMessage {
    pub role: AiRole,
    pub content: String,
}

impl AiMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: AiRole::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: AiRole::User,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<AiMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionResponse {
    pub text: String,
    pub usage: Option<AiUsage>,
}

/// Minimal completion/chat/summary surface. Providers are synchronous and
/// expected to be called from a background executor.
pub trait AiProvider: Send + Sync {
    fn id(&self) -> &str;

    fn display_name(&self) -> &str;

    fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse>;

    /// Defaults to a completion request; providers may override for a cheaper
    /// model or prompt.
    fn summarize(&self, request: &CompletionRequest) -> Result<CompletionResponse> {
        self.complete(request)
    }

    fn chat(&self, request: &CompletionRequest) -> Result<CompletionResponse> {
        self.complete(request)
    }
}

/// Empty by default; v1 has no configured provider.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn AiProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn AiProvider>) {
        self.providers
            .retain(|existing| existing.id() != provider.id());
        self.providers.push(provider);
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn AiProvider>> {
        self.providers.iter().find(|provider| provider.id() == id)
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn complete(&self, id: &str, request: &CompletionRequest) -> Result<CompletionResponse> {
        self.get(id)
            .ok_or_else(|| Error::Ai(format!("AI provider not configured: {id}")))?
            .complete(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl AiProvider for Echo {
        fn id(&self) -> &str {
            "echo"
        }

        fn display_name(&self) -> &str {
            "Echo"
        }

        fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse> {
            Ok(CompletionResponse {
                text: request
                    .messages
                    .last()
                    .map(|message| message.content.clone())
                    .unwrap_or_default(),
                usage: None,
            })
        }
    }

    #[test]
    fn empty_registry_rejects_requests() {
        let registry = ProviderRegistry::new();
        assert!(registry.is_empty());
        let request = CompletionRequest::default();
        assert!(registry.complete("echo", &request).is_err());
    }

    #[test]
    fn registered_provider_answers() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(Echo));
        let mut request = CompletionRequest::default();
        request.messages.push(AiMessage::user("你好"));
        let response = registry.complete("echo", &request).unwrap();
        assert_eq!(response.text, "你好");
    }
}
