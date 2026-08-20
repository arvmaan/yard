use std::sync::Arc;

use tracing_subscriber::EnvFilter;
use yard_herdr::{HerdrAdapter, HerdrConfig};
use yard_server::{
    app_with_reconciliation_and_paths_and_automation, config::ServerConfig,
    inventory_service::HerdrInventorySource, reconciliation_service::ReconciliationService,
    runtime_cleanup_service::RuntimeCleanupService,
};
use yard_store::SqliteProjectStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("yard_server=info")),
        )
        .init();

    let config = ServerConfig::from_env()?;
    let adapter = HerdrAdapter::new(HerdrConfig {
        binary: config.herdr_binary,
        ..HerdrConfig::default()
    });
    let runtime = Arc::new(HerdrInventorySource::new(adapter));
    let store = Arc::new(SqliteProjectStore::open(&config.database_path).await?);
    let reconciliation = ReconciliationService::new(runtime.clone(), store.clone());
    tokio::spawn(reconciliation.clone().run());
    let cleanup = RuntimeCleanupService::new(runtime.clone(), store.clone());
    tokio::spawn(cleanup.run());
    let (app, automations) = app_with_reconciliation_and_paths_and_automation(
        runtime.clone(),
        runtime.clone(),
        runtime.clone(),
        runtime.clone(),
        store.clone(),
        config.artifact_path.clone(),
        reconciliation.clone(),
        config.orchestrator_cwd.clone(),
        config.coordination_path.clone(),
        config.knowledge_path.clone(),
    );
    tokio::spawn(automations.run());
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let address = listener.local_addr()?;
    let ui_url = format!("http://{address}/");

    tracing::info!(
        address = %address,
        ui_url = %ui_url,
        database = %config.database_path.display(),
        artifacts = %config.artifact_path.display(),
        orchestrator_cwd = %config.orchestrator_cwd.display(),
        coordination = %config.coordination_path.display(),
        knowledge = %config.knowledge_path.display(),
        "Yard UI available"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
