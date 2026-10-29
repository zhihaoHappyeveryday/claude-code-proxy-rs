//! 原生 Claude Messages 执行；账号租约与请求事实仍由既有 Core/Store 持有。

use crate::{
    GenerationClientFactory,
    credential::Credential,
    transport::{ClientLease, Clients, Response, network_error},
};
use async_trait::async_trait;
use bytes::Bytes;
use gateway_core::{
    account::{
        AccountCandidate, AccountEligibilityPolicy, AccountHistorySnapshot,
        AccountSelectionContext, AccountStateChange, CredentialState, ProviderAccount,
    },
    engine::{
        AttemptContext,
        provider::{Provider, ProviderCallMetadata, ProviderRequest, ProviderStream},
    },
    error::{ClientVisibleUpstreamResponse, ProviderError, ProviderErrorKind},
    event::{
        GatewayEvent, ProtocolWireEvent, ProviderEvent, ProviderResponseHeader,
        ProviderResponseObservation, ResponseMeta,
    },
    metering::Usage,
    operation::Operation,
    provider_ports::{
        ProviderConcurrencyPool, ProviderLeaseAcquisition, ProviderLeaseRequest,
        ProviderSchedulingLeaseRequest, ProviderStorePorts,
    },
    routing::{ProviderCatalogGeneration, ProviderModelCapabilities},
    upstream::UpstreamSendState,
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
    time::SystemTime,
};

const PROTOCOL: &str = "anthropic";
const MAX_BODY: usize = 16 * 1024 * 1024;

pub fn initialize_execution(
    ports: ProviderStorePorts,
    client_factory: GenerationClientFactory,
    network: crate::NetworkConfig,
) -> Arc<dyn Provider> {
    Arc::new(ClaudeProvider {
        ports,
        network,
        clients: Clients::new(client_factory),
    })
}

struct ClaudeProvider {
    ports: ProviderStorePorts,
    network: crate::NetworkConfig,
    clients: Clients,
}

pub(super) fn failed(kind: ProviderErrorKind, sent: UpstreamSendState) -> ProviderError {
    ProviderError::new(kind, sent).with_retry_prohibited()
}
fn infrastructure() -> ProviderError {
    failed(
        ProviderErrorKind::ProviderInfrastructureUnavailable,
        UpstreamSendState::NotSent,
    )
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn resolve_request_profile(
        &self,
        configuration: &gateway_core::account::OpaqueProviderData,
    ) -> Result<gateway_core::account::OpaqueProviderData, ProviderError> {
        Ok(crate::client_profile::ClientProfile::parse(configuration)
            .map_err(|_| infrastructure())?
            .configuration())
    }

    fn default_request_profile(
        &self,
    ) -> Result<Option<gateway_core::account::OpaqueProviderData>, ProviderError> {
        Ok(Some(
            crate::client_profile::ClientProfile::default().configuration(),
        ))
    }

    fn name(&self) -> &str {
        "claude"
    }
    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(1)
    }
    fn model_catalog_is_exhaustive(&self) -> bool {
        false
    }
    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(Vec::new())
    }

    async fn execute(
        self: Arc<Self>,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        let Operation::Generate(generate) = request.operation() else {
            return Err(failed(
                ProviderErrorKind::Unsupported,
                UpstreamSendState::NotSent,
            ));
        };
        if generate.protocol_payload().protocol() != PROTOCOL {
            return Err(failed(
                ProviderErrorKind::Unsupported,
                UpstreamSendState::NotSent,
            ));
        }
        let provider = request.candidate().provider().clone();
        let model = request
            .candidate()
            .upstream_model()
            .ok_or_else(infrastructure)?
            .clone();
        let accounts = self
            .ports
            .accounts()
            .list_for_provider(&provider)
            .await
            .map_err(|_| infrastructure())?;
        let ids = accounts.iter().map(|a| a.id().clone()).collect::<Vec<_>>();
        let state = self
            .ports
            .leases()
            .load_state(
                context.client_api_key_ref(),
                &provider,
                &ids,
                ProviderConcurrencyPool::Shared,
            )
            .await
            .map_err(|_| infrastructure())?;
        let candidates = accounts
            .into_iter()
            .filter(|account| {
                matches!(account.authentication_kind(), "oauth" | "setup_token")
                    && context
                        .required_account()
                        .is_none_or(|id| id == account.id())
                    && context
                        .account_scope()
                        .is_none_or(|scope| scope.allows_model(account.id(), model.as_str()))
            })
            .map(|account| {
                // Unknown 仅表示还没有上游事实；允许首个真实请求尝试，不提前持久化 Ready。
                let account = if account.credential_state() == CredentialState::Unknown {
                    let enabled = account.enabled();
                    let quota = account.quota();
                    account.with_account_facts(enabled, CredentialState::Ready, quota, None, None)
                } else {
                    account
                };
                let signals = state
                    .signals()
                    .get(account.id())
                    .cloned()
                    .ok_or_else(infrastructure)?;
                Ok(AccountCandidate { account, signals })
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;
        let mut selection = AccountSelectionContext {
            policy: context.account_selection_policy(),
            now: SystemTime::now(),
            excluded_accounts: context.excluded_accounts().clone(),
            busy_accounts: BTreeSet::new(),
            preferred_account: None,
            preferred_account_overrides_weight: false,
            round_robin_cursor: state.round_robin_cursor(),
            eligibility: AccountEligibilityPolicy::Enforce,
            account_scope: context.account_scope().cloned(),
            reserved_concurrency: 0,
        };
        let selected = context
            .select_account(&provider, Some(model.as_str()), &candidates, &mut selection)
            .await
            .map_err(|_| infrastructure())?
            .ok_or_else(|| {
                failed(
                    ProviderErrorKind::NoEligibleAccount,
                    UpstreamSendState::NotSent,
                )
            })?;
        let account = &selected.candidate().account;
        let lease = self
            .ports
            .leases()
            .try_acquire(ProviderLeaseRequest::Scheduling(
                ProviderSchedulingLeaseRequest::new(
                    provider.clone(),
                    account.id().clone(),
                    account.revision(),
                    selection.concurrency_limit(account),
                    context.deadline(),
                )
                .with_metadata(context.lease_metadata())
                .with_cancellation(context.cancellation().clone()),
            ))
            .await
            .map_err(|_| infrastructure())?;
        let ProviderLeaseAcquisition::Acquired(lease) = lease else {
            return Err(failed(
                ProviderErrorKind::AccountCapacityUnavailable,
                UpstreamSendState::NotSent,
            ));
        };
        let loaded = self
            .ports
            .accounts()
            .load_credential(account.id(), account.revision())
            .await
            .map_err(|_| infrastructure())?;
        let credential = Credential::parse(loaded.credential.expose_to_provider())
            .map_err(|_| infrastructure())?;
        self.network
            .require_proxy(loaded.account.outbound_proxy())
            .map_err(|_| infrastructure())?;
        if credential
            .expires_at
            .is_some_and(|at| at <= chrono::Utc::now())
        {
            return Err(failed(
                ProviderErrorKind::InvalidRequest,
                UpstreamSendState::NotSent,
            ));
        }
        let mut body = generate.protocol_payload().body().clone();
        body.insert("model".into(), model.as_str().into());
        let mut wire_context = generate.protocol_payload().context().clone();
        let identity = loaded
            .account
            .client_configuration()
            .profile()
            .or_else(|| context.request_profile())
            .map(crate::client_profile::ClientProfile::parse)
            .transpose()
            .map_err(|_| infrastructure())?
            .unwrap_or_default();
        identity.http_identity.apply(
            &mut wire_context,
            body.get("stream")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        );
        let proxy = loaded.account.outbound_proxy().ok_or_else(infrastructure)?;
        let metadata = ProviderCallMetadata::new(
            provider,
            model.clone(),
            account.id().clone(),
            gateway_core::upstream::UpstreamTransport::new("http").map_err(|_| infrastructure())?,
        )
        .with_account_snapshot(Some(AccountHistorySnapshot::from_account(&loaded.account)));
        let streaming = body
            .get("stream")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let proxy = proxy.clone();
        let configuration_revision = loaded.account.client_configuration().revision();
        let source = if loaded.account.client_configuration().profile().is_some() {
            "account"
        } else {
            "request-default"
        };
        let owner = if loaded.account.client_configuration().profile().is_some() {
            crate::transport::ProfileOwner::Account
        } else {
            crate::transport::ProfileOwner::ClientKey(context.client_api_key_ref().clone())
        };
        let events = async_stream::try_stream! {
            yield ProviderEvent::observation(identity.observation(source, configuration_revision, "configuration", false)?);
            identity.validate_wire(&wire_context).map_err(|_| failed(ProviderErrorKind::InvalidRequest, UpstreamSendState::NotSent))?;
            yield ProviderEvent::observation(identity.observation(source, configuration_revision, "client-construction", false)?);
            let client = self.clients.acquire(loaded.account.id(), owner, configuration_revision, &proxy, &identity).await.map_err(|_| infrastructure())?;
            // This records applied client configuration, not a fabricated packet capture.
            yield ProviderEvent::observation(identity.observation(source, configuration_revision, "connect-or-send", true)?);
            let (mut response, headers) = self.send_request(client, &credential, &body, wire_context).await?;
            let observation = observation(headers.clone())?.with_status_code(response.status().as_u16())
                .with_provider_metadata(identity.metadata(source, configuration_revision, "response", true)?);
            yield ProviderEvent::observation(observation.clone());
            self.check_status(&mut response, headers, &credential).await?;
            if streaming {
                if !response.headers().get("content-type").and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.split(';').next().is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"))) {
                    Err(failed(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
                }
                let mut observer = super::sse::Observer::new(model.as_str());
                let mut started = ProviderEvent::canonical(GatewayEvent::Started(ResponseMeta::new("", model.as_str())));
                started.attach_observation(observation);
                yield started;
                while let Some(chunk) = response.chunk().await.map_err(|error| failed(if error.is_timeout() { ProviderErrorKind::Timeout } else { ProviderErrorKind::Transport }, UpstreamSendState::Sent))? {
                    for usage in observer.observe(&chunk)? {
                        yield ProviderEvent::canonical(GatewayEvent::Usage(usage));
                    }
                    if let Some(error) = observer.error() {
                        let wire = ProviderEvent::wire(ProtocolWireEvent::json_with_raw_sse_metadata(
                            PROTOCOL, Some("error".into()), error.clone(), chunk, None, None,
                        ).map_err(|_| infrastructure())?);
                        Err(failed(ProviderErrorKind::Unavailable, UpstreamSendState::Sent).with_atomic_client_events(vec![wire]))?;
                    } else {
                        yield ProviderEvent::wire(ProtocolWireEvent::raw_sse(PROTOCOL, chunk).map_err(|_| infrastructure())?);
                    }
                    if observer.complete() {
                        self.mark_ready(&loaded.account).await;
                        yield ProviderEvent::canonical(GatewayEvent::Completed(observer.meta().clone()));
                        return;
                    }
                }
                Err(failed(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
            } else {
                for event in self.collect_json(response, observation, &loaded.account, model.as_str()).await? {
                    yield event;
                }
            }
        };
        Ok(ProviderStream::new(metadata, events, lease))
    }
}

impl ClaudeProvider {
    async fn send_request(
        &self,
        client: ClientLease,
        credential: &Credential,
        body: &serde_json::Map<String, serde_json::Value>,
        wire_context: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(Response, Vec<ProviderResponseHeader>), ProviderError> {
        let base = self.network.api_base.trim_end_matches('/');
        let endpoint = if base.ends_with("/v1") {
            format!("{base}/messages")
        } else {
            format!("{base}/v1/messages")
        };
        let mut url = url::Url::parse(&endpoint).map_err(|_| infrastructure())?;
        url.set_query(
            wire_context
                .get("query")
                .and_then(serde_json::Value::as_str),
        );
        let mut key =
            wreq::header::HeaderValue::from_str(&format!("Bearer {}", credential.access_token))
                .map_err(|_| infrastructure())?;
        key.set_sensitive(true);
        let mut request = client
            .post(url.as_str())
            .header("authorization", key)
            .json(body);
        if !wire_context
            .get("headers")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|h| {
                h.iter().any(|h| {
                    h.get(0).and_then(serde_json::Value::as_str) == Some("anthropic-version")
                })
            })
        {
            request = request.header("anthropic-version", "2023-06-01");
        }
        let mut betas = Vec::new();
        if let Some(headers) = wire_context
            .get("headers")
            .and_then(serde_json::Value::as_array)
        {
            for header in headers {
                if let (Some(name), Some(value)) = (
                    header.get(0).and_then(serde_json::Value::as_str),
                    header.get(1).and_then(serde_json::Value::as_str),
                ) {
                    if name == "anthropic-beta" {
                        betas.push(value);
                    } else {
                        request = request.header(name, value);
                    }
                }
            }
        }
        for required in ["oauth-2025-04-20", "claude-code-20250219"] {
            if !betas
                .iter()
                .flat_map(|v| v.split(','))
                .any(|v| v.trim() == required)
            {
                betas.push(required);
            }
        }
        request = request.header("anthropic-beta", betas.join(","));
        let response = client.send(request).await.map_err(network_error)?;
        let hop_headers = response
            .headers()
            .get_all("connection")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(|v| v.trim().to_ascii_lowercase())
            .collect::<Vec<_>>();
        let headers = response
            .headers()
            .iter()
            .filter(|(name, value)| {
                safe_response_header(name.as_str())
                    && !hop_headers.iter().any(|hop| hop == name.as_str())
                    && !value
                        .as_bytes()
                        .windows(credential.access_token.len())
                        .any(|part| part == credential.access_token.as_bytes())
            })
            .map(|(name, value)| {
                ProviderResponseHeader::new(name.as_str(), Bytes::copy_from_slice(value.as_bytes()))
            })
            .collect::<Vec<_>>();
        Ok((response, headers))
    }

    async fn check_status(
        &self,
        response: &mut Response,
        headers: Vec<ProviderResponseHeader>,
        credential: &Credential,
    ) -> Result<(), ProviderError> {
        let status = response.status().as_u16();
        if status != 200 {
            let bytes = read_body(response).await?;
            let kind = match status {
                401 => ProviderErrorKind::Unauthorized,
                403 => ProviderErrorKind::PermissionDenied,
                429 => ProviderErrorKind::RateLimited,
                400..=499 => ProviderErrorKind::InvalidRequest,
                _ => ProviderErrorKind::Unavailable,
            };
            let content_type = headers
                .iter()
                .find(|h| h.name() == "content-type")
                .map(|h| h.value().to_vec());
            let body =
                String::from_utf8_lossy(&bytes).replace(&credential.access_token, "[redacted]");
            return Err(failed(kind, UpstreamSendState::Sent)
                .with_status(status)
                .with_client_visible_upstream_response(
                    ClientVisibleUpstreamResponse::new(status, content_type, Bytes::from(body))
                        .with_headers(headers),
                ));
        }
        Ok(())
    }

    async fn collect_json(
        &self,
        mut response: Response,
        observation: ProviderResponseObservation,
        account: &ProviderAccount,
        model: &str,
    ) -> Result<VecDeque<ProviderEvent>, ProviderError> {
        let bytes = read_body(&mut response).await?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| failed(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
        if value.get("type").and_then(serde_json::Value::as_str) != Some("message") {
            return Err(failed(ProviderErrorKind::Protocol, UpstreamSendState::Sent));
        }
        let meta = ResponseMeta::new(
            value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
            model,
        );
        let mut started = ProviderEvent::canonical(GatewayEvent::Started(meta.clone()));
        started.attach_observation(observation);
        let wire = ProtocolWireEvent::raw_json(PROTOCOL, Bytes::from(bytes))
            .map_err(|_| infrastructure())?;
        let mut events = VecDeque::from([started, ProviderEvent::wire(wire)]);
        if let Some(usage) = value.get("usage") {
            events.push_back(ProviderEvent::canonical(GatewayEvent::Usage(read_usage(
                usage,
            ))));
        }
        self.mark_ready(account).await;
        events.push_back(ProviderEvent::canonical(GatewayEvent::Completed(meta)));
        Ok(events)
    }
    async fn mark_ready(&self, account: &ProviderAccount) {
        // 账号健康是独立事实；写入失败不能丢弃真实响应或已有用量。
        if self
            .ports
            .accounts()
            .apply_state_change(AccountStateChange {
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                credential_state: CredentialState::Ready,
                observed_at: SystemTime::now(),
                error_reason: None,
                message: None,
            })
            .await
            .is_err()
        {
            tracing::warn!(account_id = %account.id(), "Claude account health update failed");
        }
    }
}

pub(super) fn read_usage(value: &serde_json::Value) -> Usage {
    Usage {
        input_tokens: value
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64),
        output_tokens: value
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64),
        cached_input_tokens: value
            .get("cache_read_input_tokens")
            .and_then(serde_json::Value::as_u64),
        cache_write_input_tokens: value
            .get("cache_creation_input_tokens")
            .and_then(serde_json::Value::as_u64),
        ..Usage::new()
    }
}

fn safe_response_header(name: &str) -> bool {
    name.starts_with("anthropic-")
        || matches!(
            name,
            "content-type" | "request-id" | "retry-after" | "x-should-retry"
        )
}

fn observation(
    headers: Vec<ProviderResponseHeader>,
) -> Result<ProviderResponseObservation, ProviderError> {
    let request_id = headers
        .iter()
        .find(|h| h.name() == "request-id")
        .and_then(|h| std::str::from_utf8(h.value()).ok())
        .map(str::to_owned);
    let mut observation = ProviderResponseObservation::new(
        gateway_core::upstream::UpstreamTransport::new("http").map_err(|_| infrastructure())?,
    )
    .with_status_code(200)
    .with_client_headers(headers);
    if let Some(id) = request_id {
        observation =
            observation.with_request_id(gateway_core::upstream::OpaqueUpstreamValue::new(id));
    }
    Ok(observation)
}
async fn read_body(response: &mut Response) -> Result<Vec<u8>, ProviderError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        failed(
            if error.is_timeout() {
                ProviderErrorKind::Timeout
            } else {
                ProviderErrorKind::Transport
            },
            UpstreamSendState::Sent,
        )
    })? {
        if bytes.len() + chunk.len() > MAX_BODY {
            return Err(failed(ProviderErrorKind::Protocol, UpstreamSendState::Sent));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
