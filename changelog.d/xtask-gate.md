kind: internal

Pull requests into next carry a test manifest; the tester's gate merges a batch on a temporary branch, runs their manifests on the bench box and fast-forwards next when every job passes (no devel branch).
