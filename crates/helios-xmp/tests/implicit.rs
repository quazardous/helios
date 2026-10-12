//! Modules an older sidecar leaves out because they were at their default,
//! filled in as darktable fills them when it reads the sidecar: with the
//! default of the time, not today's.

use helios_core::Value as Param;
use helios_xmp::{Layouts, Sidecar, import};

fn xmp(version: u32, end: u32, rawprepare: bool) -> String {
    let raw = if rawprepare {
        r#"<rdf:li darktable:num="0" darktable:operation="rawprepare" darktable:enabled="1" darktable:modversion="1" darktable:params="00000000000000000000000000000000000000000000000000000000"/>"#
    } else {
        ""
    };
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="{version}" darktable:iop_order_version="2" darktable:history_end="{end}">
   <darktable:history><rdf:Seq>
    {raw}
    <rdf:li darktable:num="1" darktable:operation="flip" darktable:enabled="1" darktable:modversion="2" darktable:params="ffffffff"/>
   </rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
    )
}

#[test]
fn an_older_raw_edit_without_highlights_gets_darktables_clip() {
    let sidecar = Sidecar::parse(&xmp(4, 2, true)).unwrap();
    let ops: Vec<_> = sidecar
        .history
        .iter()
        .map(|i| i.operation.as_str())
        .collect();
    assert_eq!(ops, ["rawprepare", "highlights", "flip"]);
    assert!(sidecar.history[1].inserted);
    // The end moves with it: flip stays in force.
    assert_eq!(sidecar.history_end, Some(3));
    let imported = import(&sidecar, &Layouts::built_in());
    let highlights = imported
        .nodes
        .iter()
        .find(|n| n.module == "highlights")
        .expect("highlights applied");
    assert_eq!(highlights.params.get("clip"), Some(&Param::Float(1.0)));
    assert_eq!(imported.report.inserted, ["highlights v4"]);
    let history = imported.darktable["history"].as_array().unwrap();
    assert_eq!(history[1]["inserted"], true);
    assert_eq!(history[2]["status"], "not_applied");
}

#[test]
fn a_current_edit_or_a_picture_gets_nothing_added() {
    for (version, rawprepare) in [(5, true), (4, false)] {
        let sidecar = Sidecar::parse(&xmp(version, 2, rawprepare)).unwrap();
        assert!(
            sidecar.history.iter().all(|i| !i.inserted),
            "v{version} rawprepare {rawprepare}"
        );
    }
}
