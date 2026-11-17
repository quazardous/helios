kind: fixed

A dry run asked of a command that does not take one (from an agent, a script or the palette) is now refused before it runs, instead of running for real; before, only commands sent to a window were checked. Helios's requests (opening or closing a photo, starting an export, the Library's writes, the Stacks and Decisions answers) now use the bus's own refusal: a step that fails or takes too long always stops the request.
