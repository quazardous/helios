kind: fixed

A library refresh after a scan or an import is again one cursor refresh and one read of what changed. Reading the cursor's order through the node had asked the store once per 500 photos; a `cursor.window` that asks no field now answers from the cursor's snapshot, without a store query.
