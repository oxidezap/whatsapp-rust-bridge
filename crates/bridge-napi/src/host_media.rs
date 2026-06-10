//! `HostByteSink`/`HostPullSource`/`HostBodyFactory` over JS callbacks
//! (CalleeHandled=false TSFNs), driving the streaming-media core methods.

use std::sync::Arc;

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;

use bridge_core::host::{HostBodyFactory, HostByteSink, HostPullSource};

type Tsfn<I, O> = ThreadsafeFunction<I, Promise<O>, I, Status, false>;

/// `writeChunk(chunk: Uint8Array) => Promise<void>`
pub type WriteTsfn = Tsfn<Uint8Array, ()>;
/// `pullChunk() => Promise<Uint8Array | null>`
pub type PullTsfn = Tsfn<(), Option<Uint8Array>>;
/// `getBody() => Promise<Uint8Array>`
pub type BodyTsfn = Tsfn<(), Uint8Array>;

fn anyhow_err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

pub struct NapiByteSink {
    pub write: Arc<WriteTsfn>,
}

#[async_trait]
impl HostByteSink for NapiByteSink {
    async fn write(&self, chunk: Vec<u8>) -> anyhow::Result<()> {
        let p = self
            .write
            .call_async(Uint8Array::from(chunk))
            .await
            .map_err(anyhow_err)?;
        p.await.map_err(anyhow_err)?;
        Ok(())
    }
}

pub struct NapiPullSource {
    pub pull: Arc<PullTsfn>,
}

#[async_trait]
impl HostPullSource for NapiPullSource {
    async fn next(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let p = self.pull.call_async(()).await.map_err(anyhow_err)?;
        Ok(p.await.map_err(anyhow_err)?.map(|u| u.to_vec()))
    }
}

pub struct NapiBodyFactory {
    pub get: Arc<BodyTsfn>,
}

#[async_trait]
impl HostBodyFactory for NapiBodyFactory {
    async fn body(&self) -> anyhow::Result<Vec<u8>> {
        let p = self.get.call_async(()).await.map_err(anyhow_err)?;
        Ok(p.await.map_err(anyhow_err)?.to_vec())
    }
}
