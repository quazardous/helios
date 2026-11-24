kind: fixed

The search grammar reads a Windows path as typed, quoted or not: its backslashes are no escapes (only a run of them before a `"` is, as on Windows command lines), so a `folder:` term with a Windows path selects its photos again instead of searching for a word.
