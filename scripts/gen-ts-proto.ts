/**
 * Regenerate the pure-TypeScript protobuf codec from the exact `waproto`
 * package selected by Cargo.lock.
 *
 * Resolving the schema through Cargo metadata keeps the bridge tied to its
 * single public `whatsapp-rust` dependency without a sibling-repository path
 * or a second independently versioned copy of whatsapp.proto.
 */

import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUTPUT_DIR = join(ROOT, 'ts', 'generated')
const OUTPUT_FILE = join(OUTPUT_DIR, 'whatsapp.ts')
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

const TS_PROTO_OPTIONS = [
	'outputJsonMethods=false',
	'useExactTypes=false',
	'useOptionals=all',
	'noDefaultsForOptionals=true',
	'initializeFieldsAsUndefined=false'
] as const

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
	run([
		'protoc',
		`--proto_path=${protoDir}`,
		`--plugin=protoc-gen-ts_proto=${TS_PROTO_PLUGIN}`,
		`--ts_proto_out=${tempDir}`,
		`--ts_proto_opt=${TS_PROTO_OPTIONS.join(',')}`,
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
	writeFileSync(generatedFile, generatedSource)
	renameSync(generatedFile, OUTPUT_FILE)
} finally {
	rmSync(tempDir, { recursive: true, force: true })
}

console.log(`Generated ${OUTPUT_FILE} from ${protoFile}`)
