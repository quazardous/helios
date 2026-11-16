kind: internal

Helios runs one bus, which the application builds and hands to egui-pop and to its own crates: egui-pop's points and the health points sit side by side on it, and the `bus --check` task of the lint refuses a library crate that builds a bus of its own.
