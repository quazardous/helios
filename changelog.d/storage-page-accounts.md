kind: fixed

The Storage page no longer blanks while it refreshes: it keeps its rows, marked "Refreshing…", until the new ones come. A service account signed in to twice shows as one storage row, with the import sources using it. A connector whose source is the account itself (Google Photos) takes one source per account: the Add wizard and `import.sources.add` refuse a second one, also on another account signed in as the same address, and offer to open the source already there.
