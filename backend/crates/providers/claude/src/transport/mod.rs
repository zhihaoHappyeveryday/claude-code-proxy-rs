//! 推理客户端及连接资源；OAuth 探测不使用这里的长流策略。

mod template;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use gateway_core::{
    account::{OutboundProxy, ProviderAccountId},
    error::ProviderErrorKind,
    policy::ClientApiKeyId,
    upstream::UpstreamSendState,
};
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    GenerationClientFactory,
    client_profile::{ClientProfile, TlsPolicy},
};

type Body = Pin<Box<dyn Stream<Item = Result<Bytes, wreq::Error>> + Send>>;

#[derive(Clone, PartialEq, Eq)]
struct ClientKey {
    account: ProviderAccountId,
    owner: ProfileOwner,
    configuration_revision: u64,
    proxy: OutboundProxy,
    profile: ClientProfile,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum ProfileOwner {
    Account,
    ClientKey(ClientApiKeyId),
}

pub(super) struct Clients {
    factory: GenerationClientFactory,
    cache: Arc<ClientCache>,
    construction: Arc<Semaphore>,
    requests: Arc<Semaphore>,
}

pub(super) struct ClientLease {
    client: Arc<wreq::Client>,
    cache: Arc<ClientCache>,
    _permit: OwnedSemaphorePermit,
}

struct CachedClient {
    key: ClientKey,
    client: Arc<wreq::Client>,
    accepting: bool,
    leases: usize,
}

#[derive(Default)]
struct ClientCache {
    entries: Mutex<VecDeque<CachedClient>>,
}

impl ClientCache {
    fn cached(&self, key: &ClientKey) -> Result<Option<Arc<wreq::Client>>, ()> {
        let mut entries = self.entries.lock().map_err(|_| ())?;
        // Retirement is permanent: A → B → A cannot reactivate the first A pool.
        for entry in entries.iter_mut() {
            if entry.key.account == key.account && entry.key.owner == key.owner && entry.key != *key
            {
                entry.accepting = false;
            }
        }
        entries.retain(|entry| entry.accepting || entry.leases > 0);
        let Some(index) = entries
            .iter()
            .position(|entry| entry.accepting && entry.key == *key)
        else {
            return Ok(None);
        };
        let mut entry = entries.remove(index).ok_or(())?;
        entry.leases += 1;
        let client = entry.client.clone();
        entries.push_back(entry);
        Ok(Some(client))
    }
}

impl Drop for ClientLease {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.cache.entries.lock() {
            // Update the explicit lease count under this same lock: Arc's field destructor
            // runs after Drop returns and cannot prove that concurrent final leases drained.
            if let Some(entry) = entries
                .iter_mut()
                .find(|entry| Arc::ptr_eq(&entry.client, &self.client))
            {
                entry.leases = entry.leases.saturating_sub(1);
            }
            entries.retain(|entry| entry.accepting || entry.leases > 0);
        }
    }
}

pub(super) struct Response {
    headers: wreq::header::HeaderMap,
    status: wreq::StatusCode,
    body: Body,
    _lease: ClientLease,
}

impl Clients {
    pub(super) fn new(factory: GenerationClientFactory) -> Self {
        Self {
            factory,
            cache: Arc::default(),
            construction: Arc::new(Semaphore::new(1)),
            requests: Arc::new(Semaphore::new(128)),
        }
    }

    pub(super) async fn acquire(
        &self,
        account: &ProviderAccountId,
        owner: ProfileOwner,
        configuration_revision: u64,
        proxy: &OutboundProxy,
        profile: &ClientProfile,
    ) -> Result<ClientLease, ()> {
        let permit = self
            .requests
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ())?;
        let key = ClientKey {
            account: account.clone(),
            owner,
            configuration_revision,
            proxy: proxy.clone(),
            profile: profile.clone(),
        };
        if let Some(client) = self.cache.cached(&key)? {
            return Ok(ClientLease {
                client,
                cache: self.cache.clone(),
                _permit: permit,
            });
        }
        // 合并并发创建；等待与构建均不持有缓存锁，不阻塞其他账号复用客户端。
        let construction = self
            .construction
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ())?;
        if let Some(client) = self.cache.cached(&key)? {
            return Ok(ClientLease {
                client,
                cache: self.cache.clone(),
                _permit: permit,
            });
        }
        {
            let mut entries = self.cache.entries.lock().map_err(|_| ())?;
            if entries.len() >= 32 {
                let idle = entries
                    .iter()
                    .position(|entry| entry.leases == 0)
                    .ok_or(())?;
                entries.remove(idle);
            }
        }
        let factory = self.factory;
        let proxy = proxy.clone();
        let tls = match profile.tls {
            TlsPolicy::Standard => wreq::tls::TlsOptions::builder()
                .alpn_protocols([wreq::tls::AlpnProtocol::HTTP1])
                .build(),
            TlsPolicy::Emulate { .. } => template::claude_template(),
        };
        let (client, _construction) =
            tokio::task::spawn_blocking(move || (factory(&proxy, tls), construction))
                .await
                .map_err(|_| ())?;
        let client = Arc::new(client.map_err(|_| ())?);
        self.cache
            .entries
            .lock()
            .map_err(|_| ())?
            .push_back(CachedClient {
                key,
                client: client.clone(),
                accepting: true,
                leases: 1,
            });
        Ok(ClientLease {
            client,
            cache: self.cache.clone(),
            _permit: permit,
        })
    }
}

impl ClientLease {
    pub(super) fn post(&self, url: &str) -> wreq::RequestBuilder {
        self.client.post(url)
    }

    pub(super) async fn send(self, request: wreq::RequestBuilder) -> Result<Response, wreq::Error> {
        let response = request.send().await?;
        Ok(Response {
            headers: response.headers().clone(),
            status: response.status(),
            body: Box::pin(response.bytes_stream()),
            _lease: self,
        })
    }
}

impl Response {
    pub(super) fn headers(&self) -> &wreq::header::HeaderMap {
        &self.headers
    }
    pub(super) fn status(&self) -> wreq::StatusCode {
        self.status
    }
    pub(super) async fn chunk(&mut self) -> Result<Option<Bytes>, wreq::Error> {
        self.body.next().await.transpose()
    }
}

pub(super) fn network_error(error: wreq::Error) -> gateway_core::error::ProviderError {
    crate::execution::failed(
        if error.is_timeout() {
            ProviderErrorKind::Timeout
        } else {
            ProviderErrorKind::Transport
        },
        if error.is_connect() {
            UpstreamSendState::NotSent
        } else {
            UpstreamSendState::Ambiguous
        },
    )
}
