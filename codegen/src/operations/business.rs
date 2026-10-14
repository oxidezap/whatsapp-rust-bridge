use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(/// Get business profile information for a JID.
    #[wasm_bindgen(js_name = getBusinessProfile)])),
            signature: parse_quote!(async fn get_business_profile(
        &self,
        jid: &str,
    ) -> Result<Option<Ts<crate::result_types::BusinessProfileResult>>, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let target_jid = parse_jid(jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.get_business_profile(&target_jid)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(profile),
                expression: parse_quote!(to_ts_opt(
                    profile.map(|p| business_profile_to_result(&p))
                )),
            },
        },
        Operation {
            attributes: attributes(quote!(/// Look up an order's line items and totals.
    ///
    /// Both `orderId` and `token` come from the order message itself; the token
    /// is a per-order capability, so an order cannot be read without it.
    #[wasm_bindgen(js_name = getOrder)])),
            signature: parse_quote!(async fn get_order(
        &self,
        jid: &str,
        order_id: &str,
        token: &str,
    ) -> Result<Ts<crate::result_types::OrderResult>, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let target = parse_jid(jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.business().get_order(&target, order_id, token)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(order),
                expression: parse_quote!(to_ts(crate::result_types::OrderResult {
                    products: order
                        .products
                        .iter()
                        .map(|product| crate::result_types::OrderProductResult {
                            id: product.id.clone(),
                            name: product.name.clone(),
                            price: product.price.as_ref().map(price_to_result),
                            quantity: product.quantity.map(|q| q as f64),
                            images: product.images.iter().map(product_image_to_result).collect(),
                            variant_properties: product
                                .variant_properties
                                .iter()
                                .map(|v| crate::result_types::VariantPropertyResult {
                                    name: v.name.clone(),
                                    value: v.value.clone(),
                                })
                                .collect(),
                        })
                        .collect(),
                    price_details: order.price_details.as_ref().map(|details| {
                        crate::result_types::OrderPriceDetailsResult {
                            currency: details.currency.clone(),
                            subtotal: details.subtotal.as_ref().map(price_to_result),
                            total: details.total.as_ref().map(price_to_result),
                        }
                    }),
                    creation_timestamp: order.creation_timestamp.map(|t| t as f64),
                })),
            },
        },
        Operation {
            attributes: attributes(
                quote!(/// Remove the business profile's cover photo, by the `fbid` it was set with.
    #[wasm_bindgen(js_name = removeBusinessCoverPhoto)]),
            ),
            signature: parse_quote!(async fn remove_business_cover_photo(
        &self,
        id: &str,
    ) -> Result<(), crate::errors::BridgeError>),
            prepare: parse_quote!({}),
            admission: Admission::Online,
            call: parse_quote!(client.business().remove_cover_photo(id)),
            output: ReturnAdapter::MapError,
        },
    ]
}
