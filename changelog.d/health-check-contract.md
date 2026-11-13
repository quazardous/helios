kind: internal

Health checks are a contract: each check declares what it watches, the levels it may raise and the fixes it may offer, and is owned by the module that declares it. A declaration that is unsound is refused with its reasons, and a finding outside its check's declaration shows as a problem of that check. Helios's own checks (storage, library, backups, GPU, plugins, catalogue snapshots) moved onto it.
