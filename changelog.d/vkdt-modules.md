kind: added

The first modules ported from vkdt: its exposure and its unsharp mask
(`vkdt_exposure`, `vkdt_usm`), each with a GPU variant translated from
vkdt's shader. A nightly check builds vkdt and compares Helios's results
with vkdt's own.
