use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![Operation {
        attributes: attributes(
            quote!(/// Get media connection info (auth token + upload hosts).
    ///
    /// Returns `{ auth: string, ttl: number, hosts: [{hostname: string}] }`.
    /// The core's `hosts` carry nothing but the hostname, so neither does this.
    #[wasm_bindgen(js_name = getMediaConn)]),
        ),
        signature: parse_quote!(async fn get_media_conn(
        &self,
        force: bool,
    ) -> Result<Ts<crate::result_types::MediaConnResult>, crate::errors::BridgeError>),
        prepare: parse_quote!({}),
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
    }]
}
