//! Claude HTTP 身份与 TLS 的统一配置；模板及适用性由 Provider 解释。

use gateway_admin::{
    model::observability::{DashboardWireAttribute, DashboardWireProfile},
    ports::provider::{ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::account::OpaqueProviderData;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::identity::ClientIdentity;

pub(super) const TEMPLATE_ID: &str = "claude-code-macos-arm64-2.1.312";

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ClientProfile {
    schema_version: u8,
    pub(super) http_identity: ClientIdentity,
    pub(super) tls: TlsPolicy,
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TlsPolicy {
    Standard,
    Emulate { template: TemplateRef },
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TemplateRef {
    id: String,
    revision: u32,
}

impl Default for ClientProfile {
    fn default() -> Self {
        Self {
            schema_version: 1,
            http_identity: ClientIdentity::default(),
            tls: TlsPolicy::Standard,
        }
    }
}

fn invalid(message: &'static str) -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid).with_public_message(message)
}

impl ClientProfile {
    /// Header 是声明而不是设备实测；仅在已验证的声明范围内应用固定模板。
    pub(super) fn validate_wire(
        &self,
        context: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), ()> {
        if matches!(self.tls, TlsPolicy::Standard) || self.http_identity.matches_wire(context) {
            Ok(())
        } else {
            Err(())
        }
    }

    pub(super) fn parse(data: &OpaqueProviderData) -> Result<Self, ProviderAdminError> {
        let profile: Self = serde_json::from_value(json!(data.clone().into_inner()))
            .map_err(|_| invalid("Claude 客户端配置格式无效"))?;
        if profile.schema_version != 1 {
            return Err(invalid("Claude 客户端配置版本无效"));
        }
        ClientIdentity::parse(&profile.http_identity.configuration())?;
        if let TlsPolicy::Emulate { template } = &profile.tls {
            if template.id != TEMPLATE_ID || template.revision != 1 {
                return Err(invalid("所选 Claude TLS 模板不存在或版本不可用"));
            }
            if profile.http_identity != ClientIdentity::captured_2_1_312() {
                return Err(invalid("默认客户端身份不在所选 TLS 模板的已验证范围内"));
            }
        }
        Ok(profile)
    }

    pub(super) fn configuration(&self) -> OpaqueProviderData {
        OpaqueProviderData::new(json!(self).as_object().cloned().unwrap_or_default())
    }

    pub(super) fn options() -> OpaqueProviderData {
        OpaqueProviderData::new(json!({
            "defaults": Self::default(),
            "tlsTemplates": [{
                "id": TEMPLATE_ID, "revision": 1,
                "label": "Claude Code 2.1.312 · macOS arm64",
                "evidence": "local-client-capture",
                "evidenceScope": "bare 模式、假 API Key、本地 HTTPS 代理；未验证真实 OAuth/Setup Token 调用",
                "runtimeDeclaration": "node v26.12.1",
                "protocol": "http/1.1",
                "identity": ClientIdentity::captured_2_1_312(),
            }]
        }).as_object().cloned().unwrap_or_default())
    }

    pub(super) fn preview(&self) -> OpaqueProviderData {
        let mut preview = self.http_identity.preview().into_inner();
        preview.insert("configuration".into(), json!(self));
        preview.insert("tls".into(), json!(self.tls));
        preview.insert(
            "tlsEvidence".into(),
            match self.tls {
                TlsPolicy::Standard => json!(null),
                TlsPolicy::Emulate { .. } => json!("local-client-capture"),
            },
        );
        OpaqueProviderData::new(preview)
    }

    pub(super) fn metadata(
        &self,
        source: &str,
        revision: u64,
        phase: &str,
        applied: bool,
    ) -> Result<gateway_core::event::ProviderResponseMetadata, gateway_core::error::ProviderError>
    {
        gateway_core::event::ProviderResponseMetadata::new(json!({"claudeClient": {
            "source": source, "accountConfigurationRevision": revision,
            "configuration": self, "phase": phase, "applied": applied,
            "handshakeEvidence": if matches!(self.tls, TlsPolicy::Emulate { .. }) { Some("local-client-capture") } else { None },
            "realUpstreamVerifiedAt": null
        }}).to_string()).ok_or_else(|| crate::execution::failed(
            gateway_core::error::ProviderErrorKind::ProviderInfrastructureUnavailable,
            gateway_core::upstream::UpstreamSendState::NotSent))
    }

    pub(super) fn observation(
        &self,
        source: &str,
        revision: u64,
        phase: &str,
        applied: bool,
    ) -> Result<gateway_core::event::ProviderResponseObservation, gateway_core::error::ProviderError>
    {
        let transport = gateway_core::upstream::UpstreamTransport::new("http").map_err(|_| {
            crate::execution::failed(
                gateway_core::error::ProviderErrorKind::ProviderInfrastructureUnavailable,
                gateway_core::upstream::UpstreamSendState::NotSent,
            )
        })?;
        Ok(
            gateway_core::event::ProviderResponseObservation::new(transport)
                .with_provider_metadata(self.metadata(source, revision, phase, applied)?),
        )
    }

    pub(super) fn dashboard(&self) -> DashboardWireProfile {
        let mut profile = self.http_identity.dashboard();
        let (mode, template) = match self.tls {
            TlsPolicy::Standard => ("standard", "—"),
            TlsPolicy::Emulate { .. } => ("emulate", TEMPLATE_ID),
        };
        profile.attributes.extend([
            DashboardWireAttribute {
                key: "tls_mode".into(),
                value: mode.into(),
            },
            DashboardWireAttribute {
                key: "tls_template".into(),
                value: template.into(),
            },
        ]);
        profile
    }
}
