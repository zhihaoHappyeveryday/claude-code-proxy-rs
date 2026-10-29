//! Claude Code PKCE 授权；临时材料由加密的 pending Store 持有。
use crate::{
    ClaudeAdmin,
    credential::{Credential, Kind},
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use gateway_admin::{
    model::{AdminError, provider_credentials::*},
    ports::provider::{ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::{
    account::{CredentialState, OpaqueProviderData, ProviderAccountId},
    provider_ports::*,
    routing::ProviderKind,
};
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

pub(super) const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const REDIRECT: &str = "https://platform.claude.com/oauth/code/callback";
const FULL_SCOPE: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

fn random() -> Result<String, ProviderAdminError> {
    let mut value = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut value)
        .map_err(|_| unavailable())?;
    Ok(URL_SAFE_NO_PAD.encode(value))
}
fn unavailable() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unavailable)
}
fn invalid(message: &'static str) -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid).with_public_message(message)
}
fn binding(value: String) -> Result<OAuthPendingBinding, ProviderAdminError> {
    OAuthPendingBinding::try_new(value).map_err(|_| unavailable())
}
fn owner(value: &AuthorizationOwner) -> Result<OAuthPendingBinding, ProviderAdminError> {
    binding(match value {
        AuthorizationOwner::AdminSession { admin_user_id } => format!("admin:{admin_user_id}"),
        AuthorizationOwner::AdminApiKey => "api_key".into(),
        AuthorizationOwner::System => "system".into(),
    })
}
fn method(value: &str) -> Result<Kind, ProviderAdminError> {
    match value {
        "oauth" => Ok(Kind::OAuth),
        "setup_token" => Ok(Kind::SetupToken),
        _ => Err(invalid("仅支持 OAuth 与 Setup Token")),
    }
}

impl ClaudeAdmin {
    pub(super) async fn begin_authorization(
        &self,
        pending: PendingAuthorizationMutation,
        selected: &str,
    ) -> Result<AuthorizationStarted, ProviderAdminError> {
        self.network.require_proxy(pending.outbound_proxy())?;
        let kind = method(selected)?;
        if let AuthorizationMutationTarget::Reauthorize { account_id } = pending.target() {
            let loaded = self.credential(account_id).await?;
            if loaded.account.authentication_kind() != kind.as_str() {
                return Err(invalid("重新授权不能改变账号类型"));
            }
        }
        let state = random()?;
        let verifier = random()?;
        let flow_id = random()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let ttl = self.network.authorization_ttl;
        let expires_at = Utc::now() + chrono::Duration::seconds(ttl.as_secs() as i64);
        let flow = NewOAuthPendingFlow::try_new(self.kind.clone(), binding(flow_id.clone())?, owner(pending.owner_binding().owner())?, ttl,
            OpaqueProviderData::new(json!({"version":1,"mutation":pending.to_storage_v1(),"state":state,"verifier":verifier,"kind":kind,"expires_at":expires_at}).as_object().cloned().ok_or_else(unavailable)?)).map_err(|_| unavailable())?;
        if self
            .ports
            .oauth_pending()
            .put_if_absent(flow)
            .await
            .map_err(|_| unavailable())?
            != OAuthPendingPutOutcome::Stored
        {
            return Err(unavailable());
        }
        let base = if self.network.api_base.starts_with("http://127.0.0.1:") {
            format!("{}/authorize", self.network.api_base)
        } else {
            "https://claude.com/cai/oauth/authorize".into()
        };
        let mut url = url::Url::parse(&base).map_err(|_| unavailable())?;
        url.query_pairs_mut().extend_pairs([
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT),
            (
                "scope",
                if kind == Kind::OAuth {
                    FULL_SCOPE
                } else {
                    "user:inference"
                },
            ),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
        ]);
        Ok(AuthorizationStarted {
            flow_id,
            authorization_url: url.into(),
            expires_at,
        })
    }

    pub(super) async fn finish_authorization(
        &self,
        command: CompleteAuthorization,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError> {
        let mut claim = Claim {
            port: self.ports.oauth_pending(),
            kind: self.kind.clone(),
            flow: binding(command.flow_id)?,
            owner: owner(AuthorizationOwnerBinding::from_context(&command.context).owner())?,
            claim: binding(random()?)?,
            settled: false,
        };
        let payload = match claim
            .port
            .claim_if_owner(
                &claim.kind,
                &claim.flow,
                &claim.owner,
                &claim.claim,
                Duration::from_secs(180),
            )
            .await
            .map_err(|_| unavailable())?
        {
            OAuthPendingClaimOutcome::Claimed(data) => data,
            OAuthPendingClaimOutcome::InProgress => {
                claim.settled = true;
                return Err(ProviderAdminError::new(ProviderAdminErrorKind::Conflict));
            }
            _ => {
                claim.settled = true;
                return Err(ProviderAdminError::new(ProviderAdminErrorKind::NotFound)
                    .with_public_message("授权已过期或不属于当前管理员，请重新生成链接"));
            }
        };
        let prepared = self.exchange_pending(payload, &command.callback_url).await;
        match prepared {
            Ok(prepared) => Ok(prepared.with_authorization_guard(Box::new(claim))),
            Err(error) => {
                let _ = claim.release().await;
                Err(error)
            }
        }
    }

    async fn exchange_pending(
        &self,
        payload: OpaqueProviderData,
        callback: &str,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError> {
        #[derive(Deserialize)]
        struct Pending {
            version: u8,
            mutation: Value,
            state: String,
            verifier: String,
            kind: Kind,
            expires_at: chrono::DateTime<Utc>,
        }
        let saved: Pending = serde_json::from_value(Value::Object(payload.into_inner()))
            .map_err(|_| unavailable())?;
        if saved.version != 1 || saved.expires_at <= Utc::now() {
            return Err(invalid("授权已过期，请重新生成链接"));
        }
        let pending = PendingAuthorizationMutation::from_storage_v1(saved.mutation)
            .map_err(|_| unavailable())?;
        self.network.require_proxy(pending.outbound_proxy())?;
        let (code, state) = parse_callback(callback)?;
        if state != saved.state {
            return Err(invalid("授权 state 不匹配，请使用本次授权的回填结果"));
        }
        let body = json!({"grant_type":"authorization_code","code":code,"state":state,"code_verifier":saved.verifier,"client_id":CLIENT_ID,"redirect_uri":REDIRECT});
        let locked = match pending.target() {
            AuthorizationMutationTarget::Reauthorize { account_id } => {
                Some(self.lock_credential(account_id).await?)
            }
            _ => None,
        };
        let material = self
            .exchange_token(body, pending.outbound_proxy(), saved.kind)
            .await?;
        let credential =
            match pending.target() {
                AuthorizationMutationTarget::Create { name } => {
                    PreparedAuthorizationCredential::Create(Box::new(PreparedCredentialCreate {
                        account_id: ProviderAccountId::new(format!(
                            "acct_{}",
                            uuid::Uuid::now_v7().simple()
                        ))
                        .map_err(|_| unavailable())?,
                        provider_kind: self.kind.clone(),
                        name: name.clone(),
                        email: material.email.clone(),
                        upstream_user_id: material.account_uuid.clone(),
                        upstream_account_id: material.org_uuid.clone(),
                        plan_type: None,
                        authentication_kind: material.authentication_kind.as_str().into(),
                        provider_material: material.document()?,
                        has_refresh_token: material.refresh_token.is_some(),
                        access_token_expires_at: material.expires_at,
                        next_refresh_at: None,
                        enabled: true,
                        credential_state: CredentialState::Ready,
                        credential_observed_at: Utc::now(),
                        model_access: None,
                        outbound_proxy: pending.outbound_proxy().cloned(),
                    }))
                }
                AuthorizationMutationTarget::Reauthorize { .. } => {
                    let (loaded, guard) = locked.ok_or_else(unavailable)?;
                    let old = Credential::parse(loaded.credential.expose_to_provider())?;
                    crate::refresh::verify_identity(&old, &material)?;
                    if old.account_uuid.is_some() && material.account_uuid.is_none() {
                        return Err(invalid("重新授权未返回账号身份，无法确认原账号"));
                    }
                    PreparedAuthorizationCredential::Reauthorize(Box::new(
                        crate::refresh::rotation(&loaded.account, material, guard)?,
                    ))
                }
            };
        Ok(PreparedAuthorizationCommit::new(pending, credential))
    }

    pub(super) async fn exchange_token(
        &self,
        body: Value,
        proxy: Option<&gateway_core::account::OutboundProxy>,
        kind: Kind,
    ) -> Result<Credential, ProviderAdminError> {
        self.network.require_proxy(proxy)?;
        let client = (self.client_factory)(proxy).map_err(|_| unavailable())?;
        let endpoint = if self.network.api_base.starts_with("http://127.0.0.1:") {
            format!("{}/v1/oauth/token", self.network.api_base)
        } else {
            "https://platform.claude.com/v1/oauth/token".into()
        };
        let mut response = client
            .post(endpoint)
            .header("accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|_| {
                ProviderAdminError::new(ProviderAdminErrorKind::BadGateway)
                    .with_public_message("Claude 授权服务连接失败，请检查代理后重试")
            })?;
        if !response.status().is_success() {
            return Err(ProviderAdminError::new(ProviderAdminErrorKind::BadGateway)
                .with_public_message("Claude 拒绝了授权凭据，请重新授权或稍后重试"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 256 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let token: TokenResponse = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if !token.token_type.eq_ignore_ascii_case("bearer")
            || !(1..=315_360_000).contains(&token.expires_in)
            || !token
                .scope
                .split_whitespace()
                .any(|v| v == "user:inference")
        {
            return Err(invalid("Claude 返回的授权范围或有效期无效"));
        }
        let material = Credential {
            authentication_kind: kind,
            access_token: token.access_token,
            refresh_token: if kind == Kind::OAuth {
                token.refresh_token.or_else(|| {
                    body.get("refresh_token")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
            } else {
                None
            },
            expires_at: Some(Utc::now() + chrono::Duration::seconds(token.expires_in)),
            scope: token.scope,
            account_uuid: token.account.as_ref().map(|v| v.uuid.clone()),
            org_uuid: token.organization.map(|v| v.uuid),
            email: token.account.and_then(|v| v.email_address),
        };
        if kind == Kind::OAuth && material.refresh_token.is_none() {
            return Err(invalid("OAuth 未返回可刷新凭据，请重新授权"));
        }
        material.validate()?;
        Ok(material)
    }
}

fn parse_callback(value: &str) -> Result<(String, String), ProviderAdminError> {
    let value = value.trim();
    let result = if value.starts_with("https://") {
        let url = url::Url::parse(value).map_err(|_| invalid("授权回填格式无效"))?;
        if !value.starts_with(&format!("{REDIRECT}?")) || url.fragment().is_some() {
            return Err(invalid("授权回调地址无效"));
        }
        let codes = url
            .query_pairs()
            .filter(|(k, _)| k == "code")
            .map(|(_, v)| v.into_owned())
            .collect::<Vec<_>>();
        let states = url
            .query_pairs()
            .filter(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned())
            .collect::<Vec<_>>();
        if codes.len() != 1 || states.len() != 1 {
            return Err(invalid("授权回填必须包含唯一的 code 和 state"));
        }
        (codes[0].clone(), states[0].clone())
    } else {
        let (code, state) = value
            .split_once('#')
            .ok_or_else(|| invalid("请粘贴完整的 code#state 或授权回调 URL"))?;
        (code.into(), state.into())
    };
    if result.0.is_empty()
        || result.1.is_empty()
        || result.0.len() > 8192
        || result.1.len() > 512
        || result.0.chars().any(char::is_whitespace)
        || result.1.chars().any(char::is_whitespace)
    {
        return Err(invalid("授权回填格式无效"));
    }
    Ok(result)
}
#[derive(Deserialize)]
struct TokenIdentity {
    uuid: String,
    email_address: Option<String>,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: String,
    expires_in: i64,
    scope: String,
    account: Option<TokenIdentity>,
    organization: Option<TokenIdentity>,
}

struct Claim {
    port: Arc<dyn OAuthPendingFlowPort>,
    kind: ProviderKind,
    flow: OAuthPendingBinding,
    owner: OAuthPendingBinding,
    claim: OAuthPendingBinding,
    settled: bool,
}
impl Claim {
    async fn release(&mut self) -> Result<(), AdminError> {
        let result = self
            .port
            .release_claim(&self.kind, &self.flow, &self.owner, &self.claim)
            .await;
        self.settled = true;
        result
            .map(|_| ())
            .map_err(|_| AdminError::internal("无法释放授权流程，请稍后重试"))
    }
}
#[async_trait]
impl AuthorizationCommitGuard for Claim {
    async fn commit(mut self: Box<Self>) -> Result<(), AdminError> {
        let result = self
            .port
            .consume_claim(&self.kind, &self.flow, &self.owner, &self.claim)
            .await;
        self.settled = true;
        match result {
            Ok(OAuthPendingConsumeOutcome::Consumed | OAuthPendingConsumeOutcome::NotFound) => {
                Ok(())
            }
            _ => Err(AdminError::internal(
                "授权已保存，流程结算失败；可安全重试完成操作",
            )),
        }
    }
    async fn abort(mut self: Box<Self>) -> Result<(), AdminError> {
        self.release().await
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let (port, kind, flow, owner, claim) = (
                self.port.clone(),
                self.kind.clone(),
                self.flow.clone(),
                self.owner.clone(),
                self.claim.clone(),
            );
            runtime.spawn(async move {
                let _ = port.release_claim(&kind, &flow, &owner, &claim).await;
            });
        }
    }
}
