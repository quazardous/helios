//! Helios's Lightroom catalogue importer, as Helios runs it: JSON-RPC over
//! standard input and output (see the library's documentation).

fn main() {
    helios_plugin_lightroom_catalog::serve(std::io::stdin().lock(), std::io::stdout());
}
