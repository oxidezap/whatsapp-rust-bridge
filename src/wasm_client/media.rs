//! Media download, upload and reupload.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Media ────────────────────────────────────────────────────────────

    /// Get media connection info (auth token + upload hosts).
    ///
    /// Returns `{ auth: string, ttl: number, hosts: [{hostname: string}] }`.
    /// The core's `hosts` carry nothing but the hostname, so neither does this.
    #[wasm_bindgen(js_name = getMediaConn)]
    pub async fn get_media_conn(
        &self,
        force: bool,
    ) -> Result<crate::result_types::MediaConnResult, crate::errors::BridgeError> {
        let conn = self.client.refresh_media_conn(force).await?;

        Ok(crate::result_types::MediaConnResult {
            auth: conn.auth.clone(),
            ttl: conn.ttl as f64,
            hosts: conn
                .hosts
                .iter()
                .map(|h| crate::result_types::MediaHost {
                    hostname: h.hostname.clone(),
                })
                .collect(),
        })
    }

    /// Download and decrypt media from raw parameters.
    ///
    /// Handles CDN failover, auth refresh, HMAC-SHA256 verification, and
    /// AES-256-CBC decryption internally. Returns decrypted media bytes.
    #[wasm_bindgen(js_name = downloadMedia)]
    pub async fn download_media(
        &self,
        direct_path: &str,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
    ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError> {
        let media_type = from_js_input::<crate::result_types::MediaType>("media_type", media_type)?;
        let mt: wacore::download::MediaType = media_type.into();
        let data = self
            .client
            .download_from_params(&whatsapp_rust::download::DownloadParams::encrypted(
                direct_path,
                media_key,
                file_sha256,
                file_enc_sha256,
                file_length as u64,
                mt,
            ))
            .await?;
        Ok(js_sys::Uint8Array::from(&data[..]))
    }

    /// Download, decrypt, and return a Web ReadableStream of decrypted chunks.
    ///
    /// Same as `downloadMedia`, delivered in 64 KB chunks. Neither side is
    /// bounded by that: the core has no streaming download, so
    /// `download_from_params` still resolves the whole plaintext into one
    /// `Vec<u8>` before chunking starts, and on the JS side only the queued
    /// chunks are bounded — a consumer that keeps them, to rebuild the file,
    /// ends up holding all of it. In Node.js, consume with
    /// `Readable.fromWeb(stream)`.
    #[wasm_bindgen(js_name = downloadMediaStream)]
    pub fn download_media_stream(
        &self,
        direct_path: &str,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
    ) -> Result<web_sys::ReadableStream, crate::errors::BridgeError> {
        let media_type = from_js_input::<crate::result_types::MediaType>("media_type", media_type)?;
        let mt: wacore::download::MediaType = media_type.into();
        let client = self.client.clone();
        let direct_path = direct_path.to_string();
        let media_key = media_key.to_vec();
        let file_sha256 = file_sha256.to_vec();
        let file_enc_sha256 = file_enc_sha256.to_vec();
        let file_length = file_length as u64;

        // Channel with backpressure (capacity 2 keeps memory bounded)
        let (mut tx, rx) = futures::channel::mpsc::channel::<Result<JsValue, JsValue>>(2);

        wasm_bindgen_futures::spawn_local(async move {
            use futures::SinkExt;

            match client
                .download_from_params(&whatsapp_rust::download::DownloadParams::encrypted(
                    direct_path.as_str(),
                    &media_key,
                    &file_sha256,
                    &file_enc_sha256,
                    file_length,
                    mt,
                ))
                .await
            {
                Ok(data) => {
                    // Stream in 64KB chunks to avoid holding the full buffer in JS
                    const CHUNK_SIZE: usize = 65536;
                    for chunk in data.chunks(CHUNK_SIZE) {
                        let js_chunk = js_sys::Uint8Array::from(chunk);
                        if tx.send(Ok(js_chunk.into())).await.is_err() {
                            break; // Consumer cancelled the stream
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(JsValue::from_str(&e.to_string()))).await;
                }
            }
            // tx dropped here → stream ends
        });

        let readable = wasm_streams::ReadableStream::from_stream(rx);
        Ok(readable.into_raw())
    }

    // ── Upload ────────────────────────────────────────────────────────────

    /// Upload media: encrypt in memory + upload with CDN failover and retry.
    ///
    /// Takes raw plaintext bytes. Handles AES-256-CBC encryption, HMAC-SHA256
    /// signing, multi-host CDN upload, auth refresh, and resumable upload (>=5MB).
    #[wasm_bindgen(js_name = uploadMedia)]
    pub async fn upload_media(
        &self,
        data: &[u8],
        #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
    ) -> Result<crate::result_types::UploadMediaResult, crate::errors::BridgeError> {
        let media_type = from_js_input::<crate::result_types::MediaType>("media_type", media_type)?;
        let mt: wacore::download::MediaType = media_type.into();
        let resp = self
            .client
            .upload(data.to_vec(), mt, Default::default())
            .await?;
        Ok(crate::result_types::UploadMediaResult {
            url: resp.url,
            direct_path: resp.direct_path,
            media_key: resp.media_key,
            file_sha256: resp.file_sha256,
            file_enc_sha256: resp.file_enc_sha256,
            file_length: resp.file_length as f64,
        })
    }

    /// True streaming encrypt via `MediaEncryptor`: processes plaintext chunk-by-chunk
    /// from JS ReadableStream, encrypts with AES-256-CBC, writes ciphertext to JS WritableStream.
    ///
    /// Peak memory: ~130KB (copy buffer + flush buffer + crypto state).
    #[wasm_bindgen(js_name = encryptMediaStream)]
    pub async fn encrypt_media_stream(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ReadableStream")] input: JsValue,
        #[wasm_bindgen(unchecked_param_type = "WritableStream")] output: JsValue,
        #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
    ) -> Result<crate::result_types::EncryptMediaResult, crate::errors::BridgeError> {
        let input = from_js_class::<web_sys::ReadableStream>(
            "input",
            "ReadableStream",
            "getReader",
            input,
        )?;
        let output = from_js_class::<web_sys::WritableStream>(
            "output",
            "WritableStream",
            "getWriter",
            output,
        )?;
        let media_type = from_js_input::<crate::result_types::MediaType>("media_type", media_type)?;
        use futures::SinkExt;
        use futures::StreamExt;
        use wacore::upload::MediaEncryptor;

        let mt: wacore::download::MediaType = media_type.into();

        let rs = wasm_streams::ReadableStream::from_raw(input);
        let mut reader = rs.into_stream();
        let ws = wasm_streams::WritableStream::from_raw(output);
        let mut writer = ws.into_sink();

        const FLUSH_THRESHOLD: usize = 65536;

        let mut enc = MediaEncryptor::new(mt)?;
        let mut out_buf = Vec::with_capacity(FLUSH_THRESHOLD + 16);
        let mut copy_buf = vec![0u8; FLUSH_THRESHOLD];

        while let Some(chunk_result) = reader.next().await {
            let chunk =
                chunk_result.map_err(|e| crate::errors::internal(format!("read error: {e:?}")))?;
            let arr = js_sys::Uint8Array::new(&chunk);
            let len = arr.length() as usize;
            if len == 0 {
                continue;
            }

            if len > copy_buf.len() {
                copy_buf.resize(len, 0);
            }
            arr.copy_to(&mut copy_buf[..len]);

            enc.update(&copy_buf[..len], &mut out_buf);

            if out_buf.len() >= FLUSH_THRESHOLD {
                let js_chunk = js_sys::Uint8Array::from(out_buf.as_slice());
                writer
                    .send(js_chunk.into())
                    .await
                    .map_err(|e| crate::errors::internal(format!("write error: {e:?}")))?;
                out_buf.clear();
            }
        }

        let info = enc.finalize(&mut out_buf)?;

        if !out_buf.is_empty() {
            let js_chunk = js_sys::Uint8Array::from(out_buf.as_slice());
            writer
                .send(js_chunk.into())
                .await
                .map_err(|e| crate::errors::internal(format!("write error: {e:?}")))?;
        }
        writer
            .close()
            .await
            .map_err(|e| crate::errors::internal(format!("close error: {e:?}")))?;

        Ok(crate::result_types::EncryptMediaResult {
            media_key: info.media_key.to_vec(),
            file_sha256: info.file_sha256.to_vec(),
            file_enc_sha256: info.file_enc_sha256.to_vec(),
            file_length: info.file_length as f64,
        })
    }

    /// Upload pre-encrypted media with streaming body.
    ///
    /// `get_body` is a JS function `() => ReadableStream<Uint8Array>` — called
    /// for each upload attempt (retry creates a fresh stream).
    /// Handles CDN failover, auth refresh, and resumable upload (>=5MB).
    #[wasm_bindgen(js_name = uploadEncryptedMediaStream)]
    pub async fn upload_encrypted_media_stream(
        &self,
        get_body: &js_sys::Function,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
    ) -> Result<crate::result_types::UploadMediaResult, crate::errors::BridgeError> {
        let media_type = from_js_input::<crate::result_types::MediaType>("media_type", media_type)?;
        let mt: wacore::download::MediaType = media_type.into();
        let file_length = file_length as u64;
        let token = base64_url_encode(file_enc_sha256);
        let mms_type = mt.mms_type();

        let mut force_refresh = false;

        for attempt in 0..=1u32 {
            let media_conn = self.client.refresh_media_conn(force_refresh).await?;

            let mut retry_auth = false;

            for host in &media_conn.hosts {
                // Resumable check for large files (≥5MB)
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
                        return Ok(crate::result_types::UploadMediaResult {
                            url: url.to_string(),
                            direct_path: dp.to_string(),
                            media_key: media_key.try_into()?,
                            file_sha256: file_sha256.try_into()?,
                            file_enc_sha256: file_enc_sha256.try_into()?,
                            file_length: file_length as f64,
                        });
                    }
                }

                let upload_url = format!(
                    "https://{}/mms/{}/{}?auth={}&token={}",
                    host.hostname, mms_type, token, media_conn.auth, token
                );

                // Get fresh ReadableStream from factory
                let body_stream = get_body
                    .call0(&JsValue::NULL)
                    .map_err(|e| crate::errors::internal(format!("getBody() failed: {e:?}")))?;

                // Try streaming upload via JS HTTP client
                let result = stream_upload_via_js(&self.client, &upload_url, body_stream).await;

                match result {
                    Ok(resp) if resp.status_code < 400 => {
                        let parsed: serde_json::Value = serde_json::from_slice(&resp.body)
                            .map_err(|e| {
                                crate::errors::protocol_violation(format!(
                                    "CDN upload response not JSON: {e}"
                                ))
                            })?;
                        let url = parsed
                            .get("url")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| crate::errors::internal("missing url in response"))?;
                        let dp = parsed
                            .get("direct_path")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                crate::errors::internal("missing direct_path in response")
                            })?;
                        return Ok(crate::result_types::UploadMediaResult {
                            url: url.to_string(),
                            direct_path: dp.to_string(),
                            media_key: media_key.try_into()?,
                            file_sha256: file_sha256.try_into()?,
                            file_enc_sha256: file_enc_sha256.try_into()?,
                            file_length: file_length as f64,
                        });
                    }
                    Ok(resp) if is_auth_error(resp.status_code) && attempt == 0 => {
                        force_refresh = true;
                        retry_auth = true;
                        break;
                    }
                    Ok(resp) => {
                        log::warn!(
                            "Upload to {} failed with status {}",
                            host.hostname,
                            resp.status_code
                        );
                    }
                    Err(e) => {
                        log::warn!("Upload to {} failed: {:?}", host.hostname, e);
                    }
                }
            }

            if !retry_auth {
                break;
            }
        }

        Err(crate::errors::internal("Upload failed on all hosts"))
    }

    // ── Media reupload ────────────────────────────────────────────────────

    /// Request the server to re-upload expired media.
    ///
    /// Returns the new `directPath` on success.
    /// Throws on failure (not found, decryption error, timeout, etc.).
    #[wasm_bindgen(js_name = requestMediaReupload)]
    pub async fn request_media_reupload(
        &self,
        msg_id: &str,
        chat_jid: &str,
        media_key: &[u8],
        is_from_me: bool,
        participant: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;

        let participant_jid = participant.as_deref().map(parse_jid).transpose()?;

        let req = whatsapp_rust::MediaReuploadRequest {
            msg_id,
            chat_jid: &chat,
            media_key,
            is_from_me,
            participant: participant_jid.as_ref(),
        };

        let result = self.client.media_reupload().request(&req).await?;

        match result {
            whatsapp_rust::MediaRetryResult::Success { direct_path } => Ok(direct_path),
            whatsapp_rust::MediaRetryResult::NotFound => {
                Err(crate::errors::internal("Media not found on server"))
            }
            whatsapp_rust::MediaRetryResult::DecryptionError => {
                Err(crate::errors::internal("Media decryption error"))
            }
            whatsapp_rust::MediaRetryResult::GeneralError => {
                Err(crate::errors::internal("Media reupload failed"))
            }
        }
    }
}
