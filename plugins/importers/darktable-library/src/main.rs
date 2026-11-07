//! Helios's darktable library importer, as Helios runs it: JSON-RPC over
//! standard input and output (see the library's documentation).

fn main() {
    helios_plugin_darktable_library::serve(std::io::stdin().lock(), std::io::stdout());
}
