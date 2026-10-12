//! Matching a folder's sidecars to its images: duplicates, case variants,
//! orphans, and camera names that look like duplicates.

use std::path::{Path, PathBuf};

use helios_xmp::files::{Matched, Orphan, SidecarFile, match_names, scan, sidecars_of};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn matched(list: &[&str], derived: &[(&str, &str)]) -> Matched {
    let derived: Vec<(PathBuf, String)> = derived
        .iter()
        .map(|(file, from)| (Path::new("/p").join(file), (*from).to_string()))
        .collect();
    match_names(Path::new("/p"), &names(list), |path| {
        derived
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, from)| from.clone())
    })
}

fn files(m: &Matched, image: &str) -> Vec<(String, u32, bool)> {
    m.images[image]
        .iter()
        .map(|s| {
            (
                s.path.file_name().unwrap().to_string_lossy().into_owned(),
                s.version,
                s.exact,
            )
        })
        .collect()
}

#[test]
fn every_duplicate_is_found_and_the_base_comes_first() {
    let m = matched(
        &[
            "IMG_0001_02.CR2.xmp",
            "IMG_0001.CR2",
            "IMG_0001_01.CR2.xmp",
            "IMG_0001.CR2.xmp",
            "IMG_0001_100.CR2.xmp",
            "IMG_0002.CR2",
        ],
        &[],
    );
    assert_eq!(
        files(&m, "IMG_0001.CR2"),
        [
            ("IMG_0001.CR2.xmp".into(), 0, true),
            ("IMG_0001_01.CR2.xmp".into(), 1, true),
            ("IMG_0001_02.CR2.xmp".into(), 2, true),
            ("IMG_0001_100.CR2.xmp".into(), 100, true),
        ]
    );
    // A raw without a sidecar is still there, with none.
    assert!(m.images["IMG_0002.CR2"].is_empty());
    assert!(m.orphans.is_empty());
    assert_eq!(
        m.base("IMG_0001.CR2").unwrap().path,
        Path::new("/p/IMG_0001.CR2.xmp")
    );
    assert!(m.conflicts().is_empty());
}

#[test]
fn case_variants_are_kept_and_the_exact_name_is_the_base() {
    let m = matched(
        &[
            "IMG_0001.CR2",
            "IMG_0001.CR2.XMP",
            "IMG_0001.cr2.xmp",
            "IMG_0001.CR2.xmp",
            "IMG_0001_01.cr2.xmp",
        ],
        &[],
    );
    assert_eq!(
        files(&m, "IMG_0001.CR2"),
        [
            ("IMG_0001.CR2.xmp".into(), 0, true),
            ("IMG_0001.CR2.XMP".into(), 0, false),
            ("IMG_0001.cr2.xmp".into(), 0, false),
            ("IMG_0001_01.cr2.xmp".into(), 1, false),
        ]
    );
    assert_eq!(m.conflicts(), [("IMG_0001.CR2", 0)]);
    // Without the exact name, a variant is the base: never none.
    let m = matched(&["IMG_0001.CR2", "IMG_0001.cr2.xmp"], &[]);
    assert_eq!(
        m.base("IMG_0001.CR2").unwrap().path,
        Path::new("/p/IMG_0001.cr2.xmp")
    );
}

#[test]
fn a_sidecar_without_its_image_is_an_orphan_named_after_it() {
    let m = matched(
        &[
            "IMG_0003.CR2.xmp",
            "IMG_0003_01.CR2.xmp",
            "renamed.CR2.xmp",
            "IMG_0001.CR2",
        ],
        &[("renamed.CR2.xmp", "IMG_0009.CR2")],
    );
    assert_eq!(
        m.orphans,
        [
            Orphan {
                path: "/p/IMG_0003.CR2.xmp".into(),
                image: "IMG_0003.CR2".into(),
                version: 0
            },
            Orphan {
                path: "/p/IMG_0003_01.CR2.xmp".into(),
                image: "IMG_0003.CR2".into(),
                version: 1
            },
            Orphan {
                path: "/p/renamed.CR2.xmp".into(),
                image: "IMG_0009.CR2".into(),
                version: 0
            },
        ]
    );
    assert!(m.images["IMG_0001.CR2"].is_empty());
}

#[test]
fn a_camera_name_ending_in_digits_is_not_taken_for_a_duplicate() {
    // Both readings of `DSC_01.NEF.xmp` name an image: DerivedFrom says.
    let list = ["DSC.NEF", "DSC_01.NEF", "DSC_01.NEF.xmp", "DSC.NEF.xmp"];
    let own = matched(&list, &[("DSC_01.NEF.xmp", "DSC_01.NEF")]);
    assert_eq!(
        files(&own, "DSC_01.NEF"),
        [("DSC_01.NEF.xmp".into(), 0, true)]
    );
    assert_eq!(files(&own, "DSC.NEF"), [("DSC.NEF.xmp".into(), 0, true)]);
    let dup = matched(&list, &[("DSC_01.NEF.xmp", "DSC.NEF")]);
    assert_eq!(
        files(&dup, "DSC.NEF"),
        [
            ("DSC.NEF.xmp".into(), 0, true),
            ("DSC_01.NEF.xmp".into(), 1, true)
        ]
    );
    assert!(dup.images["DSC_01.NEF"].is_empty());
    // Without DerivedFrom, the image of that exact name keeps it.
    let unknown = matched(&list, &[]);
    assert_eq!(
        files(&unknown, "DSC_01.NEF"),
        [("DSC_01.NEF.xmp".into(), 0, true)]
    );
    // One digit, or more than darktable prints, is not darktable's suffix.
    let m = matched(&["P_1.ORF.xmp", "P_0003.ORF.xmp", "P.ORF"], &[]);
    assert!(m.images["P.ORF"].is_empty());
    let orphans: Vec<_> = m
        .orphans
        .iter()
        .map(|o| (o.image.as_str(), o.version))
        .collect();
    assert_eq!(orphans, [("P_0003.ORF", 0), ("P_1.ORF", 0)]);
}

#[test]
fn a_folder_is_listed_and_sidecars_read_for_their_image() {
    let dir = std::env::temp_dir().join(format!("helios-xmp-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sidecar = |from: &str| {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/" xmpMM:DerivedFrom="{from}"/></rdf:RDF></x:xmpmeta>"#
        )
    };
    for (name, text) in [
        ("A.NEF", String::new()),
        ("A_01.NEF", String::new()),
        ("A.NEF.xmp", sidecar("A.NEF")),
        ("A_01.NEF.xmp", sidecar("A.NEF")),
        ("A_02.NEF.xmp", sidecar("A.NEF")),
    ] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    std::fs::create_dir_all(dir.join("sub.xmp")).unwrap();
    let m = scan(&dir).unwrap();
    assert_eq!(m.images.len(), 2, "{m:?}");
    let a = sidecars_of(&dir.join("A.NEF")).unwrap();
    let versions: Vec<u32> = a.iter().map(|s: &SidecarFile| s.version).collect();
    // A_01.NEF.xmp says it is a duplicate of A.NEF, not A_01.NEF's own.
    assert_eq!(versions, [0, 1, 2]);
    assert!(sidecars_of(&dir.join("A_01.NEF")).unwrap().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}
