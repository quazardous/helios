kind: added

Previews: any part of a network can be rendered at a reduced scale, half
or a quarter of full resolution, and look like the full render reduced.
Sizes in a module (a blur radius) are given in full-resolution pixels and
follow the scale on their own; previews and exact renders are cached
apart. A Gaussian blur is the first module to use it.
