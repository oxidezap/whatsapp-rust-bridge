//! `wacore::net::HttpClient` over `HostHttp`. Streaming stays at the trait
//! defaults (`false`) — the JS-backed client buffers, matching the old bridge.

use std::sync::Arc;

use async_trait::async_trait;
use wacore::net::{HttpClient, HttpRequest, HttpResponse};

use crate::host::HostHttp;

pub struct CoreHttpClient {
    host: Arc<dyn HostHttp>,
}

impl CoreHttpClient {
    pub fn new(host: Arc<dyn HostHttp>) -> Self {
        Self { host }
    }
}

#[async_trait]
impl HttpClient for CoreHttpClient {
    async fn execute(&self, request: HttpRequest) -> anyhow::Result<HttpResponse> {
        self.host.execute(request).await
    }
}
