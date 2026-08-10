# The numeric input contract of `encodeProto`

> A numeric field takes a `number`, a `bigint`, a `Long` the decoder produced, or a
> string that parses in full as a number — never `''`, `true`, `[]` or anything else
> JavaScript would silently turn into a number — and the value must be one the declared
> type can hold.

Two clarifications the sentence leans on:

- **Integer fields want an integer.** `1.5`, `NaN` and `±Infinity` are not values of
  `int32`/`int64` and friends, and neither is a magnitude the width cannot hold.
- **`float` and `double` round, because rounding is what the type means.** `1.1` in a
  `float` field becomes the nearest 32-bit float and that is the field working as
  declared. `NaN` and `±Infinity` are values of both types and pass. A *finite* value
  with no 32-bit representation — `1e39` — is an overflow, not a rounding, and throws.

`encodeProto` is called with an object the consumer built, so a value that is not the
number the field declares is the caller's own mistake. Failing there costs one
exception; coercing it writes a value nobody asked for onto the wire, under a field the
server will read as if it were meant.

## The matrix

Behaviour is identical inside each column group, so the six columns are the whole
matrix for all twelve protobuf numeric types (`sint32`/`sint64` share the `int32` and
`int64` columns; the schema declares no field of either today).

`throws` is an `Error`; nothing here is caught and re-reported. `absent` means no bytes
at all — the field is omitted. `†` marks a cell whose behaviour changed; cells where
only the exception's wording changed are not marked.

| input | `int32` `sfixed32` | `uint32` `fixed32` | `int64` `sfixed64` | `uint64` `fixed64` | `float` | `double` |
|---|---|---|---|---|---|---|
| `''` | throws † | throws † | throws † | throws † | throws † | throws † |
| `'  '` | throws † | throws † | throws † | throws † | throws † | throws † |
| `'12'` | `12` | `12` | `12` | `12` | `12` | `12` |
| `'0x10'` | `16` | `16` | `16` | `16` | `16` | `16` |
| `'1e2'` | `100` | `100` | `100` | `100` | `100` | `100` |
| `'12.0'` / `'+12'` | `12` | `12` | `12` | `12` | `12` | `12` |
| `'9007199254740992.0'` | throws | throws | exact on the wire | exact on the wire | `2**53` | `2**53` |
| `'1.0000000000000000001'` | throws | throws | throws | throws | `1` | `1` |
| `'1e-400'` | throws | throws | throws | throws | `0` | `0` |
| `'12.5'` | throws | throws | throws | throws | `12.5` | `12.5` |
| `'abc'` | throws | throws | throws | throws | throws | throws † |
| `'9007199254740993'` | throws | throws | exact on the wire | exact on the wire | `9007199254740992` | `9007199254740992` |
| `null` | absent | absent | absent | absent | absent | absent |
| `undefined` | absent | absent | absent | absent | absent | absent |
| `true` / `false` | throws | throws | throws † | throws † | throws | throws † |
| `[]` / `[5]` | throws | throws | throws † | throws † | throws | throws † |
| `{}` | throws | throws | throws | throws | throws | throws † |
| `{ low, high }` without `unsigned` | throws | throws | throws † | throws † | throws | throws † |
| a decoded `Long` | throws | throws | its value | its value | its value | its value |
| `12n` | `12` † | `12` † | `12` | `12` | `12` † | `12` † |
| `1.5` / `-1.5` | throws | throws | throws | throws | `1.5` / `-1.5` | `1.5` / `-1.5` |
| `NaN` | throws | throws | throws | throws | `NaN` | `NaN` |
| `Infinity` / `-Infinity` | throws | throws | throws | throws | `±Infinity` | `±Infinity` |
| `-1` | `-1` | throws | `-1` | throws | `-1` | `-1` |
| `2**53` | throws | throws | exact on the wire | exact on the wire | `2**53` | `2**53` |
| `2**63` | throws | throws | throws | exact on the wire | `2**63` | `2**63` |
| `1e300` | throws | throws | throws | throws | throws | `1e300` |
| `'1e400'` | throws | throws | throws | throws | throws | throws |
| `10n ** 400n` | throws | throws | throws | throws | throws | throws |
| `'Infinity'` | throws | throws | throws | throws | `Infinity` | `Infinity` |
| `FLT_MAX` (`3.4028234663852886e38`) | throws | throws | throws | throws | `FLT_MAX` | `FLT_MAX` |
| just above `FLT_MAX` | throws | throws | throws | throws | throws | that double |

*exact on the wire* means the encoder writes the value with full 64-bit precision. The
decoder hands such a value back as a protobufjs `Long` (`{ low, high, unsigned }`)
rather than a `number`, so the round trip is exact but not `===` to the input; those
cells are pinned on the bytes. What the reader does with a wide value is the read path's
contract, not this one.

A `Long` is also *input*: the writer takes one back on a 64-bit field, which is how a
decoded message re-encodes. Only a real one — `unsigned` must be a boolean, `low` and
`high` numbers. A plain `{ low, high }` data object is not a Long and is rejected like
any other object, which is what keeps `{}` and `[]` from encoding as zero.

`'1e400'` and `10n ** 400n` are finite and no double can hold either. Converting them
reaches `Infinity`, which would put an infinity on the wire that the caller never wrote —
the same silent substitution the contract exists to stop, so they throw. The literal
string `'Infinity'` is not that case: it names the value, and a float field holds it.

**An integer field reads a string digit-wise, never through `Number`.** Integer-literal
syntax goes to `BigInt`, which keeps plain digits past 2^53 exact and covers `0x`/`0o`/`0b`.
Anything else — `'1e2'`, `'12.0'`, `'+12'`, `'9.007199254740992e15'` — is expanded from
its own digits and accepted only if it names a whole number exactly. That is why
`'1.0000000000000000001'` and `'1e-400'` throw instead of arriving as `1` and `0`:
`Number` would round them to integers the caller never wrote. `float` and `double` do go
through `Number`, because rounding to the declared width is what those types mean.

An **enum** field is written as `int32` and follows that column: `{ accountType: '' }`
now throws rather than encoding `E2EE`, the zero variant.

`tests/proto-numeric-input.test.ts` pins every cell above. `sint32` and `sint64` have no
field in the schema, so their cells are pinned at the writer instead, in the same file.

## The empty string

`''` is not `0`. It is what a form field, a query parameter or a nullable column looks
like when nothing was filled in, and the bridge already says elsewhere that *absent is
absent* — the server omits a field rather than blanking it, so an absent name is
`undefined` and not `""`. Encoding `''` as `0` broke that in the other direction: it put
a real zero on the wire where the caller had nothing, in a field the server then reads
as deliberately set.

It was also the one input where the field's width decided the outcome under the old
behaviour — silently zero in a 32-bit field, silently zero in a 64-bit one, and throwing
one line later for `1.5` in that same 64-bit field. Now `''` throws everywhere.

To send nothing, send nothing: omit the key, or pass `undefined` or `null` (both are
normalised to an absent field). To send zero, send `0`.

## Error vocabulary

Every rejection is an `Error` reading `invalid <type>: <value>`.

- A rejection for the input's **kind** — the blank string, a boolean, an array, an
  object, a string that does not parse — names the field's declared type, and renders a
  string input quoted so `invalid int64: ""` is unambiguous.
- A rejection for the value's **range or integrality** comes from `@bufbuild/protobuf`'s
  own assertion and names the underlying width: `int32` for `sfixed32`, `uint32` for
  `fixed32`, `int64` for `sfixed64`, `uint64` for `fixed64`, `float32` for `float`.

## Where it lives

`ts/proto-reader.ts` holds the contract, on the `BinaryWriter` that already lives there
to take a decoded `Long` back. `scripts/gen-ts-proto.ts` points the generated codec's
`BinaryWriter` import at that module, so every generated `encode()` gets the contract
without a per-field change to generated code.
