use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::error::{AppError, Result};

const OLLAMA_URL: &str = "http://localhost:11434";

#[derive(Clone)]
pub struct OllamaClient {
    client: Client,
}

impl OllamaClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn models(&self) -> Result<Vec<String>> {
        let url = format!("{}/api/tags", OLLAMA_URL);
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnavailable { source })?;

        if !response.status().is_success() {
            return Err(AppError::OllamaHttp(response.status().as_u16()));
        }

        let tags = response
            .json::<OllamaTags>()
            .await
            .map_err(AppError::OllamaDecode)?;
        Ok(tags.models.into_iter().map(|model| model.name).collect())
    }

    pub async fn version(&self) -> Result<String> {
        let url = format!("{}/api/version", OLLAMA_URL);
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnavailable { source })?;

        if !response.status().is_success() {
            return Err(AppError::OllamaHttp(response.status().as_u16()));
        }

        let version = response
            .json::<OllamaVersion>()
            .await
            .map_err(AppError::OllamaDecode)?;
        Ok(version.version)
    }

    pub async fn generate(
        &self,
        model: &str,
        prompt: &str,
        num_ctx: Option<usize>,
    ) -> Result<String> {
        let url = format!("{}/api/generate", OLLAMA_URL);
        let options = num_ctx.map(|ctx| OllamaOptions { num_ctx: ctx });
        let request = OllamaGenerateRequest {
            model,
            prompt,
            stream: false,
            format: None,
            think: None,
            options,
        };
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnavailable { source })?;

        if !response.status().is_success() {
            return Err(AppError::OllamaHttp(response.status().as_u16()));
        }

        let generated = response
            .json::<OllamaGenerateResponse>()
            .await
            .map_err(AppError::OllamaDecode)?;
        Ok(generated.response)
    }

    pub async fn generate_json(
        &self,
        model: &str,
        prompt: &str,
        num_ctx: Option<usize>,
    ) -> Result<String> {
        let url = format!("{}/api/generate", OLLAMA_URL);
        let options = num_ctx.map(|ctx| OllamaOptions { num_ctx: ctx });
        let request = OllamaGenerateRequest {
            model,
            prompt,
            stream: false,
            format: Some(Value::String("json".to_string())),
            think: Some(false),
            options,
        };
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnavailable { source })?;

        if !response.status().is_success() {
            return Err(AppError::OllamaHttp(response.status().as_u16()));
        }

        let generated = response
            .json::<OllamaGenerateResponse>()
            .await
            .map_err(AppError::OllamaDecode)?;
        Ok(generated.response)
    }

    pub async fn generate_stream(
        &self,
        model: &str,
        prompt: &str,
        num_ctx: Option<usize>,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let url = format!("{}/api/generate", OLLAMA_URL);
        let options = num_ctx.map(|ctx| OllamaOptions { num_ctx: ctx });
        let request = OllamaGenerateRequest {
            model,
            prompt,
            stream: true,
            format: None,
            think: None,
            options,
        };
        let mut response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|source| AppError::OllamaUnavailable { source })?;

        if !response.status().is_success() {
            return Err(AppError::OllamaHttp(response.status().as_u16()));
        }

        let mut full_text = String::new();
        let mut done = false;
        let mut buffer = String::new();
        while let Some(chunk) = response.chunk().await.map_err(AppError::OllamaDecode)? {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(newline) = buffer.find('\n') {
                let line = buffer[..newline].trim().to_string();
                buffer = buffer[newline + 1..].to_string();
                if line.is_empty() {
                    continue;
                }
                let parsed =
                    serde_json::from_str::<OllamaStreamChunk>(&line).map_err(|source| {
                        AppError::InvalidJson {
                            value: line.clone(),
                            source,
                        }
                    })?;
                full_text.push_str(&parsed.response);
                let _ = tx.send(parsed.response);
                if parsed.done {
                    done = true;
                }
            }
        }
        let line = buffer.trim();
        if !line.is_empty() {
            let parsed = serde_json::from_str::<OllamaStreamChunk>(line).map_err(|source| {
                AppError::InvalidJson {
                    value: line.to_string(),
                    source,
                }
            })?;
            full_text.push_str(&parsed.response);
            let _ = tx.send(parsed.response);
            if parsed.done {
                done = true;
            }
        }
        if done {
            Ok(full_text)
        } else {
            Err(AppError::Custom(
                "Ollama stream ended unexpectedly without done=true marker".to_string(),
            ))
        }
    }
}

#[derive(Debug, Deserialize)]
struct OllamaStreamChunk {
    response: String,
    done: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaTags {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct OllamaVersion {
    version: String,
}

#[derive(Debug, Serialize)]
struct OllamaGenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    num_ctx: usize,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
}

pub fn detect_vram() -> Option<usize> {
    // 1. Try nvidia-smi (fastest and standard for CUDA/NVIDIA GPUs)
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        if let Ok(val) = text.trim().parse::<usize>() {
            return Some(val); // VRAM in MB
        }
    }

    // 2. Try Linux DRM (AMD, Intel, etc.) via /sys/class/drm
    #[cfg(target_os = "linux")]
    {
        if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("card") {
                        let vram_path = path.join("device").join("mem_info_vram_total");
                        if let Ok(content) = std::fs::read_to_string(&vram_path) {
                            if let Ok(bytes) = content.trim().parse::<u64>() {
                                let mb = (bytes / (1024 * 1024)) as usize;
                                if mb > 0 {
                                    return Some(mb);
                                }
                            }
                        }
                        // Fallback: try lspci for this card
                        if let Ok(output) = std::process::Command::new("lspci")
                            .args(["-s", &format!("{}:00.0", &name[4..]), "-v"])
                            .output()
                        {
                            let text = String::from_utf8_lossy(&output.stdout);
                            if let Some(line) = text.lines().find(|l| l.contains("Memory at")) {
                                if let Some(mb) = parse_vram_from_lspci(line) {
                                    return Some(mb);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Try PowerShell as fallback on Windows to query video controllers (covers AMD, Intel, etc.)
    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = std::process::Command::new("powershell")
            .args([
                "-Command",
                "Get-CimInstance Win32_VideoController | Measure-Object -Property AdapterRAM -Sum | Select-Object -ExpandProperty Sum",
            ])
            .output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Ok(bytes) = text.trim().parse::<u64>() {
                let mb = (bytes / (1024 * 1024)) as usize;
                if mb > 0 {
                    return Some(mb);
                }
            }
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn parse_vram_from_lspci(line: &str) -> Option<usize> {
    // Example: "Memory at f0000000 (64-bit, prefetchable) [size=8M]"
    let size_start = line.find("[size=")?;
    let size_str = &line[size_start + 6..];
    let size_end = size_str.find(']')?;
    let size_val = &size_str[..size_end];
    let multiplier = if size_val.ends_with('G') {
        1024
    } else if size_val.ends_with('M') {
        1
    } else if size_val.ends_with('K') {
        1 / 1024
    } else {
        1
    };
    let num_str = size_val.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    num_str.parse::<usize>().ok().map(|n| n * multiplier)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parse_vram_from_lspci_m() {
        let line = "Memory at f0000000 (64-bit, prefetchable) [size=8M]";
        assert_eq!(parse_vram_from_lspci(line), Some(8));
    }

    #[test]
    fn parse_vram_from_lspci_g() {
        let line = "Memory at f0000000 (64-bit, prefetchable) [size=8G]";
        assert_eq!(parse_vram_from_lspci(line), Some(8 * 1024));
    }

    #[test]
    fn parse_vram_from_lspci_invalid() {
        let line = "No size here";
        assert_eq!(parse_vram_from_lspci(line), None);
    }
}
