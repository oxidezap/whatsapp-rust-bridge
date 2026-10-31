use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeSet;
use syn::{Ident, Type, parse_quote};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    CommunityLink,
    CommunityUnlink,
    BotTheme,
    BotListEntry,
    BotListSection,
    BotDefault,
    BotList,
    ParticipantAddRequest,
    ParticipantChange,
    NewsletterMetadata,
    GroupMetadataParticipant,
    GroupEphemeralSettings,
    GroupGrowthLockInfo,
    GroupOverview,
    GroupMetadata,
    CommunityLinkFailure,
    MemoryDiagnostics,
    NewChatMessageCapping,
    CreatePoll,
    NewsletterMessage,
    NewsletterPollVote,
    IsOnWhatsApp,
    UserInfo,
    ProfilePictureInfo,
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
    Record,
    FoundOptional,
}
#[derive(Clone, Copy)]
enum Ownership {
    Borrowed,
    Owned,
}
#[derive(Clone, Copy)]
enum Codec {
    Identity,
    Adapter(&'static str),
    ByteBuf,
    ProtoMessage,
    JidOptimized,
    CloneValue,
    DisplayString,
    DisplayList,
    NumberF64,
    NumberU32,
    NewsletterRole,
    Binary,
    BinaryToVec,
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
    source: Option<Source>,
    skip_empty: bool,
}
struct Descriptor {
    projection: Projection,
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
        source: None,
        skip_empty: false,
    }
}
fn field_doc(mut field: Field, doc: &'static str) -> Field {
    field.doc = Some(doc);
    field
}

#[path = "projection_calculated.rs"]
mod calculated;
#[path = "projection_catalog.rs"]
mod extended;
#[derive(Clone)]
enum Access {
    Field(Ident),
    Method(Ident),
}
enum Source {
    Path(Vec<Access>),
    Flatten {
        container: Vec<Access>,
        inner: Vec<Access>,
    },
}
fn access(path: &str) -> Vec<Access> {
    path.split('.')
        .map(|member| match member.strip_suffix("()") {
            Some(method) => {
                Access::Method(syn::parse_str(method).expect("source method identifier"))
            }
            None => Access::Field(syn::parse_str(member).expect("source field identifier")),
        })
        .collect()
}
fn chain(mut root: TokenStream, path: &[Access]) -> TokenStream {
    for member in path {
        root = match member {
            Access::Field(field) => quote!(#root.#field),
            Access::Method(method) => quote!(#root.#method()),
        };
    }
    root
}
fn field_path(mut field: Field, path: &str) -> Field {
    field.source = Some(Source::Path(access(path)));
    field
}
fn field_flatten(mut field: Field, container: &str, inner: &str) -> Field {
    assert!(field.optional, "flattened field must be optional");
    field.source = Some(Source::Flatten {
        container: access(container),
        inner: access(inner),
    });
    field
}
fn field_skip_empty(mut field: Field) -> Field {
    field.skip_empty = true;
    field
}

impl Projection {
    fn descriptor(self) -> Descriptor {
        use Codec::*;
        match self {
            Self::NewsletterAdminProfile => Descriptor {
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
                source: parse_quote!(whatsapp_rust::wacore::iq::mediaconn::MediaConnHost),
                target: parse_quote!(MediaHost),
                function: parse_quote!(project_media_host),
                feature: "client-media",
                doc: "A single media host from `getMediaConn`.",
                ownership: Ownership::Borrowed,
                fields: vec![field("hostname", parse_quote!(String), false, CloneValue)],
            },
            Self::UploadMedia => Descriptor {
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
                projection: self,
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
            _ => extended::descriptors()
                .into_iter()
                .chain(calculated::descriptors())
                .find(|descriptor| descriptor.projection == self)
                .expect("registered projection descriptor"),
        }
    }
    pub fn typescript_name(self) -> String {
        self.descriptor().target.to_string()
    }

    pub fn envelope(self, envelope: Envelope, value: TokenStream) -> TokenStream {
        let d = self.descriptor();
        let function = d.function;
        let function = quote!(crate::result_types::#function);
        match (envelope, d.ownership) {
            (Envelope::FoundOptional, Ownership::Borrowed) => {
                quote!(to_ts_opt(#value?.into_found().as_ref().map(#function)))
            }
            (Envelope::Record, Ownership::Borrowed) => {
                quote!({let result=js_sys::Object::new();for (key,value) in &#value {let entry=#function(value);let value=serde_wasm_bindgen::to_value(&entry)?;js_sys::Reflect::set(&result,&JsValue::from_str(&key.to_string()),&value)?;}Ok(result.into())})
            }
            (Envelope::FoundOptional | Envelope::Record, Ownership::Owned) => {
                panic!("record/found envelopes require borrowed source projections")
            }
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
    let mut targets = std::collections::BTreeMap::<String, String>::new();
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
    ]
    .into_iter()
    .chain([
        Projection::BotTheme,
        Projection::BotListEntry,
        Projection::BotListSection,
        Projection::BotDefault,
        Projection::BotList,
        Projection::ParticipantAddRequest,
        Projection::ParticipantChange,
        Projection::NewsletterMetadata,
        Projection::GroupMetadataParticipant,
        Projection::GroupEphemeralSettings,
        Projection::GroupGrowthLockInfo,
        Projection::GroupOverview,
        Projection::GroupMetadata,
        Projection::CommunityLinkFailure,
        Projection::CommunityLink,
        Projection::CommunityUnlink,
        Projection::MemoryDiagnostics,
        Projection::NewChatMessageCapping,
        Projection::CreatePoll,
        Projection::NewsletterMessage,
        Projection::NewsletterPollVote,
        Projection::IsOnWhatsApp,
        Projection::UserInfo,
        Projection::ProfilePictureInfo,
    ]) {
        let d = projection.descriptor();
        let target = d.target;
        let function = d.function;
        let source = d.source;
        let feature = d.feature;
        let feature = (!feature.is_empty()).then(|| quote!(#[cfg(feature=#feature)]));
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
            let empty = f
                .skip_empty
                .then(|| quote!(#[serde(skip_serializing_if="Vec::is_empty")]));
            let optional = f
                .optional
                .then(|| quote!(#[serde(skip_serializing_if = "Option::is_none")]));
            let ty = if f.optional {
                quote!(Option<#ty>)
            } else {
                quote!(#ty)
            };
            let binary = matches!(f.codec, Codec::Binary | Codec::BinaryToVec)
                .then(|| quote!(#[tsify(type = "Uint8Array")] #[serde(with = "serde_bytes")]));
            let doc = f.doc.map(|doc| quote!(#[doc=#doc]));
            declarations.push(quote!(#doc #optional #empty #binary pub #name: #ty,));
            let convert = |value: TokenStream, reference: bool| match f.codec {
                Codec::Adapter(path) => {
                    let function: syn::Path =
                        syn::parse_str(path).expect("registered adapter path");
                    if reference {
                        quote!(#function(#value))
                    } else {
                        quote!(#function(&#value))
                    }
                }
                Codec::BinaryToVec => quote!(#value.to_vec()),
                Codec::ByteBuf => quote!(serde_bytes::ByteBuf::from(#value.to_vec())),
                Codec::ProtoMessage => {
                    if reference {
                        quote!(serde_bytes::ByteBuf::from(whatsapp_rust::waproto::codec::message_to_vec(#value)))
                    } else {
                        quote!(serde_bytes::ByteBuf::from(whatsapp_rust::waproto::codec::message_to_vec(&#value)))
                    }
                }
                Codec::JidOptimized => {
                    if reference {
                        quote!(project_jid_owned(#value))
                    } else {
                        quote!(project_jid_owned(&#value))
                    }
                }
                Codec::Identity | Codec::Binary => {
                    if reference {
                        quote!(*#value)
                    } else {
                        value
                    }
                }
                Codec::CloneValue => quote!(#value.clone()),
                Codec::DisplayString => quote!(#value.to_string()),
                Codec::DisplayList => quote!(#value.iter().map(ToString::to_string).collect()),
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
            let flattened = matches!(f.source, Some(Source::Flatten { .. }));
            let expression = match &f.source {
                None => quote!(source.#name),
                Some(Source::Path(path)) => chain(quote!(source), path),
                Some(Source::Flatten { container, inner }) => {
                    let container = chain(quote!(source), container);
                    let inner = chain(quote!(value), inner);
                    quote!(#container.as_ref().and_then(|value|#inner.as_ref()))
                }
            };
            let reference = flattened || matches!(d.ownership, Ownership::Borrowed);
            let optional_expression = if reference && !flattened {
                quote!(#expression.as_ref())
            } else {
                expression.clone()
            };
            let value = if f.optional {
                if let Codec::Adapter(path) = f.codec {
                    let function: syn::Path = syn::parse_str(path).unwrap();
                    quote!(#optional_expression.map(#function))
                } else if matches!(f.codec, Codec::Identity) && !reference {
                    expression
                } else if matches!(f.codec, Codec::NewsletterRole) {
                    quote!(#optional_expression.map(crate::wasm_client::newsletter_role_str))
                } else if let Codec::Nested(projection) = f.codec {
                    let nested = projection.descriptor();
                    let function = nested.function;
                    match (reference, nested.ownership) {
                        (true, Ownership::Borrowed) | (false, Ownership::Owned) => {
                            quote!(#optional_expression.map(#function))
                        }
                        _ => panic!("nested ownership mismatch"),
                    }
                } else if matches!(f.codec, Codec::JidOptimized) {
                    quote!(#optional_expression.map(project_jid_owned))
                } else if matches!(f.codec, Codec::CloneValue) {
                    if flattened {
                        quote!(#optional_expression.cloned())
                    } else {
                        quote!(#expression.clone())
                    }
                } else {
                    let conversion = convert(quote!(value), reference);
                    quote!(#optional_expression.map(|value|#conversion))
                }
            } else {
                convert(expression, false)
            };
            values.push(quote!(#name: #value,));
        }
        let declaration = quote!(#doc #[derive(Serialize, Tsify)] #[serde(rename_all="camelCase")]
            pub struct #target { #(#declarations)* });
        match targets.entry(target.to_string()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(declaration.to_string());
                output.extend(declaration);
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                assert_eq!(
                    entry.get(),
                    &declaration.to_string(),
                    "shared DTO schemas must agree"
                );
            }
        }
        output.extend(quote!(#feature pub(crate) fn #function(source:#source)->#target { #target {#(#values)*} }));
    }
    output.extend(quote!(
        #[cfg(feature = "client-contacts")]
        fn project_jid_owned(jid: &whatsapp_rust::wacore_binary::jid::Jid) -> String {
            let mut result = String::with_capacity(jid.user.len() + jid.server.as_str().len() + 8);
            jid.push_to(&mut result);
            result
        }
    ));
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registered_projections_have_unique_typed_keys_and_converter_names() {
        let mut keys = Vec::new();
        let mut functions = BTreeSet::new();
        for descriptor in extended::descriptors()
            .into_iter()
            .chain(calculated::descriptors())
        {
            assert!(
                !keys.contains(&descriptor.projection),
                "duplicate projection key"
            );
            keys.push(descriptor.projection);
            assert!(
                functions.insert(descriptor.function.to_string()),
                "duplicate converter"
            );
            let registered = descriptor.projection.descriptor();
            assert_eq!(registered.function, descriptor.function);
            assert_eq!(registered.target, descriptor.target);
        }
    }

    #[test]
    fn link_and_unlink_share_one_dto_and_keep_typed_converters() {
        let file = syn::parse2::<syn::File>(emit()).unwrap();
        let declarations = file
            .items
            .iter()
            .filter(|item| {
                matches!(item,
                    syn::Item::Struct(value) if value.ident == "CommunityLinkResult"
                )
            })
            .count();
        assert_eq!(declarations, 1);
        for name in ["project_community_link", "project_community_unlink"] {
            assert_eq!(
                file.items
                    .iter()
                    .filter(|item| matches!(item,
                        syn::Item::Fn(value) if value.sig.ident == name
                    ))
                    .count(),
                1
            );
        }
    }
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
