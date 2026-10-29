//! 非敏感客户端配置的独立 CAS；不更新或重新序列化账号凭据。

use super::*;
use gateway_admin::model::{
    accounts::{UpdateAccountClientProfile, UpdatedAccountClientProfile},
    audit::MutationAuditOperation,
};
use gateway_core::account::AccountClientConfiguration;

pub(super) fn from_row(row: &sqlx::postgres::PgRow) -> StoreResult<AccountClientConfiguration> {
    let revision = to_u64(get::<i64>(row, "configuration_revision")?)?;
    let profile = get::<Option<sqlx::types::Json<serde_json::Map<String, serde_json::Value>>>>(
        row,
        "provider_configuration",
    )?
    .map(|value| OpaqueProviderData::new(value.0));
    AccountClientConfiguration::new(revision, profile)
        .ok_or_else(|| invalid("invalid client configuration revision"))
}

impl PgAdminAccountStore {
    pub(super) async fn load_client_configuration(
        &self,
        account_id: &CoreProviderAccountId,
    ) -> AdminStoreResult<AccountClientConfiguration> {
        let row = sqlx::query("select provider_configuration, configuration_revision from provider_accounts where id = $1")
            .bind(account_id.as_str()).fetch_optional(&self.pool).await
            .map_err(|source| admin_store_error(ENTITY, postgres_unavailable("load client configuration", source)))?
            .ok_or_else(|| AdminStoreError::new(AdminStoreErrorKind::NotFound, ENTITY, "account not found"))?;
        from_row(&row).map_err(|error| admin_store_error(ENTITY, error))
    }

    pub(super) async fn save_client_configuration(
        &self,
        command: UpdateAccountClientProfile,
        context: &MutationContext,
    ) -> AdminStoreResult<UpdatedAccountClientProfile> {
        let expected =
            to_i64(command.expected_revision).map_err(|error| admin_store_error(ENTITY, error))?;
        if expected < 1 {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                ENTITY,
                "invalid configuration revision",
            ));
        }
        let configuration = command.configuration.map(|profile| profile.into_inner());
        if serde_json::to_vec(&configuration)
            .map_err(|_| {
                AdminStoreError::new(
                    AdminStoreErrorKind::Invalid,
                    ENTITY,
                    "invalid configuration",
                )
            })?
            .len()
            > 8192
        {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                ENTITY,
                "client configuration too large",
            ));
        }
        let mut transaction = self.pool.begin().await.map_err(|source| {
            admin_store_error(
                ENTITY,
                postgres_unavailable("begin client configuration", source),
            )
        })?;
        let result = async {
            // 与其他管理更新一致：先锁全局配置版本，再更新账号行。
            let config_revision = bump_config_revision_in_transaction(&mut transaction).await?;
            let row = sqlx::query("update provider_accounts set provider_configuration = $3,
                configuration_revision = configuration_revision + 1, updated_at = greatest(now(), updated_at)
                where id = $1 and configuration_revision = $2 returning provider_configuration, configuration_revision")
                .bind(command.account_id.as_str()).bind(expected)
                .bind(configuration.map(sqlx::types::Json)).fetch_optional(&mut *transaction).await
                .map_err(|source| postgres_unavailable("save client configuration", source))?
                .ok_or_else(|| StoreError::Conflict { source: None, entity: ENTITY,
                    id: command.account_id.as_str().to_owned(), kind: ConflictKind::StaleRevision })?;
            let configuration = from_row(&row)?;
            append_admin_audit_event_in_transaction(&mut transaction,
                mutation_audit(context, MutationAuditOperation::ProviderAccountUpdate, command.account_id.as_str(), vec!["client_configuration".into()]),
                config_revision).await?;
            Ok((config_revision, configuration))
        }.await;
        let (config_revision, configuration) = super::repository::finish_admin_transaction(
            transaction,
            result,
            "commit client configuration",
        )
        .await
        .map_err(|error| admin_store_error(ENTITY, error))?;
        Ok(UpdatedAccountClientProfile {
            config_revision: admin_revision(config_revision)?,
            configuration,
        })
    }
}
