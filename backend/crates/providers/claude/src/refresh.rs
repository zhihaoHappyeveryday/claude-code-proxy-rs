//! 凭据轮换持有容量与账号租约直到 Admin 完成持久化。
use crate::{
    ClaudeAdmin,
    credential::{Credential, Kind},
};
use gateway_admin::{
    model::{Revision, provider_credentials::*},
    ports::provider::{ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::{
    account::{LoadedCredential, ProviderAccount, ProviderAccountId},
    provider_ports::*,
};

pub(super) struct RotationGuard {
    _capacity: Box<dyn ProviderLeaseGuard>,
    _account: Box<dyn ProviderLeaseGuard>,
}
impl CredentialCommitGuard for RotationGuard {
    fn finish(self: Box<Self>) {}
}
fn unavailable() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unavailable)
}
fn conflict() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Conflict)
        .with_public_message("账号凭据正在更新，请稍后重试")
}
impl ClaudeAdmin {
    pub(super) async fn lock_credential(
        &self,
        id: &ProviderAccountId,
    ) -> Result<(LoadedCredential, Box<dyn CredentialCommitGuard>), ProviderAdminError> {
        let loaded = self.credential(id).await?;
        let policy = self
            .ports
            .runtime_policy()
            .load_refresh_policy()
            .await
            .map_err(|_| unavailable())?;
        let port = self.ports.leases();
        let ProviderLeaseAcquisition::Acquired(capacity) = port
            .try_acquire(ProviderLeaseRequest::RefreshCapacity(
                ProviderRefreshCapacityRequest::new(policy.concurrency()),
            ))
            .await
            .map_err(|_| unavailable())?
        else {
            return Err(conflict());
        };
        let ProviderLeaseAcquisition::Acquired(account) = port
            .try_acquire(ProviderLeaseRequest::Refresh(
                ProviderRefreshLeaseRequest::new(id.clone(), loaded.account.revision()),
            ))
            .await
            .map_err(|_| unavailable())?
        else {
            return Err(conflict());
        };
        // 取得租约后再次读取 fence；不使用等候前的过期 refresh token。
        let current = self.credential(id).await?;
        if current.account.revision() != loaded.account.revision() {
            return Err(conflict());
        }
        self.network
            .require_proxy(current.account.outbound_proxy())?;
        Ok((
            current,
            Box::new(RotationGuard {
                _capacity: capacity,
                _account: account,
            }),
        ))
    }
    pub(super) async fn refresh(
        &self,
        id: &ProviderAccountId,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        let (loaded, guard) = self.lock_credential(id).await?;
        let old = Credential::parse(loaded.credential.expose_to_provider())?;
        let token = old
            .refresh_token
            .as_ref()
            .filter(|_| old.authentication_kind == Kind::OAuth)
            .ok_or_else(|| {
                ProviderAdminError::new(ProviderAdminErrorKind::Invalid)
                    .with_public_message("此账号没有刷新凭据，请重新授权或替换 Setup Token")
            })?;
        let mut new = self.exchange_token(serde_json::json!({"grant_type":"refresh_token","refresh_token":token,"client_id":crate::oauth::CLIENT_ID}), loaded.account.outbound_proxy(), Kind::OAuth).await?;
        verify_identity(&old, &new)?;
        new.account_uuid = old.account_uuid;
        new.org_uuid = old.org_uuid;
        new.email = old.email;
        rotation(&loaded.account, new, guard)
    }
}
pub(super) fn verify_identity(
    old: &Credential,
    new: &Credential,
) -> Result<(), ProviderAdminError> {
    if old
        .account_uuid
        .as_ref()
        .zip(new.account_uuid.as_ref())
        .is_some_and(|(a, b)| a != b)
        || old
            .org_uuid
            .as_ref()
            .zip(new.org_uuid.as_ref())
            .is_some_and(|(a, b)| a != b)
        || old.authentication_kind != new.authentication_kind
    {
        return Err(ProviderAdminError::new(ProviderAdminErrorKind::Invalid)
            .with_public_message("授权账号与原账号身份不一致"));
    }
    Ok(())
}
pub(super) fn rotation(
    account: &ProviderAccount,
    material: Credential,
    guard: Box<dyn CredentialCommitGuard>,
) -> Result<PreparedCredentialRotation, ProviderAdminError> {
    Ok(PreparedCredentialRotation::new(
        PreparedCredentialRotationFacts {
            account_id: account.id().clone(),
            provider_kind: account.provider().clone(),
            expected_credential_revision: Revision::new(account.revision().get())
                .map_err(|_| unavailable())?,
            replacement_identity: None,
            name: account.name().into(),
            email: account.email().map(str::to_owned),
            plan_type: account.plan_type().map(str::to_owned),
            preserve_profile: true,
            preserve_credential_state: false,
            replacement_credential_state: None,
            provider_material: material.document()?,
            has_refresh_token: material.refresh_token.is_some(),
            access_token_expires_at: material.expires_at,
            next_refresh_at: None,
        },
        guard,
    ))
}
