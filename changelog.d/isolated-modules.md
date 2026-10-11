kind: added

Image processing modules can be written and tested on their own: a module
declares its parameters, ports and needs, and the framework checks every
output against that declaration. Contract tests generated from the
declaration catch non-finite values, renders that differ from one run to
the next, and tiles that differ from a whole render.
