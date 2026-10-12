kind: internal

`cargo xtask check-commits` builds and tests each commit of a series, not only its head, so bisecting keeps working; CI runs it on every push and pull request.
