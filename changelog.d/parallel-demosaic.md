kind: changed

Faster photo opening: the RCD and Markesteijn demosaic, the slowest step of opening a raw, now runs in small tiles on every processor core, with the same pixels as before. A render that is no longer wanted can give it up between tiles.
