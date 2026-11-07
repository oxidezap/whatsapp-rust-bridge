const accounts = globalThis.__signalAddressAccounts ??= new Map();
const account = accounts.get(slot) ?? new Map();
accounts.set(slot, account);
const namespace = store => {
    if (!account.has(store)) account.set(store, new Map());
    return account.get(store);
};
return {
    get(store, key) { return namespace(store).get(key) ?? null; },
    set(store, key, value) { namespace(store).set(key, value.slice()); },
    delete(store, key) { namespace(store).delete(key); },
    commitPatch() { throw new Error('unused'); },
    touchTcTokenSenderTimestamp() { throw new Error('unused'); },
    storeReceivedTcToken() { throw new Error('unused'); },
    capabilities: { enumerate: true },
    // Deliberately ignore prefix to prove the adapter checks exact user boundaries.
    listKeys(store) { return [...namespace(store).keys()]; }
};
