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
const OPTIMIZED_WIRE_IMPORT = 'import { BinaryReader, BinaryWriter } from "../proto-reader";'
const GENERATED_LONG_PARAMETER = 'function longToNumber(int64: { toString(): string }): number {'
const OPTIMIZED_LONG_PARAMETER = 'function longToNumber(int64: number | { toString(): string }): number {'
const GENERATED_LONG_CONVERSION = 'const num = globalThis.Number(int64.toString());'
const OPTIMIZED_LONG_CONVERSION =
	'const num = typeof int64 === "number" ? int64 : globalThis.Number(int64.toString());'
const SAFE_NUMBER_READER_METHODS = ['uint64', 'int64', 'sint64', 'fixed64', 'sfixed64'] as const

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
	generatedSource = replaceGeneratedContract(generatedSource, GENERATED_LONG_PARAMETER, OPTIMIZED_LONG_PARAMETER)
	generatedSource = replaceGeneratedContract(generatedSource, GENERATED_LONG_CONVERSION, OPTIMIZED_LONG_CONVERSION)
	for (const method of SAFE_NUMBER_READER_METHODS) {
		generatedSource = generatedSource.replaceAll(
			`longToNumber(reader.${method}())`,
			`reader.${method}Number()`
		)
	}
	if (generatedSource.includes('longToNumber(reader.')) {
		throw new Error('ts-proto added an int64 reader method without a safe-number specialization')
	}
	writeFileSync(generatedFile, generatedSource)
	renameSync(generatedFile, OUTPUT_FILE)
} finally {
	rmSync(tempDir, { recursive: true, force: true })
}

console.log(`Generated ${OUTPUT_FILE} from ${protoFile}`)
