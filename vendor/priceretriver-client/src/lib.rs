use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::Client;
use serde_json::Value;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Client to communicate securely with a remote or local PriceRetriver node
#[derive(Clone)]
pub struct PriceRetriverClient {
    base_url: String,
    token: Option<String>,
    client: Client,
}

impl PriceRetriverClient {
    pub fn new(base_url: impl Into<String>, token: Option<impl Into<String>>) -> Self {
        let mut base = base_url.into();
        if base.ends_with('/') {
            base.pop();
        }
        let tok = token.map(|t| t.into());

        let mut headers = HeaderMap::new();
        if let Some(ref t) = tok {
            let auth_val = format!("Bearer {}", t);
            if let Ok(val) = HeaderValue::from_str(&auth_val) {
                headers.insert(AUTHORIZATION, val);
            }
        }

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .unwrap_or_default();

        Self {
            base_url: base,
            token: tok,
            client,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Check health of PriceRetriver node
    pub async fn health(&self) -> anyhow::Result<Value> {
        let url = format!("{}/health", self.base_url);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }

    /// Get complete system status (collectors, storage, uptime)
    pub async fn status(&self) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/status", self.base_url);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }

    /// List all partition metadata stored on the node
    pub async fn list_partitions(&self) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/partitions", self.base_url);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }

    /// Query historical events matching filter
    pub async fn query_events(
        &self,
        venue: Option<&str>,
        symbol: Option<&str>,
        start_time: Option<&str>,
        end_time: Option<&str>,
        limit: Option<usize>,
    ) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/data/query", self.base_url);
        let mut req = self.client.get(&url);

        if let Some(v) = venue {
            req = req.query(&[("venue", v)]);
        }
        if let Some(s) = symbol {
            req = req.query(&[("symbol", s)]);
        }
        if let Some(st) = start_time {
            req = req.query(&[("start_time", st)]);
        }
        if let Some(et) = end_time {
            req = req.query(&[("end_time", et)]);
        }
        if let Some(l) = limit {
            req = req.query(&[("limit", l.to_string())]);
        }

        let resp = req.send().await?.error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }

    /// Get recent events from ring buffer
    pub async fn get_recent_events(&self) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/data/recent", self.base_url);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }

    /// Download a single partition file and write to local destination
    pub async fn download_file<P: AsRef<Path>>(
        &self,
        remote_path: &str,
        local_dest: P,
    ) -> anyhow::Result<u64> {
        let url = format!("{}/api/v1/data/file", self.base_url);
        let resp = self
            .client
            .get(&url)
            .query(&[("path", remote_path)])
            .send()
            .await?
            .error_for_status()?;

        let bytes = resp.bytes().await?;
        if let Some(parent) = local_dest.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&local_dest)?;
        file.write_all(&bytes)?;
        Ok(bytes.len() as u64)
    }

    /// Download multiple partitions as a compressed .tar.gz archive
    pub async fn download_archive<P: AsRef<Path>>(
        &self,
        venue: Option<&str>,
        symbol: Option<&str>,
        date: Option<&str>,
        local_dest: P,
    ) -> anyhow::Result<u64> {
        let url = format!("{}/api/v1/data/download", self.base_url);
        let mut req = self.client.get(&url);

        if let Some(v) = venue {
            req = req.query(&[("venue", v)]);
        }
        if let Some(s) = symbol {
            req = req.query(&[("symbol", s)]);
        }
        if let Some(d) = date {
            req = req.query(&[("date", d)]);
        }

        let resp = req.send().await?.error_for_status()?;
        let bytes = resp.bytes().await?;

        if let Some(parent) = local_dest.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&local_dest)?;
        file.write_all(&bytes)?;
        Ok(bytes.len() as u64)
    }

    /// Trigger remote pipeline on a partition
    pub async fn run_pipeline(
        &self,
        partition_path: &str,
        pipeline_type: &str,
        format: &str,
    ) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/pipeline/process", self.base_url);
        let body = serde_json::json!({
            "partition_path": partition_path,
            "pipeline_type": pipeline_type,
            "format": format
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }

    /// Trigger remote custom script execution
    pub async fn run_custom_script(
        &self,
        script_code: &str,
        language: &str,
        partition_path: Option<&str>,
    ) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/pipeline/custom-script", self.base_url);
        let body = serde_json::json!({
            "script_code": script_code,
            "language": language,
            "partition_path": partition_path
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }

    /// Start a collector on the remote node
    pub async fn start_venue(&self, venue: &str) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/collectors/{}/start", self.base_url, venue);
        let resp = self.client.post(&url).send().await?.error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }

    /// Stop a collector on the remote node
    pub async fn stop_venue(&self, venue: &str) -> anyhow::Result<Value> {
        let url = format!("{}/api/v1/collectors/{}/stop", self.base_url, venue);
        let resp = self.client.post(&url).send().await?.error_for_status()?;
        let val: Value = resp.json().await?;
        Ok(val)
    }
}
