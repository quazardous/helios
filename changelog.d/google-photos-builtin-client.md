kind: changed

Google Photos no longer asks for an environment variable: release packages carry Helios's own OAuth client, and Settings > Accounts > "Use my own OAuth client" takes yours (client ID and secret, kept in the system keyring) and says which client is used. With no client at all, connecting says to set one there. The `HELIOS_GOOGLE_PHOTOS_CLIENT_ID` and `HELIOS_GOOGLE_PHOTOS_CLIENT_SECRET` variables still work, after your own client and before the built-in one.
