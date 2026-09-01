//! TypeScript type generator for whatsapp-rust types.
//!
//! Parses Rust source files with `syn`, finds structs/enums with `#[derive(Serialize)]`,
//! and generates TypeScript interfaces/types.
//!
//! Usage: bun run gen:types (outputs to ../src/generated_types.rs)

use quote::ToTokens;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use syn::{Attribute, Fields, GenericArgument, Item, PathArguments, Type, TypePath};
use walkdir::WalkDir;

/// Declarations this generator writes itself, so a reference to one is not a
/// dangling name. Each is here because the derive scan cannot reach it.
const HAND_WRITTEN_DECLARATIONS: &[&str] = &["Jid", "JsonValue"];

/// Declarations the bridge writes, through Tsify or its own
/// `typescript_custom_section`s. wasm-bindgen concatenates every section into
/// one `.d.ts`, so a reference to one of these resolves there — this generator
/// just cannot see them from the core's sources.
const BRIDGE_DECLARATIONS: &[&str] = &["KeyPair"];

/// Where the core's sources live, as two roots rather than one: published
/// crates put `wacore` and `whatsapp-rust` side by side in the registry, while
/// a git checkout or a working clone nests `wacore/` inside the repository.
struct Sources {
    wacore_src: PathBuf,
    core_src: PathBuf,
}

/// Locate the core's sources on disk.
///
/// This crate does not depend on whatsapp-rust, so `cargo run` never fetches
/// it — `gen:bridge-types` runs `cargo fetch` first for that. When the sources
/// are absent every lookup here misses, and `WalkDir` over a missing directory
/// yields nothing rather than erroring: that silently produced a type file with
/// almost everything missing, which still compiled and shipped a `.d.ts`
/// referencing types it no longer declared. Hence `require_sources`.
fn find_sources() -> Sources {
    let lock = std::fs::read_to_string("../Cargo.lock").unwrap_or_default();

    // Registry layout: each crate is unpacked on its own, so the two roots are
    // siblings named `<crate>-<version>`.
    if let (Some(core), Some(wacore)) = (
        locked_registry_version(&lock, "whatsapp-rust"),
        locked_registry_version(&lock, "wacore"),
    ) && let Some(registry) = registry_root_with(&core, &wacore)
    {
        eprintln!("Using registry: {}", registry.display());
        return Sources {
            wacore_src: registry.join(format!("wacore-{wacore}")).join("src"),
            core_src: registry.join(format!("whatsapp-rust-{core}")).join("src"),
        };
    }

    // Repository layout: one root, `wacore/` nested inside it.
    let root = find_repository_root(&lock);
    Sources {
        wacore_src: root.join("wacore/src"),
        core_src: root.join("src"),
    }
}

/// A package as the lock file describes it. `source` is absent for path
/// dependencies and patches, and names the registry or git remote otherwise.
struct LockedPackage {
    version: String,
    source: Option<String>,
}

/// Parse `[[package]]` stanzas rather than scanning loose lines, so a version
/// is never read from one package and a source from another.
fn locked_package(lock: &str, crate_name: &str) -> Option<LockedPackage> {
    for stanza in lock.split("[[package]]") {
        let field = |key: &str| {
            stanza.lines().find_map(|line| {
                line.trim()
                    .strip_prefix(&format!("{key} = "))
                    .map(|value| value.trim_matches('"').to_string())
            })
        };
        if field("name").as_deref() == Some(crate_name) {
            return Some(LockedPackage {
                version: field("version")?,
                source: field("source"),
            });
        }
    }
    None
}

/// The version to look for in a registry checkout, or `None` when the lock
/// points somewhere else.
///
/// A dependency temporarily on a git revision or a patched path can share a
/// version number with a registry copy still sitting in the cache. Reading that
/// copy would generate declarations for a revision Cargo is not going to
/// compile, silently and without falling back.
fn locked_registry_version(lock: &str, crate_name: &str) -> Option<String> {
    let package = locked_package(lock, crate_name)?;
    match package.source {
        Some(source) if source.starts_with("registry+") => Some(package.version),
        _ => None,
    }
}

/// Registry roots that hold both packages. `registry/src` can carry several —
/// a private registry beside crates.io, or leftovers from an older protocol —
/// and only some of them will have what is being looked for.
fn registry_root_with(core: &str, wacore: &str) -> Option<PathBuf> {
    let cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").expect("HOME not set");
        format!("{home}/.cargo")
    });
    std::fs::read_dir(PathBuf::from(cargo_home).join("registry/src"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|root| {
            root.join(format!("whatsapp-rust-{core}")).is_dir()
                && root.join(format!("wacore-{wacore}")).is_dir()
        })
}

/// Git checkout keyed by the locked commit, else a working clone beside this
/// repository.
fn find_repository_root(lock: &str) -> PathBuf {
    for line in lock.lines() {
        if line.contains("whatsapp-rust")
            && line.contains('#')
            && let Some(hash_start) = line.rfind('#')
        {
            let full_hash = line[hash_start + 1..].trim_end_matches('"');
            // Cargo checkouts use the first 7 chars of the commit hash.
            let short_hash = &full_hash[..7.min(full_hash.len())];
            let cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| {
                let home = std::env::var("HOME").expect("HOME not set");
                format!("{home}/.cargo")
            });
            let checkouts = PathBuf::from(&cargo_home).join("git/checkouts");
            if let Ok(entries) = std::fs::read_dir(&checkouts) {
                for entry in entries.flatten() {
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("whatsapp-rust-")
                    {
                        let candidate = entry.path().join(short_hash);
                        if candidate.exists() {
                            eprintln!("Using Cargo git cache: {}", candidate.display());
                            return candidate;
                        }
                    }
                }
            }
        }
    }

    let fallback = PathBuf::from("../../whatsapp-rust");
    eprintln!("Using local path: {}", fallback.display());
    fallback
}

/// Refuse to generate from sources that are not there. Parsing nothing is not
/// an empty result, it is a broken one.
///
/// Every directory the generator reads is checked, not just the first: wacore
/// alone contributes most of the types, so a tree missing only the core's own
/// `src/` would produce a plausible file and slip past an emptiness check.
fn require_sources(sources: &Sources) {
    let required = [
        sources.wacore_src.clone(),
        sources.core_src.join("features"),
        sources.core_src.join("types"),
        sources.core_src.join("send"),
    ];
    let missing: Vec<String> = required
        .iter()
        .filter(|path| !path.exists())
        .map(|path| path.display().to_string())
        .collect();

    assert!(
        missing.is_empty(),
        "whatsapp-rust sources are incomplete\n\
         Missing: {}\n\
         This generator reads the core's sources off disk rather than depending on it, so \n\
         `cargo fetch` has to have populated them — that is what `gen:bridge-types` runs \n\
         first. A sibling clone of whatsapp-rust also works.",
        missing.join(", ")
    );
}

fn main() {
    let sources = find_sources();
    require_sources(&sources);
    let wacore_dir = sources.wacore_src.clone();
    let src_dir = sources.core_src.clone();

    let mut all_types = BTreeMap::new();

    // Aliases first: a field's type is rendered as it is parsed, so an alias
    // declared in a later file would otherwise resolve to nothing.
    for entry in WalkDir::new(&wacore_dir)
        .into_iter()
        .chain(WalkDir::new(&src_dir))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "rs"))
    {
        collect_type_aliases(entry.path());
    }

    // Parse wacore types
    for entry in WalkDir::new(wacore_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "rs"))
    {
        parse_file(entry.path(), &mut all_types);
    }

    // Parse whatsapp-rust feature types. `send` replaces a hardcoded read of
    // `send.rs`, which the core has since split into a module: the old path
    // stopped matching and the call did nothing. Nothing was lost — the types
    // it named are not `Serialize`, so this generator never emitted them — but
    // pointing at what exists keeps the guard above honest.
    let feature_dirs = ["features", "types", "send"];
    for dir in feature_dirs {
        let path = src_dir.join(dir);
        if path.exists() {
            for entry in WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "rs"))
            {
                parse_file(entry.path(), &mut all_types);
            }
        }
    }

    // Also parse send.rs for SendOptions/RevokeType
    parse_file(&src_dir.join("send.rs"), &mut all_types);

    // Reported so a collapse is visible in the build log. Deliberately not
    // asserted against a floor: the real count is what it is, and a threshold
    // close to it breaks every legitimate change to the core. What guards the
    // output is `require_sources` above and the drift check in CI, which
    // compares against the committed file instead of guessing a number.
    eprintln!(
        "parsed {} types from {} and {}",
        all_types.len(),
        sources.wacore_src.display(),
        sources.core_src.display()
    );
    assert!(
        !all_types.is_empty(),
        "no types parsed: refusing to generate an empty type file"
    );
    assert_declarations_are_closed(&all_types);

    // Build TypeScript content
    let mut ts = String::new();

    // Jid is defined in wacore-binary with cfg_attr(feature = "serde"), so the codegen
    // can't discover it automatically. Hardcoded to match serde output.
    ts.push_str("/** WhatsApp JID (Jabber ID) — identifies a user, group, or device. */\n");
    ts.push_str("export interface Jid {\n");
    ts.push_str("  user: string;\n");
    ts.push_str("  server: string;\n");
    ts.push_str("  agent: number;\n");
    ts.push_str("  device: number;\n");
    ts.push_str("  integrator: number;\n");
    ts.push_str("}\n");

    // `serde_json::Value` is the core's own type for a payload whose shape the
    // server decides (MEX responses). It has no declaration to discover.
    ts.push_str("\n/** Any JSON value, as `serde_json::Value` crosses. */\n");
    ts.push_str(
        "export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };\n",
    );

    for (name, ts_type) in &all_types {
        ts.push('\n');
        ts.push_str(&ts_type.to_typescript(name));
        ts.push('\n');
    }

    // Output as a Rust source file with typescript_custom_section
    println!("//! Auto-generated TypeScript type declarations for wasm-bindgen.");
    println!("//! Generated by: bun run gen:types");
    println!("//! DO NOT EDIT — re-run gen:types to regenerate.");
    println!();
    println!("use wasm_bindgen::prelude::*;");
    println!();

    // Pick raw string delimiter that doesn't conflict with content
    let delim = if ts.contains("\"#") { "##" } else { "#" };

    println!("#[wasm_bindgen(typescript_custom_section)]");
    println!("const _TS_GENERATED_TYPES: &str = r{delim}\"");
    print!("{ts}");
    println!("\"{delim};");

    println!();
    println!("/// The core's `Event` variants, so `wasm_client`'s coverage test can");
    println!("/// measure the bridge's dispatch against something other than itself.");
    println!("#[cfg(test)]");
    println!("pub(crate) const CORE_EVENT_VARIANTS: &[&str] = &[");
    for variant in core_event_variants(&sources.wacore_src) {
        println!("    \"{variant}\",");
    }
    println!("];");
}

/// The core's `Event` variant names, read separately because the derive scan
/// skips `Event`. An absent enum panics rather than defaulting to an empty
/// list, which would pass every check it feeds.
fn core_event_variants(wacore_src: &Path) -> Vec<String> {
    let path = wacore_src.join("types/events.rs");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    core_event_variants_in(&content)
        .unwrap_or_else(|| panic!("{} declares no `enum Event`", path.display()))
}

fn core_event_variants_in(content: &str) -> Option<Vec<String>> {
    let file = syn::parse_file(content).ok()?;
    file.items.iter().find_map(|item| match item {
        Item::Enum(e) if e.ident == "Event" => {
            Some(e.variants.iter().map(|v| v.ident.to_string()).collect())
        }
        _ => None,
    })
}

#[derive(Debug)]
enum TsTypeDef {
    Interface {
        fields: Vec<TsField>,
        doc: Option<String>,
        generics: Vec<String>,
    },
    Enum {
        variants: Vec<TsEnumVariant>,
        doc: Option<String>,
        generics: Vec<String>,
        representation: EnumRepresentation,
    },
    StringEnum {
        variants: Vec<(String, String)>, // (name, value)
        /// `true` when the enum has a `#[wire_fallback]` variant (or
        /// equivalent). The TS type widens to include `string` so unknown
        /// wire tags don't fail consumer type checks.
        has_fallback: bool,
        doc: Option<String>,
    },
    /// Numeric enum (`#[wire(kind = "int")]`) — discriminator is `i32`.
    /// Renders as `export type X = number;` plus a doc comment listing the
    /// known codes for human reference.
    IntEnum {
        codes: Vec<(String, String)>, // (variant_name, numeric_literal)
        doc: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EnumRepresentation {
    /// `#[serde(tag = "...")]` or the equivalent `WireEnum` representation.
    Internal { tag: String },
    /// `#[serde(tag = "...", content = "...")]`.
    Adjacent { tag: String, content: String },
    /// Serde's default externally-tagged representation.
    External,
    /// `#[serde(untagged)]`.
    Untagged,
}

#[derive(Debug)]
struct TsField {
    name: String,
    ts_type: String,
    optional: bool,
    doc: Option<String>,
}

#[derive(Debug)]
enum TsEnumVariant {
    /// Unit variant. `wire` is the explicit discriminator string (from
    /// `#[wire = "..."]`); empty when the variant should fall back to
    /// `to_snake_case(name)`.
    Unit { name: String, wire: String },
    Tuple {
        name: String,
        wire: String,
        inner: String,
    },
    Struct {
        name: String,
        wire: String,
        fields: Vec<TsField>,
    },
    /// `#[wire_fallback]` catch-all for tagged-payload enums — emits
    /// `{ type: string; tag: string }` so any unknown discriminator still
    /// satisfies the union.
    Fallback,
}

impl TsTypeDef {
    fn to_typescript(&self, name: &str) -> String {
        match self {
            TsTypeDef::Interface {
                fields,
                doc,
                generics,
            } => {
                let mut out = String::new();
                if let Some(d) = doc {
                    out.push_str(&format!("/** {} */\n", d.trim()));
                }
                out.push_str(&format!(
                    "export interface {}{} {{\n",
                    name,
                    render_generics(generics)
                ));
                for f in fields {
                    if let Some(d) = &f.doc {
                        out.push_str(&format!("  /** {} */\n", d.trim()));
                    }
                    let opt = if f.optional { "?" } else { "" };
                    out.push_str(&format!(
                        "  {}{}: {};\n",
                        render_property_name(&f.name),
                        opt,
                        f.ts_type
                    ));
                }
                out.push('}');
                out
            }
            TsTypeDef::Enum {
                variants,
                doc,
                generics,
                representation,
            } => {
                let mut out = String::new();
                if let Some(d) = doc {
                    out.push_str(&format!("/** {} */\n", d.trim()));
                }
                out.push_str(&format!(
                    "export type {}{} =\n",
                    name,
                    render_generics(generics)
                ));
                let parts: Vec<String> = variants
                    .iter()
                    .map(|variant| render_enum_variant(variant, representation))
                    .collect();
                out.push_str(&parts.join("\n"));
                out.push(';');
                out
            }
            TsTypeDef::StringEnum {
                variants,
                has_fallback,
                doc,
            } => {
                let mut out = String::new();
                if let Some(d) = doc {
                    out.push_str(&format!("/** {} */\n", d.trim()));
                }
                let mut parts: Vec<String> =
                    variants.iter().map(|(_, v)| format!("\"{}\"", v)).collect();
                if *has_fallback {
                    parts.push("string".to_string());
                }
                out.push_str(&format!("export type {} = {};", name, parts.join(" | ")));
                out
            }
            TsTypeDef::IntEnum { codes, doc } => {
                let mut out = String::new();
                let combined_doc = match doc {
                    Some(d) => d.clone(),
                    None => String::new(),
                };
                let codes_doc = codes
                    .iter()
                    .map(|(n, v)| format!("{}={}", v, n))
                    .collect::<Vec<_>>()
                    .join(", ");
                let final_doc = if combined_doc.is_empty() {
                    format!("Wire codes: {}", codes_doc)
                } else {
                    format!("{} Wire codes: {}", combined_doc.trim(), codes_doc)
                };
                out.push_str(&format!("/** {} */\n", final_doc));
                out.push_str(&format!("export type {} = number;", name));
                out
            }
        }
    }
}

fn render_generics(generics: &[String]) -> String {
    if generics.is_empty() {
        String::new()
    } else {
        format!("<{}>", generics.join(", "))
    }
}

fn render_property_name(name: &str) -> String {
    let mut chars = name.chars();
    let valid_start = chars
        .next()
        .is_some_and(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphabetic());
    let valid_rest = chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric());
    if valid_start && valid_rest {
        name.to_string()
    } else {
        format!("\"{}\"", escape_ts_string(name))
    }
}

fn escape_ts_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn enum_variant_tag(name: &str, wire: &str) -> String {
    if wire.is_empty() {
        to_snake_case(name)
    } else {
        wire.to_string()
    }
}

fn render_inline_fields(fields: &[TsField]) -> String {
    fields
        .iter()
        .map(|field| {
            let optional = if field.optional { "?" } else { "" };
            format!(
                "{}{}: {}",
                render_property_name(&field.name),
                optional,
                field.ts_type
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn render_enum_variant(variant: &TsEnumVariant, representation: &EnumRepresentation) -> String {
    if matches!(variant, TsEnumVariant::Fallback) {
        return match representation {
            EnumRepresentation::Internal { tag } | EnumRepresentation::Adjacent { tag, .. } => {
                format!(
                    "  | {{ {}: string; tag: string }}",
                    render_property_name(tag)
                )
            }
            EnumRepresentation::External | EnumRepresentation::Untagged => "  | string".to_string(),
        };
    }

    let (name, wire) = match variant {
        TsEnumVariant::Unit { name, wire }
        | TsEnumVariant::Tuple { name, wire, .. }
        | TsEnumVariant::Struct { name, wire, .. } => (name, wire),
        TsEnumVariant::Fallback => unreachable!(),
    };
    let variant_name = enum_variant_tag(name, wire);
    let literal = format!("\"{}\"", escape_ts_string(&variant_name));

    match representation {
        EnumRepresentation::Internal { tag } => {
            let tag = render_property_name(tag);
            match variant {
                TsEnumVariant::Unit { .. } => format!("  | {{ {tag}: {literal} }}"),
                TsEnumVariant::Tuple { inner, .. } => {
                    format!("  | {{ {tag}: {literal}; data: {inner} }}")
                }
                TsEnumVariant::Struct { fields, .. } => {
                    let fields = render_inline_fields(fields);
                    if fields.is_empty() {
                        format!("  | {{ {tag}: {literal} }}")
                    } else {
                        format!("  | {{ {tag}: {literal}; {fields} }}")
                    }
                }
                TsEnumVariant::Fallback => unreachable!(),
            }
        }
        EnumRepresentation::Adjacent { tag, content } => {
            let tag = render_property_name(tag);
            let content = render_property_name(content);
            match variant {
                TsEnumVariant::Unit { .. } => format!("  | {{ {tag}: {literal} }}"),
                TsEnumVariant::Tuple { inner, .. } => {
                    format!("  | {{ {tag}: {literal}; {content}: {inner} }}")
                }
                TsEnumVariant::Struct { fields, .. } => {
                    let fields = render_inline_fields(fields);
                    format!("  | {{ {tag}: {literal}; {content}: {{ {fields} }} }}")
                }
                TsEnumVariant::Fallback => unreachable!(),
            }
        }
        EnumRepresentation::External => match variant {
            TsEnumVariant::Unit { .. } => format!("  | {literal}"),
            TsEnumVariant::Tuple { inner, .. } => {
                format!("  | {{ {literal}: {inner} }}")
            }
            TsEnumVariant::Struct { fields, .. } => {
                let fields = render_inline_fields(fields);
                format!("  | {{ {literal}: {{ {fields} }} }}")
            }
            TsEnumVariant::Fallback => unreachable!(),
        },
        EnumRepresentation::Untagged => match variant {
            TsEnumVariant::Unit { .. } => "  | null".to_string(),
            TsEnumVariant::Tuple { inner, .. } => format!("  | {inner}"),
            TsEnumVariant::Struct { fields, .. } => {
                format!("  | {{ {} }}", render_inline_fields(fields))
            }
            TsEnumVariant::Fallback => unreachable!(),
        },
    }
}

fn parse_file(path: &Path, types: &mut BTreeMap<String, TsTypeDef>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    parse_source(&content, types);
}

fn parse_source(content: &str, types: &mut BTreeMap<String, TsTypeDef>) {
    let Ok(file) = syn::parse_file(content) else {
        return;
    };

    // Every struct in the file, private ones included, so a container that
    // serializes `into` a shadow can be described by the shadow it actually
    // writes. Shadows are private by convention, so the `is_pub` filter below
    // would never reach them.
    let shadows: std::collections::HashMap<String, &syn::ItemStruct> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(s) => Some((s.ident.to_string(), s)),
            _ => None,
        })
        .collect();

    for item in &file.items {
        match item {
            Item::Struct(s) if has_serde_derive(&s.attrs) && is_pub(&s.vis) => {
                let name = s.ident.to_string();

                // Skip types that are already generated by Tsify derives in
                // result_types.rs — avoids duplicate export conflicts.
                const TSIFY_STRUCTS: &[&str] = &[
                    "BusinessProfile",
                    "BusinessCategory",
                    "BusinessHours",
                    "BusinessHoursConfig",
                    "GroupMetadataParticipant",
                    "MembershipRequest",
                ];
                if TSIFY_STRUCTS.contains(&name.as_str()) {
                    continue;
                }

                let doc = extract_doc(&s.attrs);
                // A `serde(into)` / `serde(from)` container never writes its own
                // fields: the shadow it converts through is the shape that
                // crosses the boundary, and the two disagree on purpose (a
                // packed layout, a field split into flags). Describe the shadow.
                let shape = match serde_container_shadow(&s.attrs).filter(|shadow| *shadow != name)
                {
                    Some(shadow) => shadows.get(&shadow).copied().unwrap_or_else(|| {
                        panic!(
                            "`{name}` serializes through `{shadow}`, which is not declared in the \
                             same file. This generator resolves a shadow only among its own \
                             file's items."
                        )
                    }),
                    None => s,
                };
                let serde = parse_serde_container(&shape.attrs);
                let generics = type_parameters(&shape.generics);
                // A shadow is a codec detail and is rarely documented, while
                // the container it stands in for is. Where the two agree on a
                // field name, the container's doc is about that same value, so
                // it carries over rather than being dropped.
                let outer_docs: std::collections::HashMap<String, String> = match &s.fields {
                    Fields::Named(named) => named
                        .named
                        .iter()
                        .filter_map(|f| {
                            Some((f.ident.as_ref()?.to_string(), extract_doc(&f.attrs)?))
                        })
                        .collect(),
                    _ => std::collections::HashMap::new(),
                };
                let fields = match &shape.fields {
                    Fields::Named(named) => named
                        .named
                        .iter()
                        // Rust visibility is irrelevant to Serde. Private
                        // fields are part of the host ABI unless Serde
                        // explicitly skips them.
                        .filter(|field| !has_serde_skip(&field.attrs))
                        .map(|f| {
                            let field_name =
                                f.ident.as_ref().unwrap().to_string().replace("r#", "");
                            let (ts_type, optional) = timestamp_module_type(f)
                                .or_else(|| serialize_with_type(f))
                                .unwrap_or_else(|| rust_type_to_ts(&f.ty));
                            let serde_name = get_serde_rename(f);
                            TsField {
                                name: serde_name.unwrap_or_else(|| {
                                    apply_rename_rule(&field_name, serde.rename_all.as_deref())
                                }),
                                ts_type,
                                optional,
                                doc: extract_doc(&f.attrs)
                                    .or_else(|| outer_docs.get(&field_name).cloned()),
                            }
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                if !fields.is_empty() {
                    types.insert(
                        name,
                        TsTypeDef::Interface {
                            fields,
                            doc,
                            generics,
                        },
                    );
                }
            }
            Item::Enum(e) => {
                if !is_pub(&e.vis) {
                    continue;
                }

                // Skip types already generated by Tsify in result_types.rs
                let ename = e.ident.to_string();
                if TSIFY_GENERATED.contains(&ename.as_str()) {
                    continue;
                }

                // Sibling `<Name>Tag` enums auto-generated by `WireEnum`'s
                // tagged mode are an internal parser-dispatch helper — they
                // exist only after macro expansion (not in source), so the
                // `syn`-based codegen never sees them. No filter needed here;
                // documenting for the next reader.

                let name = e.ident.to_string();

                // Recognize `#[derive(WireEnum)]` (the unified successor to
                // `#[derive(StringEnum)]` plus the hand-written `impl
                // Serialize` blocks from before whatsapp-rust PR #567).
                // Three modes inferred from enum-level attributes:
                //   - unit-string  (default — drop-in `StringEnum` replacement)
                //   - tagged       (`#[wire(tag = "type")]` — payload variants)
                //   - int          (`#[wire(kind = "int")]`)
                if has_wire_enum_derive(&e.attrs) {
                    match wire_enum_kind(&e.attrs) {
                        WireEnumKind::Int => {
                            let codes: Vec<(String, String)> = e
                                .variants
                                .iter()
                                .filter(|v| !is_wire_fallback(&v.attrs))
                                .map(|v| {
                                    (
                                        v.ident.to_string(),
                                        get_wire_attr_lit(&v.attrs).unwrap_or_default(),
                                    )
                                })
                                .filter(|(_, lit)| !lit.is_empty())
                                .collect();
                            types.insert(
                                name,
                                TsTypeDef::IntEnum {
                                    codes,
                                    doc: extract_doc(&e.attrs),
                                },
                            );
                            continue;
                        }
                        WireEnumKind::Tagged {
                            discriminator,
                            content,
                        } => {
                            let doc = extract_doc(&e.attrs);
                            let variants: Vec<TsEnumVariant> = e
                                .variants
                                .iter()
                                .map(|v| {
                                    let vname = v.ident.to_string();
                                    if is_wire_fallback(&v.attrs) {
                                        return TsEnumVariant::Fallback;
                                    }
                                    let wire = get_wire_attr_lit(&v.attrs).unwrap_or_default();
                                    match &v.fields {
                                        Fields::Unit => TsEnumVariant::Unit { name: vname, wire },
                                        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
                                            let (ts, _) = rust_type_to_ts(&unnamed.unnamed[0].ty);
                                            TsEnumVariant::Tuple {
                                                name: vname,
                                                wire,
                                                inner: ts,
                                            }
                                        }
                                        Fields::Unnamed(_) => {
                                            TsEnumVariant::Unit { name: vname, wire }
                                        }
                                        Fields::Named(named) => {
                                            let fields: Vec<TsField> = named
                                                .named
                                                .iter()
                                                .filter(|f| !has_wire_skip(&f.attrs))
                                                .map(|f| {
                                                    let fname =
                                                        f.ident.as_ref().unwrap().to_string();
                                                    let (ts, opt) = rust_type_to_ts(&f.ty);
                                                    TsField {
                                                        name: fname,
                                                        ts_type: ts,
                                                        optional: opt,
                                                        doc: extract_doc(&f.attrs),
                                                    }
                                                })
                                                .collect();
                                            TsEnumVariant::Struct {
                                                name: vname,
                                                wire,
                                                fields,
                                            }
                                        }
                                    }
                                })
                                .collect();
                            types.insert(
                                name,
                                TsTypeDef::Enum {
                                    variants,
                                    doc,
                                    generics: type_parameters(&e.generics),
                                    representation: match content {
                                        Some(content) => EnumRepresentation::Adjacent {
                                            tag: discriminator,
                                            content,
                                        },
                                        None => EnumRepresentation::Internal { tag: discriminator },
                                    },
                                },
                            );
                            continue;
                        }
                        WireEnumKind::UnitString => {
                            let doc = extract_doc(&e.attrs);
                            let mut has_fallback = false;
                            let variants: Vec<(String, String)> = e
                                .variants
                                .iter()
                                .filter_map(|v| {
                                    if is_wire_fallback(&v.attrs) {
                                        has_fallback = true;
                                        return None;
                                    }
                                    let vname = v.ident.to_string();
                                    let value = get_wire_attr_lit(&v.attrs)
                                        .unwrap_or_else(|| to_snake_case(&vname));
                                    Some((vname, value))
                                })
                                .collect();
                            types.insert(
                                name,
                                TsTypeDef::StringEnum {
                                    variants,
                                    has_fallback,
                                    doc,
                                },
                            );
                            continue;
                        }
                    }
                }

                // `ReceiptType` writes its `Serialize` out by hand, in the shape
                // the derive it replaced produced (whatsapp-rust keeps the
                // indices in declaration order for exactly that reason). The
                // derive scan cannot see it, so the name is listed here — the
                // variants still come from the enum body, so one added upstream
                // lands in the declaration rather than going stale.
                const MANUAL_SERIALIZE_ENUMS: &[&str] = &["ReceiptType"];
                if !has_serde_derive(&e.attrs) && !MANUAL_SERIALIZE_ENUMS.contains(&name.as_str()) {
                    continue;
                }

                // Skip types that are already generated by Tsify derives in
                // result_types.rs — avoids duplicate export conflicts.
                const TSIFY_GENERATED: &[&str] = &[
                    "Event",                  // WhatsAppEvent via bridge_events! macro
                    "MemberAddMode",          // Tsify enum in result_types.rs
                    "MembershipApprovalMode", // Used as bool param, not needed
                    "PresenceStatus",         // Tsify enum in result_types.rs
                    "MembershipRequest",      // MembershipRequestResult in result_types.rs
                ];
                if TSIFY_GENERATED.contains(&name.as_str()) {
                    continue;
                }
                let doc = extract_doc(&e.attrs);
                let serde = parse_serde_container(&e.attrs);
                let representation = serde.enum_representation();
                let generics = type_parameters(&e.generics);

                // Serde's default externally-tagged unit enum is a string
                // union. Explicitly tagged and untagged enums keep their
                // configured representation even when every variant is unit.
                let all_unit = e
                    .variants
                    .iter()
                    .filter(|variant| !has_serde_skip(&variant.attrs))
                    .all(|variant| matches!(variant.fields, Fields::Unit));
                if all_unit && representation == EnumRepresentation::External {
                    let variants: Vec<(String, String)> = e
                        .variants
                        .iter()
                        .filter(|variant| !has_serde_skip(&variant.attrs))
                        .map(|v| {
                            let vname = v.ident.to_string();
                            let value = get_serde_rename_variant(v).unwrap_or_else(|| {
                                apply_rename_rule(&vname, serde.rename_all.as_deref())
                            });
                            (vname, value)
                        })
                        .collect();
                    types.insert(
                        name,
                        TsTypeDef::StringEnum {
                            variants,
                            has_fallback: false,
                            doc,
                        },
                    );
                } else {
                    // Tagged enum (serde-derived, no WireEnum)
                    let variants: Vec<TsEnumVariant> = e
                        .variants
                        .iter()
                        .filter(|variant| !has_serde_skip(&variant.attrs))
                        .map(|v| {
                            let vname = v.ident.to_string();
                            let wire = get_serde_rename_variant(v).unwrap_or_else(|| {
                                apply_rename_rule(&vname, serde.rename_all.as_deref())
                            });
                            match &v.fields {
                                Fields::Unit => TsEnumVariant::Unit { name: vname, wire },
                                Fields::Unnamed(unnamed) => {
                                    if unnamed.unnamed.len() == 1 {
                                        let (ts, _) = rust_type_to_ts(&unnamed.unnamed[0].ty);
                                        TsEnumVariant::Tuple {
                                            name: vname,
                                            wire,
                                            inner: ts,
                                        }
                                    } else {
                                        TsEnumVariant::Tuple {
                                            name: vname,
                                            wire,
                                            inner: tuple_fields_to_ts(&unnamed.unnamed),
                                        }
                                    }
                                }
                                Fields::Named(named) => {
                                    let fields: Vec<TsField> = named
                                        .named
                                        .iter()
                                        .filter(|field| !has_serde_skip(&field.attrs))
                                        .map(|f| {
                                            let fname = f.ident.as_ref().unwrap().to_string();
                                            let (ts, opt) = rust_type_to_ts(&f.ty);
                                            TsField {
                                                name: get_serde_rename(f).unwrap_or_else(|| {
                                                    apply_rename_rule(
                                                        &fname,
                                                        serde.rename_all_fields.as_deref(),
                                                    )
                                                }),
                                                ts_type: ts,
                                                optional: opt,
                                                doc: extract_doc(&f.attrs),
                                            }
                                        })
                                        .collect();
                                    TsEnumVariant::Struct {
                                        name: vname,
                                        wire,
                                        fields,
                                    }
                                }
                            }
                        })
                        .collect();
                    types.insert(
                        name,
                        TsTypeDef::Enum {
                            variants,
                            doc,
                            generics,
                            representation,
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

fn has_serde_derive(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("derive") {
            return false;
        }
        let Ok(meta) = a.parse_args::<proc_macro2::TokenStream>() else {
            return false;
        };
        let s = meta.to_string();
        s.contains("Serialize") || s.contains("serde :: Serialize")
    })
}

#[derive(Debug, Default)]
struct SerdeContainer {
    tag: Option<String>,
    content: Option<String>,
    rename_all: Option<String>,
    rename_all_fields: Option<String>,
    untagged: bool,
}

impl SerdeContainer {
    fn enum_representation(&self) -> EnumRepresentation {
        if self.untagged {
            EnumRepresentation::Untagged
        } else if let Some(tag) = &self.tag {
            if let Some(content) = &self.content {
                EnumRepresentation::Adjacent {
                    tag: tag.clone(),
                    content: content.clone(),
                }
            } else {
                EnumRepresentation::Internal { tag: tag.clone() }
            }
        } else {
            EnumRepresentation::External
        }
    }
}

fn parse_serde_container(attrs: &[Attribute]) -> SerdeContainer {
    SerdeContainer {
        tag: serde_string_argument(attrs, "tag"),
        content: serde_string_argument(attrs, "content"),
        rename_all: serde_string_argument(attrs, "rename_all"),
        rename_all_fields: serde_string_argument(attrs, "rename_all_fields"),
        untagged: serde_has_flag(attrs, "untagged"),
    }
}

fn serde_attribute_tokens(attrs: &[Attribute]) -> impl Iterator<Item = String> + '_ {
    attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("serde"))
        .filter_map(|attribute| {
            attribute
                .parse_args::<proc_macro2::TokenStream>()
                .ok()
                .map(|tokens| tokens.to_string())
        })
}

fn serde_string_argument(attrs: &[Attribute], key: &str) -> Option<String> {
    let needle = format!("{key} = \"");
    for tokens in serde_attribute_tokens(attrs) {
        if let Some(start) = tokens.find(&needle) {
            let value = &tokens[start + needle.len()..];
            if let Some(end) = value.find('"') {
                return Some(value[..end].to_string());
            }
        }
    }
    None
}

fn serde_has_flag(attrs: &[Attribute], flag: &str) -> bool {
    serde_attribute_tokens(attrs).any(|tokens| {
        tokens
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .any(|word| word == flag)
    })
}

fn has_serde_skip(attrs: &[Attribute]) -> bool {
    serde_has_flag(attrs, "skip")
}

fn type_parameters(generics: &syn::Generics) -> Vec<String> {
    let parameters: Vec<String> = generics
        .type_params()
        .map(|parameter| parameter.ident.to_string())
        .collect();
    generic_parameter_names()
        .lock()
        .unwrap()
        .extend(parameters.iter().cloned());
    parameters
}

fn tuple_fields_to_ts(fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>) -> String {
    let fields = fields
        .iter()
        .map(|field| rust_type_to_ts(&field.ty).0)
        .collect::<Vec<_>>();
    format!("[{}]", fields.join(", "))
}

fn apply_rename_rule(value: &str, rule: Option<&str>) -> String {
    match rule {
        Some("lowercase") => value.to_ascii_lowercase(),
        Some("UPPERCASE") => value.to_ascii_uppercase(),
        Some("PascalCase") => uppercase_first(value),
        Some("camelCase") => lowercase_first(value),
        Some("snake_case") => to_snake_case(value),
        Some("SCREAMING_SNAKE_CASE") => to_snake_case(value).to_ascii_uppercase(),
        Some("kebab-case") => to_snake_case(value).replace('_', "-"),
        Some("SCREAMING-KEBAB-CASE") => to_snake_case(value).replace('_', "-").to_ascii_uppercase(),
        // Unknown future Serde rename rules are deliberately not guessed.
        // Keeping the source identifier makes drift visible in generator tests.
        Some(_) | None => value.to_string(),
    }
}

fn lowercase_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn uppercase_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Detect `#[derive(WireEnum)]` (the unified successor to `StringEnum` plus
/// hand-written `impl Serialize` blocks). Matches both `WireEnum` and
/// `crate :: WireEnum` / `wacore :: WireEnum` forms produced by `quote!`.
fn has_wire_enum_derive(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("derive") {
            return false;
        }
        let Ok(meta) = a.parse_args::<proc_macro2::TokenStream>() else {
            return false;
        };
        let s = meta.to_string();
        s.contains("WireEnum")
    })
}

#[derive(Debug)]
enum WireEnumKind {
    UnitString,
    Tagged {
        discriminator: String,
        content: Option<String>,
    },
    Int,
}

/// Inspect enum-level `#[wire(...)]` attribute to determine which mode the
/// `WireEnum` derive runs in. Mirrors `wacore_derive`'s `parse_enum_level_wire`.
fn wire_enum_kind(attrs: &[Attribute]) -> WireEnumKind {
    let mut tag_field: Option<String> = None;
    let mut content_field: Option<String> = None;
    let mut int_kind = false;
    for attr in attrs {
        if !attr.path().is_ident("wire") {
            continue;
        }
        // `#[wire(tag = "type")]` and `#[wire(kind = "int")]` use parenthesized
        // nested meta. `#[wire = "..."]` uses NameValue and isn't enum-level.
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("tag") {
                if let Ok(value) = meta.value()
                    && let Ok(lit) = value.parse::<syn::LitStr>()
                {
                    tag_field = Some(lit.value());
                }
            } else if meta.path.is_ident("content") {
                if let Ok(value) = meta.value()
                    && let Ok(lit) = value.parse::<syn::LitStr>()
                {
                    content_field = Some(lit.value());
                }
            } else if meta.path.is_ident("kind")
                && let Ok(value) = meta.value()
                && let Ok(lit) = value.parse::<syn::LitStr>()
                && lit.value() == "int"
            {
                int_kind = true;
            }
            Ok(())
        });
    }
    if int_kind {
        WireEnumKind::Int
    } else if let Some(discriminator) = tag_field {
        WireEnumKind::Tagged {
            discriminator,
            content: content_field,
        }
    } else {
        WireEnumKind::UnitString
    }
}

/// Read `#[wire = "..."]` (string mode) or `#[wire = NUM]` (int mode) from
/// a variant. Returns the literal as a string in both cases — caller decides
/// how to render. Returns `None` for variants without the attribute (e.g.
/// `#[wire_fallback]` ones).
fn get_wire_attr_lit(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if !attr.path().is_ident("wire") {
            continue;
        }
        // `#[wire = "..."]` / `#[wire = 101]` — NameValue form, not parenthesized.
        if let syn::Meta::NameValue(nv) = &attr.meta
            && let syn::Expr::Lit(lit) = &nv.value
        {
            return match &lit.lit {
                syn::Lit::Str(s) => Some(s.value()),
                syn::Lit::Int(i) => Some(i.base10_digits().to_string()),
                _ => None,
            };
        }
    }
    None
}

fn is_wire_fallback(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("wire_fallback"))
}

/// Field-level `#[wire(skip)]` — exclude the field from the JSON contract
/// (used for `raw: Node` payloads on `Create`/`Link`/`Unlink`).
fn has_wire_skip(attrs: &[Attribute]) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("wire") {
            continue;
        }
        let mut skip = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                skip = true;
            }
            Ok(())
        });
        if skip {
            return true;
        }
    }
    false
}

fn is_pub(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
}

fn extract_doc(attrs: &[Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| {
            if let syn::Meta::NameValue(nv) = &a.meta
                && let syn::Expr::Lit(lit) = &nv.value
                && let syn::Lit::Str(s) = &lit.lit
            {
                return Some(s.value().trim().to_string());
            }
            None
        })
        .collect();
    if docs.is_empty() {
        None
    } else {
        Some(docs.join(" "))
    }
}

fn get_serde_rename(f: &syn::Field) -> Option<String> {
    serde_string_argument(&f.attrs, "rename")
}

/// A `DateTime` written through one of chrono's timestamp modules, which
/// replace the RFC 3339 string with an integer. The module is named on the
/// field, so the type alone cannot tell the two apart.
///
/// Named one by one rather than by an `ts_` prefix: a module this generator has
/// not read is a representation it cannot name, and guessing one publishes a
/// declaration nobody honours. An unknown one on a `DateTime` field stops
/// generation instead, the way an unplaceable type already does.
fn timestamp_module_type(field: &syn::Field) -> Option<(String, bool)> {
    if !is_datetime(&field.ty) {
        return None;
    }
    let module = serde_timestamp_module(&field.attrs)?;
    let module = module.rsplit("::").next().unwrap_or_default();
    match module {
        "ts_seconds" | "ts_milliseconds" | "ts_microseconds" | "ts_nanoseconds" => {
            Some(("number".to_string(), false))
        }
        "ts_seconds_option"
        | "ts_milliseconds_option"
        | "ts_microseconds_option"
        | "ts_nanoseconds_option" => Some(("number | null".to_string(), true)),
        other => panic!(
            "a DateTime field is serialized through `{other}`, whose shape this generator \
             cannot name. Add it beside chrono's timestamp modules."
        ),
    }
}

/// The shadow struct a container serializes through, from `serde(into)`.
///
/// Only `into`. A declaration describes what crosses the boundary, and
/// `from` / `try_from` name the deserialize direction, which can differ or
/// even point back at the container itself for validate-on-load. A container
/// carrying only those still writes its own fields.
fn serde_container_shadow(attrs: &[Attribute]) -> Option<String> {
    let path = serde_attribute_tokens(attrs)
        .into_iter()
        .find_map(|tokens| keyed_string(&tokens, "into").map(ToOwned::to_owned))?;
    // Generic arguments are the container's own, so the shadow resolves by
    // name: `into = "Packed<T>"` is the `Packed` declared alongside it.
    let name = path.split('<').next().unwrap_or_default();
    Some(
        name.rsplit("::")
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned(),
    )
}

/// A field written through one of the core's own `serialize_with` functions,
/// which can replace the field's Rust shape with one the type does not
/// describe.
///
/// Named one by one for the reason chrono's modules are: a function this
/// generator has not read is a representation it cannot name, and guessing one
/// publishes a declaration nobody honours. `None` means the field's own type
/// already names what the function writes; an unknown function stops
/// generation, the way an unplaceable type already does.
fn serialize_with_type(field: &syn::Field) -> Option<(String, bool)> {
    // A `DateTime` reaches its module through `timestamp_module_type`, which
    // reads `with` as well and runs first.
    if is_datetime(&field.ty) {
        return None;
    }
    let path = serde_serialize_with(&field.attrs)?;
    match path.rsplit("::").next().unwrap_or_default() {
        // Writes the bytes the field's own `Option<Vec<u8>>` already names.
        "serialize_optional_bytes" => None,
        // `GroupInfo::lid_pn` is a slice of pairs sorted for binary search that
        // writes the `lid_to_pn_map` object the persisted blob has always
        // carried, so the type and the wire shape disagree by design.
        "serialize_lid_pn" => Some(("Record<string, Jid>".to_string(), false)),
        other => panic!(
            "field `{}` is serialized through `{other}`, whose shape this generator cannot \
             name. Add it beside the ones already listed in `serialize_with_type`.",
            field
                .ident
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        ),
    }
}

/// The function a field's `serialize_with` names, ignoring `with` — unlike
/// [`serde_timestamp_module`], which reads both. `with = "serde_bytes"` names a
/// module whose shape the field's type already carries, so reading it here
/// would put every byte field through the table for nothing.
fn serde_serialize_with(attrs: &[Attribute]) -> Option<String> {
    serde_attribute_tokens(attrs)
        .into_iter()
        .find_map(|tokens| keyed_string(&tokens, "serialize_with").map(ToOwned::to_owned))
}

/// The module a field's `with`, or the function its `serialize_with`, names.
///
/// `serialize_with` first, and matched at a word boundary, because `with = "`
/// is a substring of `serialize_with = "` — reading the outer key off the inner
/// one names `serialize` as the module. Only the serialize direction shows up
/// in a declaration, so a lone `deserialize_with` is nothing to read.
fn serde_timestamp_module(attrs: &[Attribute]) -> Option<String> {
    for tokens in serde_attribute_tokens(attrs) {
        if let Some(path) = keyed_string(&tokens, "serialize_with") {
            return Some(path.trim_end_matches("::serialize").to_owned());
        }
        if let Some(path) = keyed_string(&tokens, "with") {
            return Some(path.to_owned());
        }
    }
    None
}

/// The string after `key = ` in an attribute's tokens, where `key` is not the
/// tail of a longer identifier.
fn keyed_string<'a>(tokens: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key} = \"");
    let mut from = 0;
    while let Some(offset) = tokens[from..].find(&needle) {
        let at = from + offset;
        let value = at + needle.len();
        let end = value + tokens[value..].find('"')?;
        let preceded = tokens[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        if !preceded {
            return Some(&tokens[value..end]);
        }
        from = end;
    }
    None
}

/// A `chrono::DateTime`, through the wrappers a field can carry it behind.
fn is_datetime(ty: &Type) -> bool {
    match ty {
        Type::Path(TypePath { path, .. }) => {
            let Some(last) = path.segments.last() else {
                return false;
            };
            if last.ident == "DateTime" {
                return true;
            }
            match last.ident.to_string().as_str() {
                "Option" | "Box" | "Arc" | "Rc" | "Cow" | "Vec" => {
                    let PathArguments::AngleBracketed(args) = &last.arguments else {
                        return false;
                    };
                    args.args
                        .iter()
                        .find_map(|arg| match arg {
                            GenericArgument::Type(inner) => Some(is_datetime(inner)),
                            _ => None,
                        })
                        .unwrap_or(false)
                }
                _ => false,
            }
        }
        Type::Reference(r) => is_datetime(&r.elem),
        _ => false,
    }
}

fn get_serde_rename_variant(v: &syn::Variant) -> Option<String> {
    serde_string_argument(&v.attrs, "rename")
}

/// Map a Rust type to a TypeScript type string.
/// Returns (ts_type, is_optional).
fn rust_type_to_ts(ty: &Type) -> (String, bool) {
    match ty {
        Type::Path(TypePath { path, .. }) => {
            let last = path.segments.last().unwrap();
            let ident = last.ident.to_string();

            match ident.as_str() {
                // Primitives
                "String" | "str" | "CompactString" => ("string".to_string(), false),
                "bool" => ("boolean".to_string(), false),
                "u8" | "u16" | "u32" | "i8" | "i16" | "i32" | "f32" | "f64" => {
                    ("number".to_string(), false)
                }
                // The bridge emits safe values as Number and falls back to a
                // decimal string outside Number's exact integer range.
                "u64" | "i64" | "u128" | "i128" => ("number | string".to_string(), false),
                "usize" | "isize" => ("number".to_string(), false),

                // Option<T> → T | null (optional field)
                "Option" => {
                    if let PathArguments::AngleBracketed(args) = &last.arguments
                        && let Some(GenericArgument::Type(inner)) = args.args.first()
                    {
                        let (inner_ts, _) = rust_type_to_ts(inner);
                        return (append_null(inner_ts), true);
                    }
                    ("any".to_string(), true)
                }

                // Vec<u8> → Uint8Array, Vec<T> → T[]
                "Vec" => {
                    if let PathArguments::AngleBracketed(args) = &last.arguments
                        && let Some(GenericArgument::Type(inner)) = args.args.first()
                    {
                        if is_u8(inner) {
                            return ("Uint8Array".to_string(), false);
                        }
                        let (inner_ts, _) = rust_type_to_ts(inner);
                        return (array_type(inner_ts), false);
                    }
                    ("any[]".to_string(), false)
                }

                // `SmallVec<[T; N]>` serializes as a sequence — the inline
                // capacity is an allocation strategy, not a wire form, so it
                // names whatever the equivalent `Vec<T>` would.
                "SmallVec" => {
                    if let PathArguments::AngleBracketed(args) = &last.arguments
                        && let Some(inner) = args.args.iter().find_map(|arg| match arg {
                            GenericArgument::Type(inner) => Some(inner),
                            _ => None,
                        })
                    {
                        return match inner {
                            Type::Array(_) => rust_type_to_ts(inner),
                            other => (array_type(rust_type_to_ts(other).0), false),
                        };
                    }
                    ("any[]".to_string(), false)
                }

                "HashSet" | "BTreeSet" => {
                    if let PathArguments::AngleBracketed(args) = &last.arguments
                        && let Some(GenericArgument::Type(inner)) = args.args.first()
                    {
                        return (array_type(rust_type_to_ts(inner).0), false);
                    }
                    ("any[]".to_string(), false)
                }

                // Box<T>, Arc<T>, Rc<T>, Cow<'a, T> → unwrap inner. Serde writes
                // the pointee, so the wrapper has no wire form to name. The first
                // *type* argument is taken rather than the first argument: `Cow`
                // leads with a lifetime, and reading position zero would miss it
                // and leave the Rust name in the emitted TypeScript.
                "Box" | "Arc" | "Rc" | "Cow" => {
                    if let PathArguments::AngleBracketed(args) = &last.arguments
                        && let Some(inner) = args.args.iter().find_map(|arg| match arg {
                            GenericArgument::Type(inner) => Some(inner),
                            _ => None,
                        })
                    {
                        return rust_type_to_ts(inner);
                    }
                    ("any".to_string(), false)
                }

                // HashMap<K, V> → Record<K, V>
                "HashMap" | "BTreeMap" => {
                    if let PathArguments::AngleBracketed(args) = &last.arguments {
                        let mut iter = args.args.iter();
                        if let (Some(GenericArgument::Type(k)), Some(GenericArgument::Type(v))) =
                            (iter.next(), iter.next())
                        {
                            let (kt, _) = rust_type_to_ts(k);
                            let (vt, _) = rust_type_to_ts(v);
                            return (format!("Record<{}, {}>", kt, vt), false);
                        }
                    }
                    ("Record<string, any>".to_string(), false)
                }

                // Duration → number (milliseconds)
                "Duration" => ("number".to_string(), false),

                // DateTime → chrono's RFC 3339 string, which is what its plain
                // `Serialize` writes. A field annotated with one of chrono's
                // `ts_*` modules writes a number instead; that is read off the
                // field, which this sees nothing of.
                "DateTime" => ("string".to_string(), false),

                // Jid → Jid (structured object with user, server, device, etc.)
                "Jid" => ("Jid".to_string(), false),

                // MessageId → string
                "MessageId" => ("string".to_string(), false),

                // MessageServerId → number
                "MessageServerId" => ("number".to_string(), false),

                // Node → any (binary protocol node)
                "Node" => ("any".to_string(), false),

                // Bytes → Uint8Array
                "Bytes" => ("Uint8Array".to_string(), false),

                // serde_json::Value → the JSON shape it serializes to. The core
                // imports it bare in places, so the path is not always there to
                // match on; nothing else outside waproto is named `Value`.
                "Value" if !path_contains_wa(path) => ("JsonValue".to_string(), false),

                // Default: a proto type resolves to the declaration
                // `proto-types.d.ts` already publishes for it, an alias
                // resolves to what it aliases, and anything else is assumed to
                // be another generated declaration.
                other => {
                    if let Some(reference) = proto_type_reference(path) {
                        return (reference, false);
                    }
                    if let Some(aliased) = resolve_type_alias(other) {
                        return aliased;
                    }
                    (render_named_type(other, &last.arguments), false)
                }
            }
        }
        Type::Reference(r) => rust_type_to_ts(&r.elem),
        Type::Tuple(t) if t.elems.is_empty() => ("void".to_string(), false),
        Type::Tuple(t) => {
            let values = t
                .elems
                .iter()
                .map(|element| rust_type_to_ts(element).0)
                .collect::<Vec<_>>();
            (format!("[{}]", values.join(", ")), false)
        }
        Type::Array(array) if is_u8(&array.elem) => ("Uint8Array".to_string(), false),
        Type::Array(array) => (array_type(rust_type_to_ts(&array.elem).0), false),
        Type::Slice(slice) if is_u8(&slice.elem) => ("Uint8Array".to_string(), false),
        Type::Slice(slice) => (array_type(rust_type_to_ts(&slice.elem).0), false),
        _ => ("any".to_string(), false),
    }
}

fn append_null(ts_type: String) -> String {
    if ts_type.split('|').any(|part| part.trim() == "null") {
        ts_type
    } else {
        format!("{ts_type} | null")
    }
}

fn array_type(element: String) -> String {
    if element.contains(" | ") {
        format!("({element})[]")
    } else {
        format!("{element}[]")
    }
}

fn render_named_type(name: &str, arguments: &PathArguments) -> String {
    referenced_names().lock().unwrap().insert(name.to_string());
    let PathArguments::AngleBracketed(arguments) = arguments else {
        return name.to_string();
    };
    let arguments = arguments
        .args
        .iter()
        .filter_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(rust_type_to_ts(ty).0),
            _ => None,
        })
        .collect::<Vec<_>>();
    if arguments.is_empty() {
        name.to_string()
    } else {
        format!("{name}<{}>", arguments.join(", "))
    }
}

fn is_u8(ty: &Type) -> bool {
    if let Type::Path(TypePath { path, .. }) = ty {
        path.segments.last().is_some_and(|s| s.ident == "u8")
    } else {
        false
    }
}

fn path_contains_wa(path: &syn::Path) -> bool {
    path.segments
        .iter()
        .any(|s| s.ident == "wa" || s.ident == "waproto")
}

/// Every message and enum `whatsapp.proto` declares, under the dotted names
/// `proto-types.d.ts` nests its declarations by.
#[derive(Default)]
struct ProtoSchema {
    messages: BTreeSet<String>,
    enums: BTreeSet<String>,
}

/// The schema manifest `scripts/gen-ts-proto.ts` writes beside the codec, off
/// the same protoc invocation. Reading it rather than reconstructing names from
/// prost's module paths is what makes a reference exact: prost lowercases a
/// parent message into a module (`SyncActionValue` → `sync_action_value`), and
/// no casing rule turns every one of those back into the name protobufjs nests
/// under (`ADVSignedDeviceIdentity` is not `AdvSignedDeviceIdentity`).
fn proto_schema() -> &'static ProtoSchema {
    static SCHEMA: OnceLock<ProtoSchema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        const MANIFEST: &str = "../ts/generated/whatsapp-surface.txt";
        let manifest = std::fs::read_to_string(MANIFEST).unwrap_or_else(|error| {
            panic!(
                "cannot read {MANIFEST}: {error}\n\
                 It is the schema manifest `bun run gen:proto-codec` writes, and the \n\
                 generated declarations resolve their waproto references against it."
            )
        });
        let mut schema = ProtoSchema::default();
        for line in manifest.lines().filter(|line| !line.starts_with('#')) {
            let columns: Vec<&str> = line.split('\t').collect();
            let Some(declared) = columns.first().filter(|name| !name.is_empty()) else {
                continue;
            };
            // `<enum>\tenum` is a declaration line rather than a field line.
            if columns.len() == 2 && columns[1] == "enum" {
                schema.enums.insert((*declared).to_string());
                continue;
            }
            schema.messages.insert((*declared).to_string());
            // The last column names the field's own type for a message or enum
            // field; an enum carries its probe values after an `=`.
            match (columns.get(4), columns.get(5)) {
                (Some(&"message"), Some(referenced)) => {
                    schema.messages.insert((*referenced).to_string());
                }
                (Some(&"enum"), Some(referenced)) => {
                    let name = referenced.split('=').next().unwrap_or(referenced);
                    schema.enums.insert(name.to_string());
                }
                _ => {}
            }
        }
        assert!(
            !schema.messages.is_empty(),
            "{MANIFEST} declared no messages"
        );
        schema
    })
}

/// A waproto type as a reference to the declaration the package already
/// publishes for it, mirroring the `history_sync` event entry in
/// `src/wasm_client.rs`.
///
/// `None` when the path is not a waproto one. An unresolvable waproto path
/// panics instead: emitting the bare name is what put twenty types that do not
/// exist into the published `.d.ts`.
fn proto_type_reference(path: &syn::Path) -> Option<String> {
    // The last such segment, not the first: the core writes both `wa::X` and
    // the fully qualified `waproto::wa::X`.
    let start = path
        .segments
        .iter()
        .rposition(|s| s.ident == "wa" || s.ident == "waproto")?;
    let segments: Vec<String> = path
        .segments
        .iter()
        .skip(start + 1)
        .map(|s| s.ident.to_string())
        .collect();
    let (leaf, parents) = segments.split_last()?;

    let schema = proto_schema();
    let matches = |declared: &&String| {
        let components: Vec<&str> = declared.split('.').collect();
        components.len() == parents.len() + 1
            && components.last() == Some(&leaf.as_str())
            && components[..parents.len()]
                .iter()
                .zip(parents)
                .all(|(component, parent)| squash_name(component) == squash_name(parent))
    };

    let message = schema.messages.iter().find(matches);
    let declared = schema.enums.iter().find(matches);
    let reference = match (message, declared) {
        (Some(message), None) => {
            let (namespaces, _) = message.rsplit_once('.').unwrap_or(("", message));
            if namespaces.is_empty() {
                format!("I{leaf}")
            } else {
                format!("{namespaces}.I{leaf}")
            }
        }
        (None, Some(declared)) => declared.clone(),
        _ => panic!(
            "waproto type `{}` resolves to {} declarations in the schema manifest",
            segments.join("::"),
            message.is_some() as usize + declared.is_some() as usize
        ),
    };
    Some(format!("import('./proto-types').proto.{reference}"))
}

/// Casing-insensitive, separator-insensitive form of a name, so a prost module
/// (`sync_action_value`) compares equal to the message it was derived from
/// (`SyncActionValue`) without depending on how the acronyms were split.
fn squash_name(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// `pub type X = …;` from the core, as source text. Resolved on demand rather
/// than eagerly: an alias can name another alias, and the table is built before
/// anything is rendered.
fn type_aliases() -> &'static Mutex<BTreeMap<String, String>> {
    static ALIASES: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    ALIASES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn collect_type_aliases(path: &Path) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    type_aliases()
        .lock()
        .unwrap()
        .extend(type_aliases_in(&content));
}

/// A private alias is not part of the serialized surface, and a generic one
/// cannot be resolved without its arguments — neither is skipped silently
/// anywhere else, so both are filtered here.
fn type_aliases_in(content: &str) -> BTreeMap<String, String> {
    let mut collected = BTreeMap::new();
    let Ok(file) = syn::parse_file(content) else {
        return collected;
    };
    for item in &file.items {
        if let Item::Type(alias) = item
            && is_pub(&alias.vis)
            && alias.generics.type_params().next().is_none()
        {
            collected.insert(
                alias.ident.to_string(),
                alias.ty.to_token_stream().to_string(),
            );
        }
    }
    collected
}

fn resolve_type_alias(name: &str) -> Option<(String, bool)> {
    thread_local! {
        static RESOLVING: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let aliased = type_aliases().lock().unwrap().get(name).cloned()?;
    let ty = syn::parse_str::<Type>(&aliased).ok()?;
    RESOLVING.with(|stack| {
        if stack.borrow().iter().any(|entry| entry == name) {
            return None;
        }
        stack.borrow_mut().push(name.to_string());
        let resolved = rust_type_to_ts(&ty);
        stack.borrow_mut().pop();
        Some(resolved)
    })
}

/// Named types the emitted declarations refer to, recorded as they are
/// rendered. Checked against what the file ends up declaring.
fn referenced_names() -> &'static Mutex<BTreeSet<String>> {
    static REFERENCED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    REFERENCED.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn generic_parameter_names() -> &'static Mutex<BTreeSet<String>> {
    static PARAMETERS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    PARAMETERS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Refuse to emit a declaration that names a type the file does not declare.
///
/// A consumer's tsconfig sets `skipLibCheck: true` — the default nearly
/// everywhere — so such a name reaches them as a silent `any` rather than an
/// error. Nineteen accumulated that way. The cost of failing here is that the
/// first core type this generator cannot place stops `bun run build` and needs
/// a mapping written for it; the alternative is publishing a type nobody has.
///
/// This sees one `typescript_custom_section` out of several, so it cannot be
/// the whole net — `tests/published-dts.test.ts` checks the concatenated file
/// wasm-bindgen actually emits. It runs first because it names the Rust type
/// that caused the break, which a TypeScript error cannot.
fn assert_declarations_are_closed(declared: &BTreeMap<String, TsTypeDef>) {
    let referenced = referenced_names().lock().unwrap().clone();
    let generics = generic_parameter_names().lock().unwrap().clone();
    let dangling = dangling_references(declared, &referenced, &generics);

    assert!(
        dangling.is_empty(),
        "the generated declarations name {} type(s) nothing declares: {}\n\
         Each needs a mapping in `rust_type_to_ts` — a waproto type resolves to \n\
         `proto-types.d.ts`, a `pub type` alias resolves to what it aliases, and \n\
         anything else needs a declaration of its own.",
        dangling.len(),
        dangling.join(", ")
    );
}

fn dangling_references(
    declared: &BTreeMap<String, TsTypeDef>,
    referenced: &BTreeSet<String>,
    generics: &BTreeSet<String>,
) -> Vec<String> {
    referenced
        .iter()
        .filter(|name| {
            !declared.contains_key(*name)
                && !HAND_WRITTEN_DECLARATIONS.contains(&name.as_str())
                && !BRIDGE_DECLARATIONS.contains(&name.as_str())
                && !generics.contains(*name)
        })
        .cloned()
        .collect()
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated_type(source: &str, name: &str) -> String {
        let mut types = BTreeMap::new();
        parse_source(source, &mut types);
        types
            .get(name)
            .unwrap_or_else(|| panic!("missing generated type {name}"))
            .to_typescript(name)
    }

    fn ts_of(source: &str) -> String {
        rust_type_to_ts(&syn::parse_str::<Type>(source).unwrap()).0
    }

    /// The variant list feeds the bridge's dispatch-coverage test, so it has to
    /// read the core's `Event` as declared: `#[non_exhaustive]`, doc comments
    /// between variants, and every payload behind a wrapper.
    #[test]
    fn core_event_variants_are_read_in_declaration_order() {
        let source = r#"
            #[derive(Clone, Serialize)]
            #[non_exhaustive]
            pub enum Event {
                Connected(Connected),
                /// A batched app-state sync left a collection unsynced.
                AppStateSyncFailed(AppStateSyncFailed),
                Messages(Box<MessageBatch>),
            }
        "#;
        assert_eq!(
            core_event_variants_in(source),
            Some(vec![
                "Connected".to_owned(),
                "AppStateSyncFailed".to_owned(),
                "Messages".to_owned(),
            ])
        );
    }

    /// A module this generator cannot name a shape for is reported rather than
    /// assumed to be an integer, and a `with` on any other type is not its
    /// business.
    #[test]
    #[should_panic(expected = "whose shape this generator cannot name")]
    fn an_unknown_timestamp_module_stops_generation() {
        generated_type(
            r#"
            #[derive(Serialize)]
            pub struct Odd {
                #[serde(with = "custom::ts_string")]
                pub at: DateTime<Utc>,
            }
        "#,
            "Odd",
        );
    }

    #[test]
    fn a_serde_with_on_another_type_is_left_alone() {
        let generated = generated_type(
            r#"
            #[derive(Serialize)]
            pub struct Payload {
                #[serde(with = "serde_bytes")]
                pub blob: Vec<u8>,
            }
        "#,
            "Payload",
        );
        assert!(generated.contains("blob: Uint8Array;"), "{generated}");
    }

    /// An empty list would pass every check it feeds, so sources that declare no
    /// `Event` are reported rather than defaulted.
    #[test]
    fn sources_without_the_event_enum_are_not_an_empty_list() {
        assert_eq!(core_event_variants_in("pub struct Event;"), None);
    }

    /// A `DateTime` crosses as chrono writes it, and chrono writes a string
    /// unless the field names one of its timestamp modules — through `with`, or
    /// through `serialize_with` naming the module's own function. Declaring the
    /// string case as a number let consumers do arithmetic on text.
    #[test]
    fn a_datetime_is_a_number_only_where_the_field_asks_for_one() {
        let source = r#"
            #[derive(Serialize)]
            pub struct Timestamps {
                pub written_at: DateTime<Utc>,
                #[serde(with = "chrono::serde::ts_seconds")]
                pub seconds: DateTime<Utc>,
                #[serde(with = "chrono::serde::ts_seconds_option")]
                pub maybe_seconds: Option<DateTime<Utc>>,
                #[serde(serialize_with = "chrono::serde::ts_seconds::serialize")]
                pub written_by_function: DateTime<Utc>,
            }
        "#;
        let generated = generated_type(source, "Timestamps");
        assert!(generated.contains("written_at: string;"), "{generated}");
        assert!(generated.contains("seconds: number;"), "{generated}");
        assert!(
            generated.contains("maybe_seconds?: number | null;"),
            "{generated}"
        );
        assert!(
            generated.contains("written_by_function: number;"),
            "{generated}"
        );
    }

    /// A container that converts through a shadow writes the shadow's fields,
    /// not its own. Reading the packed layout instead publishes a declaration
    /// no JSON on the boundary matches.
    #[test]
    fn a_serde_shadow_names_the_shape_that_crosses_the_boundary() {
        let source = r#"
            #[derive(Serialize, Deserialize)]
            #[serde(from = "PackedDe", into = "PackedDe")]
            pub struct Packed {
                /// The device this entry is about.
                device_id: u16,
                flags: u8,
                key_index: u32,
            }

            #[derive(Serialize, Deserialize)]
            struct PackedDe {
                device_id: u16,
                key_index: Option<u32>,
                #[serde(default)]
                is_hosted: bool,
            }
        "#;
        let generated = generated_type(source, "Packed");
        assert!(generated.contains("device_id: number;"), "{generated}");
        assert!(
            generated.contains("key_index?: number | null;"),
            "{generated}"
        );
        assert!(generated.contains("is_hosted: boolean;"), "{generated}");
        assert!(!generated.contains("flags"), "{generated}");
        assert!(
            generated.contains("/** The device this entry is about. */"),
            "{generated}"
        );
    }

    /// A field whose `serialize_with` rewrites its shape is named from the
    /// table, and one whose function only writes what the type already says
    /// falls through to the type.
    #[test]
    fn a_serialize_with_field_is_named_by_what_the_function_writes() {
        let source = r#"
            #[derive(Serialize)]
            pub struct Mapped {
                #[serde(rename = "lid_to_pn_map", serialize_with = "serialize_lid_pn")]
                lid_pn: Box<[LidPnPair]>,
                #[serde(serialize_with = "crate::serde_helpers::serialize_optional_bytes")]
                pub token: Option<Vec<u8>>,
            }
        "#;
        let generated = generated_type(source, "Mapped");
        assert!(
            generated.contains("lid_to_pn_map: Record<string, Jid>;"),
            "{generated}"
        );
        assert!(
            generated.contains("token?: Uint8Array | null;"),
            "{generated}"
        );
        assert!(!generated.contains("LidPnPair"), "{generated}");
    }

    /// Every wrapper a waproto type is written behind in the core reaches the
    /// same reference. A mapping written only for the bare case leaks the Rust
    /// name out of one of these.
    #[test]
    fn waproto_references_survive_every_wrapper_the_core_uses() {
        const KEY: &str = "import('./proto-types').proto.IMessageKey";
        assert_eq!(ts_of("wa::MessageKey"), KEY);
        assert_eq!(ts_of("Option<wa::MessageKey>"), format!("{KEY} | null"));
        assert_eq!(ts_of("Vec<wa::MessageKey>"), format!("{KEY}[]"));
        assert_eq!(ts_of("Box<wa::MessageKey>"), KEY);
        assert_eq!(ts_of("Arc<wa::MessageKey>"), KEY);
        assert_eq!(
            ts_of("Option<Box<waproto::wa::MessageKey>>"),
            format!("{KEY} | null")
        );
    }

    /// The namespace comes from the schema manifest, not from re-casing prost's
    /// module name, and a proto enum is not declared with the `I` prefix a
    /// message carries.
    #[test]
    fn waproto_references_name_the_declaration_proto_types_publishes() {
        assert_eq!(
            ts_of("Box<wa::sync_action_value::ArchiveChatAction>"),
            "import('./proto-types').proto.SyncActionValue.IArchiveChatAction"
        );
        assert_eq!(
            ts_of("wa::client_payload::user_agent::Platform"),
            "import('./proto-types').proto.ClientPayload.UserAgent.Platform"
        );
    }

    /// An enum no field references still has a declaration to point at, so the
    /// manifest has to name it — otherwise resolution fails on a type that is
    /// right there in `proto-types.d.ts`.
    #[test]
    fn an_enum_no_field_references_still_resolves() {
        assert_eq!(
            ts_of("wa::CollectionName"),
            "import('./proto-types').proto.CollectionName"
        );
        assert_eq!(
            ts_of("wa::sync_action_value::settings_sync_action::SettingKey"),
            "import('./proto-types').proto.SyncActionValue.SettingsSyncAction.SettingKey"
        );
    }

    #[test]
    #[should_panic(expected = "resolves to 0 declarations")]
    fn a_waproto_type_the_schema_does_not_declare_fails_generation() {
        ts_of("wa::NoSuchProtoMessage");
    }

    #[test]
    fn only_public_non_generic_aliases_are_collected() {
        let collected = type_aliases_in(
            r#"
                pub type MessageSecret = [u8; MESSAGE_SECRET_SIZE];
                type Internal = [u8; 32];
                pub type Wrapper<T> = Vec<T>;
            "#,
        );

        assert_eq!(
            collected.keys().collect::<Vec<_>>(),
            vec!["MessageSecret"],
            "collected {collected:?}"
        );
        assert_eq!(collected["MessageSecret"], "[u8 ; MESSAGE_SECRET_SIZE]");
    }

    #[test]
    fn a_smallvec_names_what_the_equivalent_vec_would() {
        // `ReportingBytes` in the core is `SmallVec<[u8; 20]>`: the inline
        // capacity is an allocation choice, and serde writes the same sequence
        // either way, so the declaration must not move when the core takes one.
        assert_eq!(ts_of("SmallVec<[u8; 20]>"), "Uint8Array");
        assert_eq!(ts_of("Option<SmallVec<[u8; 20]>>"), "Uint8Array | null");
        assert_eq!(ts_of("SmallVec<[Jid; 4]>"), "Jid[]");
    }

    #[test]
    fn a_type_alias_resolves_to_what_it_aliases() {
        type_aliases()
            .lock()
            .unwrap()
            .extend(type_aliases_in("pub type AliasUnderTest = [u8; 32];"));
        assert_eq!(ts_of("AliasUnderTest"), "Uint8Array");
        assert_eq!(ts_of("Vec<AliasUnderTest>"), "Uint8Array[]");
        assert_eq!(ts_of("Option<AliasUnderTest>"), "Uint8Array | null");
    }

    /// The mapping must not swallow the ordinary case: a type this generator
    /// declares is still referenced by name.
    #[test]
    fn a_generated_declaration_is_still_referenced_by_name() {
        assert_eq!(ts_of("Option<MessageInfo>"), "MessageInfo | null");
        assert_eq!(ts_of("Vec<DeviceInfo>"), "DeviceInfo[]");
    }

    #[test]
    fn a_name_nothing_declares_is_reported_rather_than_emitted() {
        let mut declared = BTreeMap::new();
        parse_source(
            r#"
                #[derive(Serialize)]
                pub struct Holder { known: Holder, unknown: Unplaceable, jid: Jid, generic: T }
            "#,
            &mut declared,
        );
        let referenced = ["Holder", "Unplaceable", "Jid", "T"]
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        let generics = ["T".to_string()].into_iter().collect();

        assert_eq!(
            dangling_references(&declared, &referenced, &generics),
            vec!["Unplaceable".to_string()]
        );
    }

    #[test]
    fn serde_private_fields_generics_and_binary_types_are_preserved() {
        let generated = generated_type(
            r#"
                #[derive(Serialize, Deserialize)]
                #[serde(try_from = "Input<T>")]
                pub struct Input<T> {
                    private_name: CompactString,
                    values: Vec<T>,
                    maybe_bytes: Option<Vec<u8>>,
                    large: u64,
                    #[serde(skip)]
                    internal: String,
                }
            "#,
            "Input",
        );

        assert!(generated.contains("export interface Input<T>"));
        assert!(generated.contains("private_name: string;"));
        assert!(generated.contains("values: T[];"));
        assert!(generated.contains("maybe_bytes?: Uint8Array | null;"));
        assert!(generated.contains("large: number | string;"));
        assert!(!generated.contains("internal"));
    }

    #[test]
    fn adjacent_tagged_enum_keeps_payloads_and_concrete_generic_arguments() {
        let generated = generated_type(
            r#"
                #[derive(Serialize, Deserialize)]
                #[serde(tag = "type", content = "data", rename_all = "snake_case")]
                pub enum Outcome<T> {
                    Value(T),
                    Structured { optional: Option<CompactString> },
                    Empty,
                }
            "#,
            "Outcome",
        );

        assert!(generated.contains("export type Outcome<T>"));
        assert!(generated.contains("{ type: \"value\"; data: T }"));
        assert!(generated.contains("{ type: \"structured\"; data: { optional?: string | null } }"));
        assert!(generated.contains("{ type: \"empty\" }"));

        let (concrete, optional) = rust_type_to_ts(
            &syn::parse_str::<Type>("Outcome<Box<Option<CompactString>>>").unwrap(),
        );
        assert_eq!(concrete, "Outcome<string | null>");
        assert!(!optional);
    }

    #[test]
    fn serde_external_and_internal_representations_are_not_conflated() {
        let external = generated_type(
            r#"
                #[derive(Serialize)]
                pub enum External { Item(u32), Empty }
            "#,
            "External",
        );
        assert!(external.contains("{ \"Item\": number }"));
        assert!(external.contains("| \"Empty\""));

        let internal = generated_type(
            r#"
                #[derive(Serialize)]
                #[serde(tag = "kind", rename_all = "snake_case")]
                pub enum Internal { Item { renamed_field: u32 }, Empty }
            "#,
            "Internal",
        );
        assert!(internal.contains("{ kind: \"item\"; renamed_field: number }"));
        assert!(internal.contains("{ kind: \"empty\" }"));
    }

    #[test]
    fn adjacent_wire_enum_uses_declared_tags_and_nested_content() {
        let generated = generated_type(
            r#"
                #[derive(WireEnum)]
                #[wire(tag = "type", content = "data")]
                pub enum Protocol {
                    #[wire = "contact"]
                    Contact { addressing_mode: Mode },
                    #[wire = "devices"]
                    DevicesV2,
                    #[wire = "feature"]
                    Features(Vec<Feature>),
                }
            "#,
            "Protocol",
        );

        assert!(generated.contains("{ type: \"contact\"; data: { addressing_mode: Mode } }"));
        assert!(generated.contains("{ type: \"devices\" }"));
        assert!(generated.contains("{ type: \"feature\"; data: Feature[] }"));
    }
}
