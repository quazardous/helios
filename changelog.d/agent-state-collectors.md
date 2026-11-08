kind: added

Agents and scenarios read Helios's state from three debug collectors: `helios.library` (images shown, picked and rejected, the selection, the image under the cursor, the size of the image open), `helios.values` (the image's parameters by path, as `param.get` reads them) and `helios.jobs` (jobs counted by state, each job's kind and state), so a scenario checks `Expect(Value(..))` instead of a command's result.
