//! 通过 Anthropic 模型目录验证凭据连通性；不执行生成，不记录上游正文。

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::Utc;
use gateway_admin::{
    model::provider_credentials::{ProviderModel, ProviderModels},
    ports::provider::{ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::routing::UpstreamModelId;
use serde::Deserialize;

use crate::credential::Credential;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 10;

#[derive(Deserialize)]
struct Page {
    data: Vec<Model>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    #[serde(default)]
    display_name: String,
}

fn failed(message: &'static str) -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::BadGateway).with_public_message(message)
}

fn network_error(error: reqwest::Error) -> ProviderAdminError {
    // reqwest 错误可能含有上游 URL；不将它或原始响应加入公开错误及日志。
    if error.is_timeout() {
        failed("Claude 连接测试超时")
    } else {
        failed("无法连接 Claude 上游，请检查地址、代理和证书")
    }
}

pub(super) async fn read_models(
    client: reqwest::Client,
    credential: Credential,
    base: &str,
) -> Result<ProviderModels, ProviderAdminError> {
    tokio::time::timeout(
        Duration::from_secs(10),
        read_pages(client, credential, base),
    )
    .await
    .map_err(|_| failed("Claude 连接测试超时"))?
}

async fn read_pages(
    client: reqwest::Client,
    credential: Credential,
    base: &str,
) -> Result<ProviderModels, ProviderAdminError> {
    let base = base.trim_end_matches('/');
    let endpoint = if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    };
    let mut key =
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", credential.access_token))
            .map_err(|_| crate::invalid())?;
    key.set_sensitive(true);
    let mut cursor = None;
    let mut cursors = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut models = Vec::new();
    for _ in 0..MAX_PAGES {
        let mut request = client
            .get(&endpoint)
            .header("authorization", key.clone())
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("anthropic-version", "2023-06-01")
            .query(&[("limit", "1000")]);
        if let Some(after) = cursor.as_deref() {
            request = request.query(&[("after_id", after)]);
        }
        let mut response = request.send().await.map_err(network_error)?;
        match response.status().as_u16() {
            200 => {}
            401 | 403 => return Err(failed("Claude 拒绝了凭据，请检查授权及访问权限")),
            429 => return Err(failed("Claude 请求受限，请稍后重试")),
            300..=399 => return Err(failed("Claude 上游返回重定向，请直接填写最终 API 地址")),
            _ => return Err(failed("Claude 上游返回错误状态")),
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network_error)? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(failed("Claude 模型目录响应过大"));
            }
            body.extend_from_slice(&chunk);
        }
        let page: Page =
            serde_json::from_slice(&body).map_err(|_| failed("Claude 上游未返回有效模型目录"))?;
        if page.data.len() > 1000 {
            return Err(failed("Claude 模型目录超过请求条数限制"));
        }
        for model in page.data {
            let id = UpstreamModelId::new(model.id.clone())
                .map_err(|_| failed("Claude 上游返回无效模型标识"))?;
            if model.display_name.len() > 512 {
                return Err(failed("Claude 上游返回无效模型名称"));
            }
            if ids.insert(model.id.clone()) {
                models.push(ProviderModel {
                    id,
                    name: if model.display_name.is_empty() {
                        model.id
                    } else {
                        model.display_name
                    },
                });
            }
        }
        if !page.has_more {
            return Ok(ProviderModels {
                models,
                observed_at: Some(Utc::now()),
            });
        }
        let next = page
            .last_id
            .filter(|value| !value.is_empty() && value.len() <= 512)
            .ok_or_else(|| failed("Claude 模型目录分页信息无效"))?;
        if !cursors.insert(next.clone()) {
            return Err(failed("Claude 模型目录分页未前进"));
        }
        cursor = Some(next);
    }
    Err(failed("Claude 模型目录超过分页限制"))
}
