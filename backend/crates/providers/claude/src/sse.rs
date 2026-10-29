//! 仅旁路观测用量和终态；交付始终使用原始 chunk，不等待帧边界。
use super::execution::{failed, read_usage};
use gateway_core::{
    error::{ProviderError, ProviderErrorKind},
    event::ResponseMeta,
    metering::Usage,
    upstream::UpstreamSendState,
};

pub(super) struct Observer {
    line: Vec<u8>,
    data: Vec<u8>,
    previous_cr: bool,
    complete: bool,
    started: bool,
    error: Option<serde_json::Value>,
    meta: ResponseMeta,
}
impl Observer {
    pub fn new(model: &str) -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            previous_cr: false,
            complete: false,
            started: false,
            error: None,
            meta: ResponseMeta::new("", model),
        }
    }
    pub fn complete(&self) -> bool {
        self.complete
    }
    pub fn error(&self) -> Option<&serde_json::Value> {
        self.error.as_ref()
    }
    pub fn meta(&self) -> &ResponseMeta {
        &self.meta
    }
    pub fn observe(&mut self, chunk: &[u8]) -> Result<Vec<Usage>, ProviderError> {
        let mut usage = Vec::new();
        for &byte in chunk {
            if byte == b'\n' && self.previous_cr {
                self.previous_cr = false;
                continue;
            }
            self.previous_cr = byte == b'\r';
            if byte == b'\r' || byte == b'\n' {
                if self.line.is_empty() {
                    if !self.data.is_empty() {
                        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&self.data) {
                            match value.get("type").and_then(serde_json::Value::as_str) {
                                Some("message_start") => {
                                    if self.started {
                                        return Err(failed(
                                            ProviderErrorKind::Protocol,
                                            UpstreamSendState::Sent,
                                        ));
                                    }
                                    let message = value.get("message").ok_or_else(|| {
                                        failed(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
                                    })?;
                                    let id = message
                                        .get("id")
                                        .and_then(serde_json::Value::as_str)
                                        .filter(|id| !id.is_empty())
                                        .ok_or_else(|| {
                                            failed(
                                                ProviderErrorKind::Protocol,
                                                UpstreamSendState::Sent,
                                            )
                                        })?;
                                    self.started = true;
                                    self.meta = ResponseMeta::new(
                                        id,
                                        self.meta.model().unwrap_or_default(),
                                    );
                                    if let Some(value) = message.get("usage") {
                                        usage.push(read_usage(value));
                                    }
                                }
                                Some("message_delta") => {
                                    if let Some(value) = value.get("usage") {
                                        usage.push(read_usage(value));
                                    }
                                }
                                Some("message_stop") => {
                                    if !self.started || self.complete {
                                        return Err(failed(
                                            ProviderErrorKind::Protocol,
                                            UpstreamSendState::Sent,
                                        ));
                                    }
                                    self.complete = true;
                                }
                                Some("error") => self.error = Some(value),
                                _ => {}
                            }
                        }
                        self.data.clear();
                    }
                } else if let Some(data) = self.line.strip_prefix(b"data:") {
                    if !self.data.is_empty() {
                        self.data.push(b'\n');
                    }
                    self.data
                        .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
                }
                self.line.clear();
            } else {
                self.line.push(byte);
            }
            if self.line.len() + self.data.len() > 2 * 1024 * 1024 {
                return Err(failed(ProviderErrorKind::Protocol, UpstreamSendState::Sent));
            }
        }
        Ok(usage)
    }
}
