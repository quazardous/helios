kind: changed

Importing from a service (Google Photos): when the service asks to wait (a rate limit or a daily quota), the import now pauses and goes on by itself at the time the service gives, instead of failing after a few waits. When the service refuses the account's sign-in, the import pauses until you sign in again (or try it again). The Queue panel shows the import as paused, with why and when it goes on. What was already imported stays; a photo cut mid-download never enters the Library.
