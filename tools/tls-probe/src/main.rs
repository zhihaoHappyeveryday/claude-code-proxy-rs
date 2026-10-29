use futures_util::StreamExt;
use std::{io::Write, net::SocketAddr, time::Duration};
// Reuse the production template; expected handshakes come from the independent CLI capture.
#[path = "../../../backend/crates/providers/claude/src/transport/template.rs"]
mod template;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: claude-tls-probe <upstream-port> <proxy-port> <test-ca.pem>".into());
    }
    let upstream_port: u16 = args[0].parse()?;
    let proxy_port: u16 = args[1].parse()?;
    if upstream_port == 0 || proxy_port == 0 {
        return Err("ports must be nonzero".into());
    }
    let pem = std::fs::read(&args[2])?;
    let client = wreq::Client::builder()
        .no_proxy()
        .proxy(wreq::Proxy::all(format!(
            "https://probe:fixture@localhost:{proxy_port}"
        ))?)
        .resolve("localhost", SocketAddr::from(([127, 0, 0, 1], proxy_port)))
        .tls_cert_store(wreq::tls::trust::CertStore::from_pem_stack(&pem)?)
        .redirect(wreq::redirect::Policy::none())
        .retry(wreq::retry::Policy::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .http1_only()
        .tls_options(template::claude_template())
        .build()?;
    let response = client
        .post(format!("https://localhost:{upstream_port}/v1/messages"))
        .header("content-type", "application/json")
        .header("authorization", "Bearer local-fixture-only")
        .body(r#"{"model":"claude-sonnet-4-5","max_tokens":8,"stream":true,"messages":[{"role":"user","content":"probe"}]}"#)
        .send()
        .await?
        .error_for_status()?;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        std::io::stdout().write_all(&chunk?)?;
        std::io::stdout().flush()?;
    }
    Ok(())
}
