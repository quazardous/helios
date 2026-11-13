kind: fixed

Many clients editing the same photos through one headless node no longer slow every other call down: opening a photo's edit (`library.edit.head`, `library.edit.save`) found its newest version by reading its history from the vault once per diverged version, holding the vault the whole time. It now reads the photo's versions once.
