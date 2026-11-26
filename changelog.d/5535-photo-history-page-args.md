kind: fixed

`photo.history` refuses a negative `from` or a `limit` below 1 with a clear reason, instead of reading the newest page as if they were absent.
