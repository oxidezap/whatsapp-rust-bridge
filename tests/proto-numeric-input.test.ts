/**
 * The numeric input contract of `encodeProto`, pinned cell by cell.
 *
 * A numeric field takes a `number`, a `bigint`, or a string that parses in full
 * as a number — never `''`, `true`, `[]` or anything else JavaScript would
 * silently turn into a number — and the value must be one the declared type can
 * hold. `docs/proto-numeric-input.md` carries the same matrix in prose.
 *
 * Reads the codec directly, so it needs no `bun run build`.
 *
 * Run: bun test tests/proto-numeric-input.test.ts
 */
import { describe, expect, test } from "bun:test";
import { decodeProto, encodeProto } from "../ts/proto";
import { BinaryReader, BinaryWriter } from "../ts/proto-reader";

const FLT_MAX = 3.4028234663852886e38;
const ABOVE_FLT_MAX = 3.4028235677973366e38;
const TWO_53 = 2 ** 53;
const TWO_63 = 2 ** 63;
/** A finite value no double can hold; converting it reaches Infinity. */
const HUGE = 10n ** 400n;

/** One field per protobuf numeric type, picked out of the real schema. */
const FIELDS = {
  int32: { message: "BotLinkedAccountsMetadata", field: "acErrorCode" },
  uint32: { message: "ADVDeviceIdentity", field: "rawId" },
  fixed32: { message: "ConsumerApplication.StatusTextMesage", field: "textArgb" },
  sfixed32: { message: "EphemeralSetting", field: "duration" },
  int64: { message: "AIHomeState", field: "lastFetchTime" },
  uint64: { message: "ADVDeviceIdentity", field: "timestamp" },
  fixed64: { message: "SignedPreKeyRecordStructure", field: "timestamp" },
  sfixed64: { message: "EphemeralSetting", field: "timestamp" },
  float: { message: "ContextInfo.DataSharingContext.Parameters", field: "floatData" },
  double: {
    message: "AIRichResponseLatexMetadata.AIRichResponseLatexExpression",
    field: "width",
  },
} as const;

type TypeName = keyof typeof FIELDS;

const TYPES = Object.keys(FIELDS) as TypeName[];
const SIGNED_INTS: TypeName[] = ["int32", "sfixed32", "int64", "sfixed64"];
const UNSIGNED_INTS: TypeName[] = ["uint32", "fixed32", "uint64", "fixed64"];
const INTS: TypeName[] = [...SIGNED_INTS, ...UNSIGNED_INTS];
const INTS_32: TypeName[] = ["int32", "uint32", "fixed32", "sfixed32"];
const INTS_64: TypeName[] = ["int64", "uint64", "fixed64", "sfixed64"];
const FLOATS: TypeName[] = ["float", "double"];

type Outcome =
  /** The field is written and reads back as `value`. */
  | { kind: "value"; value: number }
  /** No bytes at all: an absent field, not a zero. */
  | { kind: "absent" }
  /**
   * Written exactly, but past the decoder's safe-integer ceiling, so only the
   * bytes are checked — the read path is a separate contract.
   */
  | { kind: "wire"; same: bigint }
  | { kind: "throws"; message: string };

const encode = (type: TypeName, input: unknown): Uint8Array =>
  encodeProto(FIELDS[type].message, { [FIELDS[type].field]: input });

const decode = (type: TypeName, bytes: Uint8Array): unknown =>
  (decodeProto(FIELDS[type].message, bytes) as Record<string, unknown>)[FIELDS[type].field];

/**
 * Outcomes for a subset of the types. Declared total so the case literals can
 * be plain spreads; the first test below checks the spreads really do cover
 * every type, which is what keeps a cell from going missing unnoticed.
 */
const spread = (types: TypeName[], outcome: Outcome): Record<TypeName, Outcome> =>
  Object.fromEntries(types.map((type) => [type, outcome])) as Record<TypeName, Outcome>;

/** Same rejection everywhere: the message names the field's own declared type. */
const rejectedEverywhere = (rendered: string): Record<TypeName, Outcome> =>
  Object.fromEntries(
    TYPES.map((type) => [type, { kind: "throws", message: `invalid ${type}: ${rendered}` }]),
  ) as Record<TypeName, Outcome>;

const accepted = (value: number): Outcome => ({ kind: "value", value });

/**
 * A rejection for the input's *kind* names the field's declared type; a
 * rejection for its range or integrality is Buf's own assertion, which names
 * the underlying 32/64-bit type.
 */
const BUF_ASSERT: Record<TypeName, string> = {
  int32: "int32",
  sfixed32: "int32",
  uint32: "uint32",
  fixed32: "uint32",
  int64: "int64",
  sfixed64: "int64",
  uint64: "uint64",
  fixed64: "uint64",
  float: "float32",
  double: "double",
};

const outOfRange = (types: TypeName[], rendered: string): Record<TypeName, Outcome> =>
  Object.fromEntries(
    types.map((type) => [
      type,
      { kind: "throws", message: `invalid ${BUF_ASSERT[type]}: ${rendered}` },
    ]),
  ) as Record<TypeName, Outcome>;

interface Case {
  label: string;
  input: unknown;
  outcomes: Record<TypeName, Outcome>;
}

const CASES: Case[] = [
  // Not numbers. JavaScript would turn every one of these into a number.
  { label: "''", input: "", outcomes: rejectedEverywhere('""') },
  { label: "'  '", input: "  ", outcomes: rejectedEverywhere('"  "') },
  { label: "'abc'", input: "abc", outcomes: rejectedEverywhere('"abc"') },
  { label: "true", input: true, outcomes: rejectedEverywhere("boolean") },
  { label: "false", input: false, outcomes: rejectedEverywhere("boolean") },
  { label: "[]", input: [], outcomes: rejectedEverywhere("object") },
  { label: "[5]", input: [5], outcomes: rejectedEverywhere("object") },
  { label: "{}", input: {}, outcomes: rejectedEverywhere("object") },

  // Absent stays absent; the codec never writes a zero the caller did not send.
  { label: "null", input: null, outcomes: spread(TYPES, { kind: "absent" }) },
  {
    label: "undefined",
    input: undefined,
    outcomes: spread(TYPES, { kind: "absent" }),
  },

  // Lossless parses.
  { label: "'12'", input: "12", outcomes: spread(TYPES, accepted(12)) },
  { label: "'0x10'", input: "0x10", outcomes: spread(TYPES, accepted(16)) },
  { label: "12n", input: 12n, outcomes: spread(TYPES, accepted(12)) },
  { label: "'1e2'", input: "1e2", outcomes: spread(TYPES, accepted(100)) },
  { label: "'12.0'", input: "12.0", outcomes: spread(TYPES, accepted(12)) },
  { label: "'+12'", input: "+12", outcomes: spread(TYPES, accepted(12)) },

  // An integer field reads the digits, not `Number(text)`, so a fraction below
  // double precision cannot arrive rounded to a whole number.
  {
    label: "'1.0000000000000000001'",
    input: "1.0000000000000000001",
    outcomes: {
      ...spread(INTS, { kind: "throws", message: 'invalid @: "1.0000000000000000001"' }),
      ...spread(FLOATS, accepted(1)),
    },
  },
  {
    label: "'1e-400'",
    input: "1e-400",
    outcomes: {
      ...spread(INTS, { kind: "throws", message: 'invalid @: "1e-400"' }),
      ...spread(FLOATS, accepted(0)),
    },
  },
  // Exact but not *safe*: 2^53 written in two syntaxes BigInt cannot read.
  {
    label: "'9007199254740992.0'",
    input: "9007199254740992.0",
    outcomes: {
      ...outOfRange(INTS_32, "9007199254740992"),
      ...spread(INTS_64, { kind: "wire", same: 9007199254740992n }),
      ...spread(FLOATS, accepted(TWO_53)),
    },
  },
  {
    label: "'9.007199254740992e15'",
    input: "9.007199254740992e15",
    outcomes: {
      ...outOfRange(INTS_32, "9007199254740992"),
      ...spread(INTS_64, { kind: "wire", same: 9007199254740992n }),
      ...spread(FLOATS, accepted(TWO_53)),
    },
  },
  // A `{ low, high }` object without the `unsigned` discriminant is data, not a
  // Long; reading it as one is how `{}` and `[]` used to encode as zero.
  {
    label: "{ low, high } without unsigned",
    input: { low: 1, high: 2 },
    outcomes: rejectedEverywhere("object"),
  },
  // A Long whose words are not words is not a Long: `low >>> 0` would truncate
  // it to a whole number the caller never wrote.
  {
    label: "Long with a fractional low",
    input: { low: 1.5, high: 0, unsigned: true },
    outcomes: rejectedEverywhere("object"),
  },
  { label: "'0e100'", input: "0e100", outcomes: spread(TYPES, accepted(0)) },
  // A padded string names the same value as an unpadded one, for NaN as much as
  // for the infinities and for the digits.
  {
    label: "' NaN '",
    input: " NaN ",
    outcomes: {
      ...spread(INTS, { kind: "throws", message: 'invalid @: " NaN "' }),
      ...spread(FLOATS, accepted(NaN)),
    },
  },
  {
    label: "' 12 '",
    input: " 12 ",
    outcomes: spread(TYPES, accepted(12)),
  },

  // Fractions: a value an integer field cannot hold, a value a float field can.
  {
    label: "'12.5'",
    input: "12.5",
    outcomes: {
      ...spread(INTS, { kind: "throws", message: 'invalid @: "12.5"' }),
      ...spread(FLOATS, accepted(12.5)),
    },
  },
  {
    label: "1.5",
    input: 1.5,
    outcomes: {
      ...spread(INTS_64, { kind: "throws", message: "invalid @: 1.5" }),
      ...outOfRange(INTS_32, "1.5"),
      ...spread(FLOATS, accepted(1.5)),
    },
  },
  {
    label: "-1.5",
    input: -1.5,
    outcomes: {
      ...spread(INTS_64, { kind: "throws", message: "invalid @: -1.5" }),
      ...outOfRange(INTS_32, "-1.5"),
      ...spread(FLOATS, accepted(-1.5)),
    },
  },

  // NaN and the infinities are values of a float field and of nothing else.
  {
    label: "NaN",
    input: NaN,
    outcomes: {
      ...spread(INTS_64, { kind: "throws", message: "invalid @: NaN" }),
      ...outOfRange(INTS_32, "NaN"),
      ...spread(FLOATS, accepted(NaN)),
    },
  },
  {
    label: "Infinity",
    input: Infinity,
    outcomes: {
      ...spread(INTS_64, { kind: "throws", message: "invalid @: Infinity" }),
      ...outOfRange(INTS_32, "Infinity"),
      ...spread(FLOATS, accepted(Infinity)),
    },
  },
  {
    label: "-Infinity",
    input: -Infinity,
    outcomes: {
      ...spread(INTS_64, { kind: "throws", message: "invalid @: -Infinity" }),
      ...outOfRange(INTS_32, "-Infinity"),
      ...spread(FLOATS, accepted(-Infinity)),
    },
  },

  // Range, which is the one place signedness and width still differ.
  {
    label: "-1",
    input: -1,
    outcomes: {
      ...spread(SIGNED_INTS, accepted(-1)),
      ...outOfRange(UNSIGNED_INTS, "-1"),
      ...spread(FLOATS, accepted(-1)),
    },
  },
  {
    label: "2**53",
    input: TWO_53,
    outcomes: {
      ...outOfRange(INTS_32, "9007199254740992"),
      ...spread(INTS_64, { kind: "wire", same: 9007199254740992n }),
      ...spread(FLOATS, accepted(TWO_53)),
    },
  },
  {
    label: "2**63",
    input: TWO_63,
    outcomes: {
      ...outOfRange(INTS_32, "9223372036854776000"),
      ...outOfRange(["int64", "sfixed64"], "9223372036854776000"),
      ...spread(["uint64", "fixed64"], {
        kind: "wire",
        same: 9223372036854775808n,
      }),
      ...spread(FLOATS, accepted(TWO_63)),
    },
  },
  {
    label: "'9007199254740993'",
    input: "9007199254740993",
    outcomes: {
      ...outOfRange(INTS_32, "9007199254740992"),
      ...spread(INTS_64, { kind: "wire", same: 9007199254740993n }),
      ...spread(FLOATS, accepted(9007199254740992)),
    },
  },
  // A finite input that only reaches infinity by overflowing the conversion is
  // not the infinity the caller asked for.
  {
    label: "'1e400'",
    input: "1e400",
    outcomes: rejectedEverywhere('"1e400"'),
  },
  {
    label: "10n ** 400n",
    input: HUGE,
    outcomes: {
      ...outOfRange(INTS_32, "Infinity"),
      ...spread(FLOATS, { kind: "throws", message: `invalid @: ${HUGE}` }),
      ...outOfRange(INTS_64, String(HUGE)),
    },
  },
  {
    label: "'Infinity'",
    input: "Infinity",
    outcomes: {
      ...spread(INTS, { kind: "throws", message: 'invalid @: "Infinity"' }),
      ...spread(FLOATS, accepted(Infinity)),
    },
  },
  {
    label: "1e300",
    input: 1e300,
    outcomes: {
      ...outOfRange(INTS, "1e+300"),
      ...outOfRange(["float"], "1e+300"),
      double: accepted(1e300),
    },
  },
  {
    label: "FLT_MAX",
    input: FLT_MAX,
    outcomes: {
      ...outOfRange(INTS, "3.4028234663852886e+38"),
      float: accepted(FLT_MAX),
      double: accepted(FLT_MAX),
    },
  },
  {
    label: "just above FLT_MAX",
    input: ABOVE_FLT_MAX,
    outcomes: {
      ...outOfRange(INTS, "3.4028235677973366e+38"),
      ...outOfRange(["float"], "3.4028235677973366e+38"),
      double: accepted(ABOVE_FLT_MAX),
    },
  },
];

describe("encodeProto numeric input matrix", () => {
  test("every case states an outcome for every numeric type", () => {
    for (const { label, outcomes } of CASES) {
      expect([label, Object.keys(outcomes).sort()]).toEqual([label, [...TYPES].sort()]);
    }
  });

  for (const { label, input, outcomes } of CASES) {
    for (const type of TYPES) {
      const outcome = outcomes[type];
      test(`${type} <- ${label}`, () => {
        if (outcome.kind === "throws") {
          expect(() => encode(type, input)).toThrow(outcome.message.replace("@", type));
          return;
        }

        const bytes = encode(type, input);
        if (outcome.kind === "absent") {
          expect(bytes.length).toBe(0);
          return;
        }
        if (outcome.kind === "wire") {
          expect(bytes).toEqual(encode(type, outcome.same));
          return;
        }
        const decoded = decode(type, bytes);
        if (Number.isNaN(outcome.value)) {
          expect(decoded as number).toBeNaN();
        } else {
          expect(decoded).toBe(outcome.value);
        }
      });
    }
  }
});

describe("the empty string is not a zero", () => {
  // The sharpest cell in the matrix: `''` usually means "not filled in", and
  // writing it as 0 puts a real value on the wire where there was none.
  for (const type of ["int32", "uint32", "fixed32", "sfixed32"] as TypeName[]) {
    test(`${type} (32-bit) rejects '' instead of encoding zero`, () => {
      expect(() => encode(type, "")).toThrow(`invalid ${type}: ""`);
      expect(encode(type, 0).length).toBeGreaterThan(0);
    });
  }

  for (const type of ["int64", "uint64", "fixed64", "sfixed64"] as TypeName[]) {
    test(`${type} (64-bit) rejects '' instead of encoding zero`, () => {
      expect(() => encode(type, "")).toThrow(`invalid ${type}: ""`);
      expect(encode(type, 0).length).toBeGreaterThan(0);
    });
  }

  test("an unfilled field is omitted, not zeroed", () => {
    expect(encode("uint64", undefined).length).toBe(0);
    expect(encode("uint64", null).length).toBe(0);
  });

  test("an enum field rejects '' too — the wire type is int32", () => {
    expect(() => encodeProto("AdvDeviceIdentity", { accountType: "" })).toThrow(
      'invalid int32: ""',
    );
    expect(encodeProto("AdvDeviceIdentity", { accountType: 0 }).length).toBeGreaterThan(0);
  });

  test("a nested message rejects '' the same way a top-level one does", () => {
    expect(() =>
      encodeProto("Message", { locationMessage: { degreesLatitude: "" } }),
    ).toThrow('invalid double: ""');
  });
});

describe("what the encoder accepts, the decoder reads back", () => {
  const ROUNDTRIP: [TypeName, unknown, number][] = [
    ["int32", -42, -42],
    ["int32", "42", 42],
    ["int32", 42n, 42],
    ["uint32", 4294967295, 4294967295],
    ["fixed32", "0x10", 16],
    ["sfixed32", -2147483648, -2147483648],
    ["int64", -9007199254740991, -9007199254740991],
    ["int64", "9007199254740991", 9007199254740991],
    ["uint64", 9007199254740991n, 9007199254740991],
    ["fixed64", "12", 12],
    ["sfixed64", -1, -1],
    ["float", 1.5, 1.5],
    ["float", FLT_MAX, FLT_MAX],
    ["float", Infinity, Infinity],
    ["double", 1e300, 1e300],
    ["double", "12.5", 12.5],
    ["double", -0.1, -0.1],
  ];

  for (const [type, input, expected] of ROUNDTRIP) {
    test(`${type} <- ${String(input)}`, () => {
      expect(decode(type, encode(type, input))).toBe(expected);
    });
  }
});

describe("sint32 and sint64 hold the same contract", () => {
  // The schema declares no zigzag field, so these two are exercised at the
  // writer rather than through encodeProto. Without the overrides the base
  // writer takes '' as zero, which is what these assert against.
  const write = (method: "sint32" | "sint64", value: unknown): Uint8Array =>
    (new BinaryWriter()[method] as (v: unknown) => BinaryWriter)(value).finish();

  test("sint32 rejects non-numbers and non-integers", () => {
    expect(() => write("sint32", "")).toThrow('invalid sint32: ""');
    expect(() => write("sint32", true)).toThrow("invalid sint32: boolean");
    expect(() => write("sint32", [])).toThrow("invalid sint32: object");
    expect(() => write("sint32", "1e400")).toThrow('invalid sint32: "1e400"');
    expect(() => write("sint32", 1.5)).toThrow("invalid int32: 1.5");
  });

  test("sint64 rejects non-numbers and non-integers", () => {
    expect(() => write("sint64", "")).toThrow('invalid sint64: ""');
    expect(() => write("sint64", true)).toThrow("invalid sint64: boolean");
    expect(() => write("sint64", [])).toThrow("invalid sint64: object");
    expect(() => write("sint64", "12.5")).toThrow('invalid sint64: "12.5"');
    expect(() => write("sint64", 1.5)).toThrow("invalid sint64: 1.5");
  });

  test("sint32 takes a number, a bigint and a string by value", () => {
    const expected = write("sint32", -42);
    expect(write("sint32", "-42")).toEqual(expected);
    expect(write("sint32", -42n)).toEqual(expected);
    expect(new BinaryReader(expected).sint32()).toBe(-42);
  });

  test("sint64 takes a number, a bigint and a string by value", () => {
    const expected = write("sint64", -9007199254740991);
    expect(write("sint64", "-9007199254740991")).toEqual(expected);
    expect(write("sint64", -9007199254740991n)).toEqual(expected);
    expect(write("sint64", "1e2")).toEqual(write("sint64", 100n));
    expect(new BinaryReader(expected).sint64Value()).toBe(-9007199254740991);
  });
});
