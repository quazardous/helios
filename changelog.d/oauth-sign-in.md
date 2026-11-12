kind: added

Sources that sign in with an online account (OAuth) open the system browser on the service's consent page and get the answer back on a local port, with PKCE. The tokens are kept in the system keyring, renewed when they expire, and revoked on signing out; an access the service no longer honours shows the account as needing attention. The client id comes from the environment (`HELIOS_<SOURCE>_CLIENT_ID`), never built in.
