(
/** Share persistent namespaces within one fixture account. */
function signalAddressStorage(slot) {
const accounts = globalThis.__signalAddressAccounts ??= new Map();
const account = accounts.get(slot) ?? new Map();
accounts.set(slot, account);
/** Keep fixture namespaces separate, including missing-key reads. */
const namespace = store => {
    if (!account.has(store)) account.set(store, new Map());
    return account.get(store);
};
const reads = [];
return {
    reads,
    /** Record per-key reads so tests can distinguish the fallback path. */
    get(store, key) { reads.push(["get", store, key]); return namespace(store).get(key) ?? null; },
    /** Return found rows in reverse order to exercise unordered host batches. */
    getMany(store, keys) {
        reads.push(["getMany", store, keys.length]);
        return keys.toReversed().filter(key => namespace(store).has(key))
            .map(key => [key, namespace(store).get(key)]);
    },
    set(store, key, value) { namespace(store).set(key, value.slice()); },
    delete(store, key) { namespace(store).delete(key); },
    commitPatch() { throw new Error('unused'); },
    touchTcTokenSenderTimestamp() { throw new Error('unused'); },
    storeReceivedTcToken() { throw new Error('unused'); },
    capabilities: { enumerate: true },
    // Deliberately ignore prefix to prove the adapter checks exact user boundaries.
    /** Expose all keys so the adapter must enforce exact user matching. */
    listKeys(store) { return [...namespace(store).keys()]; }
};

});
