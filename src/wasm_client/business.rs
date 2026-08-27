//! Business profile and catalog.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Business profile ───────────────────────────────────────────────

    /// Get business profile information for a JID.
    #[wasm_bindgen(js_name = getBusinessProfile)]
    pub async fn get_business_profile(
        &self,
        jid: &str,
    ) -> Result<Option<crate::result_types::BusinessProfileResult>, crate::errors::BridgeError>
    {
        let target_jid = parse_jid(jid)?;
        let profile = self
            .client
            .online()
            .await?
            .get_business_profile(&target_jid)
            .await?;
        Ok(profile.map(|p| business_profile_to_result(&p)))
    }

    // ── Business catalog ─────────────────────────────────────────────────

    /// Fetch one page of a business's product catalog.
    ///
    /// Paginate by feeding `afterCursor` back through `options.after` until it
    /// comes back absent.
    #[wasm_bindgen(js_name = getCatalog)]
    pub async fn get_catalog(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_optional_param_type = "CatalogOptionsInput | null")]
        options: JsValue,
    ) -> Result<crate::result_types::CatalogResult, crate::errors::BridgeError> {
        use whatsapp_rust::features::CatalogOptions;

        let target = parse_jid(jid)?;
        let input =
            from_js_input::<Option<crate::result_types::CatalogOptionsInput>>("options", options)?
                .unwrap_or_default();
        // Start from the core's defaults and override only what was supplied,
        // so an omitted field takes the core's value and not a second one.
        let mut opts = CatalogOptions::default();
        if let Some(limit) = input.limit {
            opts.limit = limit;
        }
        if let Some(width) = input.image_width {
            opts.image_width = width;
        }
        if let Some(height) = input.image_height {
            opts.image_height = height;
        }
        if let Some(allow) = input.allow_shop_source {
            opts.allow_shop_source = allow;
        }
        opts.after = input.after;

        let catalog = self
            .client
            .online()
            .await?
            .business()
            .get_catalog(&target, &opts)
            .await?;
        Ok(crate::result_types::CatalogResult {
            products: catalog.products.iter().map(product_to_result).collect(),
            after_cursor: catalog.after_cursor.clone(),
            before_cursor: catalog.before_cursor.clone(),
        })
    }

    /// Fetch one page of a business's product collections, each with its first
    /// products inline.
    #[wasm_bindgen(js_name = getCollections)]
    pub async fn get_collections(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_optional_param_type = "CollectionOptionsInput | null")]
        options: JsValue,
    ) -> Result<crate::result_types::CollectionsResult, crate::errors::BridgeError> {
        let options = from_js_input::<Option<crate::result_types::CollectionOptionsInput>>(
            "options", options,
        )?;
        use whatsapp_rust::features::CollectionOptions;

        let target = parse_jid(jid)?;
        let input = options.unwrap_or_default();
        let mut opts = CollectionOptions::default();
        if let Some(limit) = input.collection_limit {
            opts.collection_limit = limit;
        }
        if let Some(limit) = input.item_limit {
            opts.item_limit = limit;
        }
        if let Some(width) = input.image_width {
            opts.image_width = width;
        }
        if let Some(height) = input.image_height {
            opts.image_height = height;
        }
        opts.after = input.after;

        let collections = self
            .client
            .online()
            .await?
            .business()
            .get_collections(&target, &opts)
            .await?;
        Ok(crate::result_types::CollectionsResult {
            collections: collections
                .collections
                .iter()
                .map(|collection| crate::result_types::CollectionResult {
                    id: collection.id.clone(),
                    name: collection.name.clone(),
                    products: collection.products.iter().map(product_to_result).collect(),
                    review_status: collection.review_status.clone(),
                    can_appeal: collection.can_appeal,
                    reject_reason: collection.reject_reason.clone(),
                    commerce_url: collection.commerce_url.clone(),
                })
                .collect(),
            after_cursor: collections.after_cursor.clone(),
        })
    }

    /// Look up an order's line items and totals.
    ///
    /// Both `orderId` and `token` come from the order message itself; the token
    /// is a per-order capability, so an order cannot be read without it.
    #[wasm_bindgen(js_name = getOrder)]
    pub async fn get_order(
        &self,
        jid: &str,
        order_id: &str,
        token: &str,
    ) -> Result<crate::result_types::OrderResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let order = self
            .client
            .online()
            .await?
            .business()
            .get_order(&target, order_id, token)
            .await?;

        Ok(crate::result_types::OrderResult {
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
        })
    }

    /// Apply a delta to the account's own business profile.
    ///
    /// Omitted fields are left untouched; an empty value clears one. The core
    /// rejects a delta with nothing set.
    #[wasm_bindgen(js_name = updateBusinessProfile)]
    pub async fn update_business_profile(
        &self,
        #[wasm_bindgen(unchecked_param_type = "BusinessProfileUpdateInput")] update: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let update =
            from_js_input::<crate::result_types::BusinessProfileUpdateInput>("update", update)?;
        use wacore::iq::business::{
            BusinessHoursConfig, BusinessHoursUpdate, BusinessProfileUpdate,
        };

        let business_hours = update
            .business_hours
            .map(|hours| {
                let config = hours
                    .config
                    .into_iter()
                    .map(|entry| {
                        let day = entry.day_of_week.as_str().into();
                        let mode = entry.mode.as_str().into();
                        // A range is set at both ends or neither: the core's
                        // constructors admit no half-set config, so a lone
                        // endpoint has to be refused here rather than dropped.
                        match (entry.open_time, entry.close_time) {
                            (Some(open), Some(close)) => {
                                Ok(BusinessHoursConfig::with_hours(day, mode, open, close))
                            }
                            (None, None) => Ok(BusinessHoursConfig::new(day, mode)),
                            (open, _) => {
                                let missing = if open.is_some() {
                                    "closeTime"
                                } else {
                                    "openTime"
                                };
                                Err(crate::errors::BridgeError::InvalidArgument {
                                    field: "businessHours.config".into(),
                                    reason: format!(
                                        "{} is missing {missing}: an opening range needs both ends",
                                        entry.day_of_week
                                    ),
                                })
                            }
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok::<_, crate::errors::BridgeError>(BusinessHoursUpdate {
                    timezone: hours.timezone,
                    note: hours.note,
                    config,
                })
            })
            .transpose()?;

        let delta = BusinessProfileUpdate {
            address: update.address,
            latitude: update.latitude,
            longitude: update.longitude,
            description: update.description,
            email: update.email,
            websites: update.websites,
            categories: update.categories,
            business_hours,
        };
        self.client
            .online()
            .await?
            .business()
            .update_profile(&delta)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Point the business profile at an already-uploaded cover photo.
    ///
    /// Takes the `biz-cover-photo` upload receipt, not image bytes. **No
    /// consumer can produce that receipt today**: the core's upload pipeline
    /// requires `url` + `direct_path`, and the cover-photo endpoint returns
    /// `fbid`/`meta_hmac`/`ts` instead, so the upload leg does not exist. The
    /// protocol side is complete, and this is here for when it does.
    #[wasm_bindgen(js_name = setBusinessCoverPhoto)]
    pub async fn set_business_cover_photo(
        &self,
        #[wasm_bindgen(unchecked_param_type = "CoverPhotoUploadInput")] upload: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let upload = from_js_input::<crate::result_types::CoverPhotoUploadInput>("upload", upload)?;
        let timestamp = upload.timestamp.parse::<i64>().map_err(|e| {
            crate::errors::BridgeError::InvalidArgument {
                field: "timestamp".into(),
                reason: format!("must be the upload's ts: {e}"),
            }
        })?;
        self.client
            .online()
            .await?
            .business()
            .set_cover_photo(wacore::iq::business::CoverPhotoUpload {
                id: upload.id,
                token: upload.token,
                timestamp,
            })
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Remove the business profile's cover photo, by the `fbid` it was set with.
    #[wasm_bindgen(js_name = removeBusinessCoverPhoto)]
    pub async fn remove_business_cover_photo(
        &self,
        id: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .online()
            .await?
            .business()
            .remove_cover_photo(id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }
}
