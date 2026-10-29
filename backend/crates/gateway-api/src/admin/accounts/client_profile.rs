//! 账号客户端配置独立于凭据及普通账号编辑；保存需要当前版本。

use super::*;
use crate::auth::SessionState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Update {
    account_id: String,
    expected_revision: u64,
    // 缺少字段不能被解释成主动清除覆盖。
    configuration: Value,
}

#[derive(Serialize)]
pub(super) struct Data {
    revision: u64,
    configuration: Option<Map<String, Value>>,
}

impl From<gateway_core::account::AccountClientConfiguration> for Data {
    fn from(configuration: gateway_core::account::AccountClientConfiguration) -> Self {
        Self {
            revision: configuration.revision(),
            configuration: configuration.profile().map(|p| p.clone().into_inner()),
        }
    }
}

pub(super) async fn read<S>(
    _: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<AccountIdQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let account_id = query.into_id().map_err(map_wire_error)?;
    let value = state
        .admin_services()
        .accounts()
        .client_profile(&account_id)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(Data::from(value)),
    ))
}

pub(super) async fn update<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<Update>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let account_id = AccountIdQuery {
        account_id: request.account_id,
    }
    .into_id()
    .map_err(map_wire_error)?;
    if request.expected_revision == 0 || request.expected_revision > i64::MAX as u64 {
        return Err(map_wire_error(WireValidationError::new("expectedRevision")));
    }
    let configuration = match request.configuration {
        Value::Null => None,
        Value::Object(fields)
            if serde_json::to_vec(&fields).is_ok_and(|value| value.len() <= 8192) =>
        {
            Some(OpaqueProviderData::new(fields))
        }
        _ => return Err(map_wire_error(WireValidationError::new("configuration"))),
    };
    let command = gateway_admin::model::accounts::UpdateAccountClientProfile {
        account_id,
        expected_revision: request.expected_revision,
        configuration,
    };
    let value = state
        .admin_services()
        .accounts()
        .update_client_profile(&auth.context().mutation_context(), command)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(Data::from(value)),
    ))
}
