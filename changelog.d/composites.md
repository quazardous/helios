kind: internal

The engine expands a node standing for a network when it plans a render:
a macro (a saved sub-network exposing a few chosen parameters) or a
module made of several kernels. Each inner node is planned, cached, tiled
and run on the GPU like any node, and a macro shares its results with
the flat network it packages.
