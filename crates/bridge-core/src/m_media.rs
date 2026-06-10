//! Streaming media — engine-agnostic ports of `downloadMediaStream`,
//! `encryptMediaStream`, `uploadEncryptedMediaStream`. The Web-Streams the WASM
//! build used are replaced by the `HostByteSink`/`HostPullSource`/
//! `HostBodyFactory` seam (the napi binding drives these over TSFNs). Logic
//! (the `MediaEncryptor` loop + the CDN-failover/resumable/auth-refresh state
//! machine) is reproduced 1:1 from the old `wasm_client.rs`.

use std::sync::Arc;

use crate::client::WhatsAppClientCore;
use crate::errors::{self, BridgeError};
use crate::helpers;
use crate::host::{HostBodyFactory, HostByteSink, HostPullSource};
use crate::methods::snake;
use crate::result_types;
use crate::value::BridgeValue;

const CHUNK_SIZE: usize = 65536;

fn parse_mt(media_type: serde_json::Value) -> Result<wacore::download::MediaType, BridgeError> {
    let mt: result_types::MediaType = serde_json::from_value(media_type)
        .map_err(|e| errors::invalid_arg("mediaType", e.to_string()))?;
    Ok(mt.into())
}

fn sink_err(e: anyhow::Error) -> BridgeError {
    errors::internal(format!("media sink: {e}"))
}

impl WhatsAppClientCore {
    /// Download + decrypt fully (the underlying `download_from_params` buffers),
    /// then stream the bytes out in 64 KB chunks via the sink.
    pub async fn download_media_stream(
        &self,
        direct_path: &str,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        media_type: serde_json::Value,
        sink: Arc<dyn HostByteSink>,
    ) -> Result<(), BridgeError> {
        let mt = parse_mt(media_type)?;
        let params = whatsapp_rust::download::DownloadParams::encrypted(
            direct_path,
            media_key,
            file_sha256,
            file_enc_sha256,
            file_length as u64,
            mt,
        );
        let data = self.client.download_from_params(&params).await?;
        for chunk in data.chunks(CHUNK_SIZE) {
            sink.write(chunk.to_vec()).await.map_err(sink_err)?;
        }
        Ok(())
    }

    /// Streaming AES-256-CBC encrypt: pull plaintext chunks, push ciphertext,
    /// return the `EncryptMediaResult` (camelCase). Peak memory ~one flush buffer.
    pub async fn encrypt_media_stream(
        &self,
        source: Arc<dyn HostPullSource>,
        sink: Arc<dyn HostByteSink>,
        media_type: serde_json::Value,
    ) -> Result<BridgeValue, BridgeError> {
        use wacore::upload::MediaEncryptor;

        let mt = parse_mt(media_type)?;
        let mut enc = MediaEncryptor::new(mt)?;
        let mut out_buf = Vec::with_capacity(CHUNK_SIZE + 16);

        while let Some(chunk) = source.next().await.map_err(sink_err)? {
            if chunk.is_empty() {
                continue;
            }
            enc.update(&chunk, &mut out_buf);
            if out_buf.len() >= CHUNK_SIZE {
                sink.write(std::mem::take(&mut out_buf))
                    .await
                    .map_err(sink_err)?;
                out_buf = Vec::with_capacity(CHUNK_SIZE + 16);
            }
        }

        let info = enc.finalize(&mut out_buf)?;
        if !out_buf.is_empty() {
            sink.write(out_buf).await.map_err(sink_err)?;
        }

        snake(&result_types::EncryptMediaResult {
            media_key: info.media_key.to_vec(),
            file_sha256: info.file_sha256.to_vec(),
            file_enc_sha256: info.file_enc_sha256.to_vec(),
            file_length: info.file_length as f64,
        })
    }

    /// Upload pre-encrypted media with CDN failover, resumable check (≥5 MB) and
    /// auth refresh. `body_factory` yields a fresh full body per attempt.
    pub async fn upload_encrypted_media_stream(
        &self,
        body_factory: Arc<dyn HostBodyFactory>,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        media_type: serde_json::Value,
    ) -> Result<BridgeValue, BridgeError> {
        let mt = parse_mt(media_type)?;
        let file_length = file_length as u64;
        let token = helpers::base64_url_encode(file_enc_sha256);
        let mms_type = mt.mms_type();

        let mut force_refresh = false;

        for attempt in 0..=1u32 {
            let media_conn = self.client.refresh_media_conn(force_refresh).await?;
            let mut retry_auth = false;

            for host in &media_conn.hosts {
                if file_length >= 5 * 1024 * 1024 {
                    let check_url = format!(
                        "https://{}/mms/{}/{}?auth={}&token={}&resume=1",
                        host.hostname, mms_type, token, media_conn.auth, token
                    );
                    let check_req = wacore::net::HttpRequest::post(check_url)
                        .with_header("Origin", "https://web.whatsapp.com");
                    if let Ok(resp) = self.client.http_client.execute(check_req).await
                        && resp.status_code < 400
                        && let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&resp.body)
                        && parsed.get("resume").and_then(|v| v.as_str()) == Some("complete")
                        && let (Some(url), Some(dp)) = (
                            parsed.get("url").and_then(|v| v.as_str()),
                            parsed.get("direct_path").and_then(|v| v.as_str()),
                        )
                    {
                        return upload_result(
                            url,
                            dp,
                            media_key,
                            file_sha256,
                            file_enc_sha256,
                            file_length,
                        );
                    }
                }

                let upload_url = format!(
                    "https://{}/mms/{}/{}?auth={}&token={}",
                    host.hostname, mms_type, token, media_conn.auth, token
                );
                let body = body_factory.body().await.map_err(sink_err)?;
                let req = wacore::net::HttpRequest::post(upload_url)
                    .with_header("Content-Type", "application/octet-stream")
                    .with_header("Origin", "https://web.whatsapp.com")
                    .with_body(body);

                match self.client.http_client.execute(req).await {
                    Ok(resp) if resp.status_code < 400 => {
                        let parsed: serde_json::Value = serde_json::from_slice(&resp.body)
                            .map_err(|e| {
                                errors::protocol_violation(format!(
                                    "CDN upload response not JSON: {e}"
                                ))
                            })?;
                        let url = parsed
                            .get("url")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| errors::internal("missing url in response"))?;
                        let dp = parsed
                            .get("direct_path")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| errors::internal("missing direct_path in response"))?;
                        return upload_result(
                            url,
                            dp,
                            media_key,
                            file_sha256,
                            file_enc_sha256,
                            file_length,
                        );
                    }
                    Ok(resp) if helpers::is_auth_error(resp.status_code) && attempt == 0 => {
                        force_refresh = true;
                        retry_auth = true;
                        break;
                    }
                    Ok(resp) => {
                        log::warn!(
                            "upload to {} failed: status {}",
                            host.hostname,
                            resp.status_code
                        );
                    }
                    Err(e) => {
                        log::warn!("upload to {} failed: {e:?}", host.hostname);
                    }
                }
            }

            if !retry_auth {
                break;
            }
        }

        Err(errors::internal("Upload failed on all hosts"))
    }
}

fn upload_result(
    url: &str,
    direct_path: &str,
    media_key: &[u8],
    file_sha256: &[u8],
    file_enc_sha256: &[u8],
    file_length: u64,
) -> Result<BridgeValue, BridgeError> {
    snake(&result_types::UploadMediaResult {
        url: url.to_string(),
        direct_path: direct_path.to_string(),
        media_key: media_key.try_into()?,
        file_sha256: file_sha256.try_into()?,
        file_enc_sha256: file_enc_sha256.try_into()?,
        file_length: file_length as f64,
    })
}
