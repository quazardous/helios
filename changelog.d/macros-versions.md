kind: added

Macros: folding a macro changed while open asks what to do with the
changes: save them as the macro's next version (the default), expose
them, discard them, or keep it open; each is one undo step that keeps
the image's look. Other nodes on the macro keep their version, and the
Modules panel offers them the newer one ("Update to 2"), carrying their
values by name. Agents pass `changes` to `helios_macro_fold`.
