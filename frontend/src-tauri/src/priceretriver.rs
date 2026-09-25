use priceretriver_client::PriceRetriverClient;
use serde_json::Value;
use std::path::Path;

#[tauri::command]
pub async fn priceretriver_status(
    node_url: String,
    token: Option<String>,
) -> Result<Value, String> {
    let client = PriceRetriverClient::new(node_url, token);
    client.status().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn priceretriver_list_partitions(
    node_url: String,
    token: Option<String>,
) -> Result<Value, String> {
    let client = PriceRetriverClient::new(node_url, token);
    client.list_partitions().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn priceretriver_download_data(
    node_url: String,
    token: Option<String>,
    venue: Option<String>,
    symbol: Option<String>,
    dest_path: String,
) -> Result<u64, String> {
    let client = PriceRetriverClient::new(node_url, token);
    client
        .download_archive(
            venue.as_deref(),
            symbol.as_deref(),
            None,
            Path::new(&dest_path),
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn priceretriver_run_pipeline(
    node_url: String,
    token: Option<String>,
    partition_path: String,
    pipeline_type: String,
    format: String,
) -> Result<Value, String> {
    let client = PriceRetriverClient::new(node_url, token);
    client
        .run_pipeline(&partition_path, &pipeline_type, &format)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn priceretriver_run_custom_script(
    node_url: String,
    token: Option<String>,
    script_code: String,
    language: String,
    partition_path: Option<String>,
) -> Result<Value, String> {
    let client = PriceRetriverClient::new(node_url, token);
    client
        .run_custom_script(&script_code, &language, partition_path.as_deref())
        .await
        .map_err(|e| e.to_string())
}
