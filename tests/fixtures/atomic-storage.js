// Synchronous copy-on-commit Map host for Rust/WASM contract tests.
// Its transaction scope is this JS realm; it is not a persistent storage adapter.
const backing = globalThis[slot] ??= { rows: new Map(), failure: null }
const encode = value => new TextEncoder().encode(JSON.stringify(value))
const decode = bytes => bytes == null ? null : JSON.parse(new TextDecoder().decode(bytes))
const tokenRow = jid => {
  const bytes = backing.rows.get(`tc_token:${jid}`)
  return bytes == null ? null : JSON.parse(new TextDecoder().decode(bytes), (key, value, context) =>
    (key === 'token_timestamp' || key === 'sender_timestamp') && value !== null ? BigInt(context.source) : value)
}
const encodeToken = row => new TextEncoder().encode(
  `{"token":${JSON.stringify(row.token)},"token_timestamp":${row.token_timestamp},"sender_timestamp":${row.sender_timestamp ?? 'null'}}`)
function transaction(change) {
  const next = new Map(backing.rows)
  change(next)
  if (backing.failure === 'before') throw new Error('before commit')
  backing.rows = next
  if (backing.failure === 'after') throw new Error('after commit')
}
function index(next, key, item) {
  const entries = decode(next.get(key)) ?? []
  if (!entries.includes(item)) entries.push(item)
  next.set(key, encode(entries))
}
return {
  async get(store, key) { return backing.rows.get(`${store}:${key}`)?.slice() ?? null },
  async set(store, key, bytes) { backing.rows.set(`${store}:${key}`, bytes.slice()) },
  async delete(store, key) { backing.rows.delete(`${store}:${key}`) },
  async commitPatch(name, state, removed, added, maintainIndex) {
    transaction(next => {
      for (const key of removed) next.delete(`mutation_mac:${key}`)
      for (const [key, bytes] of added) next.set(`mutation_mac:${key}`, bytes.slice())
      next.set(`sync_version:${name}`, state.slice())
      if (maintainIndex) {
        const entries = new Set(decode(next.get('meta:mutation_mac_keys')) ?? [])
        for (const key of removed) entries.delete(key)
        for (const [key] of added) entries.add(key)
        next.set('meta:mutation_mac_keys', encode([...entries]))
      }
    })
  },
  async touchTcTokenSenderTimestamp(jid, timestamp) {
    transaction(next => {
      const incoming = BigInt(timestamp)
      const row = tokenRow(jid) ?? { token: [], token_timestamp: incoming, sender_timestamp: null }
      if (row.sender_timestamp === null || incoming > row.sender_timestamp) row.sender_timestamp = incoming
      next.set(`tc_token:${jid}`, encodeToken(row))
      index(next, 'meta:tc_token_jids', jid)
    })
  },
  async storeReceivedTcToken(jid, bytes, timestamp) {
    transaction(next => {
      const incoming = BigInt(timestamp)
      const row = tokenRow(jid) ?? { token: [], token_timestamp: incoming, sender_timestamp: null }
      if (row.token.length === 0 || incoming >= row.token_timestamp) {
        row.token = [...bytes]
        row.token_timestamp = incoming
      }
      next.set(`tc_token:${jid}`, encodeToken(row))
      index(next, 'meta:tc_token_jids', jid)
    })
  },
  setFailure(mode) { backing.failure = mode }
}
