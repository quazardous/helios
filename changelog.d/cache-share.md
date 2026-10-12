kind: changed

The render cache now uses at most a quarter of the machine's memory by default
instead of a fixed 2 GiB; the share, or a fixed size, is a machine setting.
A render never evicts what it still needs, so a small budget no longer makes
it compute the same step twice.
