//! 宿主 HTTP 客户端的证书信任配置，不依赖任何上游 Provider。

/// 显式选择共享 TLS 加密实现，允许在并行初始化时重复调用。
pub fn ensure_rustls_provider() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ClientTlsError {
    #[error("无法读取自定义 CA 文件")]
    ReadCertificate,
    #[error("自定义 CA 文件不包含有效 PEM 证书")]
    InvalidCertificate,
    #[error("无法创建 HTTP TLS 客户端")]
    BuildClient,
    #[error("出站代理配置无效")]
    InvalidProxy,
}

/// 管理端凭据探测使用显式出口，不继承进程代理，也不将凭据随重定向转发。
pub fn build_probe_client(
    proxy: Option<&gateway_core::account::OutboundProxy>,
) -> Result<reqwest::Client, ClientTlsError> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10));
    if let Some(proxy) = proxy {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy.expose_url()).map_err(|_| ClientTlsError::InvalidProxy)?,
        );
    }
    build_reqwest_client(builder)
}

/// 生成由 Core 请求截止控制总时长，避免把目录探测的短超时套用于 SSE。
pub fn build_generation_client(
    proxy: &gateway_core::account::OutboundProxy,
    tls: wreq::tls::TlsOptions,
) -> Result<wreq::Client, ClientTlsError> {
    let builder = wreq::Client::builder()
        .no_proxy()
        .proxy(wreq::Proxy::all(proxy.expose_url()).map_err(|_| ClientTlsError::InvalidProxy)?)
        .redirect(wreq::redirect::Policy::none())
        .retry(wreq::retry::Policy::never())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .tls_cert_store(generation_trust()?)
        .http1_only()
        .tls_options(tls)
        .connect_timeout(std::time::Duration::from_secs(10))
        .pool_max_idle_per_host(8)
        .pool_idle_timeout(std::time::Duration::from_secs(90));
    builder.build().map_err(|_| ClientTlsError::BuildClient)
}

/// 信任材料是进程快照，变更 CA 后重启；不在推理热路径反复读文件。
fn generation_trust() -> Result<wreq::tls::trust::CertStore, ClientTlsError> {
    static TRUST: std::sync::OnceLock<Result<wreq::tls::trust::CertStore, ClientTlsError>> =
        std::sync::OnceLock::new();
    TRUST
        .get_or_init(|| {
            let roots = rustls_native_certs::load_native_certs();
            let mut builder = wreq::tls::trust::CertStore::builder()
                .add_der_certs(roots.certs.iter().map(|cert| cert.as_ref()));
            if let Some(ca) = supplemental_ca()? {
                builder = builder.add_stack_pem_certs(ca.pem);
            }
            builder
                .build()
                .map_err(|_| ClientTlsError::InvalidCertificate)
        })
        .clone()
}

/// 使用系统根证书，可通过项目变量或标准 SSL_CERT_FILE 补充 CA。
pub fn build_reqwest_client(
    builder: reqwest::ClientBuilder,
) -> Result<reqwest::Client, ClientTlsError> {
    ensure_rustls_provider();
    let mut builder = builder.use_rustls_tls();
    if let Some(ca) = supplemental_ca()? {
        for certificate in ca.certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    builder.build().map_err(|_| ClientTlsError::BuildClient)
}

struct SupplementalCa {
    pem: Vec<u8>,
    certificates: Vec<reqwest::Certificate>,
}

fn supplemental_ca() -> Result<Option<SupplementalCa>, ClientTlsError> {
    let path = ["CLAUDE_PROXY_CA_CERTIFICATE", "SSL_CERT_FILE"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty());
    let Some(path) = path else { return Ok(None) };
    let pem = std::fs::read(path).map_err(|_| ClientTlsError::ReadCertificate)?;
    let certificates = reqwest::Certificate::from_pem_bundle(&pem)
        .map_err(|_| ClientTlsError::InvalidCertificate)?;
    if certificates.is_empty() {
        return Err(ClientTlsError::InvalidCertificate);
    }
    Ok(Some(SupplementalCa { pem, certificates }))
}
