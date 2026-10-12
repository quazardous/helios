kind: internal

A module can declare the region outside which its output is a constant (a
drawn mask shape is empty away from it): the engine fills tiles outside it
without running the module or computing what it reads.
