//! Claude 出站策略：必须显式绑定代理，调试替身只能使用本机环回地址。
use gateway_admin::ports::provider::{ProviderAdminError, ProviderAdminErrorKind};
use gateway_core::account::OutboundProxy;

#[derive(Clone)]
pub struct NetworkConfig {
    pub(crate) api_base: String,
    pub(crate) authorization_ttl: std::time::Duration,
    required_proxy: Option<String>,
}
impl NetworkConfig {
    pub fn from_environment() -> Result<Self, &'static str> {
        let api_base = match std::env::var("CPR_CLAUDE_TEST_UPSTREAM") {
            Ok(base) => {
                let url = url::Url::parse(&base).map_err(|_| "Claude 测试端点无效")?;
                if !cfg!(debug_assertions)
                    || !matches!(
                        (url.scheme(), url.host_str()),
                        ("http", Some("127.0.0.1")) | ("https", Some("localhost"))
                    )
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || url.path() != "/"
                {
                    return Err("Claude 测试端点仅允许调试构建使用本机环回地址");
                }
                base.trim_end_matches('/').to_owned()
            }
            Err(std::env::VarError::NotPresent) => "https://api.anthropic.com".into(),
            Err(_) => return Err("Claude 测试端点无效"),
        };
        let authorization_ttl = match std::env::var("CPR_CLAUDE_TEST_FLOW_TTL_SECONDS") {
            Ok(value) if api_base.starts_with("http://127.0.0.1:") => {
                let seconds: u64 = value.parse().map_err(|_| "Claude 测试授权有效期无效")?;
                if !(1..=1800).contains(&seconds) {
                    return Err("Claude 测试授权有效期无效");
                }
                std::time::Duration::from_secs(seconds)
            }
            Err(std::env::VarError::NotPresent) => std::time::Duration::from_secs(1800),
            _ => return Err("Claude 测试授权有效期仅限本地模拟端点"),
        };
        let required_proxy = match std::env::var("CPR_CLAUDE_REQUIRED_PROXY_ENDPOINT") {
            Ok(value) => {
                let proxy = OutboundProxy::parse(&value).map_err(|_| "Claude 指定代理无效")?;
                if proxy.expose_url() != proxy.endpoint() {
                    return Err("指定代理地址不能包含认证材料");
                }
                Some(proxy.endpoint())
            }
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => return Err("Claude 指定代理无效"),
        };
        Ok(Self {
            api_base,
            authorization_ttl,
            required_proxy,
        })
    }
    pub(crate) fn require_proxy(
        &self,
        proxy: Option<&OutboundProxy>,
    ) -> Result<(), ProviderAdminError> {
        let proxy = proxy.ok_or_else(|| invalid("Claude 账号必须绑定出站代理"))?;
        let required = self.required_proxy.as_ref().ok_or_else(|| {
            invalid("部署尚未设置 CPR_CLAUDE_REQUIRED_PROXY_ENDPOINT，已阻止 Claude 上游访问")
        })?;
        if &proxy.endpoint() != required {
            return Err(invalid("Claude 账号必须使用部署指定的代理出口"));
        }
        Ok(())
    }
}
fn invalid(message: &'static str) -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid).with_public_message(message)
}
