kind: fixed

The search grammar reads a Windows path as typed: `folder:C:\Photos\2024` and `folder:"C:\Photos\2024 trip"` keep their backslashes (only a run of them before a `"` is an escape, as on Windows command lines), so a folder term on Windows selects its photos again instead of searching for a word.
