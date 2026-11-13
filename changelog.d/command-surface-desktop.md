kind: added

Agents and the `helios` CLI reach the desktop's window commands through its library's command endpoint: `commands.list` lists them beside the node's (`scope`: node or window), each saying whether it applies in a given window and, when greyed, why ("Only in the Helios window"); `command.run` runs one in a window under the agent's name, refused with that reason where it does not apply.
