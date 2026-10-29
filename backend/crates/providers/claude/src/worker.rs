//! 到期扫描由 Host 调度，写入仍经过 Admin 的 CAS、审计和快照发布。
use futures::StreamExt;
use gateway_admin::{
    AdminServices,
    model::{MutationActor, MutationContext},
};
use gateway_core::{
    account::{ProviderAccountId, ProviderRefreshQuery},
    provider_ports::{ProviderStorePorts, provider_refresh_backoff_at},
    routing::ProviderKind,
    task::*,
};
use std::{
    collections::HashMap,
    num::NonZeroU32,
    sync::Mutex,
    time::{Duration, SystemTime},
};

pub fn oauth_refresh_worker(
    ports: ProviderStorePorts,
    admin: AdminServices,
) -> Result<WorkerContribution, WorkerDefinitionError> {
    let id = WorkerId::try_new(WorkerKind::OAuthRefresh, "claude")?;
    let schedule = WorkerSchedule::try_new(
        Duration::from_secs(3),
        Duration::from_secs(30),
        Duration::from_secs(60),
        Duration::from_secs(90),
        Duration::from_secs(30),
    )?;
    let lease = WorkerLeaseRequest::try_new(id.clone(), Duration::from_secs(90))?;
    Ok(WorkerContribution::Registration(
        WorkerRegistration::try_new(
            id,
            WorkerRunnable::Scheduled {
                schedule,
                lease: Some(lease),
                task: Box::new(RefreshTask {
                    ports,
                    admin,
                    scan: Mutex::default(),
                }),
            },
        )?,
    ))
}
struct RefreshTask {
    ports: ProviderStorePorts,
    admin: AdminServices,
    scan: Mutex<RefreshScan>,
}
/// 保留跨轮扫描进度，避免一个失败或停用批次挡住后面的账号。
/// 只保留账号 ID 与重试时刻，不缓存明文凭据；进程重启可安全重建。
#[derive(Default)]
struct RefreshScan {
    visited: Vec<ProviderAccountId>,
    retry_at: HashMap<ProviderAccountId, SystemTime>,
}
impl RefreshTask {
    fn excluded(&self, now: SystemTime) -> Result<Vec<ProviderAccountId>, WorkerTaskError> {
        let mut scan = self
            .scan
            .lock()
            .map_err(|_| WorkerTaskError::safe("Claude 刷新游标不可用"))?;
        scan.retry_at.retain(|_, at| *at > now);
        Ok(scan
            .visited
            .iter()
            .chain(scan.retry_at.keys())
            .cloned()
            .collect())
    }

    async fn refresh_one(&self, id: ProviderAccountId) -> Result<(), WorkerTaskError> {
        let mutation = MutationContext {
            actor: MutationActor::System,
            request_id: format!("claude-refresh-{}", uuid::Uuid::now_v7().simple()),
        };
        let state = self.ports.credential_state();
        if self
            .admin
            .accounts()
            .refresh(&mutation, id.clone())
            .await
            .is_ok()
        {
            let _ = state.clear_refresh_backoff(&id).await;
            return Ok(());
        }
        let attempt = state
            .record_refresh_backoff(&id, Duration::from_secs(3600))
            .await
            .unwrap_or(1);
        let retry_at =
            provider_refresh_backoff_at(&id, SystemTime::now(), attempt, "claude-refresh")
                .map_err(|_| WorkerTaskError::safe("Claude 刷新退避不可用"))?;
        self.scan
            .lock()
            .map_err(|_| WorkerTaskError::safe("Claude 刷新游标不可用"))?
            .retry_at
            .insert(id, retry_at);
        Err(WorkerTaskError::safe(
            "Claude 自动刷新失败，请检查账号授权和代理",
        ))
    }
}
impl ScheduledTask for RefreshTask {
    fn run_cycle(
        &self,
        context: WorkerCycleContext,
    ) -> futures::future::BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let policy = self
                .ports
                .runtime_policy()
                .load_refresh_policy()
                .await
                .map_err(|_| WorkerTaskError::safe("Claude 刷新策略读取失败"))?;
            let now = SystemTime::now();
            let provider = ProviderKind::new("claude")
                .map_err(|_| WorkerTaskError::safe("Claude Provider 无效"))?;
            let limit =
                NonZeroU32::new(50).ok_or_else(|| WorkerTaskError::safe("Claude 刷新批次无效"))?;
            let mut failed = false;
            loop {
                let accounts = self
                    .ports
                    .accounts()
                    .list_refresh_candidates(ProviderRefreshQuery::new(
                        provider.clone(),
                        now + policy.staggered_refresh_bound(),
                        now,
                        now,
                        self.excluded(now)?,
                        limit,
                    ))
                    .await
                    .map_err(|_| WorkerTaskError::safe("Claude 到期账号读取失败"))?;
                {
                    let mut scan = self
                        .scan
                        .lock()
                        .map_err(|_| WorkerTaskError::safe("Claude 刷新游标不可用"))?;
                    if accounts.is_empty() {
                        scan.visited.clear();
                        return if failed {
                            Err(WorkerTaskError::safe("Claude 部分账号刷新失败"))
                        } else {
                            Ok(())
                        };
                    }
                    scan.visited
                        .extend(accounts.iter().map(|loaded| loaded.account.id().clone()));
                }
                let ids = accounts
                    .into_iter()
                    .filter(|loaded| {
                        loaded.account.enabled()
                            && loaded.account.authentication_kind() == "oauth"
                            && loaded.account.access_token_expires_at().is_some_and(|at| {
                                policy.is_refresh_due_staggered(loaded.account.id(), at, now)
                            })
                    })
                    .map(|loaded| loaded.account.id().clone());
                let work = futures::stream::iter(ids)
                    .map(|id| self.refresh_one(id))
                    .buffer_unordered(policy.concurrency().get() as usize)
                    .collect::<Vec<_>>();
                tokio::select! {
                    () = context.cancellation().cancelled() => return Ok(()),
                    results = work => failed |= results.iter().any(Result::is_err),
                }
            }
        })
    }
}
