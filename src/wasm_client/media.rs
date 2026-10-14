//! Media download, upload and reupload.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

fn download_stream(
    client: Arc<whatsapp_rust::Client>,
    direct_path: String,
    media_key: Vec<u8>,
    file_sha256: Vec<u8>,
    file_enc_sha256: Vec<u8>,
    file_length: u64,
    mt: wacore::download::MediaType,
) -> Result<web_sys::ReadableStream, crate::errors::BridgeError> {
    let params = whatsapp_rust::download::DownloadParams::encrypted(
        direct_path,
        &media_key,
        &file_sha256,
        &file_enc_sha256,
        file_length,
        mt,
    )
    .map_err(crate::errors::download_preparation_error)?;
    Ok(buffered_download_stream(async move {
        client.download(&params).await
    }))
}

fn buffered_download_stream<F, E>(download: F) -> web_sys::ReadableStream
where
    F: std::future::Future<Output = Result<Vec<u8>, E>> + 'static,
    E: std::fmt::Display + 'static,
{
    let (mut tx, rx) = futures::channel::mpsc::channel::<Result<JsValue, JsValue>>(2);
    wasm_bindgen_futures::spawn_local(async move {
        use futures::SinkExt;
        match download.await {
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
    readable.into_raw()
}

async fn encrypt_stream(
    input: web_sys::ReadableStream,
    output: web_sys::WritableStream,
    mt: wacore::download::MediaType,
) -> Result<Ts<crate::result_types::EncryptMediaResult>, crate::errors::BridgeError> {
    use futures::SinkExt;
    use futures::StreamExt;
    use wacore::upload::MediaEncryptor;

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

    to_ts(crate::result_types::EncryptMediaResult {
        media_key: info.media_key.to_vec(),
        file_sha256: info.file_sha256.to_vec(),
        file_enc_sha256: info.file_enc_sha256.to_vec(),
        file_length: info.file_length as f64,
    })
}

async fn upload_encrypted_stream(
    core: &CoreClient,
    get_body: &js_sys::Function,
    media_key: &[u8],
    file_sha256: &[u8],
    file_enc_sha256: &[u8],
    file_length: f64,
    mt: wacore::download::MediaType,
) -> Result<Ts<crate::result_types::UploadMediaResult>, crate::errors::BridgeError> {
    let file_length = file_length as u64;
    let token = base64_url_encode(file_enc_sha256);
    let mms_type = mt.mms_type();

    let mut force_refresh = false;

    for attempt in 0..=1u32 {
        // The cache decides whether the socket is touched at all, and the
        // HTTP calls below never touch it — except on the retry, where
        // `force_refresh` bypasses the cache and the IQ is certain. Still
        // withdrawable there: the retry is reached only after the CDN
        // refused an attempt, so nothing that attempt did stands.
        let client = if force_refresh {
            core.online().await?
        } else {
            core.unwaited(Unwaited::Cached)
        };
        let media_conn = client.refresh_media_conn(force_refresh).await?;

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
                if let Ok(resp) = client.http_client().execute(check_req).await
                    && resp.status_code < 400
                    && let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&resp.body)
                    && parsed.get("resume").and_then(|v| v.as_str()) == Some("complete")
                    && let (Some(url), Some(dp)) = (
                        parsed.get("url").and_then(|v| v.as_str()),
                        parsed.get("direct_path").and_then(|v| v.as_str()),
                    )
                {
                    return to_ts(crate::result_types::UploadMediaResult {
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
            let result = stream_upload_via_js(client, &upload_url, body_stream).await;

            match result {
                Ok(resp) if resp.status_code < 400 => {
                    let parsed: serde_json::Value =
                        serde_json::from_slice(&resp.body).map_err(|e| {
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
                    return to_ts(crate::result_types::UploadMediaResult {
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

include!("generated/media.rs");

#[cfg(test)]
mod stream_adapter_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    async fn consume(stream: web_sys::ReadableStream, body: &str) -> JsValue {
        let result = js_sys::Function::new_with_args("stream", body)
            .call1(&JsValue::NULL, &stream)
            .unwrap();
        wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(result))
            .await
            .unwrap()
    }

    #[test]
    async fn buffered_download_keeps_chunks_and_consumer_cancellation() {
        let stream = buffered_download_stream(async { Ok::<_, String>(vec![7u8; 2 * 65536 + 3]) });
        let result = consume(
            stream,
            r#"return (async () => {
            const lengths = []; const reader = stream.getReader();
            for (;;) { const {done, value} = await reader.read(); if (done) break;
                if (!(value instanceof Uint8Array) || value[0] !== 7) throw new Error('chunk');
                lengths.push(value.length); }
            return lengths;
        })();"#,
        )
        .await;
        let lengths: Vec<u32> = serde_wasm_bindgen::from_value(result).unwrap();
        assert_eq!(lengths, [65536, 65536, 3]);
        let stream = buffered_download_stream(async { Ok::<_, String>(vec![9u8; 16 * 65536]) });
        let result = consume(
            stream,
            r#"return (async () => {
            const reader = stream.getReader(); const first = await reader.read();
            await reader.cancel('host cancelled');
            const last = await reader.read(); return [first.value.length, last.done];
        })();"#,
        )
        .await;
        let result = js_sys::Array::from(&result);
        assert_eq!(result.get(0).as_f64(), Some(65536.0));
        assert_eq!(result.get(1).as_bool(), Some(true));
    }

    #[test]
    async fn buffered_download_keeps_the_existing_late_string_error() {
        let stream = buffered_download_stream(async { Err::<Vec<u8>, _>("download failed") });
        let result = consume(
            stream,
            r#"return stream.getReader().read().then(
            () => { throw new Error('expected stream error'); }, error => [typeof error, error]);"#,
        )
        .await;
        let result: Vec<String> = serde_wasm_bindgen::from_value(result).unwrap();
        assert_eq!(result, ["string", "download failed"]);
    }
}
