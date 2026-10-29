//! 本项目修改说明与原始来源见仓库根目录 NOTICE
//! 启动模式共享的基础设施与 Provider 组装，不启动 HTTP 或 Worker

use std::{path::PathBuf, sync::Arc};

use gateway_admin::ports::provider::ProviderAdminRegistry;
use gateway_core::{engine::provider::ProviderRegistry, time::DeploymentTimeZone};
use gateway_host::{ConfigError, HostBundle, HostConfig, outbound::HttpClient};
use gateway_store::{StoreBundle, StoreConfig};

use super::BootstrapError;

pub(super) enum Mode {
    Server,
    Command,
    Help,
}

pub(super) struct Environment {
    pub(super) host: HostBundle,
    pub(super) store: StoreBundle,
    pub(super) timezone: DeploymentTimeZone,
    pub(super) plugin_cache: PathBuf,
    pub(super) version: String,
    pub(super) plugin_http: Arc<HttpClient>,
}

impl Environment {
    pub(super) async fn initialize(
        host: HostConfig,
        store: StoreConfig,
        mode: Mode,
    ) -> Result<Self, BootstrapError> {
        let timezone = host.timezone;
        let store = store.with_timezone(timezone);
        let plugin_cache = host.runtime_data_dir().join("plugins");
        let host = match mode {
            Mode::Server => gateway_host::initialize(host).await?,
            Mode::Command | Mode::Help => gateway_host::initialize_command_line(host).await?,
        };
        host.report_startup_ready("Host");
        let store = match mode {
            Mode::Server => gateway_store::initialize(store).await?,
            Mode::Command => gateway_store::initialize_command_line(store).await?,
            Mode::Help => gateway_store::initialize_read_only(store).await?,
        };
        host.report_startup_ready("Store");
        let version = host
            .system_operations()
            .version()
            .await
            .map_err(|_| ConfigError::InvalidField("host.version"))?
            .version;
        let plugin_http = Arc::new(HttpClient::new()?);
        Ok(Self {
            host,
            store,
            timezone,
            plugin_cache,
            version,
            plugin_http,
        })
    }
}

pub(super) struct Providers {
    pub(super) core: ProviderRegistry,
    pub(super) admin: ProviderAdminRegistry,
}

impl Providers {
    pub(super) fn initialize(store: &StoreBundle) -> Result<Self, BootstrapError> {
        let network = gateway_provider_claude::NetworkConfig::from_environment()
            .map_err(gateway_admin::model::AdminError::invalid)?;
        let core = ProviderRegistry::new([gateway_provider_claude::initialize_execution(
            store.provider_ports(),
            |proxy, tls| {
                gateway_host::tls::build_generation_client(proxy, tls)
                    .map_err(|_| "无法创建 Claude 生成客户端")
            },
            network.clone(),
        )])?;
        let claude = gateway_provider_claude::initialize(
            store.provider_ports(),
            |proxy| {
                gateway_host::tls::build_probe_client(proxy)
                    .map_err(|_| "无法创建 Claude 连接测试客户端")
            },
            network,
        )
        .map_err(|_| gateway_admin::model::AdminError::invalid("Claude 账号管理初始化失败"))?;
        let admin = ProviderAdminRegistry::new([claude])
            .map_err(|_| gateway_admin::model::AdminError::invalid("Provider 管理身份冲突"))?;
        Ok(Self { core, admin })
    }
}
