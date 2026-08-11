/**
 * Regenerate the pure-TypeScript protobuf codec from the exact `waproto`
 * package selected by Cargo.lock.
 *
 * Resolving the schema through Cargo metadata keeps the bridge tied to its
 * single public `whatsapp-rust` dependency without a sibling-repository path
 * or a second independently versioned copy of whatsapp.proto.
 */

import { fromBinary } from '@bufbuild/protobuf'
import {
	FieldDescriptorProto_Label as Label,
	FieldDescriptorProto_Type as Type,
	FileDescriptorSetSchema,
	type DescriptorProto,
	type FieldDescriptorProto
} from '@bufbuild/protobuf/wkt'
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUTPUT_DIR = join(ROOT, 'ts', 'generated')
const OUTPUT_FILE = join(OUTPUT_DIR, 'whatsapp.ts')
const SURFACE_FILE = join(OUTPUT_DIR, 'whatsapp-surface.txt')
const PROTO_PACKAGE_NAME = 'waproto'
const PROTO_SOURCE_PATH = ['src', 'whatsapp.proto'] as const
const TS_PROTO_PLUGIN = join(ROOT, 'node_modules', '.bin', 'protoc-gen-ts_proto')
const GENERATED_WIRE_IMPORT = 'import { BinaryReader, BinaryWriter } from "@bufbuild/protobuf/wire";'
const OPTIMIZED_WIRE_IMPORT =
	'import { BinaryReader, BinaryWriter, type Int64, type Long } from "../proto-reader";'
const GENERATED_BUILTIN =
	'type Builtin = Date | Function | Uint8Array | string | number | boolean | undefined;'
const OPTIMIZED_BUILTIN =
	'type Builtin = Date | Function | Uint8Array | string | number | boolean | undefined | Long;'
/**
 * ts-proto's own 64-bit conversion, dead once every read goes through the
 * reader's `*Value()` methods. Removing it keeps the ceiling it enforces from
 * reading like the codec's contract.
 */
const GENERATED_LONG_HELPER = `function longToNumber(int64: { toString(): string }): number {
  const num = globalThis.Number(int64.toString());
  if (num > globalThis.Number.MAX_SAFE_INTEGER) {
    throw new globalThis.Error("Value is larger than Number.MAX_SAFE_INTEGER");
  }
  if (num < globalThis.Number.MIN_SAFE_INTEGER) {
    throw new globalThis.Error("Value is smaller than Number.MIN_SAFE_INTEGER");
  }
  return num;
}

`
const INT64_READER_METHODS = ['uint64', 'int64', 'sint64', 'fixed64', 'sfixed64'] as const
const INT64_FIELD_TYPE = 'Int64'

// ts-proto wraps `MessageFns<…>` onto following lines for long type names.
const MESSAGE_CODEC_START = /^export const ([A-Za-z0-9_]+):(?:$| MessageFns<)/
const INT64_FIELD_READ = new RegExp(
	`^\\s+message\\.([A-Za-z0-9_]+) = reader\\.(?:${INT64_READER_METHODS.join('|')})Value\\(\\);$`
)
const INTERFACE_START = /^export interface ([A-Za-z0-9_]+) \{$/
const DECLARATION_END = /^\}$/

/**
 * Retype the 64-bit fields ts-proto declares as `number`. Which fields those
 * are is only visible in each message's decode block, so collect them there and
 * rewrite the matching interface members.
 */
const retypeInt64Fields = (source: string): string => {
	const lines = source.split('\n')
	const int64Fields = new Map<string, Set<string>>()
	let codecName: string | undefined
	let reads = 0

	for (const line of lines) {
		const codecStart = MESSAGE_CODEC_START.exec(line)
		if (codecStart) {
			codecName = codecStart[1]!
			continue
		}
		const read = INT64_FIELD_READ.exec(line)
		if (!read) continue
		if (codecName === undefined) {
			throw new Error(`64-bit read outside a message codec: ${line.trim()}`)
		}
		reads++
		const fields = int64Fields.get(codecName) ?? new Set<string>()
		fields.add(read[1]!)
		int64Fields.set(codecName, fields)
	}

	const expectedReads = INT64_READER_METHODS.reduce(
		(total, method) => total + source.split(`reader.${method}Value()`).length - 1,
		0
	)
	if (reads !== expectedReads) {
		throw new Error(`ts-proto emitted a 64-bit read in an unhandled shape (${reads}/${expectedReads})`)
	}

	let interfaceName: string | undefined
	let pending: Set<string> | undefined
	for (let index = 0; index < lines.length; index++) {
		const line = lines[index]!
		const interfaceStart = INTERFACE_START.exec(line)
		if (interfaceStart) {
			interfaceName = interfaceStart[1]!
			pending = new Set(int64Fields.get(interfaceName) ?? [])
			continue
		}
		if (pending === undefined) continue
		if (DECLARATION_END.test(line)) {
			if (pending.size > 0) {
				throw new Error(`${interfaceName} declares no member for ${[...pending].join(', ')}`)
			}
			pending = undefined
			continue
		}
		const member = /^(\s+)([A-Za-z0-9_]+)\?: number \| undefined;$/.exec(line)
		if (!member || !pending.delete(member[2]!)) continue
		lines[index] = `${member[1]}${member[2]}?: ${INT64_FIELD_TYPE} | undefined;`
	}

	return lines.join('\n')
}

const GENERATED_DECODE_DECLARATION = '  decode(input: BinaryReader | Uint8Array, length?: number): T;'
const MERGING_DECODE_DECLARATION =
	'  decode(input: BinaryReader | Uint8Array, length?: number, into?: T): T;'

// ts-proto wraps a long signature or a long `createBase…` call over several
// lines, so both shapes are matched across newlines rather than per line.
const DECODE_SIGNATURE = /decode\(\s*input: BinaryReader \| Uint8Array,\s*length\?: number,?\s*\): ([A-Za-z0-9_]+) \{/g
const DECODE_BASE = /(const end = length === undefined \? reader\.len : reader\.pos \+ length;\s+const message =\s+)(createBase[A-Za-z0-9_]+\(\);)/g
const SINGULAR_MESSAGE_READ =
	/message\.([A-Za-z0-9_]+) =(\s*[A-Za-z0-9_]+\s*)\.decode\(\s*reader,\s*reader\.uint32\(\),?\s*\);/g
const SINGULAR_MESSAGE_READ_ANY = /message\.[A-Za-z0-9_]+ =\s*[A-Za-z0-9_]+\s*\.decode\(/g

/**
 * A singular message field read twice merges, as `MergeFrom` would — that is
 * what the wire format defines, so parsing two concatenated encodings equals
 * parsing each and merging. ts-proto assigns instead, dropping everything the
 * earlier instance carried, which left this codec and every other
 * implementation reading the same well-formed bytes as different objects.
 *
 * Threading the value already read back through `decode` is how protobufjs does
 * it, and it keeps the cost on the repeat path: `into` is undefined on a field's
 * first read, which is every read in an ordinary message.
 */
const mergeRepeatedMessageFields = (source: string): string => {
	let signatures = 0
	let bases = 0
	let reads = 0
	let merged = source.replace(DECODE_SIGNATURE, (_match, type: string) => {
		signatures++
		return `decode(input: BinaryReader | Uint8Array, length?: number, into?: ${type}): ${type} {`
	})
	merged = merged.replace(DECODE_BASE, (_match, head: string, base: string) => {
		bases++
		return `${head}into ?? ${base}`
	})
	merged = merged.replace(SINGULAR_MESSAGE_READ, (_match, field: string, codec: string) => {
		reads++
		return `message.${field} =${codec}.decode(reader, reader.uint32(), message.${field});`
	})
	if (signatures === 0 || signatures !== bases) {
		throw new Error(`ts-proto emitted a decode in an unhandled shape (${signatures} signatures, ${bases} bodies)`)
	}
	const singularReads = source.match(SINGULAR_MESSAGE_READ_ANY)?.length ?? 0
	if (reads === 0 || reads !== singularReads) {
		throw new Error(`ts-proto emitted a singular message read in an unhandled shape (${reads}/${singularReads})`)
	}
	return replaceGeneratedContract(merged, GENERATED_DECODE_DECLARATION, MERGING_DECODE_DECLARATION)
}

const FIELD_CASE = /^[ \t]*case \d+: \{$/gm
const GUARDED_FIELD_CASE = /^[ \t]*case (\d+): \{\n[ \t]*if \(tag (?:!==|===) (\d+)\) \{$/gm

/**
 * A field carrying a wire type the schema does not declare is an unknown field,
 * and the guard each case opens with is the whole of what makes this codec skip
 * it instead of reading those bytes as that field. No ts-proto option promises
 * the guard, so losing it has to fail the build rather than turn the codec into
 * a parser differential quietly.
 */
const assertWireTypeGuards = (source: string): void => {
	const cases = source.match(FIELD_CASE)?.length ?? 0
	let guarded = 0
	for (const [, field, tag] of source.matchAll(GUARDED_FIELD_CASE)) {
		if (Number(tag) >>> 3 !== Number(field)) {
			throw new Error(`ts-proto guarded field ${field} with tag ${tag}, which is field ${Number(tag) >>> 3}`)
		}
		guarded++
	}
	if (cases === 0 || guarded !== cases) {
		throw new Error(`ts-proto emitted a field case with no wire type guard (${guarded}/${cases})`)
	}
}

const TS_PROTO_OPTIONS = [
	'outputJsonMethods=false',
	'useExactTypes=false',
	'useOptionals=all',
	'noDefaultsForOptionals=true',
	'initializeFieldsAsUndefined=false'
] as const

// Protobuf keyword per descriptor type, so the manifest reads like the .proto
// it came from rather than like a descriptor dump.
const TYPE_KEYWORD: Partial<Record<Type, string>> = {
	[Type.DOUBLE]: 'double',
	[Type.FLOAT]: 'float',
	[Type.INT64]: 'int64',
	[Type.UINT64]: 'uint64',
	[Type.INT32]: 'int32',
	[Type.FIXED64]: 'fixed64',
	[Type.FIXED32]: 'fixed32',
	[Type.BOOL]: 'bool',
	[Type.STRING]: 'string',
	[Type.MESSAGE]: 'message',
	[Type.BYTES]: 'bytes',
	[Type.UINT32]: 'uint32',
	[Type.ENUM]: 'enum',
	[Type.SFIXED32]: 'sfixed32',
	[Type.SFIXED64]: 'sfixed64',
	[Type.SINT32]: 'sint32',
	[Type.SINT64]: 'sint64'
}

const HEADER = [
	'# Generated by scripts/gen-ts-proto.ts from waproto/src/whatsapp.proto. Do not edit.',
	'# <message>\t<field>\t<number>\t<label>\t<type>[\t<enum probe values or message type>]',
	'# <label> is optional | required | repeated | repeated packed | map.',
	'# A line with only <message> is a message the schema declares with no fields.',
	''
].join('\n')

const LABEL_KEYWORD: Partial<Record<Label, string>> = {
	[Label.OPTIONAL]: 'optional',
	[Label.REQUIRED]: 'required',
	[Label.REPEATED]: 'repeated'
}

interface CargoPackage {
	name: string
	manifest_path: string
}

interface CargoMetadata {
	packages: CargoPackage[]
}

const run = (command: readonly string[], stdout: 'pipe' | 'inherit' = 'inherit'): Uint8Array => {
	const result = Bun.spawnSync({ cmd: [...command], cwd: ROOT, stdout, stderr: 'inherit' })
	if (result.exitCode !== 0) {
		throw new Error(`${command[0]} exited with status ${result.exitCode}`)
	}
	return result.stdout
}

const replaceGeneratedContract = (source: string, expected: string, replacement: string): string => {
	const first = source.indexOf(expected)
	if (first < 0 || first !== source.lastIndexOf(expected)) {
		throw new Error(`ts-proto output must contain exactly one occurrence of: ${expected}`)
	}
	return source.replace(expected, replacement)
}

/**
 * Flatten the compiled schema into one tab-separated line per field:
 *
 *   <message path>  <declared name>  <number>  <label>  <type>  [type ref]
 *
 * `tests/proto-schema-surface.test.ts` replays this against the generated
 * codec, so the schema — not the generator's own output — is what says which
 * fields must cross, under which name, at which number. Keeping it as text
 * rather than a descriptor blob also puts an upstream renumbering in the diff.
 */
const buildSurface = (descriptor: Uint8Array): string => {
	const set = fromBinary(FileDescriptorSetSchema, descriptor)
	const mapEntries = new Map<string, DescriptorProto>()
	const enumProbes = new Map<string, string>()
	const lines: string[] = []

	// The first declared value, plus a second where one exists: an enum tested
	// only at its head cannot catch a codec that collapses the other values.
	const indexEnum = (path: string, declared: { value: { number: number }[] }): void => {
		const numbers = declared.value.slice(0, 2).map(value => value.number)
		enumProbes.set(path, (numbers.length > 0 ? numbers : [0]).join(','))
	}

	const index = (scope: string, message: DescriptorProto): void => {
		const path = `${scope}.${message.name}`
		if (message.options?.mapEntry) mapEntries.set(path, message)
		for (const declared of message.enumType) indexEnum(`${path}.${declared.name}`, declared)
		for (const nested of message.nestedType) index(path, nested)
	}

	const typeRef = (field: FieldDescriptorProto, local: (p: string) => string): string => {
		if (field.type === Type.ENUM) {
			// No fallback: a proto2 enum need not start at zero, and defaulting an
			// unresolved reference to 0 would hand the test a value the schema
			// never declares.
			const probes = enumProbes.get(field.typeName)
			if (probes === undefined) throw new Error(`unresolved enum ${field.typeName} on ${field.name}`)
			return `${local(field.typeName)}=${probes}`
		}
		if (field.type === Type.MESSAGE) return local(field.typeName)
		return ''
	}

	const describe = (field: FieldDescriptorProto, local: (p: string) => string): [string, string, string] => {
		const entry = field.type === Type.MESSAGE ? mapEntries.get(field.typeName) : undefined
		if (!entry) {
			const label = LABEL_KEYWORD[field.label]
			if (!label) throw new Error(`unhandled label on ${field.name}`)
			const keyword = TYPE_KEYWORD[field.type]
			if (!keyword) throw new Error(`unhandled type on ${field.name}`)
			// Packedness decides the wire type of a repeated scalar, and proto2
			// defaults to unpacked, so it has to be recorded rather than inferred.
			const packed = field.label === Label.REPEATED && field.options?.packed === true
			return [packed ? `${label} packed` : label, keyword, typeRef(field, local)]
		}
		const key = entry.field.find(candidate => candidate.number === 1)!
		const value = entry.field.find(candidate => candidate.number === 2)!
		const keyKeyword = TYPE_KEYWORD[key.type]
		const valueKeyword = TYPE_KEYWORD[value.type]
		if (!keyKeyword || !valueKeyword) throw new Error(`unhandled map type on ${field.name}`)
		return ['map', `map<${keyKeyword},${valueKeyword}>`, typeRef(value, local)]
	}

	const emit = (scope: string, local: (p: string) => string, message: DescriptorProto): void => {
		const path = `${scope}.${message.name}`
		if (message.options?.mapEntry) return
		// A message with no fields still has a generated type to resolve, so it
		// gets a line of its own rather than being inferred from its fields.
		if (message.field.length === 0) lines.push(local(path))
		for (const field of message.field) {
			const [label, keyword, ref] = describe(field, local)
			const columns = [local(path), field.name, String(field.number), label, keyword]
			if (ref) columns.push(ref)
			lines.push(columns.join('\t'))
		}
		for (const nested of message.nestedType) emit(path, local, nested)
	}

	for (const file of set.file) {
		const root = `.${file.package}`
		for (const declared of file.enumType) indexEnum(`${root}.${declared.name}`, declared)
		for (const message of file.messageType) index(root, message)
	}
	for (const file of set.file) {
		const root = `.${file.package}`
		const local = (path: string): string => (path.startsWith(`${root}.`) ? path.slice(root.length + 1) : path)
		for (const message of file.messageType) emit(root, local, message)
	}

	return `${HEADER}${lines.join('\n')}\n`
}

const metadata = JSON.parse(
	new TextDecoder().decode(run(['cargo', 'metadata', '--format-version', '1', '--locked'], 'pipe'))
) as CargoMetadata
const protoPackages = metadata.packages.filter(pkg => pkg.name === PROTO_PACKAGE_NAME)

if (protoPackages.length !== 1) {
	throw new Error(`expected one resolved ${PROTO_PACKAGE_NAME} package, found ${protoPackages.length}`)
}

const manifestDir = dirname(protoPackages[0]!.manifest_path)
const protoFile = join(manifestDir, ...PROTO_SOURCE_PATH)
const protoDir = dirname(protoFile)
const tempDir = join(OUTPUT_DIR, `.ts-proto-${process.pid}`)

mkdirSync(tempDir, { recursive: false })
try {
	const descriptorFile = join(tempDir, 'whatsapp.desc')
	run([
		'protoc',
		`--proto_path=${protoDir}`,
		`--plugin=protoc-gen-ts_proto=${TS_PROTO_PLUGIN}`,
		`--ts_proto_out=${tempDir}`,
		`--ts_proto_opt=${TS_PROTO_OPTIONS.join(',')}`,
		`--descriptor_set_out=${descriptorFile}`,
		protoFile
	])
	const generatedFile = join(tempDir, 'whatsapp.ts')
	let generatedSource = readFileSync(generatedFile, 'utf8')
	generatedSource = replaceGeneratedContract(generatedSource, GENERATED_WIRE_IMPORT, OPTIMIZED_WIRE_IMPORT)
	generatedSource = replaceGeneratedContract(generatedSource, GENERATED_BUILTIN, OPTIMIZED_BUILTIN)
	for (const method of INT64_READER_METHODS) {
		generatedSource = generatedSource.replaceAll(
			`longToNumber(reader.${method}())`,
			`reader.${method}Value()`
		)
	}
	generatedSource = replaceGeneratedContract(generatedSource, GENERATED_LONG_HELPER, '')
	if (generatedSource.includes('longToNumber')) {
		throw new Error('ts-proto added an int64 conversion without a 64-bit specialization')
	}
	generatedSource = retypeInt64Fields(generatedSource)
	generatedSource = mergeRepeatedMessageFields(generatedSource)
	assertWireTypeGuards(generatedSource)
	writeFileSync(generatedFile, generatedSource)
	writeFileSync(SURFACE_FILE, buildSurface(readFileSync(descriptorFile)))
	renameSync(generatedFile, OUTPUT_FILE)
} finally {
	rmSync(tempDir, { recursive: true, force: true })
}

console.log(`Generated ${OUTPUT_FILE} and ${SURFACE_FILE} from ${protoFile}`)
