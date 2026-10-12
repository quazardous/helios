kind: changed

A node read by several others over different areas is now computed once over
the area they all need, each reading its part of it: a network of 58 mask
nodes renders about nine times faster with a sixth of the memory, and an area
inside one already rendered is shown without computing anything.
