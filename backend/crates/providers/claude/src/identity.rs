//! Claude Code 客户端身份。只解析本地配置；不读凭据，不查询发布站点。
//!
//! 默认环境来自本机 Claude Code 2.1.312 的隔离捕获，不声称官方最新版本。
//! 原生客户端优先，避免改 UA 后与正文中的 billing 版本、签名不一致。

use gateway_admin::{
    model::observability::{DashboardWireAttribute, DashboardWireProfile, DashboardWireTarget},
    ports::provider::{ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::account::OpaqueProviderData;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ClientIdentity {
    cli_version: String,
    sdk_version: String,
    os: OperatingSystem,
    arch: Architecture,
    node_version: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
enum OperatingSystem {
    Linux,
    MacOS,
    Windows,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Architecture {
    Arm64,
    X64,
}

impl Default for ClientIdentity {
    fn default() -> Self {
        Self::captured_2_1_312()
    }
}

impl ClientIdentity {
    /// Immutable identity scope of template revision 1. Do not widen when defaults change.
    pub(super) fn captured_2_1_312() -> Self {
        Self {
            cli_version: "2.1.312".into(),
            sdk_version: "0.128.0".into(),
            os: OperatingSystem::MacOS,
            arch: Architecture::Arm64,
            node_version: "26.12.1".into(),
        }
    }
}

fn invalid() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid).with_public_message(
        "Claude Code 身份无效：版本须为 X.Y.Z，系统为 Linux/MacOS/Windows，架构为 arm64/x64",
    )
}

impl ClientIdentity {
    pub(super) fn parse(data: &OpaqueProviderData) -> Result<Self, ProviderAdminError> {
        let identity: Self =
            serde_json::from_value(serde_json::Value::Object(data.clone().into_inner()))
                .map_err(|_| invalid())?;
        if [
            &identity.cli_version,
            &identity.sdk_version,
            &identity.node_version,
        ]
        .into_iter()
        .any(|version| !valid_version(version))
        {
            return Err(invalid());
        }
        Ok(identity)
    }

    pub(super) fn configuration(&self) -> OpaqueProviderData {
        // All fields serialize to a JSON object; keeping this conversion local avoids opaque
        // provider JSON leaking into request assembly or UI-specific code.
        OpaqueProviderData::new(json!(self).as_object().cloned().unwrap_or_default())
    }

    fn user_agent(&self) -> String {
        format!("claude-cli/{} (external, sdk-cli)", self.cli_version)
    }

    fn os(&self) -> &'static str {
        match self.os {
            OperatingSystem::Linux => "Linux",
            OperatingSystem::MacOS => "MacOS",
            OperatingSystem::Windows => "Windows",
        }
    }

    fn arch(&self) -> &'static str {
        match self.arch {
            Architecture::Arm64 => "arm64",
            Architecture::X64 => "x64",
        }
    }

    fn version_source(&self) -> &'static str {
        if self == &Self::captured_2_1_312() {
            "local-capture"
        } else {
            "custom"
        }
    }

    fn headers(&self) -> [(&'static str, String); 8] {
        [
            ("user-agent", self.user_agent()),
            ("x-app", "cli".into()),
            ("x-stainless-lang", "js".into()),
            ("x-stainless-package-version", self.sdk_version.clone()),
            ("x-stainless-os", self.os().into()),
            ("x-stainless-arch", self.arch().into()),
            ("x-stainless-runtime", "node".into()),
            (
                "x-stainless-runtime-version",
                format!("v{}", self.node_version),
            ),
        ]
    }

    pub(super) fn matches_wire(
        &self,
        context: &serde_json::Map<String, serde_json::Value>,
    ) -> bool {
        let Some(headers) = context.get("headers").and_then(serde_json::Value::as_array) else {
            return false;
        };
        self.headers()
            .iter()
            .filter(|(name, _)| !matches!(*name, "x-app" | "x-stainless-lang"))
            .all(|(name, expected)| {
                let values = headers
                    .iter()
                    .filter(|h| h[0] == *name)
                    .filter_map(|h| h[1].as_str())
                    .collect::<Vec<_>>();
                values.len() == 1
                    && if *name == "user-agent" {
                        native_user_agent(values[0])
                            && values[0].split_once(' ').map(|(version, _)| version)
                                == expected.split_once(' ').map(|(version, _)| version)
                    } else {
                        values[0] == expected
                    }
            })
    }

    pub(super) fn preview(&self) -> OpaqueProviderData {
        OpaqueProviderData::new(json!({
            "configuration": self, "userAgent": self.user_agent(),
            "policy": "native-first", "versionSource": self.version_source(),
            "verifiedAt": null,
            "headers": self.headers().into_iter().map(|(name, value)| (name.to_owned(), value.into())).collect::<serde_json::Map<String, serde_json::Value>>()
        }).as_object().cloned().unwrap_or_default())
    }

    /// 补全非原生调用者时，整体替换身份字段，避免混入旧 SDK/平台参数。
    /// 原生请求保留已有字段，不将另一套默认环境拼入客户端身份。
    pub(super) fn apply(
        &self,
        context: &mut serde_json::Map<String, serde_json::Value>,
        streaming: bool,
    ) {
        let mut headers = context
            .remove("headers")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let agents = headers
            .iter()
            .filter(|h| h[0] == "user-agent")
            .collect::<Vec<_>>();
        let native = agents.len() == 1 && agents[0][1].as_str().is_some_and(native_user_agent);
        if !native {
            let defaults = self.headers();
            headers.retain(|header| !defaults.iter().any(|(name, _)| header[0] == *name));
            headers.extend(
                defaults
                    .into_iter()
                    .map(|(name, value)| json!([name, value])),
            );
        }
        for (name, value) in [
            ("x-app", "cli".to_owned()),
            ("accept", "application/json".to_owned()),
            ("x-client-request-id", uuid::Uuid::now_v7().to_string()),
        ] {
            if !headers.iter().any(|header| header[0] == name) {
                headers.push(json!([name, value]));
            }
        }
        headers.retain(|header| header[0] != "x-stainless-helper-method");
        if streaming {
            headers.push(json!(["x-stainless-helper-method", "stream"]));
        }
        context.insert("headers".into(), headers.into());
    }

    pub(super) fn dashboard(&self) -> DashboardWireProfile {
        DashboardWireProfile {
            provider: "claude".into(),
            product: "Claude Code".into(),
            version: self.cli_version.clone(),
            build: None,
            target: DashboardWireTarget {
                os_type: self.os().into(),
                os_version: "—".into(),
                arch: self.arch().into(),
                terminal: format!("Node v{}", self.node_version),
            },
            user_agent: self.user_agent(),
            attributes: [
                ("client_identifier", "cli"),
                ("token_auth", "Bearer"),
                ("sdk_version", self.sdk_version.as_str()),
                ("identity_policy", "native-first"),
                ("version_source", self.version_source()),
            ]
            .into_iter()
            .map(|(key, value)| DashboardWireAttribute {
                key: key.into(),
                value: value.into(),
            })
            .collect(),
            verified_at: None,
            release: None,
        }
    }
}

fn valid_version(version: &str) -> bool {
    let parts = version.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 6
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}

fn native_user_agent(agent: &str) -> bool {
    agent.len() <= 512
        && agent.strip_prefix("claude-cli/").is_some_and(|tail| {
            tail.split_once(' ').is_some_and(|(version, entry)| {
                valid_version(version) && entry.starts_with('(') && entry.ends_with(')')
            })
        })
}
