use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Get business profile information for a JID.
                #[wasm_bindgen(js_name = getBusinessProfile)]
            )),
            signature: parse_quote!(
                async fn get_business_profile(
                    &self,
                    jid: &str,
                ) -> Result<Option<Ts<crate::result_types::BusinessProfileResult>>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(target_jid),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.get_business_profile(&target_jid)),
            output: ReturnAdapter::Dto {
                projection: Projection::BusinessProfile,
                envelope: Envelope::Optional,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Look up an order's line items and totals.
                ///
                /// Both `orderId` and `token` come from the order message itself; the token
                /// is a per-order capability, so an order cannot be read without it.
                #[wasm_bindgen(js_name = getOrder)]
            )),
            signature: parse_quote!(
                async fn get_order(
                    &self,
                    jid: &str,
                    order_id: &str,
                    token: &str,
                ) -> Result<Ts<crate::result_types::OrderResult>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.business().get_order(&target, order_id, token)),
            output: ReturnAdapter::Dto {
                projection: Projection::Order,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Remove the business profile's cover photo, by the `fbid` it was set with.
                #[wasm_bindgen(js_name = removeBusinessCoverPhoto)]
            )),
            signature: parse_quote!(
                async fn remove_business_cover_photo(
                    &self,
                    id: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![],
            admission: Admission::Online,
            call: parse_quote!(client.business().remove_cover_photo(id)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch one page of a business's product catalog.
                ///
                /// Paginate by feeding `afterCursor` back through `options.after` until it
                /// comes back absent.
                #[wasm_bindgen(js_name = getCatalog)]
            )),
            signature: parse_quote!(
                async fn get_catalog(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_optional_param_type = "CatalogOptionsInput | null")]
                    options: JsValue,
                ) -> Result<Ts<crate::result_types::CatalogResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
                Input::Decode {
                    binding: parse_quote!(options),
                    ty: parse_quote!(Option<crate::result_types::CatalogOptionsInput>),
                    field: parse_quote!("options"),
                    value: parse_quote!(options),
                },
                Input::Codec(InputCodec::DefaultOverlay {
                    binding: parse_quote!(opts),
                    input: parse_quote!(options),
                    ty: parse_quote!(whatsapp_rust::features::CatalogOptions),
                    optional: vec![
                        (parse_quote!(limit), parse_quote!(limit)),
                        (parse_quote!(image_width), parse_quote!(image_width)),
                        (parse_quote!(image_height), parse_quote!(image_height)),
                        (
                            parse_quote!(allow_shop_source),
                            parse_quote!(allow_shop_source),
                        ),
                    ],
                    direct: vec![(parse_quote!(after), parse_quote!(after))],
                }),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.business().get_catalog(&target, &opts)),
            output: ReturnAdapter::Dto {
                projection: Projection::Catalog,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch one page of a business's product collections, each with its first
                /// products inline.
                #[wasm_bindgen(js_name = getCollections)]
            )),
            signature: parse_quote!(
                async fn get_collections(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_optional_param_type = "CollectionOptionsInput | null")]
                    options: JsValue,
                ) -> Result<Ts<crate::result_types::CollectionsResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(options),
                    ty: parse_quote!(Option<crate::result_types::CollectionOptionsInput>),
                    field: parse_quote!("options"),
                    value: parse_quote!(options),
                },
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
                Input::Codec(InputCodec::DefaultOverlay {
                    binding: parse_quote!(opts),
                    input: parse_quote!(options),
                    ty: parse_quote!(whatsapp_rust::features::CollectionOptions),
                    optional: vec![
                        (
                            parse_quote!(collection_limit),
                            parse_quote!(collection_limit),
                        ),
                        (parse_quote!(item_limit), parse_quote!(item_limit)),
                        (parse_quote!(image_width), parse_quote!(image_width)),
                        (parse_quote!(image_height), parse_quote!(image_height)),
                    ],
                    direct: vec![(parse_quote!(after), parse_quote!(after))],
                }),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.business().get_collections(&target, &opts)),
            output: ReturnAdapter::Dto {
                projection: Projection::Collections,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Apply a delta to the account's own business profile.
                ///
                /// Omitted fields are left untouched; an empty value clears one. The core
                /// rejects a delta with nothing set.
                #[wasm_bindgen(js_name = updateBusinessProfile)]
            )),
            signature: parse_quote!(
                async fn update_business_profile(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "BusinessProfileUpdateInput")] update: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(update),
                    ty: parse_quote!(crate::result_types::BusinessProfileUpdateInput),
                    field: parse_quote!("update"),
                    value: parse_quote!(update),
                },
                Input::Preparation(Preparation::BusinessProfile),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.business().update_profile(&delta)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Point the business profile at an already-uploaded cover photo.
                ///
                /// Takes the `biz-cover-photo` upload receipt, not image bytes. **No
                /// consumer can produce that receipt today**: the core's upload pipeline
                /// requires `url` + `direct_path`, and the cover-photo endpoint returns
                /// `fbid`/`meta_hmac`/`ts` instead, so the upload leg does not exist. The
                /// protocol side is complete, and this is here for when it does.
                #[wasm_bindgen(js_name = setBusinessCoverPhoto)]
            )),
            signature: parse_quote!(
                async fn set_business_cover_photo(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "CoverPhotoUploadInput")] upload: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(upload),
                    ty: parse_quote!(crate::result_types::CoverPhotoUploadInput),
                    field: parse_quote!("upload"),
                    value: parse_quote!(upload),
                },
                Input::Codec(InputCodec::Scalar {
                    binding: parse_quote!(timestamp),
                    value: parse_quote!(upload.timestamp),
                    ty: parse_quote!(i64),
                    field: parse_quote!("timestamp"),
                    prefix: Some(parse_quote!("must be the upload's ts: ")),
                    optional: false,
                }),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.business().set_cover_photo(
                wacore::iq::business::CoverPhotoUpload {
                    id: upload.id,
                    token: upload.token,
                    timestamp,
                }
            )),
            output: ReturnAdapter::MapError,
        },
    ]
}
