//! Streaming-media napi methods. Sync fns that build the callback TSFNs (so the
//! `Function` params aren't held across an await) and run the core work via
//! `env.spawn_future`, returning a JS Promise. The TSFNs are `unref`'d.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use bridge_core::host::{HostBodyFactory, HostByteSink, HostPullSource};

use crate::WasmWhatsAppClient;
use crate::host_media::{NapiBodyFactory, NapiByteSink, NapiPullSource};
use crate::methods::napi_err;
use crate::value_js::JsBridgeValue;

fn mt(media_type: String) -> serde_json::Value {
    serde_json::Value::String(media_type)
}

#[napi]
impl WasmWhatsAppClient {
    /// Download + decrypt, streaming the bytes to `onChunk(chunk)` (64 KB chunks).
    #[napi(js_name = "downloadMediaStream")]
    pub fn download_media_stream<'env>(
        &self,
        env: &'env Env,
        direct_path: String,
        media_key: Uint8Array,
        file_sha256: Uint8Array,
        file_enc_sha256: Uint8Array,
        file_length: f64,
        media_type: String,
        on_chunk: Function<Uint8Array, Promise<()>>,
    ) -> Result<PromiseRaw<'env, ()>> {
        let mut tsfn = on_chunk.build_threadsafe_function().build()?;
        tsfn.unref(env)?;
        let sink: Arc<dyn HostByteSink> = Arc::new(NapiByteSink {
            write: Arc::new(tsfn),
        });
        let core = self.core.clone();
        let (dp, mk, fs, fes, mtv) = (
            direct_path,
            media_key.to_vec(),
            file_sha256.to_vec(),
            file_enc_sha256.to_vec(),
            mt(media_type),
        );
        env.spawn_future(async move {
            core.download_media_stream(&dp, &mk, &fs, &fes, file_length, mtv, sink)
                .await
                .map_err(napi_err)
        })
    }

    /// Streaming encrypt: pull plaintext via `pullChunk()`, push ciphertext via
    /// `writeChunk(chunk)`; resolves with the `EncryptMediaResult`.
    #[napi(js_name = "encryptMediaStream")]
    pub fn encrypt_media_stream<'env>(
        &self,
        env: &'env Env,
        pull_chunk: Function<(), Promise<Option<Uint8Array>>>,
        write_chunk: Function<Uint8Array, Promise<()>>,
        media_type: String,
    ) -> Result<PromiseRaw<'env, JsBridgeValue>> {
        let mut pull = pull_chunk.build_threadsafe_function().build()?;
        pull.unref(env)?;
        let mut write = write_chunk.build_threadsafe_function().build()?;
        write.unref(env)?;
        let source: Arc<dyn HostPullSource> = Arc::new(NapiPullSource {
            pull: Arc::new(pull),
        });
        let sink: Arc<dyn HostByteSink> = Arc::new(NapiByteSink {
            write: Arc::new(write),
        });
        let core = self.core.clone();
        let mtv = mt(media_type);
        env.spawn_future(async move {
            core.encrypt_media_stream(source, sink, mtv)
                .await
                .map(JsBridgeValue)
                .map_err(napi_err)
        })
    }

    /// Upload pre-encrypted media; `getBody()` yields a fresh body per attempt
    /// (CDN failover / auth refresh). Resolves with the `UploadMediaResult`.
    #[napi(js_name = "uploadEncryptedMediaStream")]
    pub fn upload_encrypted_media_stream<'env>(
        &self,
        env: &'env Env,
        get_body: Function<(), Promise<Uint8Array>>,
        media_key: Uint8Array,
        file_sha256: Uint8Array,
        file_enc_sha256: Uint8Array,
        file_length: f64,
        media_type: String,
    ) -> Result<PromiseRaw<'env, JsBridgeValue>> {
        let mut tsfn = get_body.build_threadsafe_function().build()?;
        tsfn.unref(env)?;
        let factory: Arc<dyn HostBodyFactory> = Arc::new(NapiBodyFactory {
            get: Arc::new(tsfn),
        });
        let core = self.core.clone();
        let (mk, fs, fes, mtv) = (
            media_key.to_vec(),
            file_sha256.to_vec(),
            file_enc_sha256.to_vec(),
            mt(media_type),
        );
        env.spawn_future(async move {
            core.upload_encrypted_media_stream(factory, &mk, &fs, &fes, file_length, mtv)
                .await
                .map(JsBridgeValue)
                .map_err(napi_err)
        })
    }
}
