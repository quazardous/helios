kind: internal

The link between the desktop and a node has a measured baseline: a test runs today's link (the inspect relay stream and the control endpoint) and bus-bridge's Unix socket link side by side on the same Helios buses, checks both deliver each fact once and in order and answer requests, and reports their latency, burst throughput, time to resume after a drop and request round trip.
