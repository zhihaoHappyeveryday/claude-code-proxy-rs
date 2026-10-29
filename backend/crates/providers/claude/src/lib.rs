//! Claude Code 账号管理、授权凭据及原生 Messages 执行。

mod catalog;
mod client_profile;
mod credential;
mod execution;
mod identity;
mod network;
mod oauth;
mod refresh;
mod sse;
mod transport;
mod worker;
pub use network::NetworkConfig;
pub use worker::oauth_refresh_worker;

pub use execution::initialize_execution;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use gateway_admin::{
    model::{
        observability::{CalculatedBillingBreakdown, DashboardWireProfile, ProviderBillingInput},
        provider_credentials::*,
    },
    ports::provider::{ProviderAdmin, ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::{
    account::{
        CredentialState, LoadedCredential, OutboundProxy, ProviderAccountId, ProviderAccountStore,
    },
    operation::Operation,
    routing::{ProviderKind, UpstreamModelId},
};

/// 宿主为指定出口提供带有统一证书信任和网络限制的客户端。
pub type ClientFactory = fn(Option<&OutboundProxy>) -> Result<reqwest::Client, &'static str>;

/// 生成客户端必须显式传入代理；TLS 参数由 Claude 模板解析得到。
pub type GenerationClientFactory =
    fn(&OutboundProxy, wreq::tls::TlsOptions) -> Result<wreq::Client, &'static str>;

/// 仅获取账号存储及 HTTP 客户端构造能力，不依赖数据库、HTTP 路由或 Host。
pub fn initialize(
    ports: gateway_core::provider_ports::ProviderStorePorts,
    client_factory: ClientFactory,
    network: NetworkConfig,
) -> Result<Arc<dyn ProviderAdmin>, ProviderAdminError> {
    Ok(Arc::new(ClaudeAdmin {
        kind: ProviderKind::new("claude").map_err(|_| invalid())?,
        accounts: ports.accounts(),
        ports,
        client_factory,
        network,
    }))
}

struct ClaudeAdmin {
    kind: ProviderKind,
    accounts: Arc<dyn ProviderAccountStore>,
    ports: gateway_core::provider_ports::ProviderStorePorts,
    client_factory: ClientFactory,
    network: NetworkConfig,
}

impl ClaudeAdmin {
    async fn credential(
        &self,
        id: &ProviderAccountId,
    ) -> Result<LoadedCredential, ProviderAdminError> {
        let loaded = self
            .accounts
            .load_current_credential(id)
            .await
            .map_err(|_| ProviderAdminError::new(ProviderAdminErrorKind::Unavailable))?;
        if loaded.account.provider() != &self.kind {
            return Err(invalid());
        }
        Ok(loaded)
    }
}

fn invalid() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid)
        .with_public_message("Claude Code 账号配置无效")
}

fn unsupported() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unsupported)
        .with_public_message("此 Claude 能力尚未开放")
}

#[async_trait]
impl ProviderAdmin for ClaudeAdmin {
    fn provider_kind(&self) -> &ProviderKind {
        &self.kind
    }

    async fn account_unavailable(&self, _account_id: &ProviderAccountId) {}

    async fn connection_test_operation(
        &self,
        _model: &UpstreamModelId,
        _text: &str,
    ) -> Result<Operation, ProviderAdminError> {
        Err(unsupported())
    }

    fn dashboard_wire_profile(&self) -> Option<DashboardWireProfile> {
        Some(client_profile::ClientProfile::default().dashboard())
    }

    fn configured_wire_profile(
        &self,
        configuration: &gateway_core::account::OpaqueProviderData,
    ) -> Option<DashboardWireProfile> {
        client_profile::ClientProfile::parse(configuration)
            .ok()
            .map(|profile| profile.dashboard())
    }

    fn default_client_profile(&self) -> Option<gateway_core::account::OpaqueProviderData> {
        Some(client_profile::ClientProfile::default().configuration())
    }

    fn client_profile_options(
        &self,
    ) -> Result<gateway_core::account::OpaqueProviderData, ProviderAdminError> {
        Ok(client_profile::ClientProfile::options())
    }

    fn preview_client_profile(
        &self,
        configuration: &gateway_core::account::OpaqueProviderData,
    ) -> Result<gateway_core::account::OpaqueProviderData, ProviderAdminError> {
        Ok(client_profile::ClientProfile::parse(configuration)?.preview())
    }

    fn calculated_billing(
        &self,
        _input: &ProviderBillingInput,
    ) -> Result<Option<CalculatedBillingBreakdown>, ProviderAdminError> {
        Ok(None)
    }

    async fn prepare_import(
        &self,
        command: PrepareCredentialImport,
    ) -> Result<PreparedCredentialImport, ProviderAdminError> {
        let input: credential::Import = serde_json::from_value(serde_json::Value::Object(
            command.document.into_provider_data().into_inner(),
        ))
        .map_err(|_| invalid())?;
        input.validate()?;
        self.network
            .require_proxy(command.default_outbound_proxy.as_ref())?;
        if input.expires_at.is_some_and(|at| at <= Utc::now()) {
            return Err(invalid());
        }
        let name = input.name.trim().to_owned();
        let material = input.credential().document()?;
        Ok(PreparedCredentialImport {
            provider_kind: self.kind.clone(),
            credentials: vec![PreparedCredentialCreate {
                account_id: ProviderAccountId::new(format!(
                    "acct_{}",
                    uuid::Uuid::now_v7().simple()
                ))
                .map_err(|_| invalid())?,
                provider_kind: self.kind.clone(),
                name,
                email: None,
                upstream_user_id: None,
                upstream_account_id: None,
                plan_type: None,
                authentication_kind: input.authentication_kind.as_str().into(),
                provider_material: material,
                has_refresh_token: false,
                access_token_expires_at: input.expires_at,
                next_refresh_at: None,
                enabled: true,
                credential_state: CredentialState::Unknown,
                credential_observed_at: Utc::now(),
                model_access: None,
                outbound_proxy: command.default_outbound_proxy,
            }],
        })
    }

    async fn account_configuration(
        &self,
        id: &ProviderAccountId,
    ) -> Result<Option<ProviderDocument>, ProviderAdminError> {
        let loaded = self.credential(id).await?;
        if loaded.account.authentication_kind() == "api_key" {
            return Ok(Some(ProviderDocument::new(gateway_core::account::OpaqueProviderData::new(
                serde_json::json!({"authentication_kind":"legacy_api_key","migration_required":true}).as_object().cloned().ok_or_else(invalid)?))));
        }
        let material = credential::Credential::parse(loaded.credential.expose_to_provider())?;
        Ok(Some(material.configuration()))
    }

    async fn start_authorization(
        &self,
        pending: PendingAuthorizationMutation,
    ) -> Result<AuthorizationStarted, ProviderAdminError> {
        self.begin_authorization(pending, "oauth").await
    }
    async fn start_authorization_method(
        &self,
        pending: PendingAuthorizationMutation,
        method: &str,
    ) -> Result<AuthorizationStarted, ProviderAdminError> {
        self.begin_authorization(pending, method).await
    }
    async fn complete_authorization(
        &self,
        command: CompleteAuthorization,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError> {
        self.finish_authorization(command).await
    }
    async fn prepare_rotation(
        &self,
        command: PrepareCredentialRotation,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Replacement {
            access_token: String,
            expires_at: Option<chrono::DateTime<Utc>>,
        }
        let replacement: Replacement = serde_json::from_value(serde_json::Value::Object(
            command.provider_material.into_provider_data().into_inner(),
        ))
        .map_err(|_| invalid())?;
        let id = ProviderAccountId::new(command.account.id).map_err(|_| invalid())?;
        let (loaded, guard) = self.lock_credential(&id).await?;
        let mut material = credential::Credential::parse(loaded.credential.expose_to_provider())?;
        if material.authentication_kind != credential::Kind::SetupToken
            || replacement.expires_at.is_some_and(|at| at <= Utc::now())
        {
            return Err(invalid());
        }
        material.access_token = replacement.access_token;
        material.expires_at = replacement.expires_at;
        material.validate()?;
        Ok(refresh::rotation(&loaded.account, material, guard)?.requiring_verification())
    }
    async fn prepare_refresh(
        &self,
        command: PrepareCredentialRefresh,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        let id = ProviderAccountId::new(command.account.id).map_err(|_| invalid())?;
        self.refresh(&id).await
    }
    async fn quota(
        &self,
        _request: ProviderQuotaRequest,
    ) -> Result<ProviderQuota, ProviderAdminError> {
        Err(unsupported())
    }
    async fn models(
        &self,
        id: &ProviderAccountId,
        _refresh: bool,
    ) -> Result<ProviderModels, ProviderAdminError> {
        let loaded = self.credential(id).await?;
        let credential = credential::Credential::parse(loaded.credential.expose_to_provider())?;
        self.network
            .require_proxy(loaded.account.outbound_proxy())?;
        let client = (self.client_factory)(loaded.account.outbound_proxy()).map_err(|message| {
            ProviderAdminError::new(ProviderAdminErrorKind::Unavailable)
                .with_public_message(message)
        })?;
        catalog::read_models(client, credential, &self.network.api_base).await
    }
    async fn export_credentials(
        &self,
        _credentials: Vec<ProviderExportCredentialInput>,
    ) -> Result<ProviderExport, ProviderAdminError> {
        Err(unsupported())
    }
}
