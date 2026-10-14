use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Get media connection info (auth token + upload hosts).
                ///
                /// Returns `{ auth: string, ttl: number, hosts: [{hostname: string}] }`.
                /// The core's `hosts` carry nothing but the hostname, so neither does this.
                #[wasm_bindgen(js_name = getMediaConn)]
            )),
            signature: parse_quote!(
                async fn get_media_conn(
                    &self,
                    force: bool,
                ) -> Result<Ts<crate::result_types::MediaConnResult>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![],
            admission: Admission::OnlineIf {
                condition: parse_quote!(force),
                otherwise: Unwaited::Cached,
            },
            call: parse_quote!(client.refresh_media_conn(force)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(conn),
                expression: parse_quote!(to_ts(crate::result_types::MediaConnResult {
                    auth: conn.auth.clone(),
                    ttl: conn.ttl as f64,
                    hosts: conn
                        .hosts
                        .iter()
                        .map(|h| crate::result_types::MediaHost {
                            hostname: h.hostname.clone(),
                        })
                        .collect(),
                })),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Download and decrypt media from raw parameters.
                ///
                /// Handles CDN failover, auth refresh, HMAC-SHA256 verification, and
                /// AES-256-CBC decryption internally. Returns decrypted media bytes.
                #[wasm_bindgen(js_name = downloadMedia)]
            )),
            signature: parse_quote!(
                async fn download_media(
                    &self,
                    direct_path: &str,
                    media_key: &[u8],
                    file_sha256: &[u8],
                    file_enc_sha256: &[u8],
                    file_length: f64,
                    #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
                ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(media_type),
                    ty: parse_quote!(crate::result_types::MediaType),
                    field: parse_quote!("media_type"),
                    value: parse_quote!(media_type),
                },
                Input::Convert(parse_quote!({
                    let mt: wacore::download::MediaType = media_type.into();
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Cached),
            call: parse_quote!(client.download(
                &whatsapp_rust::download::DownloadParams::encrypted(
                    direct_path,
                    media_key,
                    file_sha256,
                    file_enc_sha256,
                    file_length as u64,
                    mt,
                )
            )),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(data),
                body: parse_quote!({ Ok(js_sys::Uint8Array::from(&data[..])) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Upload media: encrypt in memory + upload with CDN failover and retry.
                ///
                /// Takes raw plaintext bytes. Handles AES-256-CBC encryption, HMAC-SHA256
                /// signing, multi-host CDN upload, auth refresh, and resumable upload (>=5MB).
                #[wasm_bindgen(js_name = uploadMedia)]
            )),
            signature: parse_quote!(
                async fn upload_media(
                    &self,
                    data: &[u8],
                    #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
                ) -> Result<Ts<crate::result_types::UploadMediaResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(media_type),
                    ty: parse_quote!(crate::result_types::MediaType),
                    field: parse_quote!("media_type"),
                    value: parse_quote!(media_type),
                },
                Input::Convert(parse_quote!({
                    let mt: wacore::download::MediaType = media_type.into();
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Cached),
            call: parse_quote!(client.upload(data.to_vec(), mt, Default::default())),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(resp),
                body: parse_quote!({
                    to_ts(crate::result_types::UploadMediaResult {
                        url: resp.url,
                        direct_path: resp.direct_path,
                        media_key: resp.media_key,
                        file_sha256: resp.file_sha256,
                        file_enc_sha256: resp.file_enc_sha256,
                        file_length: resp.file_length as f64,
                    })
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Request the server to re-upload expired media.
                ///
                /// Returns the new `directPath` on success.
                /// Throws on failure (not found, decryption error, timeout, etc.).
                #[wasm_bindgen(js_name = requestMediaReupload)]
            )),
            signature: parse_quote!(
                async fn request_media_reupload(
                    &self,
                    msg_id: &str,
                    chat_jid: &str,
                    media_key: &[u8],
                    is_from_me: bool,
                    participant: Option<String>,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(chat),
                    value: parse_quote!(chat_jid),
                },
                Input::Convert(parse_quote!({
                    let participant_jid = participant.as_deref().map(parse_jid).transpose()?;
                    let req = whatsapp_rust::MediaReuploadRequest {
                        msg_id,
                        chat_jid: &chat,
                        media_key,
                        is_from_me,
                        participant: participant_jid.as_ref(),
                    };
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.media_reupload().request(&req)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({
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
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Download, decrypt, and return a Web ReadableStream of decrypted chunks.
                ///
                /// Same as `downloadMedia`, delivered in 64 KB chunks. Neither side is
                /// bounded by that: the core has no streaming download, so
                /// `download` still resolves the whole plaintext into one
                /// `Vec<u8>` before chunking starts, and on the JS side only the queued
                /// chunks are bounded — a consumer that keeps them, to rebuild the file,
                /// ends up holding all of it. In Node.js, consume with
                /// `Readable.fromWeb(stream)`.
                #[wasm_bindgen(js_name = downloadMediaStream)]
            )),
            signature: parse_quote!(
                fn download_media_stream(
                    &self,
                    direct_path: &str,
                    media_key: &[u8],
                    file_sha256: &[u8],
                    file_enc_sha256: &[u8],
                    file_length: f64,
                    #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
                ) -> Result<web_sys::ReadableStream, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(media_type),
                    ty: parse_quote!(crate::result_types::MediaType),
                    field: parse_quote!("media_type"),
                    value: parse_quote!(media_type),
                },
                Input::Convert(parse_quote!({
                    let mt: wacore::download::MediaType = media_type.into();

                    let direct_path = direct_path.to_string();
                    let media_key = media_key.to_vec();
                    let file_sha256 = file_sha256.to_vec();
                    let file_enc_sha256 = file_enc_sha256.to_vec();
                    let file_length = file_length as u64;
                })),
            ],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::Deferred),
            call: parse_quote!(download_stream(
                client.clone(),
                direct_path,
                media_key,
                file_sha256,
                file_enc_sha256,
                file_length,
                mt
            )),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// True streaming encrypt via `MediaEncryptor`: processes plaintext chunk-by-chunk
                /// from JS ReadableStream, encrypts with AES-256-CBC, writes ciphertext to JS WritableStream.
                ///
                /// Peak memory: ~130KB (copy buffer + flush buffer + crypto state).
                #[wasm_bindgen(js_name = encryptMediaStream)]
            )),
            signature: parse_quote!(
                async fn encrypt_media_stream(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "ReadableStream")] input: JsValue,
                    #[wasm_bindgen(unchecked_param_type = "WritableStream")] output: JsValue,
                    #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
                ) -> Result<Ts<crate::result_types::EncryptMediaResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Class {
                    binding: parse_quote!(input),
                    ty: parse_quote!(web_sys::ReadableStream),
                    field: parse_quote!("input"),
                    class: parse_quote!("ReadableStream"),
                    method: parse_quote!("getReader"),
                    value: parse_quote!(input),
                },
                Input::Class {
                    binding: parse_quote!(output),
                    ty: parse_quote!(web_sys::WritableStream),
                    field: parse_quote!("output"),
                    class: parse_quote!("WritableStream"),
                    method: parse_quote!("getWriter"),
                    value: parse_quote!(output),
                },
                Input::Decode {
                    binding: parse_quote!(media_type),
                    ty: parse_quote!(crate::result_types::MediaType),
                    field: parse_quote!("media_type"),
                    value: parse_quote!(media_type),
                },
                Input::Convert(parse_quote!({
                    let mt: wacore::download::MediaType = media_type.into();
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Adapter(AdmissionAdapter::StreamEncryption),
            call: parse_quote!(encrypt_stream(input, output, mt)),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Upload pre-encrypted media with streaming body.
                ///
                /// `get_body` is a JS function `() => ReadableStream<Uint8Array>` — called
                /// for each upload attempt (retry creates a fresh stream).
                /// Handles CDN failover, auth refresh, and resumable upload (>=5MB).
                #[wasm_bindgen(js_name = uploadEncryptedMediaStream)]
            )),
            signature: parse_quote!(
                async fn upload_encrypted_media_stream(
                    &self,
                    get_body: &js_sys::Function,
                    media_key: &[u8],
                    file_sha256: &[u8],
                    file_enc_sha256: &[u8],
                    file_length: f64,
                    #[wasm_bindgen(unchecked_param_type = "MediaType")] media_type: JsValue,
                ) -> Result<Ts<crate::result_types::UploadMediaResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(media_type),
                    ty: parse_quote!(crate::result_types::MediaType),
                    field: parse_quote!("media_type"),
                    value: parse_quote!(media_type),
                },
                Input::Convert(parse_quote!({
                    let mt: wacore::download::MediaType = media_type.into();
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Adapter(AdmissionAdapter::StreamingRetry),
            call: parse_quote!(upload_encrypted_stream(
                &self.client,
                get_body,
                media_key,
                file_sha256,
                file_enc_sha256,
                file_length,
                mt
            )),
            output: ReturnAdapter::Direct,
        },
    ]
}
