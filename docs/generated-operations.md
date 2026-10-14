# Generated client operations

`codegen/src/operations/` is the source for selected concrete client bindings.
Run `bun run gen:operations` after editing a catalog and commit the files under
`src/wasm_client/generated/`. `bun run check:operations` checks drift without
writing. The normal build generates them too; CI checks the output and compiles
the emitted calls against the locked core.

An operation declares its Rust signature and wasm-bindgen name, preparation,
admission, one core call and result projection. The generator shares the async
wrapper and error routing. Preparation runs before admission, so invalid input
can reject during reconnect. `OnlineIf` records the media connection's explicit
force/cache distinction. Projections retain the existing DTOs, optional fields,
JID conversions, numeric formats and errors.

The catalog uses typed Rust syntax through `syn` and `quote`. It does not resolve
the core's entire Rust API or infer semantics from a signature. The compiler
checks the call and projection; the published declaration and runtime tests
check the foreign contract. Adding an operation does not add it to other Cargo
features: each generated file is included by its existing feature-gated domain.

The current scope covers group operations and structured group/community
results, business profile/order results, and media connection metadata. The
catalog itself lists the methods. Streams, storage callbacks, client lifecycle,
packed event codecs, participant action dispatch, and batches that change
admission after their first effect remain explicit adapters. Do not introduce
retries, a second reconnect loop or generic runtime dispatch while generating
their boundary code.

When extending the scope, preserve argument validation order and the public
JS/TS names, optionality and rejection channel. Async typed inputs must use
`JsValue` and the existing fallible input helpers. Keep custom serde, monetary
values, protobuf codecs and buffer ownership explicit. Run the affected
settlement, reconnect/withdrawal and result-projection tests, the published
declaration check, and the unchanged package-size/WASM-shape gates.
