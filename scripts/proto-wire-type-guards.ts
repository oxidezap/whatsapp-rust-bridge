/**
 * The build-time check that the generated codec still skips a field carrying a
 * wire type the schema does not declare.
 *
 * Its own module so `tests/proto-wire-type-guards.test.ts` can mutate a codec
 * and watch it fail: importing `gen-ts-proto.ts` would run protoc.
 */

import { fromBinary } from '@bufbuild/protobuf'
import {
	FieldDescriptorProto_Label as Label,
	FieldDescriptorProto_Type as Type,
	FileDescriptorSetSchema,
	type DescriptorProto,
	type FieldDescriptorProto
} from '@bufbuild/protobuf/wkt'

const WIRE_TYPE: Partial<Record<Type, number>> = {
	[Type.INT32]: 0,
	[Type.INT64]: 0,
	[Type.UINT32]: 0,
	[Type.UINT64]: 0,
	[Type.SINT32]: 0,
	[Type.SINT64]: 0,
	[Type.BOOL]: 0,
	[Type.ENUM]: 0,
	[Type.DOUBLE]: 1,
	[Type.FIXED64]: 1,
	[Type.SFIXED64]: 1,
	[Type.STRING]: 2,
	[Type.BYTES]: 2,
	[Type.MESSAGE]: 2,
	[Type.FLOAT]: 5,
	[Type.FIXED32]: 5,
	[Type.SFIXED32]: 5
}
const LENGTH_DELIMITED = 2
// Multiplied rather than shifted: a field number may reach 2^29 - 1, and `<<`
// is a signed 32-bit operator, so the top two orders of magnitude would come
// back negative and match no tag the codec carries.
const TAG_FIELD_FACTOR = 8

/**
 * Every tag the schema lets a field arrive under. A repeated scalar has two —
 * packedness decides which one an encoder writes, and a parser accepts both
 * either way — and everything else has exactly one.
 */
const declaredTags = (field: FieldDescriptorProto): number[] => {
	const wire = WIRE_TYPE[field.type]
	if (wire === undefined) throw new Error(`unhandled type on ${field.name}`)
	const tag = field.number * TAG_FIELD_FACTOR + wire
	return field.label === Label.REPEATED && wire !== LENGTH_DELIMITED
		? [tag, field.number * TAG_FIELD_FACTOR + LENGTH_DELIMITED]
		: [tag]
}

/** Codec name as ts-proto flattens it, to the tags each of its fields declares. */
export const declaredTagsByCodec = (descriptor: Uint8Array): Map<string, Map<number, Set<number>>> => {
	const set = fromBinary(FileDescriptorSetSchema, descriptor)
	const codecs = new Map<string, Map<number, Set<number>>>()

	const collect = (scope: string, local: (path: string) => string, message: DescriptorProto): void => {
		const path = `${scope}.${message.name}`
		const tags = new Map<number, Set<number>>()
		// Map entries included: ts-proto emits a codec for each one.
		for (const field of message.field) tags.set(field.number, new Set(declaredTags(field)))
		codecs.set(local(path).replaceAll('.', '_'), tags)
		for (const nested of message.nestedType) collect(path, local, nested)
	}

	for (const file of set.file) {
		const root = `.${file.package}`
		const local = (path: string): string => (path.startsWith(`${root}.`) ? path.slice(root.length + 1) : path)
		for (const message of file.messageType) collect(root, local, message)
	}
	return codecs
}

// ts-proto wraps `MessageFns<…>` onto following lines for long type names.
const MESSAGE_CODEC_START = /^export const ([A-Za-z0-9_]+):(?:$| MessageFns<)/
// Indentation is captured rather than fixed: prettier indents the whole codec
// two spaces further when ts-proto wraps a long `MessageFns<…>` onto its own
// line, and a guard only counts when it sits directly inside its own case.
const DECODE_FIELD_CASE = /^(\s+)case (\d+): \{$/
const DECODE_TAG_GUARD = /^(\s+)if \(tag (!==|===) (\d+)\) \{$/
const GUARD_INDENT = '  '
const SOLE_TAG_OPERATOR = '!=='
const ALTERNATIVE_TAG_OPERATOR = '==='
/**
 * The decode loop's epilogue, which every case that leaves on `break` falls
 * into. Without the skip a mismatched tag leaves its case without consuming the
 * field, and the bytes it framed are read as the tags that follow; without the
 * throw, framing that is malformed rather than unknown truncates the message
 * where a caller cannot see it.
 */
const FIELD_NUMBER_DISPATCH = 'switch (tag >>> 3) {'
const UNKNOWN_FIELD_SKIP = [
	'if (tag >>> 3 === 0 || (tag & 7) === 4) {',
	'throw new RangeError(`illegal protobuf tag ${tag} at offset ${reader.pos}`);',
	'}',
	'reader.skip(tag & 7);'
]

interface Guard {
	tag: number
	operator: string
	/** The statement the guarded block leaves on: `break` or `continue`. */
	exit: string | undefined
	/** How many statements the block holds, so a read cannot hide before the exit. */
	statements: number
}

/**
 * What the guarded block does, which is the other half of the guard: a `!==`
 * block leaves the case on the one tag that does not match, and a `===` block
 * reads and continues on the one that does. A guard that falls through instead
 * reads the bytes it was meant to skip.
 */
const guardBlock = (lines: string[], index: number, indent: string): Pick<Guard, 'exit' | 'statements'> => {
	const close = `${indent}${GUARD_INDENT}}`
	for (let scan = index + 1; scan < lines.length; scan++) {
		if (lines[scan] !== close) continue
		const body = lines.slice(index + 1, scan).map(line => line.trim()).filter(line => line !== '')
		const last = body[body.length - 1]
		const exit = last === 'break;' || last === 'continue;' ? last.slice(0, -1) : undefined
		return { exit, statements: body.length }
	}
	return { exit: undefined, statements: 0 }
}

/**
 * A field carrying a wire type the schema does not declare is an unknown field,
 * and the tag guards each case opens with are the whole of what makes this codec
 * skip it instead of reading those bytes as that field. No ts-proto option
 * promises those guards, so the build has to fail rather than quietly turn the
 * codec into a parser differential when a case loses a guard, guards the wrong
 * tag, inverts the comparison, falls out of either path, or stops being
 * recognisable here at all.
 *
 * The shapes checked are the two ts-proto emits, both paths of each. A field
 * with one declared tag leaves its case on `tag !== declared` and continues the
 * read loop after reading, because falling out of the switch instead reaches
 * `reader.skip(tag & 7)` and skips a field that was already read. A repeated
 * numeric field — which arrives packed or unpacked, both the schema's — reads
 * and continues under one `tag === declared` branch per form, and leaves the
 * case for anything else.
 */
export const assertWireTypeGuards = (source: string, descriptor: Uint8Array): void => {
	const declared = declaredTagsByCodec(descriptor)
	const decoded = new Map<string, Set<number>>()
	const lines = source.split('\n')
	let codec: string | undefined
	let field: number | undefined
	let indent = ''
	let guards: Guard[] = []
	let tail: { indent: string; text: string } | undefined
	let direct: string[] = []
	let codecAt = 0

	const flush = (): void => {
		if (field === undefined) return
		const expected = declared.get(codec!)?.get(field)
		if (expected === undefined) {
			throw new Error(`${codec} decodes field ${field}, which the schema does not declare`)
		}
		const seen = [...new Set(guards.map(guard => guard.tag))].sort((a, b) => a - b)
		const want = [...expected].sort((a, b) => a - b)
		if (seen.join(',') !== want.join(',')) {
			throw new Error(`${codec} field ${field} guards tags [${seen}], the schema declares [${want}]`)
		}
		const sole = want.length === 1
		const wanted = sole ? SOLE_TAG_OPERATOR : ALTERNATIVE_TAG_OPERATOR
		const operators = [...new Set(guards.map(guard => guard.operator))]
		if (operators.length !== 1 || operators[0] !== wanted) {
			throw new Error(`${codec} field ${field} guards with [${operators}], expected ${wanted}`)
		}
		const exit = sole ? 'break' : 'continue'
		const exits = [...new Set(guards.map(guard => guard.exit ?? 'nothing'))]
		if (exits.length !== 1 || exits[0] !== exit) {
			throw new Error(`${codec} field ${field} leaves its guard on [${exits}], expected ${exit}`)
		}
		// A mismatched tag has to leave on the spot. Anything running first moves
		// the reader before the skip that consumes the field it did not match.
		if (sole && guards[0]!.statements !== 1) {
			throw new Error(`${codec} field ${field} runs ${guards[0]!.statements} statements before leaving its guard`)
		}
		// Nothing may run before the last guard: a read ahead of one moves the
		// reader before the tag it does not match is skipped, whether it sits at
		// the top of the case or between the two branches of a repeated field.
		const lastGuard = direct.lastIndexOf('guard')
		const early = direct.slice(0, Math.max(lastGuard, 0)).filter(text => text !== 'guard' && text !== '}')
		if (lastGuard !== 0 && early.length === 0 && direct[0] !== 'guard') early.push(direct[0] ?? 'nothing')
		if (early.length > 0) {
			throw new Error(`${codec} field ${field} runs [${early[0]}] before its guard`)
		}
		// The path the matching tag takes, which the guard says nothing about: a
		// case that reads and then falls out of the switch reaches
		// `reader.skip(tag & 7)` and skips the field after the one it just read.
		const ends = sole ? 'continue;' : 'break;'
		if (tail?.indent !== `${indent}${GUARD_INDENT}` || tail.text !== ends) {
			throw new Error(`${codec} field ${field} ends its case on [${tail?.text ?? 'nothing'}], expected ${ends}`)
		}
		decoded.get(codec!)!.add(field)
		field = undefined
		guards = []
	}

	/** The epilogue is per decode loop, so it is checked once per codec. */
	const finishCodec = (end: number): void => {
		if (codec === undefined) return
		const body = lines.slice(codecAt, end).map(line => line.trim())
		// ts-proto emits no `default`, and one would be a way past the epilogue
		// that leaves it textually intact: an unknown tag reaching a branch that
		// continues never has its payload consumed.
		if (body.some(line => line.startsWith('default:'))) {
			throw new Error(`${codec} decodes through a default branch, which bypasses the unknown-field skip`)
		}
		flush()
		// Every case is written as a field number, so the switch has to be reading
		// one: `switch (tag)` would send each declared tag to the epilogue instead.
		if (!body.includes(FIELD_NUMBER_DISPATCH)) {
			throw new Error(`${codec} does not dispatch its decode on the field number`)
		}
		const at = body.indexOf(UNKNOWN_FIELD_SKIP[0]!)
		if (at < 0 || UNKNOWN_FIELD_SKIP.some((text, offset) => body[at + offset] !== text)) {
			throw new Error(`${codec} does not skip the unknown field its cases break out for`)
		}
	}

	for (let index = 0; index < lines.length; index++) {
		const line = lines[index]!
		const codecStart = MESSAGE_CODEC_START.exec(line)
		if (codecStart) {
			finishCodec(index)
			codec = codecStart[1]!
			codecAt = index
			if (!declared.has(codec)) throw new Error(`${codec} is a codec the schema does not declare`)
			decoded.set(codec, new Set())
			continue
		}
		const caseStart = DECODE_FIELD_CASE.exec(line)
		if (caseStart) {
			flush()
			if (codec === undefined) throw new Error(`decode case outside a codec: ${line.trim()}`)
			indent = caseStart[1]!
			field = Number(caseStart[2])
			tail = undefined
			direct = []
			continue
		}
		if (field === undefined) continue
		if (line === `${indent}}` || line === `${indent}default: {`) {
			flush()
			continue
		}
		const guard = DECODE_TAG_GUARD.exec(line)
		if (guard && guard[1] === `${indent}${GUARD_INDENT}`) {
			guards.push({ tag: Number(guard[3]), operator: guard[2]!, ...guardBlock(lines, index, indent) })
		}
		const text = line.trim()
		if (text === '') continue
		const own = line.slice(0, line.length - text.length)
		if (own === `${indent}${GUARD_INDENT}`) direct.push(guard ? 'guard' : text)
		tail = { indent: own, text }
	}
	finishCodec(lines.length)

	// Every field the schema declares gets a case, so anything missing here is a
	// case that stopped being recognisable rather than one that reads correctly.
	for (const [name, fields] of declared) {
		const covered = decoded.get(name)
		if (covered === undefined) throw new Error(`${name} has no generated codec`)
		const missing = [...fields.keys()].filter(number => !covered.has(number))
		if (missing.length > 0) {
			throw new Error(`${name} decodes no guarded case for field ${missing.join(', ')}`)
		}
	}
}
