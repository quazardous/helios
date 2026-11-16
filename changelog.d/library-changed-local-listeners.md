kind: changed

`library.changed`'s listeners (the volumes, the Library read again, the albums, the stacks) are local listeners on the interface's thread, each doing its own part inside the pass, instead of marking their place for the persistence layer to do their part afterwards. The bus's order dump marks them `local`.
