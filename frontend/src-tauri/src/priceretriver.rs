use priceretriver_client::PriceRetriverClient;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

const MAX_SCRIPT_BYTES: usize = 64 * 1024;

fn validated_node_url(raw: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|e| format!("invalid PriceRetriver node URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("PriceRetriver node URL must use http or https".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("PriceRetriver node URL must not embed credentials".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "PriceRetriver node URL has no host".to_string())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    let allowlisted = std::env::var("ARBITRAGEUR_PRICERETRIVER_ALLOWED_HOSTS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| entry.eq_ignore_ascii_case(host));
    if !loopback && !allowlisted {
        return Err(format!(
            "PriceRetriver host '{host}' is not loopback; add it explicitly to ARBITRAGEUR_PRICERETRIVER_ALLOWED_HOSTS"
        ));
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn export_root() -> Result<PathBuf, String> {
    let root = std::env::var_os("ARBITRAGEUR_PRICERETRIVER_DEST_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".arbitrageur/priceretriver-exports"))
        })
        .ok_or_else(|| "cannot resolve PriceRetriver export directory".to_string())?;
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("cannot create PriceRetriver export directory: {e}"))?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize PriceRetriver export directory: {e}"))?;
    Ok(root)
}

fn validated_destination(raw: &str) -> Result<PathBuf, String> {
    let raw_path = Path::new(raw.trim());
    if raw.trim().is_empty()
        || raw_path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("PriceRetriver destination must stay inside the export directory".into());
    }
    let root = export_root()?;
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        root.join(raw_path)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| "PriceRetriver destination has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create PriceRetriver destination directory: {e}"))?;
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize PriceRetriver destination: {e}"))?;
    if !parent.starts_with(&root) {
        return Err(format!(
            "PriceRetriver destination must be under {}",
            root.display()
        ));
    }
    let file_name = candidate
        .file_name()
        .ok_or_else(|| "PriceRetriver destination needs a file name".to_string())?;
    Ok(parent.join(file_name))
}

fn validate_remote_relative_path(raw: &str, field: &str) -> Result<(), String> {
    let path = Path::new(raw);
    if raw.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(format!("{field} must be a relative path without '..'"));
    }
    Ok(())
}

fn require_custom_script_enabled() -> Result<(), String> {
    if std::env::var("ARBITRAGEUR_PRICERETRIVER_ENABLE_CUSTOM_SCRIPT")
        .ok()
        .as_deref()
        == Some("1")
    {
        Ok(())
    } else {
        Err(
            "custom PriceRetriver scripts are disabled; set ARBITRAGEUR_PRICERETRIVER_ENABLE_CUSTOM_SCRIPT=1 to opt in"
                .into(),
        )
    }
}

#[tauri::command]
pub async fn priceretriver_status(
    node_url: String,
    token: Option<String>,
) -> Result<Value, String> {
    let client = PriceRetriverClient::new(validated_node_url(&node_url)?, token);
    client.status().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn priceretriver_list_partitions(
    node_url: String,
    token: Option<String>,
) -> Result<Value, String> {
    let client = PriceRetriverClient::new(validated_node_url(&node_url)?, token);
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
    let destination = validated_destination(&dest_path)?;
    let client = PriceRetriverClient::new(validated_node_url(&node_url)?, token);
    client
        .download_archive(venue.as_deref(), symbol.as_deref(), None, destination)
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
    validate_remote_relative_path(&partition_path, "partition_path")?;
    if pipeline_type.trim().is_empty()
        || pipeline_type.len() > 64
        || !pipeline_type
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err("pipeline_type contains invalid characters".into());
    }
    if format.trim().is_empty()
        || format.len() > 32
        || !format
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err("format contains invalid characters".into());
    }
    let client = PriceRetriverClient::new(validated_node_url(&node_url)?, token);
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
    require_custom_script_enabled()?;
    if script_code.is_empty() || script_code.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "script_code must contain 1..={MAX_SCRIPT_BYTES} bytes"
        ));
    }
    if !matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "python" | "sql"
    ) {
        return Err("custom script language must be python or sql".into());
    }
    if let Some(path) = partition_path.as_deref() {
        validate_remote_relative_path(path, "partition_path")?;
    }
    let client = PriceRetriverClient::new(validated_node_url(&node_url)?, token);
    client
        .run_custom_script(&script_code, &language, partition_path.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_url_defaults_to_loopback_only() {
        assert!(validated_node_url("http://127.0.0.1:8080").is_ok());
        assert!(validated_node_url("http://localhost:8080").is_ok());
        assert!(validated_node_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(validated_node_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn destination_rejects_parent_traversal() {
        assert!(validated_destination("../../etc/passwd").is_err());
        let err = validated_destination("/etc/passwd").unwrap_err();
        assert!(err.contains("must be under"));
    }

    #[test]
    fn custom_script_is_opt_in_and_bounded() {
        assert!(require_custom_script_enabled().is_err());
        assert!(!matches!(
            validate_remote_relative_path("../partition", "partition_path"),
            Ok(())
        ));
    }
}
