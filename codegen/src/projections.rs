use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeSet;
use syn::{Ident, Type, parse_quote};

#[derive(Clone, Copy)]
pub enum Projection {
    UploadMedia,
    SignalSessionMigration,
    NewsletterFollower,
    NewsletterAdminProfile,
    NewsletterAdminInfo,
    NewsletterReactionCount,
    OwnUsername,
    BlocklistEntry,
    FetchStatus,
    MediaConn,
    MediaHost,
    Price,
    SalePrice,
    ProductImage,
    ProductVideo,
    ImporterAddress,
    VariantProperty,
    OrderPriceDetails,
    OrderProduct,
    Order,
    Product,
    Catalog,
    Collection,
    Collections,
    BusinessCategory,
    BusinessHoursConfig,
    BusinessHours,
    BusinessProfile,
    MembershipRequest,
    CommunitySubgroup,
    SignalSessionInfo,
    ProfilePicture,
}
#[derive(Clone, Copy)]
pub enum Envelope {
    Value,
    Optional,
    List,
    Values,
}
#[derive(Clone, Copy)]
enum Ownership {
    Borrowed,
    Owned,
}
#[derive(Clone, Copy)]
enum Codec {
    Identity,
    CloneValue,
    DisplayString,
    NumberF64,
    NumberU32,
    NewsletterRole,
    Binary,
    WireAsStr,
    Nested(Projection),
    List(Projection),
}
struct Field {
    name: Ident,
    ty: Type,
    optional: bool,
    codec: Codec,
    doc: Option<&'static str>,
}
struct Descriptor {
    source: Type,
    target: Ident,
    function: Ident,
    feature: &'static str,
    doc: &'static str,
    ownership: Ownership,
    fields: Vec<Field>,
}
fn field(name: &str, ty: Type, optional: bool, codec: Codec) -> Field {
    Field {
        name: format_ident!("{name}"),
        ty,
        optional,
        codec,
        doc: None,
    }
}
fn field_doc(mut field: Field, doc: &'static str) -> Field {
    field.doc = Some(doc);
    field
}

impl Projection {
    fn descriptor(self) -> Descriptor {
        use Codec::*;
        match self {
            Self::NewsletterAdminProfile => Descriptor {
                source: parse_quote!(whatsapp_rust::features::NewsletterAdminProfile),
                target: parse_quote!(NewsletterAdminProfileResult),
                function: parse_quote!(project_newsletter_admin_profile),
                feature: "client-newsletter",
                doc: "An admin's published profile on a newsletter.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("id", parse_quote!(String), true, CloneValue),
                    field("name", parse_quote!(String), false, CloneValue),
                    field("picture_id", parse_quote!(String), true, CloneValue),
                    field(
                        "picture_direct_path",
                        parse_quote!(String),
                        true,
                        CloneValue,
                    ),
                ],
            },
            Self::NewsletterAdminInfo => Descriptor {
                source: parse_quote!(whatsapp_rust::features::NewsletterAdminInfo),
                target: parse_quote!(NewsletterAdminInfoResult),
                function: parse_quote!(project_newsletter_admin_info),
                feature: "client-newsletter",
                doc: "Result from `newsletterAdminInfo`.\n\n`adminCount` has no query of its own — it rides along with the admin\nprofile. Absent means the server withheld it (it answers only admins and\nowners), never zero.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("admin_count", parse_quote!(f64), true, NumberF64),
                    field(
                        "admin_profile",
                        parse_quote!(NewsletterAdminProfileResult),
                        true,
                        Nested(Projection::NewsletterAdminProfile),
                    ),
                    field("admin_profiles_enabled", parse_quote!(bool), true, Identity),
                ],
            },
            Self::NewsletterReactionCount => Descriptor {
                source: parse_quote!(whatsapp_rust::features::NewsletterReactionCount),
                target: parse_quote!(NewsletterReactionCountResult),
                function: parse_quote!(project_newsletter_reaction_count),
                feature: "client-newsletter",
                doc: "A reaction tally on a newsletter message.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("code", parse_quote!(String), false, CloneValue),
                    field("count", parse_quote!(f64), false, NumberF64),
                ],
            },
            Self::OwnUsername => Descriptor {
                source: parse_quote!(whatsapp_rust::features::OwnUsername),
                target: parse_quote!(OwnUsernameResult),
                function: parse_quote!(project_own_username),
                feature: "client-contacts",
                doc: "Result from `getUsername`: this account's own Meta username.\n\nEvery field is optional because the server omits the ones that do not\napply. An account with no username at all comes back as `null` from the\nmethod rather than as an all-absent object.",
                ownership: Ownership::Owned,
                fields: vec![
                    field_doc(
                        field("username", parse_quote!(String), true, Identity),
                        "The handle, without the display-only `@` prefix.",
                    ),
                    field_doc(
                        field("state", parse_quote!(String), true, Identity),
                        "`ACTIVE` or `RESERVED`.",
                    ),
                    field_doc(
                        field("key", parse_quote!(String), true, Identity),
                        "The numeric username key that guards lookups of this account by handle.",
                    ),
                ],
            },
            Self::BlocklistEntry => Descriptor {
                source: parse_quote!(whatsapp_rust::wacore::iq::blocklist::BlocklistEntry),
                target: parse_quote!(BlocklistEntryResult),
                function: parse_quote!(project_blocklist_entry),
                feature: "client-contacts",
                doc: "A single entry from `fetchBlocklist`.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("jid", parse_quote!(String), false, DisplayString),
                    field("timestamp", parse_quote!(f64), true, NumberF64),
                ],
            },
            Self::FetchStatus => Descriptor {
                source: parse_quote!(whatsapp_rust::wacore::iq::usync::UserInfo),
                target: parse_quote!(FetchStatusResult),
                function: parse_quote!(project_fetch_status),
                feature: "client-contacts",
                doc: "Result from `fetchStatus`.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("jid", parse_quote!(String), false, DisplayString),
                    field("status", parse_quote!(String), true, CloneValue),
                ],
            },
            Self::MediaConn => Descriptor {
                source: parse_quote!(whatsapp_rust::mediaconn::MediaConn),
                target: parse_quote!(MediaConnResult),
                function: parse_quote!(project_media_conn),
                feature: "client-media",
                doc: "Result from `getMediaConn`.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("auth", parse_quote!(String), false, CloneValue),
                    field("ttl", parse_quote!(f64), false, NumberF64),
                    field(
                        "hosts",
                        parse_quote!(Vec<MediaHost>),
                        false,
                        List(Projection::MediaHost),
                    ),
                ],
            },
            Self::MediaHost => Descriptor {
                source: parse_quote!(whatsapp_rust::wacore::iq::mediaconn::MediaConnHost),
                target: parse_quote!(MediaHost),
                function: parse_quote!(project_media_host),
                feature: "client-media",
                doc: "A single media host from `getMediaConn`.",
                ownership: Ownership::Borrowed,
                fields: vec![field("hostname", parse_quote!(String), false, CloneValue)],
            },
            Self::UploadMedia => Descriptor {
                source: parse_quote!(whatsapp_rust::upload::UploadResponse),
                target: parse_quote!(UploadMediaResult),
                function: parse_quote!(project_upload_media),
                feature: "client-media",
                doc: "Result from `uploadMedia`.",
                ownership: Ownership::Owned,
                fields: vec![
                    field("url", parse_quote!(String), false, Identity),
                    field("direct_path", parse_quote!(String), false, Identity),
                    field("media_key", parse_quote!([u8; 32]), false, Binary),
                    field("file_sha256", parse_quote!([u8; 32]), false, Binary),
                    field("file_enc_sha256", parse_quote!([u8; 32]), false, Binary),
                    field("file_length", parse_quote!(f64), false, NumberF64),
                ],
            },
            Self::SignalSessionMigration => Descriptor {
                source: parse_quote!(whatsapp_rust::features::SignalSessionMigration),
                target: parse_quote!(SignalSessionMigrationResult),
                function: parse_quote!(project_signal_session_migration),
                feature: "client-signal",
                doc: "Counts produced while moving pairwise sessions between identifier namespaces.",
                ownership: Ownership::Owned,
                fields: vec![
                    field("migrated", parse_quote!(u32), false, NumberU32),
                    field("skipped", parse_quote!(u32), false, NumberU32),
                    field("total", parse_quote!(u32), false, NumberU32),
                ],
            },
            Self::NewsletterFollower => Descriptor {
                source: parse_quote!(whatsapp_rust::features::NewsletterFollower),
                target: parse_quote!(NewsletterFollowerResult),
                function: parse_quote!(project_newsletter_follower),
                feature: "client-newsletter",
                doc: "One follower of a newsletter, from `newsletterFollowers`.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("jid", parse_quote!(String), false, DisplayString),
                    field_doc(
                        field("phone_jid", parse_quote!(String), true, DisplayString),
                        "Withheld by the server when the follower's privacy settings hide it.",
                    ),
                    field("display_name", parse_quote!(String), true, CloneValue),
                    field("username", parse_quote!(String), true, CloneValue),
                    field("role", parse_quote!(String), true, NewsletterRole),
                    field("follow_time", parse_quote!(f64), true, NumberF64),
                    field(
                        "admin_profile",
                        parse_quote!(NewsletterAdminProfileResult),
                        true,
                        Nested(Projection::NewsletterAdminProfile),
                    ),
                ],
            },
            Self::Price => Descriptor {
                source: parse_quote!(whatsapp_rust::features::Price),
                target: parse_quote!(PriceResult),
                function: parse_quote!(project_price),
                feature: "client-business",
                doc: "A price, in thousandths of the currency's main unit.\n\nWhatsApp scales money by 1000, not 100, and the protobuf field is an\n`int64`. `amount1000` therefore crosses as a **string**: a `number` is exact\nonly below 2^53, and a large order would be silently wrong rather than\nrejected. Dividing by 1000 for display is the consumer's decision.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field_doc(
                        field("amount_1000", parse_quote!(String), false, DisplayString),
                        "Thousandths of one currency unit: `\"1990\"` is 1.99 in `currency`.",
                    ),
                    field_doc(
                        field("currency", parse_quote!(String), true, CloneValue),
                        "ISO 4217 code. Absent when the server sends a price without one.",
                    ),
                ],
            },
            Self::SalePrice => Descriptor {
                source: parse_quote!(whatsapp_rust::features::SalePrice),
                target: parse_quote!(SalePriceResult),
                function: parse_quote!(project_sale_price),
                feature: "client-business",
                doc: "A sale price and the window it applies to.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field(
                        "price",
                        parse_quote!(PriceResult),
                        false,
                        Nested(Projection::Price),
                    ),
                    field("start_date", parse_quote!(String), true, CloneValue),
                    field("end_date", parse_quote!(String), true, CloneValue),
                ],
            },
            Self::ProductImage => Descriptor {
                source: parse_quote!(whatsapp_rust::features::ProductImage),
                target: parse_quote!(ProductImageResult),
                function: parse_quote!(project_product_image),
                feature: "client-business",
                doc: "",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("id", parse_quote!(String), true, CloneValue),
                    field("request_image_url", parse_quote!(String), true, CloneValue),
                    field("original_image_url", parse_quote!(String), true, CloneValue),
                ],
            },
            Self::ProductVideo => Descriptor {
                source: parse_quote!(whatsapp_rust::features::ProductVideo),
                target: parse_quote!(ProductVideoResult),
                function: parse_quote!(project_product_video),
                feature: "client-business",
                doc: "",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("id", parse_quote!(String), true, CloneValue),
                    field("original_video_url", parse_quote!(String), true, CloneValue),
                    field("thumbnail_url", parse_quote!(String), true, CloneValue),
                ],
            },
            Self::ImporterAddress => Descriptor {
                source: parse_quote!(whatsapp_rust::features::ImporterAddress),
                target: parse_quote!(ImporterAddressResult),
                function: parse_quote!(project_importer_address),
                feature: "client-business",
                doc: "A postal address, as sent for a product's importer of record.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("street1", parse_quote!(String), true, CloneValue),
                    field("street2", parse_quote!(String), true, CloneValue),
                    field("city", parse_quote!(String), true, CloneValue),
                    field("region", parse_quote!(String), true, CloneValue),
                    field("postal_code", parse_quote!(String), true, CloneValue),
                    field("country_code", parse_quote!(String), true, CloneValue),
                ],
            },
            Self::VariantProperty => Descriptor {
                source: parse_quote!(whatsapp_rust::features::VariantProperty),
                target: parse_quote!(VariantPropertyResult),
                function: parse_quote!(project_variant_property),
                feature: "client-business",
                doc: "One dimension of a chosen product variant, e.g. `Size` / `Large`.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("name", parse_quote!(String), true, CloneValue),
                    field("value", parse_quote!(String), true, CloneValue),
                ],
            },
            Self::OrderPriceDetails => Descriptor {
                source: parse_quote!(whatsapp_rust::features::OrderPriceDetails),
                target: parse_quote!(OrderPriceDetailsResult),
                function: parse_quote!(project_order_price_details),
                feature: "client-business",
                doc: "",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("currency", parse_quote!(String), true, CloneValue),
                    field(
                        "subtotal",
                        parse_quote!(PriceResult),
                        true,
                        Nested(Projection::Price),
                    ),
                    field(
                        "total",
                        parse_quote!(PriceResult),
                        true,
                        Nested(Projection::Price),
                    ),
                ],
            },
            Self::OrderProduct => Descriptor {
                source: parse_quote!(whatsapp_rust::features::OrderProduct),
                target: parse_quote!(OrderProductResult),
                function: parse_quote!(project_order_product),
                feature: "client-business",
                doc: "A line item on an order: a snapshot, so the price is what was quoted when\nthe order was placed.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("id", parse_quote!(String), true, CloneValue),
                    field("name", parse_quote!(String), true, CloneValue),
                    field(
                        "price",
                        parse_quote!(PriceResult),
                        true,
                        Nested(Projection::Price),
                    ),
                    field("quantity", parse_quote!(f64), true, NumberF64),
                    field(
                        "images",
                        parse_quote!(Vec<ProductImageResult>),
                        false,
                        List(Projection::ProductImage),
                    ),
                    field_doc(
                        field(
                            "variant_properties",
                            parse_quote!(Vec<VariantPropertyResult>),
                            false,
                            List(Projection::VariantProperty),
                        ),
                        "Empty for a product with no variants. Without it a variant order cannot\nbe fulfilled: id, name and price are shared across one listing's\nvariants.",
                    ),
                ],
            },
            Self::Order => Descriptor {
                source: parse_quote!(whatsapp_rust::features::Order),
                target: parse_quote!(OrderResult),
                function: parse_quote!(project_order),
                feature: "client-business",
                doc: "Result from `getOrder`.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field(
                        "products",
                        parse_quote!(Vec<OrderProductResult>),
                        false,
                        List(Projection::OrderProduct),
                    ),
                    field(
                        "price_details",
                        parse_quote!(OrderPriceDetailsResult),
                        true,
                        Nested(Projection::OrderPriceDetails),
                    ),
                    field_doc(
                        field("creation_timestamp", parse_quote!(f64), true, NumberF64),
                        "Unix seconds, as sent.",
                    ),
                ],
            },
            Self::Product => Descriptor {
                source: parse_quote!(whatsapp_rust::features::Product),
                target: parse_quote!(ProductResult),
                function: parse_quote!(project_product),
                feature: "client-business",
                doc: "A catalog product. Only `id` is guaranteed; the server omits rather than\nblanks, so an absent name is absent, not `\"\"`.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("id", parse_quote!(String), false, CloneValue),
                    field("retailer_id", parse_quote!(String), true, CloneValue),
                    field("name", parse_quote!(String), true, CloneValue),
                    field("description", parse_quote!(String), true, CloneValue),
                    field("url", parse_quote!(String), true, CloneValue),
                    field_doc(
                        field("shimmed_url", parse_quote!(String), true, CloneValue),
                        "The link-shimmed form of `url`, carried alongside it rather than\ninstead. Which to open is a consumer's call.",
                    ),
                    field(
                        "price",
                        parse_quote!(PriceResult),
                        true,
                        Nested(Projection::Price),
                    ),
                    field(
                        "sale_price",
                        parse_quote!(SalePriceResult),
                        true,
                        Nested(Projection::SalePrice),
                    ),
                    field("is_hidden", parse_quote!(bool), true, Identity),
                    field("is_sanctioned", parse_quote!(bool), true, Identity),
                    field("max_available", parse_quote!(f64), true, NumberF64),
                    field("availability", parse_quote!(String), true, WireAsStr),
                    field("review_status", parse_quote!(String), true, CloneValue),
                    field("can_appeal", parse_quote!(bool), true, Identity),
                    field("belongs_to", parse_quote!(bool), true, Identity),
                    field(
                        "images",
                        parse_quote!(Vec<ProductImageResult>),
                        false,
                        List(Projection::ProductImage),
                    ),
                    field(
                        "videos",
                        parse_quote!(Vec<ProductVideoResult>),
                        false,
                        List(Projection::ProductVideo),
                    ),
                    field(
                        "compliance_category",
                        parse_quote!(String),
                        true,
                        CloneValue,
                    ),
                    field(
                        "country_code_origin",
                        parse_quote!(String),
                        true,
                        CloneValue,
                    ),
                    field("importer_name", parse_quote!(String), true, CloneValue),
                    field(
                        "importer_address",
                        parse_quote!(ImporterAddressResult),
                        true,
                        Nested(Projection::ImporterAddress),
                    ),
                ],
            },
            Self::Catalog => Descriptor {
                source: parse_quote!(whatsapp_rust::features::Catalog),
                target: parse_quote!(CatalogResult),
                function: parse_quote!(project_catalog),
                feature: "client-business",
                doc: "One page of a business catalog.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field(
                        "products",
                        parse_quote!(Vec<ProductResult>),
                        false,
                        List(Projection::Product),
                    ),
                    field_doc(
                        field("after_cursor", parse_quote!(String), true, CloneValue),
                        "Feed back as `after` for the next page; absent on the last one.",
                    ),
                    field_doc(
                        field("before_cursor", parse_quote!(String), true, CloneValue),
                        "Carried by the response, though the catalog query takes only `after`.",
                    ),
                ],
            },
            Self::Collection => Descriptor {
                source: parse_quote!(whatsapp_rust::features::Collection),
                target: parse_quote!(CollectionResult),
                function: parse_quote!(project_collection),
                feature: "client-business",
                doc: "A named group of products within a catalog.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("id", parse_quote!(String), true, CloneValue),
                    field("name", parse_quote!(String), true, CloneValue),
                    field_doc(
                        field(
                            "products",
                            parse_quote!(Vec<ProductResult>),
                            false,
                            List(Projection::Product),
                        ),
                        "A prefix, and it says so only by its length: the collections query\ncarries no per-collection cursor, so `products.length === itemLimit`\nmeans more may exist.",
                    ),
                    field("review_status", parse_quote!(String), true, CloneValue),
                    field("can_appeal", parse_quote!(bool), true, Identity),
                    field("reject_reason", parse_quote!(String), true, CloneValue),
                    field("commerce_url", parse_quote!(String), true, CloneValue),
                ],
            },
            Self::Collections => Descriptor {
                source: parse_quote!(whatsapp_rust::features::Collections),
                target: parse_quote!(CollectionsResult),
                function: parse_quote!(project_collections),
                feature: "client-business",
                doc: "One page of a business's collections.\n\nForward cursor only — the collections paging object has no `before`, and\nthe asymmetry with the catalog is the wire's, not an oversight.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field(
                        "collections",
                        parse_quote!(Vec<CollectionResult>),
                        false,
                        List(Projection::Collection),
                    ),
                    field("after_cursor", parse_quote!(String), true, CloneValue),
                ],
            },
            Self::BusinessCategory => Descriptor {
                source: parse_quote!(whatsapp_rust::wacore::iq::business::BusinessCategory),
                target: parse_quote!(BusinessCategoryResult),
                function: parse_quote!(project_business_category),
                feature: "client-business",
                doc: "Business category info.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("id", parse_quote!(String), false, CloneValue),
                    field("name", parse_quote!(String), false, CloneValue),
                ],
            },
            Self::BusinessHoursConfig => Descriptor {
                source: parse_quote!(whatsapp_rust::wacore::iq::business::BusinessHoursConfig),
                target: parse_quote!(BusinessHoursConfigResult),
                function: parse_quote!(project_business_hours_config),
                feature: "client-business",
                doc: "Business hours config for a day.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("day_of_week", parse_quote!(String), false, DisplayString),
                    field("mode", parse_quote!(String), false, DisplayString),
                    field("open_time", parse_quote!(f64), true, NumberF64),
                    field("close_time", parse_quote!(f64), true, NumberF64),
                ],
            },
            Self::BusinessHours => Descriptor {
                source: parse_quote!(whatsapp_rust::wacore::iq::business::BusinessHours),
                target: parse_quote!(BusinessHoursResult),
                function: parse_quote!(project_business_hours),
                feature: "client-business",
                doc: "Business hours.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("timezone", parse_quote!(String), true, CloneValue),
                    field(
                        "business_config",
                        parse_quote!(Vec<BusinessHoursConfigResult>),
                        true,
                        List(Projection::BusinessHoursConfig),
                    ),
                ],
            },
            Self::BusinessProfile => Descriptor {
                source: parse_quote!(whatsapp_rust::wacore::iq::business::BusinessProfile),
                target: parse_quote!(BusinessProfileResult),
                function: parse_quote!(project_business_profile),
                feature: "client-business",
                doc: "Result from `getBusinessProfile`.",
                ownership: Ownership::Borrowed,
                fields: vec![
                    field("wid", parse_quote!(String), true, DisplayString),
                    field("description", parse_quote!(String), false, CloneValue),
                    field("email", parse_quote!(String), true, CloneValue),
                    field("website", parse_quote!(Vec<String>), false, CloneValue),
                    field(
                        "categories",
                        parse_quote!(Vec<BusinessCategoryResult>),
                        false,
                        List(Projection::BusinessCategory),
                    ),
                    field("address", parse_quote!(String), true, CloneValue),
                    field(
                        "business_hours",
                        parse_quote!(BusinessHoursResult),
                        false,
                        Nested(Projection::BusinessHours),
                    ),
                ],
            },
            Self::ProfilePicture => Descriptor {
                source: parse_quote!(
                    whatsapp_rust::wacore::iq::contacts::SetProfilePictureResponse
                ),
                target: parse_quote!(ProfilePictureResult),
                function: parse_quote!(project_profile_picture),
                feature: "client-contacts",
                ownership: Ownership::Owned,
                doc: "Result from `updateProfilePicture` or `removeProfilePicture`.",
                fields: vec![field("id", parse_quote!(String), false, Identity)],
            },
            Self::MembershipRequest => Descriptor {
                source: parse_quote!(whatsapp_rust::wacore::iq::groups::MembershipRequest),
                target: parse_quote!(MembershipRequestResult),
                function: parse_quote!(project_membership_request),
                feature: "client-groups",
                ownership: Ownership::Borrowed,
                doc: "Result from `groupRequestParticipantsList`.",
                fields: vec![
                    field("jid", parse_quote!(String), false, DisplayString),
                    field("request_time", parse_quote!(f64), true, NumberF64),
                ],
            },
            Self::CommunitySubgroup => Descriptor {
                source: parse_quote!(whatsapp_rust::features::CommunitySubgroup),
                target: parse_quote!(CommunitySubgroupResult),
                function: parse_quote!(project_community_subgroup),
                feature: "client-groups",
                ownership: Ownership::Owned,
                doc: "A subgroup returned by a parent-group metadata query.",
                fields: vec![
                    field("id", parse_quote!(String), false, DisplayString),
                    field("subject", parse_quote!(String), false, Identity),
                    field("participant_count", parse_quote!(f64), true, NumberF64),
                    field("creation", parse_quote!(f64), true, NumberF64),
                    field("owner", parse_quote!(String), true, DisplayString),
                    field("is_default_sub_group", parse_quote!(bool), false, Identity),
                    field("is_general_chat", parse_quote!(bool), false, Identity),
                ],
            },
            Self::SignalSessionInfo => Descriptor {
                source: parse_quote!(whatsapp_rust::features::SignalSessionInfo),
                target: parse_quote!(SignalSessionInfoResult),
                function: parse_quote!(project_signal_session_info),
                feature: "client-signal",
                ownership: Ownership::Owned,
                doc: "Read-only information from a currently open pairwise session.",
                fields: vec![
                    field("base_key", parse_quote!(Vec<u8>), false, Binary),
                    field("registration_id", parse_quote!(u32), false, Identity),
                ],
            },
        }
    }
    pub fn envelope(self, envelope: Envelope, value: TokenStream) -> TokenStream {
        let d = self.descriptor();
        let function = d.function;
        let function = quote!(crate::result_types::#function);
        match (envelope, d.ownership) {
            (Envelope::Value, Ownership::Borrowed) => quote!(to_ts(#function(&#value))),
            (Envelope::Value, Ownership::Owned) => quote!(to_ts(#function(#value))),
            (Envelope::Optional, Ownership::Borrowed) => {
                quote!(to_ts_opt(#value.as_ref().map(#function)))
            }
            (Envelope::Optional, Ownership::Owned) => quote!(to_ts_opt(#value.map(#function))),
            (Envelope::Values, Ownership::Borrowed) => {
                quote!(to_ts_vec(#value.values().map(#function).collect()))
            }
            (Envelope::Values, Ownership::Owned) => {
                panic!("values envelope requires borrowed projection")
            }
            (Envelope::List, ownership) => {
                let iterator = match ownership {
                    Ownership::Borrowed => quote!(#value.iter()),
                    Ownership::Owned => quote!(#value.into_iter()),
                };
                quote!(to_ts_vec(#iterator.map(#function).collect()))
            }
        }
    }
}
pub fn emit() -> TokenStream {
    let mut output = TokenStream::new();
    for projection in [
        Projection::UploadMedia,
        Projection::SignalSessionMigration,
        Projection::NewsletterFollower,
        Projection::NewsletterAdminProfile,
        Projection::NewsletterAdminInfo,
        Projection::NewsletterReactionCount,
        Projection::OwnUsername,
        Projection::BlocklistEntry,
        Projection::FetchStatus,
        Projection::MediaConn,
        Projection::MediaHost,
        Projection::Price,
        Projection::SalePrice,
        Projection::ProductImage,
        Projection::ProductVideo,
        Projection::ImporterAddress,
        Projection::VariantProperty,
        Projection::OrderPriceDetails,
        Projection::OrderProduct,
        Projection::Order,
        Projection::Product,
        Projection::Catalog,
        Projection::Collection,
        Projection::Collections,
        Projection::BusinessCategory,
        Projection::BusinessHoursConfig,
        Projection::BusinessHours,
        Projection::BusinessProfile,
        Projection::MembershipRequest,
        Projection::CommunitySubgroup,
        Projection::SignalSessionInfo,
        Projection::ProfilePicture,
    ] {
        let d = projection.descriptor();
        let target = d.target;
        let function = d.function;
        let source = d.source;
        let feature = d.feature;
        let doc = (!d.doc.is_empty()).then(|| {
            let doc = d.doc;
            quote!(#[doc=#doc])
        });
        let source = match d.ownership {
            Ownership::Owned => quote!(#source),
            Ownership::Borrowed => quote!(&#source),
        };
        let mut declarations = Vec::new();
        let mut values = Vec::new();
        let mut names = BTreeSet::new();
        for f in d.fields {
            let name = f.name;
            assert!(names.insert(name.to_string()), "duplicate DTO field");
            let ty = f.ty;
            let optional = f
                .optional
                .then(|| quote!(#[serde(skip_serializing_if = "Option::is_none")]));
            let ty = if f.optional {
                quote!(Option<#ty>)
            } else {
                quote!(#ty)
            };
            let binary = matches!(f.codec, Codec::Binary)
                .then(|| quote!(#[tsify(type = "Uint8Array")] #[serde(with = "serde_bytes")]));
            let doc = f.doc.map(|doc| quote!(#[doc=#doc]));
            declarations.push(quote!(#doc #optional #binary pub #name: #ty,));
            let convert = |value: TokenStream, reference: bool| match f.codec {
                Codec::Identity | Codec::Binary => {
                    if reference {
                        quote!(*#value)
                    } else {
                        value
                    }
                }
                Codec::CloneValue => quote!(#value.clone()),
                Codec::DisplayString => quote!(#value.to_string()),
                Codec::WireAsStr => quote!(#value.as_str().to_owned()),
                Codec::NumberU32 => {
                    if reference {
                        quote!(*#value as u32)
                    } else {
                        quote!(#value as u32)
                    }
                }
                Codec::NewsletterRole => quote!(crate::wasm_client::newsletter_role_str(#value)),
                Codec::NumberF64 => {
                    if reference {
                        quote!(*#value as f64)
                    } else {
                        quote!(#value as f64)
                    }
                }
                Codec::Nested(projection) => {
                    let nested = projection.descriptor();
                    let function = nested.function;
                    match (nested.ownership, reference) {
                        (Ownership::Borrowed, false) => quote!(#function(&#value)),
                        _ => quote!(#function(#value)),
                    }
                }
                Codec::List(projection) => {
                    let function = projection.descriptor().function;
                    quote!(#value.iter().map(#function).collect())
                }
            };
            let value = if f.optional {
                if matches!(f.codec, Codec::Identity) && matches!(d.ownership, Ownership::Owned) {
                    quote!(source.#name)
                } else if matches!(f.codec, Codec::NewsletterRole) {
                    quote!(source.#name.as_ref().map(crate::wasm_client::newsletter_role_str))
                } else if let Codec::Nested(projection) = f.codec {
                    let nested = projection.descriptor();
                    let function = nested.function;
                    match (d.ownership, nested.ownership) {
                        (Ownership::Borrowed, Ownership::Borrowed) => {
                            quote!(source.#name.as_ref().map(#function))
                        }
                        (Ownership::Owned, Ownership::Owned) => quote!(source.#name.map(#function)),
                        _ => panic!("optional nested ownership must match"),
                    }
                } else if matches!(f.codec, Codec::CloneValue) {
                    quote!(source.#name.clone())
                } else if matches!(d.ownership, Ownership::Borrowed) {
                    let expression = convert(quote!(value), true);
                    quote!(source.#name.as_ref().map(|value| #expression))
                } else {
                    let expression = convert(quote!(value), false);
                    quote!(source.#name.map(|value| #expression))
                }
            } else {
                convert(quote!(source.#name), false)
            };
            values.push(quote!(#name: #value,));
        }
        output.extend(quote!(#doc #[derive(Serialize, Tsify)] #[serde(rename_all="camelCase")]
            pub struct #target { #(#declarations)* }
            #[cfg(feature=#feature)] pub(crate) fn #function(source:#source)->#target { #target {#(#values)*} }
        ));
    }
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schemas_and_all_envelopes_are_rust_syntax() {
        syn::parse2::<syn::File>(emit()).unwrap();
        for envelope in [Envelope::Value, Envelope::Optional, Envelope::List] {
            syn::parse2::<syn::Expr>(
                Projection::MembershipRequest.envelope(envelope, quote!(value)),
            )
            .unwrap();
        }
    }
}
