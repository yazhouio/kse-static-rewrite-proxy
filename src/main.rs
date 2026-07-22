use std::error::Error;
use std::fs;

use kse_static_rewrite_proxy::config::EffectiveConfig;
use kse_static_rewrite_proxy::proxy::KseRewriteProxy;
use pingora::proxy::http_proxy_service;
use pingora::server::Server;
use pingora::services::listening::Service;
use tracing_subscriber::EnvFilter;

const DEFAULT_CONFIG_PATH: &str = "/etc/kse-console/config.yaml";

fn main() {
    if let Err(error) = run() {
        eprintln!("kse-static-rewrite-proxy failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    init_logging()?;
    let config_path =
        std::env::var("KSE_REWRITE_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_owned());
    let config = EffectiveConfig::from_yaml(&fs::read_to_string(&config_path)?)?;
    let listen = config.listen().to_string();
    let admin_listen = config.admin_listen().to_string();
    let proxy = KseRewriteProxy::new(config)?;
    let admin_app = proxy.admin_app();

    let mut server = Server::new(None)?;
    server.bootstrap();
    let mut service = http_proxy_service(&server.configuration, proxy);
    service.add_tcp(&listen);
    let mut admin_service = Service::new("admin".to_owned(), admin_app);
    admin_service.add_tcp(&admin_listen);
    server.add_service(service);
    server.add_service(admin_service);
    server.run_forever();
}

fn init_logging() -> Result<(), Box<dyn Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .try_init()?;
    Ok(())
}
